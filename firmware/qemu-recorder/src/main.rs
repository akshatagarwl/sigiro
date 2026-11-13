#![no_std]
#![no_main]
mod flash;
use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use cortex_m_rt::entry;
use flash::HostNor;
use micropb::{MessageEncode, PbEncoder};
use opentelemetry_micropb::heapless::{
    collector_::logs_::v1_::ExportLogsServiceRequest,
    common_::v1_::{AnyValue, AnyValue_::Value, KeyValue},
    logs_::v1_::{LogRecord, ResourceLogs, ScopeLogs},
    resource_::v1_::Resource,
};
use otlp_outbox::{Ack, BatchId, Error, Outbox, Outcome, Signal};
use semihosting::{
    fs::{self, File},
    io::{Read, Write},
};

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("simulator storage unexpectedly suspended"),
    }
}
fn id(ordinal: u32) -> BatchId {
    let mut id = *b"qemu-lab-id-0000";
    id[12..].copy_from_slice(&ordinal.to_be_bytes());
    BatchId(id)
}
fn encode(ordinal: u32, bytes: &mut [u8]) -> usize {
    let mut key = KeyValue::default();
    key.set_key("service.name".try_into().unwrap())
        .set_value(AnyValue {
            value: Some(Value::StringValue("sigiro-micro-qemu".try_into().unwrap())),
        });
    let mut resource = Resource::default();
    resource.attributes.push(key).unwrap();
    let mut log = LogRecord::default();
    log.set_time_unix_nano(1_794_495_600_123_456_789 + ordinal as u64)
        .set_body(AnyValue {
            value: Some(Value::StringValue("QEMU: π\0🍵".try_into().unwrap())),
        });
    let mut scope = ScopeLogs::default();
    scope.log_records.push(log).unwrap();
    let mut group = ResourceLogs::default();
    group.set_resource(resource);
    group.scope_logs.push(scope).unwrap();
    let mut request = ExportLogsServiceRequest::default();
    request.resource_logs.push(group).unwrap();
    let size = bytes.len();
    let mut remaining = bytes;
    request.encode(&mut PbEncoder::new(&mut remaining)).unwrap();
    size - remaining.len()
}

#[entry]
fn main() -> ! {
    let mut command = [0u8; 12];
    File::open(c"command.bin")
        .unwrap()
        .read_exact(&mut command)
        .unwrap();
    let ordinal = u32::from_le_bytes(command[4..8].try_into().unwrap());
    let cut = u32::from_le_bytes(command[8..12].try_into().unwrap());
    let flash = HostNor {
        mutations: 0,
        cut_after: (cut != u32::MAX).then_some(cut),
        return_error: command[1] == 1,
    };
    let mut outbox = Outbox::new(flash, 0..flash::CAPACITY as u32);
    let mut scratch = [0u8; 512];
    let mut payload = [0u8; 384];
    let (status, count) = block_on(async {
        match command[0] {
            1 => {
                let mut used = encode(ordinal, &mut payload);
                if command[2] == 1 {
                    payload[used..used + 7].copy_from_slice(b"\xba\x3e\x04raw!");
                    used += 7;
                } else if command[2] == 2 {
                    payload[0] = 0xff;
                    used = 1;
                }
                match outbox
                    .push(id(ordinal), Signal::Logs, &payload[..used], &mut scratch)
                    .await
                {
                    Ok(()) => (0, 1),
                    Err(Error::Storage(sequential_storage::Error::FullStorage)) => (2, 0),
                    Err(_) => (5, 0),
                }
            }
            2 => match outbox.peek(&mut scratch).await.unwrap() {
                Some(batch) => {
                    fs::write(c"record.bin", batch.wire()).unwrap();
                    (0, 1)
                }
                None => {
                    fs::write(c"record.bin", []).unwrap();
                    (1, 0)
                }
            },
            3 => {
                let mut bytes = [0u8; otlp_outbox::ACK_SIZE];
                File::open(c"ack.bin")
                    .unwrap()
                    .read_exact(&mut bytes)
                    .unwrap();
                match Ack::decode(&bytes) {
                    Ok(ack) => match outbox.apply_ack(ack, &mut scratch).await {
                        Ok(true) => (0, 1),
                        Ok(false) => (3, 0),
                        Err(Error::MismatchedAck) => (4, 0),
                        Err(_) => (5, 0),
                    },
                    Err(_) => (4, 0),
                }
            }
            4 => {
                let mut count = 0;
                loop {
                    let used = encode(ordinal + count, &mut payload);
                    match outbox
                        .push(
                            id(ordinal + count),
                            Signal::Logs,
                            &payload[..used],
                            &mut scratch,
                        )
                        .await
                    {
                        Ok(()) => {
                            count += 1;
                            assert!(count < 1024);
                        }
                        Err(Error::Storage(sequential_storage::Error::FullStorage)) => {
                            break (2, count)
                        }
                        Err(_) => break (5, count),
                    }
                }
            }
            // Test fixture only: synthesize acceptance to exercise queue wrap/erase.
            5 => {
                for n in 0..ordinal {
                    let used = encode(n, &mut payload);
                    outbox
                        .push(id(n), Signal::Logs, &payload[..used], &mut scratch)
                        .await
                        .unwrap();
                    assert!(outbox
                        .apply_ack(
                            Ack {
                                id: id(n),
                                signal: Signal::Logs,
                                outcome: Outcome::FullAcceptance
                            },
                            &mut scratch
                        )
                        .await
                        .unwrap());
                }
                (0, ordinal)
            }
            // Test fixtures only, never production export paths.
            6 => {
                let mut count = 0;
                loop {
                    let used = encode(count, &mut payload);
                    match outbox
                        .push(id(count), Signal::Logs, &payload[..used], &mut scratch)
                        .await
                    {
                        Ok(()) => count += 1,
                        Err(Error::Storage(sequential_storage::Error::FullStorage)) => break,
                        Err(error) => panic!("{error:?}"),
                    }
                }
                for n in 0..count / 2 {
                    assert!(outbox
                        .apply_ack(
                            Ack {
                                id: id(n),
                                signal: Signal::Logs,
                                outcome: Outcome::FullAcceptance
                            },
                            &mut scratch
                        )
                        .await
                        .unwrap());
                }
                (0, count / 2)
            }
            7 => {
                let mut output = File::create(c"dump.bin").unwrap();
                let mut count = 0;
                while let Some(batch) = outbox.peek(&mut scratch).await.unwrap() {
                    output
                        .write_all(&(batch.wire().len() as u32).to_le_bytes())
                        .unwrap();
                    output.write_all(batch.wire()).unwrap();
                    let ack = Ack {
                        id: batch.id,
                        signal: batch.signal,
                        outcome: Outcome::FullAcceptance,
                    };
                    assert!(outbox.apply_ack(ack, &mut scratch).await.unwrap());
                    count += 1;
                    assert!(count < 1024);
                }
                (0, count)
            }
            _ => panic!("unknown simulator command"),
        }
    });
    let mutations = outbox.destroy().mutations;
    let mut report = [0u8; 9];
    report[0] = status;
    report[1..5].copy_from_slice(&mutations.to_le_bytes());
    report[5..9].copy_from_slice(&count.to_le_bytes());
    fs::write(c"status.bin", report).unwrap();
    semihosting::println!("QEMU status={status} count={count} flash_mutated_bytes={mutations}");
    semihosting::process::exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    semihosting::eprintln!("FIRMWARE PANIC: {info}");
    semihosting::process::exit(1)
}

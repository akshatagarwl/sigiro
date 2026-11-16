use embedded_storage_file::{NorMemoryAsync, NorMemoryInram};
use futures::executor::block_on;
use otlp_outbox::{Ack, Batch, BatchId, Error, Outbox, Outcome, Signal};

type Flash = NorMemoryAsync<NorMemoryInram<1, 1, 4096>>;
fn queue() -> Outbox<Flash> {
    Outbox::new(NorMemoryAsync::new(NorMemoryInram::new(8192)), 0..8192)
}
fn ack(id: u8, outcome: Outcome) -> Ack {
    Ack {
        id: BatchId([id; 16]),
        signal: Signal::Logs,
        outcome,
    }
}

#[test]
fn restart_preserves_original_bytes_and_identity() {
    block_on(async {
        let mut queue = queue();
        let mut scratch = [0; 512];
        let payload = b"\x0a\x03\xff\0\x80";
        queue
            .push(BatchId([7; 16]), Signal::Logs, payload, &mut scratch)
            .await
            .unwrap();
        let mut queue = Outbox::new(queue.destroy(), 0..8192);
        let batch = queue.peek(&mut scratch).await.unwrap().unwrap();
        assert_eq!(batch.payload, payload);
        assert_eq!(batch.id, BatchId([7; 16]));
    });
}

#[test]
fn only_matching_full_application_acceptance_removes() {
    block_on(async {
        let mut queue = queue();
        let mut scratch = [0; 512];
        queue
            .push(BatchId([1; 16]), Signal::Logs, b"raw", &mut scratch)
            .await
            .unwrap();
        queue
            .push(BatchId([2; 16]), Signal::Logs, b"next", &mut scratch)
            .await
            .unwrap();
        for outcome in [
            Outcome::BrokerOnly,
            Outcome::RetryLater,
            Outcome::Partial,
            Outcome::Permanent,
        ] {
            assert!(!queue
                .apply_ack(ack(1, outcome), &mut scratch)
                .await
                .unwrap());
            assert_eq!(
                queue.peek(&mut scratch).await.unwrap().unwrap().id,
                BatchId([1; 16])
            );
        }
        assert!(matches!(
            queue
                .apply_ack(ack(9, Outcome::FullAcceptance), &mut scratch)
                .await,
            Err(Error::MismatchedAck)
        ));
        let mut wrong_signal = ack(1, Outcome::FullAcceptance);
        wrong_signal.signal = Signal::Metrics;
        assert!(matches!(
            queue.apply_ack(wrong_signal, &mut scratch).await,
            Err(Error::MismatchedAck)
        ));
        assert!(queue
            .apply_ack(ack(1, Outcome::FullAcceptance), &mut scratch)
            .await
            .unwrap());
        assert!(matches!(
            queue
                .apply_ack(ack(1, Outcome::FullAcceptance), &mut scratch)
                .await,
            Err(Error::MismatchedAck)
        ));
        assert_eq!(
            queue.peek(&mut scratch).await.unwrap().unwrap().payload,
            b"next"
        );
    });
}

#[test]
fn full_queue_never_overwrites_oldest() {
    block_on(async {
        let mut queue = queue();
        let mut scratch = [0; 512];
        let mut full = false;
        for n in 0..128u8 {
            match queue
                .push(BatchId([n; 16]), Signal::Logs, &[n; 384], &mut scratch)
                .await
            {
                Ok(()) => {}
                Err(Error::Storage(sequential_storage::Error::FullStorage)) => {
                    full = true;
                    break;
                }
                Err(error) => panic!("{error:?}"),
            }
        }
        assert!(full);
        assert_eq!(
            queue.peek(&mut scratch).await.unwrap().unwrap().id,
            BatchId([0; 16])
        );
    });
}

#[test]
fn malformed_records_and_acknowledgements_are_rejected() {
    let ack = ack(1, Outcome::FullAcceptance);
    let bytes = ack.encode();
    assert_eq!(Ack::decode(&bytes).unwrap(), ack);
    for n in 0..bytes.len() {
        assert!(Ack::decode(&bytes[..n]).is_err());
    }
    let mut invalid = bytes;
    invalid[21] = 0;
    assert!(Ack::decode(&invalid).is_err());
    invalid = bytes;
    invalid[23] = 1;
    assert!(Ack::decode(&invalid).is_err());
    for n in 0..28 {
        assert!(Batch::parse(&[0u8; 28][..n]).is_err());
    }
}

#[test]
fn undersized_buffer_does_not_enqueue() {
    block_on(async {
        let mut queue = queue();
        assert!(matches!(
            queue
                .push(BatchId([1; 16]), Signal::Logs, b"payload", &mut [0; 8])
                .await,
            Err(Error::BufferTooSmall)
        ));
        assert!(queue.peek(&mut [0; 512]).await.unwrap().is_none());
    });
}

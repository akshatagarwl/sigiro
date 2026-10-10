use std::process::Command;

use arrow::array::{StringArray, TimestampMicrosecondArray};
use bytes::Bytes;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use prost::Message;
use sigiro_segments::{
    InputFormat, SegmentMetadata, Signal, transform_logs, transform_traces, write_segment,
};

const TRACE_ID: [u8; 16] = [
    0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe,
];
const SPAN_ID: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
const TRACE_START_NS: u64 = 1_700_000_000_000_001_000;
const TRACE_END_NS: u64 = 1_700_000_000_000_009_000;
const LOG_TIME_NS: u64 = 1_700_000_000_123_456_000;

#[test]
fn protobuf_traces_decode_to_arrow_and_parquet_segment() {
    let request = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource("checkout")),
            scope_spans: vec![ScopeSpans {
                scope: Some(scope()),
                spans: vec![Span {
                    trace_id: TRACE_ID.to_vec(),
                    span_id: SPAN_ID.to_vec(),
                    name: "POST /checkout".to_string(),
                    kind: SpanKind::Server as i32,
                    start_time_unix_nano: TRACE_START_NS,
                    end_time_unix_nano: TRACE_END_NS,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let bytes = request.encode_to_vec();

    let batch = transform_traces(&bytes, InputFormat::Protobuf).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_string(&batch, "span_name", 0, "POST /checkout");
    assert_string(&batch, "trace_id", 0, "1032547698badcfe1032547698badcfe");

    let segment = write_segment(&batch).unwrap();
    assert_eq!(
        segment.metadata,
        SegmentMetadata {
            signal: Signal::Spans,
            min_time_us: 1_700_000_000_000_001,
            max_time_us: 1_700_000_000_000_001,
            rows: 1,
            bytes: segment.parquet.len() as u64,
        }
    );

    let read = read_parquet(&segment.parquet);
    assert_eq!(read.num_rows(), 1);
    assert_string(&read, "span_name", 0, "POST /checkout");
    assert_string(&read, "trace_id", 0, "1032547698badcfe1032547698badcfe");
    assert_timestamp(&read, "timestamp", 0, 1_700_000_000_000_001);
}

#[test]
fn json_logs_decode_to_arrow_and_parquet_segment() {
    let request = ExportLogsServiceRequest {
        resource_logs: vec![opentelemetry_proto::tonic::logs::v1::ResourceLogs {
            resource: Some(resource("worker")),
            scope_logs: vec![opentelemetry_proto::tonic::logs::v1::ScopeLogs {
                scope: Some(scope()),
                log_records: vec![LogRecord {
                    time_unix_nano: LOG_TIME_NS,
                    observed_time_unix_nano: LOG_TIME_NS + 2_000,
                    trace_id: TRACE_ID.to_vec(),
                    span_id: SPAN_ID.to_vec(),
                    event_name: "checkout.failed".to_string(),
                    severity_number: 17,
                    severity_text: "ERROR".to_string(),
                    body: Some(AnyValue {
                        value: Some(Value::StringValue("card declined".to_string())),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let bytes = serde_json::to_vec(&request).unwrap();

    let (batch, rejected) = transform_logs(&bytes, InputFormat::Json).unwrap();
    assert_eq!(rejected, 0);
    assert_eq!(batch.num_rows(), 1);
    assert_string(&batch, "body", 0, "card declined");
    assert_string(&batch, "service_name", 0, "worker");

    let segment = write_segment(&batch).unwrap();
    assert_eq!(segment.metadata.signal, Signal::Logs);
    assert_eq!(segment.metadata.min_time_us, 1_700_000_000_123_456);
    assert_eq!(segment.metadata.max_time_us, 1_700_000_000_123_456);
    assert_eq!(segment.metadata.rows, 1);
    assert_eq!(segment.metadata.bytes, segment.parquet.len() as u64);

    let read = read_parquet(&segment.parquet);
    assert_eq!(read.num_rows(), 1);
    assert_string(&read, "body", 0, "card declined");
    assert_string(&read, "service_name", 0, "worker");
    assert_timestamp(&read, "timestamp", 0, 1_700_000_000_123_456);
}

#[test]
fn duckdb_reads_written_trace_segment() {
    if Command::new("duckdb").arg("--version").output().is_err() {
        println!("skipping duckdb cross-reader test because duckdb is not on PATH");
        return;
    }

    let request = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource("checkout")),
            scope_spans: vec![ScopeSpans {
                scope: Some(scope()),
                spans: vec![
                    Span {
                        trace_id: TRACE_ID.to_vec(),
                        span_id: SPAN_ID.to_vec(),
                        name: "POST /checkout".to_string(),
                        kind: SpanKind::Server as i32,
                        start_time_unix_nano: TRACE_START_NS,
                        end_time_unix_nano: TRACE_END_NS,
                        ..Default::default()
                    },
                    Span {
                        trace_id: TRACE_ID.to_vec(),
                        span_id: [0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10].to_vec(),
                        parent_span_id: SPAN_ID.to_vec(),
                        name: "SELECT inventory".to_string(),
                        kind: SpanKind::Client as i32,
                        start_time_unix_nano: TRACE_START_NS + 3_000,
                        end_time_unix_nano: TRACE_END_NS + 5_000,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let batch = transform_traces(&request.encode_to_vec(), InputFormat::Protobuf).unwrap();
    let segment = write_segment(&batch).unwrap();
    let mut file = tempfile::Builder::new()
        .suffix(".parquet")
        .tempfile()
        .unwrap();
    std::io::Write::write_all(&mut file, &segment.parquet).unwrap();
    let sql = format!(
        "SELECT count(*) AS rows, min(epoch_us(timestamp)) AS min_time_us FROM '{}'",
        file.path().display()
    );

    let output = Command::new("duckdb")
        .args(["-csv", "-c", &sql])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "duckdb failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "rows,min_time_us\n2,1700000000000001\n"
    );
}

fn read_parquet(bytes: &[u8]) -> arrow::record_batch::RecordBatch {
    let reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::copy_from_slice(bytes))
        .unwrap()
        .build()
        .unwrap();
    let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
    arrow::compute::concat_batches(&batches[0].schema(), &batches).unwrap()
}

fn resource(service_name: &str) -> Resource {
    Resource {
        attributes: vec![KeyValue {
            key: "service.name".to_string(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(service_name.to_string())),
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn scope() -> InstrumentationScope {
    InstrumentationScope {
        name: "sigiro-test".to_string(),
        version: "1.0.0".to_string(),
        ..Default::default()
    }
}

fn assert_string(
    batch: &arrow::record_batch::RecordBatch,
    column: &str,
    row: usize,
    expected: &str,
) {
    let idx = batch.schema().index_of(column).unwrap();
    let array = batch
        .column(idx)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(array.value(row), expected);
}

fn assert_timestamp(
    batch: &arrow::record_batch::RecordBatch,
    column: &str,
    row: usize,
    expected: i64,
) {
    let idx = batch.schema().index_of(column).unwrap();
    let array = batch
        .column(idx)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .unwrap();
    assert_eq!(array.value(row), expected);
}

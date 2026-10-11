use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use sigiro_segments::{Table, empty_segment};

fn columns(bytes: Vec<u8>) -> (Vec<String>, usize) {
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(bytes)).unwrap();
    let names = reader
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let rows = reader.build().unwrap().map(|b| b.unwrap().num_rows()).sum();
    (names, rows)
}

#[test]
fn an_empty_spans_segment_has_the_spans_columns_and_no_rows() {
    let (names, rows) = columns(empty_segment(Table::Spans).unwrap());
    assert_eq!(rows, 0);
    assert_eq!(names.len(), 25);
    assert_eq!(
        &names[..4],
        ["timestamp", "end_timestamp", "duration", "trace_id"]
    );
}

#[test]
fn every_metric_table_has_an_empty_segment() {
    for table in [
        Table::Logs,
        Table::MetricsGauge,
        Table::MetricsSum,
        Table::MetricsHistogram,
        Table::MetricsExpHistogram,
    ] {
        let (names, rows) = columns(empty_segment(table).unwrap());
        assert_eq!(rows, 0, "{table:?}");
        assert_eq!(names[0], "timestamp", "{table:?}");
    }
}

#[test]
fn profiles_have_no_hosted_schema_yet() {
    assert!(empty_segment(Table::Profiles).is_none());
}

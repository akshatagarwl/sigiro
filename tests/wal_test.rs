use arrow::array::{Int32Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use sigiro::wal::{read_wal, write_wal};
use std::sync::Arc;

fn make_batch(n: usize) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("name", DataType::Utf8, true),
    ]));
    let ids: Vec<i32> = (0..n as i32).collect();
    let names: Vec<&str> = (0..n).map(|_| "test").collect();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int32Array::from(ids)),
            Arc::new(StringArray::from(names)),
        ],
    )
    .unwrap()
}

#[test]
fn roundtrip_wal() {
    let dir = tempfile::tempdir().unwrap();
    let wal_dir = dir.path().join("wal");

    let spans = [make_batch(5)];
    let logs = [make_batch(3)];
    let entries: Vec<(&str, &[RecordBatch])> =
        vec![("sigiro_spans", &spans), ("sigiro_logs", &logs)];

    let path = write_wal(&wal_dir, "proj1", &entries).unwrap();
    assert!(path.exists());

    let read_back = read_wal(&path).unwrap();
    assert_eq!(read_back.len(), 2);
    assert_eq!(read_back[0].0, "sigiro_spans");
    assert_eq!(read_back[0].1.len(), 1);
    assert_eq!(read_back[0].1[0].num_rows(), 5);
    assert_eq!(read_back[1].0, "sigiro_logs");
    assert_eq!(read_back[1].1[0].num_rows(), 3);
}

#[test]
fn empty_batches_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let wal_dir = dir.path().join("wal");

    let logs = [make_batch(2)];
    let entries: Vec<(&str, &[RecordBatch])> = vec![("sigiro_spans", &[]), ("sigiro_logs", &logs)];

    let path = write_wal(&wal_dir, "proj1", &entries).unwrap();
    let read_back = read_wal(&path).unwrap();
    // Empty spans entry should be skipped
    assert_eq!(read_back.len(), 1);
    assert_eq!(read_back[0].0, "sigiro_logs");
}

#[test]
fn multiple_batches_per_table() {
    let dir = tempfile::tempdir().unwrap();
    let wal_dir = dir.path().join("wal");

    let batches = [make_batch(3), make_batch(7)];
    let entries: Vec<(&str, &[RecordBatch])> = vec![("sigiro_spans", &batches)];

    let path = write_wal(&wal_dir, "proj1", &entries).unwrap();
    let read_back = read_wal(&path).unwrap();
    assert_eq!(read_back[0].1.len(), 2);
    assert_eq!(read_back[0].1[0].num_rows(), 3);
    assert_eq!(read_back[0].1[1].num_rows(), 7);
}

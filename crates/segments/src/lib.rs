use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use arrow::array::{Array, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::errors::ParquetError;
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};

mod otel_proto;
pub mod otlp;
pub mod read_only_sql;
pub use read_only_sql::{ALLOWED_TABLES, validate_read_only, validate_read_only_with};
mod proto;

pub use otlp::{
    DecodeError, InputFormat, MetricBatches, SkippedMetrics, transform_logs, transform_metrics,
    transform_traces,
};

const ROW_GROUP_SIZE: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Table {
    Spans,
    Logs,
    MetricsGauge,
    MetricsSum,
    MetricsHistogram,
    MetricsExpHistogram,
    Profiles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub key: String,
    pub table: Table,
    pub min_time_us: i64,
    pub max_time_us: i64,
    pub rows: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentMetadata {
    pub table: Table,
    pub min_time_us: i64,
    pub max_time_us: i64,
    pub rows: u64,
    pub bytes: u64,
}

impl SegmentMetadata {
    pub fn with_key(self, key: impl Into<String>) -> Segment {
        Segment {
            key: key.into(),
            table: self.table,
            min_time_us: self.min_time_us,
            max_time_us: self.max_time_us,
            rows: self.rows,
            bytes: self.bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenSegment {
    pub metadata: SegmentMetadata,
    pub parquet: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum SegmentWriteError {
    #[error("cannot write an empty segment")]
    EmptyBatch,
    #[error("missing timestamp column")]
    MissingTimestamp,
    #[error("timestamp column must be Timestamp(Microsecond, None)")]
    UnsupportedTimestamp,
    #[error("unsupported segment schema")]
    UnsupportedSchema,
    #[error("parquet write error: {0}")]
    Parquet(#[from] ParquetError),
}

pub fn write_segment(
    table: Table,
    batch: &RecordBatch,
) -> Result<WrittenSegment, SegmentWriteError> {
    if batch.num_rows() == 0 {
        return Err(SegmentWriteError::EmptyBatch);
    }

    let (min_time_us, max_time_us) = time_bounds_us(batch)?;
    let mut parquet = Vec::new();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_max_row_group_row_count(Some(ROW_GROUP_SIZE))
        .build();
    let mut writer = ArrowWriter::try_new(&mut parquet, batch.schema(), Some(props))?;
    writer.write(batch)?;
    writer.close()?;
    let bytes = parquet.len() as u64;

    Ok(WrittenSegment {
        metadata: SegmentMetadata {
            table,
            min_time_us,
            max_time_us,
            rows: batch.num_rows() as u64,
            bytes,
        },
        parquet,
    })
}

fn time_bounds_us(batch: &RecordBatch) -> Result<(i64, i64), SegmentWriteError> {
    let index = batch
        .schema()
        .index_of("timestamp")
        .map_err(|_| SegmentWriteError::MissingTimestamp)?;
    match batch.schema().field(index).data_type() {
        DataType::Timestamp(TimeUnit::Microsecond, None) => {}
        _ => return Err(SegmentWriteError::UnsupportedTimestamp),
    }
    let timestamps = batch
        .column(index)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .ok_or(SegmentWriteError::UnsupportedTimestamp)?;
    let mut bounds = (i64::MAX, i64::MIN);
    for row in 0..timestamps.len() {
        if timestamps.is_valid(row) {
            let value = timestamps.value(row);
            bounds.0 = bounds.0.min(value);
            bounds.1 = bounds.1.max(value);
        }
    }
    if bounds.0 == i64::MAX {
        return Err(SegmentWriteError::MissingTimestamp);
    }
    Ok(bounds)
}

#[nutype::nutype(
    validate(predicate = |id: &str| {
        id.len() == 32
            && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && id.bytes().any(|b| b != b'0')
    }),
    derive(Debug, Clone, PartialEq, Eq, Hash, Into, AsRef, Serialize, Deserialize)
)]
pub struct TraceId(String);

#[nutype::nutype(
    validate(predicate = |id: &str| {
        id.len() == 16
            && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && id.bytes().any(|b| b != b'0')
    }),
    derive(Debug, Clone, PartialEq, Eq, Hash, Into, AsRef, Serialize, Deserialize)
)]
pub struct SpanId(String);

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest key is already in use: {0}")]
    KeyInUse(String),
    #[error("segment is not pending: {0}")]
    NotPending(String),
    #[error("segment is not live: {0}")]
    NotLive(String),
    #[error("segment table does not match manifest entry: {0}")]
    TableMismatch(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "manifest_version")]
    version: u32,
    generation: u64,
    pending: BTreeMap<String, Table>,
    live: BTreeMap<String, Segment>,
}

const fn manifest_version() -> u32 {
    1
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: manifest_version(),
            generation: 0,
            pending: BTreeMap::new(),
            live: BTreeMap::new(),
        }
    }
}

impl Manifest {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn register(&mut self, key: &str, table: Table) -> Result<(), ManifestError> {
        if self.pending.contains_key(key) || self.live.contains_key(key) {
            return Err(ManifestError::KeyInUse(key.into()));
        }
        self.pending.insert(key.into(), table);
        self.generation += 1;
        Ok(())
    }

    pub fn commit(&mut self, segment: Segment) -> Result<(), ManifestError> {
        self.take_pending(&segment)?;
        self.live.insert(segment.key.clone(), segment);
        self.generation += 1;
        Ok(())
    }

    pub fn replace(&mut self, inputs: &[&str], output: Segment) -> Result<(), ManifestError> {
        for key in inputs {
            match self.live.get(*key) {
                None => return Err(ManifestError::NotLive((*key).into())),
                Some(input) if input.table != output.table => {
                    return Err(ManifestError::TableMismatch((*key).into()));
                }
                Some(_) => {}
            }
        }
        self.take_pending(&output)?;
        for key in inputs {
            self.live.remove(*key);
        }
        self.live.insert(output.key.clone(), output);
        self.generation += 1;
        Ok(())
    }

    pub fn abandon(&mut self, key: &str) {
        if self.pending.remove(key).is_some() {
            self.generation += 1;
        }
    }

    pub fn expire(&mut self, table: Table, cutoff_us: i64) -> Vec<String> {
        let expired: Vec<String> = self
            .live
            .values()
            .filter(|s| s.table == table && s.max_time_us < cutoff_us)
            .map(|s| s.key.clone())
            .collect();
        if !expired.is_empty() {
            for key in &expired {
                self.live.remove(key);
            }
            self.generation += 1;
        }
        expired
    }

    pub fn live(&self, table: Table, from_us: i64, to_us: i64) -> Vec<&Segment> {
        self.live
            .values()
            .filter(|s| s.table == table && s.min_time_us <= to_us && s.max_time_us >= from_us)
            .collect()
    }

    pub fn orphans<'a>(&self, stored: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
        let known: BTreeSet<&str> = self
            .pending
            .keys()
            .chain(self.live.keys())
            .map(String::as_str)
            .collect();
        stored.filter(|key| !known.contains(key)).collect()
    }

    fn take_pending(&mut self, segment: &Segment) -> Result<(), ManifestError> {
        match self.pending.get(&segment.key) {
            None => Err(ManifestError::NotPending(segment.key.clone())),
            Some(table) if *table != segment.table => {
                Err(ManifestError::TableMismatch(segment.key.clone()))
            }
            Some(_) => {
                self.pending.remove(&segment.key);
                Ok(())
            }
        }
    }
}

//! Large span attribute values, stored once per day.
//!
//! GenAI instrumentation puts whole prompts and responses into span
//! attributes, and repeats them: every model call in a conversation carries the
//! full history so far, often under two or three keys. Kept inline in
//! `span_attributes`, those copies dominate storage and every query that reads
//! the column.
//!
//! Each attribute value whose JSON encoding is at least
//! [`LARGE_VALUE_BYTES`] is moved out of `span_attributes` into
//! `sigiro_span_values`, keyed by a hash of that encoding, and the span keeps
//! `{"key": "<hash>"}` in `span_attribute_refs`. A value row is stamped with the
//! last microsecond of the UTC day of the span that referenced it, so the
//! writer can store each distinct value once per day, and retention, which
//! deletes rows older than a cutoff, never deletes a value before the spans of
//! that day that refer to it.

use arrow::array::{StringBuilder, TimestampMicrosecondBuilder};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::error::ArrowError;
use arrow::record_batch::RecordBatch;
use serde_json::{Map, Value as JsonValue};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::otel_proto::tonic::common::v1::KeyValue;
use crate::otlp::any_value_to_json;

/// Values whose JSON encoding is at least this many bytes are stored apart.
pub const LARGE_VALUE_BYTES: usize = 1024;

const DAY_US: i64 = 86_400_000_000;

/// The last microsecond of the UTC day containing `timestamp_us`.
pub fn day_end_us(timestamp_us: i64) -> i64 {
    (timestamp_us.div_euclid(DAY_US) + 1) * DAY_US - 1
}

/// Hex of the first 16 bytes of the SHA-256 of a value's JSON encoding.
pub fn value_hash(json: &str) -> String {
    hex::encode(&Sha256::digest(json.as_bytes())[..16])
}

pub fn span_values_schema() -> Schema {
    Schema::new(vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("hash", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
    ])
}

/// Rows for `sigiro_span_values`, one per large value occurrence. The writer
/// drops the ones it already stored.
#[derive(Default)]
pub(crate) struct SpanValueBuilders {
    timestamp: TimestampMicrosecondBuilder,
    hash: StringBuilder,
    value: StringBuilder,
}

impl SpanValueBuilders {
    /// Split span attributes into the inline JSON object and, when any value
    /// is large, the `{"key": "<hash>"}` refs object.
    pub(crate) fn split(
        &mut self,
        attrs: &[KeyValue],
        timestamp_us: i64,
    ) -> Result<(String, Option<String>), serde_json::Error> {
        let mut inline = Map::new();
        let mut refs = Map::new();
        for kv in attrs {
            let Some(value) = kv.value.as_ref().and_then(any_value_to_json) else {
                continue;
            };
            let json = serde_json::to_string(&value)?;
            if json.len() < LARGE_VALUE_BYTES {
                inline.insert(kv.key.clone(), value);
                continue;
            }
            let hash = value_hash(&json);
            self.timestamp.append_value(day_end_us(timestamp_us));
            self.hash.append_value(&hash);
            self.value.append_value(&json);
            refs.insert(kv.key.clone(), JsonValue::String(hash));
        }
        let refs = if refs.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&refs)?)
        };
        Ok((serde_json::to_string(&inline)?, refs))
    }

    pub(crate) fn finish(mut self) -> Result<RecordBatch, ArrowError> {
        RecordBatch::try_new(
            Arc::new(span_values_schema()),
            vec![
                Arc::new(self.timestamp.finish()),
                Arc::new(self.hash.finish()),
                Arc::new(self.value.finish()),
            ],
        )
    }
}

/// Put referenced values back into a span's attributes. `lookup` returns the
/// stored JSON encoding for a hash; a ref with no stored value is skipped.
pub fn merge_span_values<'a>(
    span_attributes: &str,
    span_attribute_refs: &str,
    lookup: impl Fn(&str) -> Option<&'a str>,
) -> Result<String, serde_json::Error> {
    let mut attrs: Map<String, JsonValue> = serde_json::from_str(span_attributes)?;
    let refs: Map<String, JsonValue> = serde_json::from_str(span_attribute_refs)?;
    for (key, hash) in refs {
        if let Some(json) = hash.as_str().and_then(&lookup) {
            attrs.insert(key, serde_json::from_str(json)?);
        }
    }
    serde_json::to_string(&attrs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otel_proto::tonic::common::v1::{AnyValue, any_value::Value};

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(value.into())),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn day_end_is_last_microsecond_of_the_utc_day() {
        assert_eq!(day_end_us(0), DAY_US - 1);
        assert_eq!(day_end_us(DAY_US - 1), DAY_US - 1);
        assert_eq!(day_end_us(DAY_US), 2 * DAY_US - 1);
        assert_eq!(day_end_us(-1), -1);
    }

    #[test]
    fn split_moves_only_large_values_and_merge_restores_them() {
        let big = "x".repeat(LARGE_VALUE_BYTES);
        let attrs = [kv("small", "a"), kv("input.value", &big), kv("copy", &big)];
        let mut builders = SpanValueBuilders::default();
        let (inline, refs) = builders.split(&attrs, 5).unwrap();
        assert_eq!(inline, r#"{"small":"a"}"#);
        let refs = refs.unwrap();
        let values = builders.finish().unwrap();
        assert_eq!(values.num_rows(), 2);

        let stored = serde_json::to_string(&big).unwrap();
        let hash = value_hash(&stored);
        assert_eq!(
            refs,
            format!(r#"{{"copy":"{hash}","input.value":"{hash}"}}"#)
        );
        let merged = merge_span_values(&inline, &refs, |h| (h == hash).then_some(stored.as_str()));
        let mut expected = Map::new();
        expected.insert("small".into(), "a".into());
        expected.insert("input.value".into(), big.clone().into());
        expected.insert("copy".into(), big.into());
        assert_eq!(merged.unwrap(), serde_json::to_string(&expected).unwrap());
    }

    #[test]
    fn split_without_large_values_has_no_refs_or_rows() {
        let mut builders = SpanValueBuilders::default();
        let (inline, refs) = builders.split(&[kv("k", "v")], 0).unwrap();
        assert_eq!((inline.as_str(), refs), (r#"{"k":"v"}"#, None));
        assert_eq!(builders.finish().unwrap().num_rows(), 0);
    }
}

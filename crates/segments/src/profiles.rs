use std::sync::Arc;

use arrow::array::{Int64Array, ListBuilder, StringBuilder, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use prost::Message;
use serde_json::{Map, Value as JsonValue};

use crate::otel_proto::tonic::collector::profiles::v1development::ExportProfilesServiceRequest;
use crate::otel_proto::tonic::profiles::v1development::ProfilesDictionary;
use crate::{DecodeError, SpanId, TraceId};

/// Frames kept per stack (leaf side). Deeper stacks lose their outermost
/// callers.
const MAX_FRAMES: usize = 512;

/// The Profile attribute that names the Go runtime profile (cpu, heap,
/// goroutine, mutex, block). pprof has no such field and mutex and block
/// share sample types, so this is how an OTLP producer tells them apart.
const PROFILE_NAME_ATTR: &str = "go.pprof.name";

pub fn transform_otlp_profiles(bytes: &[u8]) -> Result<RecordBatch, DecodeError> {
    transform_profiles_request(ExportProfilesServiceRequest::decode(bytes)?)
}

/// Resolve pprofile dictionary encoding into flat profile rows as a RecordBatch.
/// Resolution chain:
///   Sample.stack_index → Dictionary.stack_table[].location_indices[]
///   → Dictionary.location_table[].lines[].function_index
///   → Dictionary.function_table[].name_strindex
///   → Dictionary.string_table[]
pub(crate) fn profile_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("profile_id", DataType::Utf8, true),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, false),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("profile_type", DataType::Utf8, true),
        Field::new(
            "stack_trace",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new("value", DataType::Int64, true),
        Field::new("unit", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        // Per-sample labels as a JSON object (OTLP Sample attributes).
        // Appended last so existing tables gain it with
        // `ALTER TABLE … ADD COLUMN`.
        Field::new("labels", DataType::Utf8, true),
    ]))
}

/// The display name of a sample type. Mutex and block profiles share the
/// pprof sample types `contentions` and `delay`, so with a known profile kind
/// they are stored as `mutex_count`, `mutex_duration`, `block_count` and
/// `block_duration`. The goroutine profile's `goroutine` type is stored as `goroutines`.
/// Everything else keeps its pprof name.
pub(crate) fn display_profile_type(kind: Option<&str>, sample_type: &str) -> String {
    match (kind, sample_type) {
        (Some(kind @ ("mutex" | "block")), "contentions") => format!("{kind}_count"),
        (Some(kind @ ("mutex" | "block")), "delay") => format!("{kind}_duration"),
        (_, "goroutine") => "goroutines".to_string(),
        _ => sample_type.to_string(),
    }
}

/// Column builders for profile rows.
struct Rows {
    profile_ids: StringBuilder,
    trace_ids: StringBuilder,
    span_ids: StringBuilder,
    service_names: StringBuilder,
    timestamps: Vec<i64>,
    profile_types: StringBuilder,
    stack_traces: ListBuilder<StringBuilder>,
    values: Vec<i64>,
    units: StringBuilder,
    resource_attrs: StringBuilder,
    labels: StringBuilder,
}

struct Row<'a, S: AsRef<str>> {
    profile_id: &'a str,
    link: Option<&'a (TraceId, SpanId)>,
    service_name: &'a str,
    timestamp_us: i64,
    profile_type: &'a str,
    stack: &'a [S],
    value: i64,
    unit: &'a str,
    resource_attributes: Option<&'a str>,
    labels: Option<&'a str>,
}

impl Rows {
    fn new() -> Self {
        Self {
            profile_ids: StringBuilder::new(),
            trace_ids: StringBuilder::new(),
            span_ids: StringBuilder::new(),
            service_names: StringBuilder::new(),
            timestamps: Vec::new(),
            profile_types: StringBuilder::new(),
            stack_traces: ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
                "item",
                DataType::Utf8,
                true,
            ))),
            values: Vec::new(),
            units: StringBuilder::new(),
            resource_attrs: StringBuilder::new(),
            labels: StringBuilder::new(),
        }
    }

    fn push<S: AsRef<str>>(&mut self, row: Row<'_, S>) {
        self.profile_ids.append_value(row.profile_id);
        match row.link {
            Some((trace_id, span_id)) => {
                self.trace_ids.append_value(trace_id.as_ref());
                self.span_ids.append_value(span_id.as_ref());
            }
            None => {
                self.trace_ids.append_null();
                self.span_ids.append_null();
            }
        }
        self.service_names.append_value(row.service_name);
        self.timestamps.push(row.timestamp_us);
        self.profile_types.append_value(row.profile_type);
        for frame in row.stack {
            self.stack_traces.values().append_value(frame.as_ref());
        }
        self.stack_traces.append(true);
        self.values.push(row.value);
        self.units.append_value(row.unit);
        self.resource_attrs.append_option(row.resource_attributes);
        self.labels.append_option(row.labels);
    }

    fn finish(mut self) -> Result<RecordBatch, DecodeError> {
        RecordBatch::try_new(
            profile_schema(),
            vec![
                Arc::new(self.profile_ids.finish()),
                Arc::new(self.trace_ids.finish()),
                Arc::new(self.span_ids.finish()),
                Arc::new(self.service_names.finish()),
                Arc::new(TimestampMicrosecondArray::from(self.timestamps)),
                Arc::new(self.profile_types.finish()),
                Arc::new(self.stack_traces.finish()),
                Arc::new(Int64Array::from(self.values)),
                Arc::new(self.units.finish()),
                Arc::new(self.resource_attrs.finish()),
                Arc::new(self.labels.finish()),
            ],
        )
        .map_err(DecodeError::from)
    }
}

fn labels_json(map: Map<String, JsonValue>) -> Option<String> {
    if map.is_empty() {
        None
    } else {
        Some(JsonValue::Object(map).to_string())
    }
}

fn transform_profiles_request(
    request: ExportProfilesServiceRequest,
) -> Result<RecordBatch, DecodeError> {
    let mut rows = Rows::new();
    let Some(dict) = request.dictionary else {
        // No dictionary = no samples can be resolved. Return empty batch.
        tracing::debug!("profiles request has no dictionary, returning empty batch");
        return rows.finish();
    };
    let string_table = &dict.string_table;

    let resolve_str = |idx: i32| -> Result<&str, DecodeError> {
        usize::try_from(idx)
            .ok()
            .and_then(|i| string_table.get(i))
            .map(String::as_str)
            .ok_or_else(|| {
                DecodeError::Parse(format!(
                    "string_table index {idx} out of bounds (table has {} entries)",
                    string_table.len()
                ))
            })
    };

    /// Resolve a stack from sample.stack_index through the dictionary tables.
    /// Returns function names from root to leaf, expanding inlined frames,
    /// capped at MAX_FRAMES frames. Returns Err on any index out of bounds —
    /// malformed profiles are rejected entirely.
    fn resolve_stack<'a>(
        stack_index: i32,
        dict: &'a ProfilesDictionary,
        string_table: &'a [String],
    ) -> Result<Vec<std::borrow::Cow<'a, str>>, DecodeError> {
        let stack = usize::try_from(stack_index)
            .ok()
            .and_then(|i| dict.stack_table.get(i))
            .ok_or_else(|| {
                DecodeError::Parse(format!(
                    "stack_table index {stack_index} out of bounds (table has {} entries)",
                    dict.stack_table.len()
                ))
            })?;
        let mut frames = Vec::with_capacity(stack.location_indices.len().min(MAX_FRAMES));
        'locations: for (frame_pos, &loc_idx) in stack.location_indices.iter().enumerate() {
            let loc = usize::try_from(loc_idx)
                .ok()
                .and_then(|i| dict.location_table.get(i))
                .ok_or_else(|| {
                    DecodeError::Parse(format!(
                        "location_table index {loc_idx} out of bounds at stack {stack_index} frame {frame_pos} (table has {} entries)",
                        dict.location_table.len()
                    ))
                })?;
            if loc.lines.is_empty() {
                if loc.address == 0 {
                    return Err(DecodeError::Parse(format!(
                        "location index {loc_idx} has neither lines nor an address at stack {stack_index} frame {frame_pos}"
                    )));
                }
                frames.push(std::borrow::Cow::Owned(format!("0x{:x}", loc.address)));
            }
            // lines[0] is the innermost inlined function; the last line is
            // the caller it was inlined into. Leaf-first, like the stack.
            for line in &loc.lines {
                if frames.len() == MAX_FRAMES {
                    break 'locations;
                }
                let func = usize::try_from(line.function_index)
                    .ok()
                    .and_then(|i| dict.function_table.get(i))
                    .ok_or_else(|| {
                        DecodeError::Parse(format!(
                            "function_table index {} out of bounds at stack {stack_index} frame {frame_pos} (table has {} entries)",
                            line.function_index, dict.function_table.len()
                        ))
                    })?;
                let name = usize::try_from(func.name_strindex)
                    .ok()
                    .and_then(|i| string_table.get(i))
                    .ok_or_else(|| {
                        DecodeError::Parse(format!(
                            "string_table index {} out of bounds for function name at stack {stack_index} frame {frame_pos} (table has {} entries)",
                            func.name_strindex, string_table.len()
                        ))
                    })?;
                frames.push(std::borrow::Cow::Borrowed(name.as_str()));
            }
            if frames.len() >= MAX_FRAMES {
                break;
            }
        }
        frames.reverse();
        Ok(frames)
    }

    fn resolve_link(
        link_index: i32,
        dict: &ProfilesDictionary,
    ) -> Result<Option<(TraceId, SpanId)>, DecodeError> {
        if link_index == 0 {
            return Ok(None);
        }

        let link_index = usize::try_from(link_index).map_err(|_| {
            DecodeError::Parse(format!(
                "link_table index {link_index} must not be negative"
            ))
        })?;
        let link = dict.link_table.get(link_index).ok_or_else(|| {
            DecodeError::Parse(format!(
                "link_table index {link_index} out of bounds (table has {} entries)",
                dict.link_table.len()
            ))
        })?;
        let trace_id = TraceId::try_new(hex::encode(&link.trace_id)).map_err(|error| {
            DecodeError::Parse(format!(
                "invalid profile link trace_id at index {link_index}: {error}"
            ))
        })?;
        let span_id = SpanId::try_new(hex::encode(&link.span_id)).map_err(|error| {
            DecodeError::Parse(format!(
                "invalid profile link span_id at index {link_index}: {error}"
            ))
        })?;

        Ok(Some((trace_id, span_id)))
    }

    let resolve_attrs = |indices: &[i32]| -> Result<Map<String, JsonValue>, DecodeError> {
        let mut map = Map::new();
        for &idx in indices {
            let attr = usize::try_from(idx)
                .ok()
                .and_then(|i| dict.attribute_table.get(i))
                .ok_or_else(|| {
                    DecodeError::Parse(format!(
                        "attribute_table index {idx} out of bounds (table has {} entries)",
                        dict.attribute_table.len()
                    ))
                })?;
            let key = resolve_str(attr.key_strindex)?;
            if let Some(value) = attr.value.as_ref().and_then(crate::otlp::any_value_to_json) {
                map.insert(key.to_string(), value);
            }
        }
        Ok(map)
    };

    for rp in &request.resource_profiles {
        let service_name = extract_service_name(rp.resource.as_ref())
            .ok_or_else(|| DecodeError::Parse("resource_profiles entry missing service.name attribute — every profile must identify its service".into()))?;

        let res_attrs_json = rp
            .resource
            .as_ref()
            .filter(|r| !r.attributes.is_empty())
            .map(|r| crate::otlp::attrs_to_json(&r.attributes));

        for sp in &rp.scope_profiles {
            for profile in &sp.profiles {
                let ts_us = (profile.time_unix_nano / 1000) as i64;
                let pid_hex;
                let pid = if profile.profile_id.is_empty() {
                    ""
                } else {
                    pid_hex = hex::encode(&profile.profile_id);
                    pid_hex.as_str()
                };

                let profile_attrs = resolve_attrs(&profile.attribute_indices)?;
                let kind = profile_attrs
                    .get(PROFILE_NAME_ATTR)
                    .and_then(JsonValue::as_str);

                // Resolve profile_type and unit from sample_type
                let (ptype, punit) = match profile.sample_type.as_ref() {
                    Some(st) => (
                        display_profile_type(kind, resolve_str(st.type_strindex)?),
                        resolve_str(st.unit_strindex)?,
                    ),
                    None => (String::new(), ""),
                };

                for sample in &profile.samples {
                    let stack = resolve_stack(sample.stack_index, &dict, string_table)?;
                    let link = resolve_link(sample.link_index, &dict)?;
                    let labels = labels_json(resolve_attrs(&sample.attribute_indices)?);
                    // A sample with only timestamps counts one event per timestamp.
                    let timestamp_count =
                        i64::try_from(sample.timestamps_unix_nano.len()).unwrap_or(i64::MAX);
                    let sample_values = std::iter::once(timestamp_count)
                        .take(usize::from(sample.values.is_empty()))
                        .chain(sample.values.iter().copied());

                    // For multi-value profiles, write one row per value
                    for val in sample_values {
                        rows.push(Row {
                            profile_id: pid,
                            link: link.as_ref(),
                            service_name,
                            timestamp_us: ts_us,
                            profile_type: &ptype,
                            stack: &stack,
                            value: val,
                            unit: punit,
                            resource_attributes: res_attrs_json.as_deref(),
                            labels: labels.as_deref(),
                        });
                    }
                }
            }
        }
    }

    rows.finish()
}

fn extract_service_name(
    resource: Option<&crate::otel_proto::tonic::resource::v1::Resource>,
) -> Option<&str> {
    let resource = resource?;
    for attr in &resource.attributes {
        if attr.key == "service.name" {
            let val = attr.value.as_ref()?;
            let inner = val.value.as_ref()?;
            if let crate::otel_proto::tonic::common::v1::any_value::Value::StringValue(s) = inner {
                return Some(s.as_str());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_and_block_types_are_disambiguated() {
        assert_eq!(
            display_profile_type(Some("mutex"), "contentions"),
            "mutex_count"
        );
        assert_eq!(
            display_profile_type(Some("block"), "delay"),
            "block_duration"
        );
        assert_eq!(display_profile_type(None, "contentions"), "contentions");
        assert_eq!(
            display_profile_type(Some("goroutine"), "goroutine"),
            "goroutines"
        );
        assert_eq!(display_profile_type(Some("cpu"), "cpu"), "cpu");
    }
}

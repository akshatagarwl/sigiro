use std::io::Read;
use std::sync::Arc;

use arrow::array::{Int64Array, ListArray, ListBuilder, StringBuilder, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use prost::Message;

use crate::otel_proto::tonic::collector::profiles::v1development::ExportProfilesServiceRequest;
use crate::{DecodeError, SpanId, TraceId};

pub fn transform_otlp_profiles(bytes: &[u8]) -> Result<RecordBatch, DecodeError> {
    transform_profiles_request(ExportProfilesServiceRequest::decode(bytes)?)
}

/// Resolve pprofile dictionary encoding into flat profile rows as a RecordBatch.
/// Resolution chain:
///   Sample.stack_index → Dictionary.stack_table[].location_indices[]
///   → Dictionary.location_table[].line[].function_index
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
    ]))
}

fn stack_trace_builder() -> ListBuilder<StringBuilder> {
    ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        true,
    )))
}

fn append_stack_trace<S: AsRef<str>>(builder: &mut ListBuilder<StringBuilder>, stack: &[S]) {
    for frame in stack {
        builder.values().append_value(frame.as_ref());
    }
    builder.append(true);
}

fn empty_stack_trace_array() -> ListArray {
    stack_trace_builder().finish()
}

fn transform_profiles_request(
    request: ExportProfilesServiceRequest,
) -> Result<RecordBatch, DecodeError> {
    let dict = match request.dictionary {
        Some(d) => d,
        None => {
            // No dictionary = no samples can be resolved. Return empty batch.
            tracing::debug!("profiles request has no dictionary, returning empty batch");
            return RecordBatch::try_new(
                profile_schema(),
                vec![
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(empty_stack_trace_array()),
                    Arc::new(Int64Array::from(Vec::<i64>::new())),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                ],
            )
            .map_err(DecodeError::from);
        }
    };
    let string_table = &dict.string_table;

    let schema = profile_schema();

    let mut profile_ids = StringBuilder::new();
    let mut trace_ids = StringBuilder::new();
    let mut span_ids = StringBuilder::new();
    let mut service_names = StringBuilder::new();
    let mut timestamps = Vec::new();
    let mut profile_types = StringBuilder::new();
    let mut stack_traces = stack_trace_builder();
    let mut values = Vec::new();
    let mut units = StringBuilder::new();
    let mut resource_attrs = StringBuilder::new();

    let resolve_str = |idx: i32| -> Result<&str, DecodeError> {
        string_table
            .get(idx as usize)
            .map(String::as_str)
            .ok_or_else(|| {
                DecodeError::Parse(format!(
                    "string_table index {idx} out of bounds (table has {} entries)",
                    string_table.len()
                ))
            })
    };

    /// Resolve a stack from sample.stack_index through the dictionary tables.
    /// Returns function names from root to leaf, capped at 128 frames.
    /// Returns Err on any index out of bounds — malformed profiles are rejected entirely.
    fn resolve_stack<'a>(
        stack_index: i32,
        dict: &'a crate::otel_proto::tonic::profiles::v1development::ProfilesDictionary,
        string_table: &'a [String],
    ) -> Result<Vec<&'a str>, DecodeError> {
        let stack = dict.stack_table.get(stack_index as usize).ok_or_else(|| {
            DecodeError::Parse(format!(
                "stack_table index {stack_index} out of bounds (table has {} entries)",
                dict.stack_table.len()
            ))
        })?;
        let mut frames = Vec::with_capacity(stack.location_indices.len().min(128));
        for (frame_pos, &loc_idx) in stack.location_indices.iter().take(128).enumerate() {
            let loc = dict.location_table.get(loc_idx as usize).ok_or_else(|| {
                DecodeError::Parse(format!(
                    "location_table index {loc_idx} out of bounds at stack {stack_index} frame {frame_pos} (table has {} entries)",
                    dict.location_table.len()
                ))
            })?;
            let line = loc.line.first().ok_or_else(|| {
                DecodeError::Parse(format!(
                    "location index {loc_idx} has no line entries at stack {stack_index} frame {frame_pos}"
                ))
            })?;
            let func = dict.function_table.get(line.function_index as usize).ok_or_else(|| {
                DecodeError::Parse(format!(
                    "function_table index {} out of bounds at stack {stack_index} frame {frame_pos} (table has {} entries)",
                    line.function_index, dict.function_table.len()
                ))
            })?;
            let name = string_table.get(func.name_strindex as usize).ok_or_else(|| {
                DecodeError::Parse(format!(
                    "string_table index {} out of bounds for function name at stack {stack_index} frame {frame_pos} (table has {} entries)",
                    func.name_strindex, string_table.len()
                ))
            })?;
            frames.push(name.as_str());
        }
        frames.reverse();
        Ok(frames)
    }

    fn resolve_link(
        link_index: i32,
        dict: &crate::otel_proto::tonic::profiles::v1development::ProfilesDictionary,
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

    for rp in &request.resource_profiles {
        let service_name = extract_service_name(rp.resource.as_ref())
            .ok_or_else(|| DecodeError::Parse("resource_profiles entry missing service.name attribute — every profile must identify its service".into()))?;

        let res_attrs_json = if let Some(r) = &rp.resource {
            if r.attributes.is_empty() {
                None
            } else {
                Some(format!(
                    "{{{}}}",
                    r.attributes
                        .iter()
                        .map(|a| format!(
                            "\"{}\":\"{}\"",
                            a.key,
                            a.value
                                .as_ref()
                                .map(|v| format!("{v:?}"))
                                .unwrap_or_else(|| format!("<MISSING VALUE for key '{}'>", a.key))
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                ))
            }
        } else {
            None
        };

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

                // Resolve profile_type and unit from sample_type
                let (ptype, punit) = match profile.sample_type.as_ref() {
                    Some(st) => (
                        resolve_str(st.type_strindex)?,
                        resolve_str(st.unit_strindex)?,
                    ),
                    None => ("", ""),
                };

                for sample in &profile.sample {
                    let stack = resolve_stack(sample.stack_index, &dict, string_table)?;
                    let link = resolve_link(sample.link_index, &dict)?;
                    let sample_values = std::iter::once(0i64)
                        .take(usize::from(sample.values.is_empty()))
                        .chain(sample.values.iter().copied());

                    // For multi-value profiles, write one row per value
                    for val in sample_values {
                        profile_ids.append_value(pid);
                        match &link {
                            Some((trace_id, span_id)) => {
                                trace_ids.append_value(trace_id.as_ref());
                                span_ids.append_value(span_id.as_ref());
                            }
                            None => {
                                trace_ids.append_null();
                                span_ids.append_null();
                            }
                        }
                        service_names.append_value(service_name);
                        timestamps.push(ts_us);
                        profile_types.append_value(ptype);
                        append_stack_trace(&mut stack_traces, &stack);

                        values.push(val);
                        units.append_value(punit);
                        match &res_attrs_json {
                            Some(j) => resource_attrs.append_value(j),
                            None => resource_attrs.append_null(),
                        }
                    }
                }
            }
        }
    }

    if timestamps.is_empty() {
        return RecordBatch::try_new(
            schema,
            vec![
                Arc::new(profile_ids.finish()),
                Arc::new(trace_ids.finish()),
                Arc::new(span_ids.finish()),
                Arc::new(service_names.finish()),
                Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                Arc::new(profile_types.finish()),
                Arc::new(stack_traces.finish()),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(units.finish()),
                Arc::new(resource_attrs.finish()),
            ],
        )
        .map_err(DecodeError::from);
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(profile_ids.finish()),
            Arc::new(trace_ids.finish()),
            Arc::new(span_ids.finish()),
            Arc::new(service_names.finish()),
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(profile_types.finish()),
            Arc::new(stack_traces.finish()),
            Arc::new(Int64Array::from(values)),
            Arc::new(units.finish()),
            Arc::new(resource_attrs.finish()),
        ],
    )
    .map_err(DecodeError::from)
}

/// Decode either a Pyroscope PushRequest or an SDK raw pprof profile into an
/// Arrow RecordBatch using the same schema as transform_profiles.
pub fn transform_pyroscope_profiles(
    body: Vec<u8>,
    sdk_name: Option<&str>,
    max_profile_bytes: usize,
) -> Result<RecordBatch, DecodeError> {
    let decompressed = decompress_profile(body, max_profile_bytes, "Pyroscope request")?;
    tracing::debug!("pyroscope: payload {} bytes", decompressed.len());

    // Stage 2: Node sends raw pprof with the application name in the query;
    // pyroscope-rs sends a PushRequest whose series already carries service_name.
    let push_request = if let Some(name) = sdk_name {
        let service_name = name.split_once('{').map_or(name, |(base, _)| base).trim();
        if service_name.is_empty() {
            return Err(DecodeError::Parse(
                "Pyroscope SDK name must contain a non-empty application name".into(),
            ));
        }
        crate::pyroscope_proto::push::PushRequest {
            series: vec![crate::pyroscope_proto::push::RawProfileSeries {
                labels: vec![crate::pyroscope_proto::types::LabelPair {
                    name: "service_name".to_string(),
                    value: service_name.to_string(),
                }],
                samples: vec![crate::pyroscope_proto::push::RawSample {
                    raw_profile: decompressed,
                    id: String::new(),
                }],
            }],
        }
    } else {
        crate::pyroscope_proto::push::PushRequest::decode(&decompressed[..]).map_err(|e| {
            tracing::debug!("pyroscope PushRequest decode failed: {:?}", e);
            DecodeError::Parse(format!("push request decode failed: {e}"))
        })?
    };
    tracing::debug!("pyroscope: series count={}", push_request.series.len());

    let schema = profile_schema();

    let mut profile_ids = StringBuilder::new();
    let mut trace_ids = StringBuilder::new();
    let mut span_ids = StringBuilder::new();
    let mut service_names = StringBuilder::new();
    let mut timestamps = Vec::<i64>::new();
    let mut profile_types = StringBuilder::new();
    let mut stack_traces = stack_trace_builder();
    let mut values = Vec::<i64>::new();
    let mut units = StringBuilder::new();
    let mut resource_attrs = StringBuilder::new();
    let mut num_rows = 0usize;

    for series in &push_request.series {
        // Extract service.name from labels
        let service_name = series
            .labels
            .iter()
            .find(|l| l.name == "service_name")
            .map(|l| l.value.as_str())
            .unwrap_or("");

        for sample in &series.samples {
            // Decode each embedded pprof synchronously; the whole transform runs
            // in one blocking-pool task from the HTTP handler.
            let decompressed = decompress_profile(
                sample.raw_profile.clone(),
                max_profile_bytes,
                "embedded pprof profile",
            )?;
            let pprof_profile = crate::pyroscope_proto::google::Profile::decode(&decompressed[..])
                .map_err(|e| DecodeError::Parse(format!("pprof profile decode failed: {e}")))?;

            // Time and duration in nanoseconds → microseconds
            let ts_us = pprof_profile.time_nanos / 1000;

            if service_name.is_empty() {
                return Err(DecodeError::Parse(
                    "Pyroscope series is missing the service_name label".into(),
                ));
            }

            let resolve_string = |index: i64, field: &str| -> Result<&str, DecodeError> {
                let index = usize::try_from(index).map_err(|_| {
                    DecodeError::Parse(format!(
                        "pprof {field} string index {index} must not be negative"
                    ))
                })?;
                pprof_profile
                    .string_table
                    .get(index)
                    .map(String::as_str)
                    .ok_or_else(|| {
                        DecodeError::Parse(format!(
                            "pprof {field} string index {index} out of bounds (table has {} entries)",
                            pprof_profile.string_table.len()
                        ))
                    })
            };

            let sample_types = pprof_profile
                .sample_type
                .iter()
                .enumerate()
                .map(|(index, sample_type)| {
                    let profile_type =
                        resolve_string(sample_type.r#type, &format!("sample_type[{index}].type"))?;
                    let unit =
                        resolve_string(sample_type.unit, &format!("sample_type[{index}].unit"))?;
                    if profile_type.is_empty() || unit.is_empty() {
                        return Err(DecodeError::Parse(format!(
                            "pprof sample_type[{index}] must have non-empty type and unit"
                        )));
                    }
                    Ok((profile_type, unit))
                })
                .collect::<Result<Vec<_>, DecodeError>>()?;

            let locations = &pprof_profile.location;
            let functions = &pprof_profile.function;
            let string_table = &pprof_profile.string_table;

            for (sample_index, pprof_sample) in pprof_profile.sample.iter().enumerate() {
                if pprof_sample.value.len() != sample_types.len() {
                    return Err(DecodeError::Parse(format!(
                        "pprof sample[{sample_index}] has {} values but the profile declares {} sample types",
                        pprof_sample.value.len(),
                        sample_types.len()
                    )));
                }

                let mut stack = Vec::with_capacity(pprof_sample.location_id.len().min(128));
                for (frame_index, &location_id) in
                    pprof_sample.location_id.iter().take(128).enumerate()
                {
                    let location = locations
                        .iter()
                        .find(|location| location.id == location_id)
                        .ok_or_else(|| {
                            DecodeError::Parse(format!(
                                "pprof sample[{sample_index}] frame[{frame_index}] references missing location id {location_id}"
                            ))
                        })?;
                    let Some(line) = location.line.first() else {
                        stack.push(format!("0x{:x}", location.address));
                        continue;
                    };
                    let function = functions
                        .iter()
                        .find(|function| function.id == line.function_id)
                        .ok_or_else(|| {
                            DecodeError::Parse(format!(
                                "pprof location id {location_id} references missing function id {}",
                                line.function_id
                            ))
                        })?;
                    let function_name_index = usize::try_from(function.name).map_err(|_| {
                        DecodeError::Parse(format!(
                            "pprof function id {} has negative name index {}",
                            function.id, function.name
                        ))
                    })?;
                    let function_name = string_table.get(function_name_index).ok_or_else(|| {
                        DecodeError::Parse(format!(
                            "pprof function id {} name index {function_name_index} out of bounds (table has {} entries)",
                            function.id,
                            string_table.len()
                        ))
                    })?;
                    if function_name.is_empty() {
                        return Err(DecodeError::Parse(format!(
                            "pprof function id {} has an empty name",
                            function.id
                        )));
                    }
                    stack.push(function_name.clone());
                }
                // pprof stores leaf-first; Sigiro stores caller-first for folded stacks.
                stack.reverse();

                for ((profile_type, unit), &value) in sample_types.iter().zip(&pprof_sample.value) {
                    profile_ids.append_value(&sample.id);
                    trace_ids.append_null();
                    span_ids.append_null();
                    service_names.append_value(service_name);
                    timestamps.push(ts_us);
                    profile_types.append_value(profile_type);
                    append_stack_trace(&mut stack_traces, &stack);

                    values.push(value);
                    units.append_value(unit);
                    resource_attrs.append_null();
                    num_rows += 1;
                }
            }
        }
    }

    if num_rows == 0 {
        return RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(empty_stack_trace_array()),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
            ],
        )
        .map_err(DecodeError::from);
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(profile_ids.finish()),
            Arc::new(trace_ids.finish()),
            Arc::new(span_ids.finish()),
            Arc::new(service_names.finish()),
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(profile_types.finish()),
            Arc::new(stack_traces.finish()),
            Arc::new(Int64Array::from(values)),
            Arc::new(units.finish()),
            Arc::new(resource_attrs.finish()),
        ],
    )
    .map_err(DecodeError::from)
}

fn decompress_profile(
    bytes: Vec<u8>,
    max_profile_bytes: usize,
    description: &str,
) -> Result<Vec<u8>, DecodeError> {
    if bytes.len() > max_profile_bytes {
        return Err(DecodeError::TooLarge(format!(
            "{description} exceeds the {max_profile_bytes}-byte ingest limit"
        )));
    }
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Ok(bytes);
    }

    let read_limit = u64::try_from(max_profile_bytes)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or_else(|| {
            DecodeError::Parse("ingest body limit cannot be represented as u64".into())
        })?;
    let mut decompressed = Vec::new();
    flate2::read::GzDecoder::new(&bytes[..])
        .take(read_limit)
        .read_to_end(&mut decompressed)
        .map_err(|e| DecodeError::Parse(format!("{description} gzip is invalid: {e}")))?;
    if decompressed.len() > max_profile_bytes {
        return Err(DecodeError::TooLarge(format!(
            "{description} exceeds the {max_profile_bytes}-byte ingest limit after decompression"
        )));
    }
    Ok(decompressed)
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

//! Zero-copy OTLP to Arrow transformation.
//!
//! Replaces the vendored otlp2records crate with direct builder append —
//! no VRL interpreter, no intermediate representation.

use std::sync::Arc;

use arrow::array::{
    Array, Float64Builder, Int32Builder, Int64Builder, StringBuilder, TimestampMicrosecondBuilder,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::error::ArrowError;
use arrow::record_batch::RecordBatch;
use serde_json::Value as JsonValue;

// ============================================================================
// Public types
// ============================================================================

/// Input format for OTLP decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputFormat {
    Protobuf,
    Json,
}

/// Errors that can occur during OTLP decoding.
#[derive(Debug)]
pub enum DecodeError {
    Protobuf(prost::DecodeError),
    Json(serde_json::Error),
    Parse(String),
    Unsupported(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Protobuf(e) => write!(f, "protobuf decode error: {e}"),
            DecodeError::Json(e) => write!(f, "json decode error: {e}"),
            DecodeError::Parse(msg) => write!(f, "parse error: {msg}"),
            DecodeError::Unsupported(msg) => write!(f, "unsupported payload: {msg}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::Protobuf(e) => Some(e),
            DecodeError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<prost::DecodeError> for DecodeError {
    fn from(e: prost::DecodeError) -> Self {
        DecodeError::Protobuf(e)
    }
}

impl From<serde_json::Error> for DecodeError {
    fn from(e: serde_json::Error) -> Self {
        DecodeError::Json(e)
    }
}

/// Result of transforming OTLP metrics to Arrow RecordBatches.
#[derive(Debug)]
pub struct MetricBatches {
    pub gauge: Option<RecordBatch>,
    pub sum: Option<RecordBatch>,
    pub histogram: Option<RecordBatch>,
    pub exp_histogram: Option<RecordBatch>,
    pub skipped: SkippedMetrics,
}

/// Metrics that were skipped during processing.
#[derive(Debug, Default)]
pub struct SkippedMetrics {
    pub summaries: usize,
    pub nan_values: usize,
    pub infinity_values: usize,
    pub missing_values: usize,
}

impl SkippedMetrics {
    pub fn has_skipped(&self) -> bool {
        self.summaries > 0 || self.nan_values > 0 || self.infinity_values > 0 || self.missing_values > 0
    }
}

// ============================================================================
// Trace direct builders
// ============================================================================

/// Arrow builders for the flat trace span schema.
pub struct TraceBuilders {
    timestamp: TimestampMicrosecondBuilder,
    end_timestamp: Int64Builder,
    duration: Int64Builder,
    trace_id: StringBuilder,
    span_id: StringBuilder,
    parent_span_id: StringBuilder,
    trace_state: StringBuilder,
    service_name: StringBuilder,
    service_namespace: StringBuilder,
    service_instance_id: StringBuilder,
    span_name: StringBuilder,
    span_kind: Int32Builder,
    status_code: Int32Builder,
    status_message: StringBuilder,
    resource_attributes: StringBuilder,
    scope_name: StringBuilder,
    scope_version: StringBuilder,
    scope_attributes: StringBuilder,
    span_attributes: StringBuilder,
    events_json: StringBuilder,
    links_json: StringBuilder,
    dropped_attributes_count: Int32Builder,
    dropped_events_count: Int32Builder,
    dropped_links_count: Int32Builder,
    flags: Int32Builder,
}

impl TraceBuilders {
    pub fn new() -> Self {
        Self {
            timestamp: TimestampMicrosecondBuilder::new(),
            end_timestamp: Int64Builder::new(),
            duration: Int64Builder::new(),
            trace_id: StringBuilder::new(),
            span_id: StringBuilder::new(),
            parent_span_id: StringBuilder::new(),
            trace_state: StringBuilder::new(),
            service_name: StringBuilder::new(),
            service_namespace: StringBuilder::new(),
            service_instance_id: StringBuilder::new(),
            span_name: StringBuilder::new(),
            span_kind: Int32Builder::new(),
            status_code: Int32Builder::new(),
            status_message: StringBuilder::new(),
            resource_attributes: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            scope_version: StringBuilder::new(),
            scope_attributes: StringBuilder::new(),
            span_attributes: StringBuilder::new(),
            events_json: StringBuilder::new(),
            links_json: StringBuilder::new(),
            dropped_attributes_count: Int32Builder::new(),
            dropped_events_count: Int32Builder::new(),
            dropped_links_count: Int32Builder::new(),
            flags: Int32Builder::new(),
        }
    }

    pub fn append_span(
        &mut self,
        span: &opentelemetry_proto::tonic::trace::v1::Span,
        resource: Option<&opentelemetry_proto::tonic::resource::v1::Resource>,
        scope: Option<&opentelemetry_proto::tonic::common::v1::InstrumentationScope>,
    ) -> Result<(), DecodeError> {
        let start = i64::try_from(span.start_time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!(
                "timestamp overflow: start_time_unix_nano {} exceeds i64::MAX",
                span.start_time_unix_nano
            ))
        })?;
        let end = i64::try_from(span.end_time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!(
                "timestamp overflow: end_time_unix_nano {} exceeds i64::MAX",
                span.end_time_unix_nano
            ))
        })?;

        self.timestamp.append_value(start / 1000);
        self.end_timestamp.append_value(end / 1_000_000);

        let dur = end.saturating_sub(start);
        if dur < 0 {
            return Err(DecodeError::Parse(
                "span end_time must be >= start_time (negative duration detected)".into(),
            ));
        }
        self.duration.append_value(dur / 1_000_000);

        let trace_id = hex::encode(&span.trace_id);
        let span_id = hex::encode(&span.span_id);
        let parent_span_id = hex::encode(&span.parent_span_id);

        if trace_id.is_empty() {
            return Err(DecodeError::Parse(
                "trace_id is empty — every span must have a valid trace ID".into(),
            ));
        }
        if span_id.is_empty() {
            return Err(DecodeError::Parse(
                "span_id is empty — every span must have a valid span ID".into(),
            ));
        }

        self.trace_id.append_value(&trace_id);
        self.span_id.append_value(&span_id);
        if parent_span_id.is_empty() {
            self.parent_span_id.append_null();
        } else {
            self.parent_span_id.append_value(&parent_span_id);
        }

        if span.trace_state.is_empty() {
            self.trace_state.append_null();
        } else {
            self.trace_state.append_value(&span.trace_state);
        }

        let service_name = extract_string_attr(resource, "service.name").unwrap_or_default();
        if service_name.is_empty()
            || service_name == "unknown"
            || service_name == "unknown_service"
            || service_name.starts_with("unknown_service:")
        {
            return Err(DecodeError::Parse(
                "span with unusable service_name detected — set OTEL_SERVICE_NAME (or service.name resource attribute) on the sending application".into(),
            ));
        }
        self.service_name.append_value(&service_name);
        self.service_namespace
            .append_option(extract_string_attr(resource, "service.namespace"));
        self.service_instance_id
            .append_option(extract_string_attr(resource, "service.instance.id"));

        if span.name.is_empty() {
            return Err(DecodeError::Parse(
                "span name is empty — every span must have a name describing the operation".into(),
            ));
        }
        self.span_name.append_value(&span.name);
        self.span_kind.append_value(span.kind);

        let (code, message) = span
            .status
            .as_ref()
            .map(|s| (s.code, s.message.clone()))
            .unwrap_or((0, String::new()));
        self.status_code.append_value(code);
        if message.is_empty() {
            self.status_message.append_null();
        } else {
            self.status_message.append_value(&message);
        }

        self.resource_attributes
            .append_value(attrs_to_json(resource.map(|r| &r.attributes[..]).unwrap_or(&[])));
        self.scope_name
            .append_option(scope.map(|s| s.name.as_str()));
        self.scope_version
            .append_option(scope.map(|s| s.version.as_str()));
        self.scope_attributes
            .append_value(attrs_to_json(scope.map(|s| &s.attributes[..]).unwrap_or(&[])));
        self.span_attributes
            .append_value(attrs_to_json(&span.attributes));

        self.events_json.append_value(events_to_json(&span.events));
        self.links_json.append_value(links_to_json(&span.links));

        self.dropped_attributes_count
            .append_value(span.dropped_attributes_count as i32);
        self.dropped_events_count
            .append_value(span.dropped_events_count as i32);
        self.dropped_links_count
            .append_value(span.dropped_links_count as i32);
        self.flags.append_value(span.flags as i32);

        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch, ArrowError> {
        let schema = traces_schema();
        let arrays: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(self.timestamp.finish()),
            Arc::new(self.end_timestamp.finish()),
            Arc::new(self.duration.finish()),
            Arc::new(self.trace_id.finish()),
            Arc::new(self.span_id.finish()),
            Arc::new(self.parent_span_id.finish()),
            Arc::new(self.trace_state.finish()),
            Arc::new(self.service_name.finish()),
            Arc::new(self.service_namespace.finish()),
            Arc::new(self.service_instance_id.finish()),
            Arc::new(self.span_name.finish()),
            Arc::new(self.span_kind.finish()),
            Arc::new(self.status_code.finish()),
            Arc::new(self.status_message.finish()),
            Arc::new(self.resource_attributes.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.scope_version.finish()),
            Arc::new(self.scope_attributes.finish()),
            Arc::new(self.span_attributes.finish()),
            Arc::new(self.events_json.finish()),
            Arc::new(self.links_json.finish()),
            Arc::new(self.dropped_attributes_count.finish()),
            Arc::new(self.dropped_events_count.finish()),
            Arc::new(self.dropped_links_count.finish()),
            Arc::new(self.flags.finish()),
        ];
        RecordBatch::try_new(Arc::new(schema), arrays)
    }
}

impl Default for TraceBuilders {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Public decode functions
// ============================================================================

pub fn transform_traces(bytes: &[u8], format: InputFormat) -> Result<RecordBatch, DecodeError> {
    match format {
        InputFormat::Protobuf => decode_traces_protobuf(bytes),
        InputFormat::Json => decode_traces_json(bytes),
    }
}

pub fn transform_logs(bytes: &[u8], format: InputFormat) -> Result<RecordBatch, DecodeError> {
    match format {
        InputFormat::Protobuf => decode_logs_protobuf(bytes),
        InputFormat::Json => decode_logs_json(bytes),
    }
}

pub fn transform_metrics(bytes: &[u8], format: InputFormat) -> Result<MetricBatches, DecodeError> {
    match format {
        InputFormat::Protobuf => decode_metrics_protobuf(bytes),
        InputFormat::Json => decode_metrics_json(bytes),
    }
}

fn decode_traces_protobuf(bytes: &[u8]) -> Result<RecordBatch, DecodeError> {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use prost::Message;

    let request = ExportTraceServiceRequest::decode(bytes)?;
    let mut builders = TraceBuilders::new();

    for resource_spans in request.resource_spans {
        let resource = resource_spans.resource.as_ref();
        for scope_spans in resource_spans.scope_spans {
            let scope = scope_spans.scope.as_ref();
            for span in scope_spans.spans {
                builders.append_span(&span, resource, scope)?;
            }
        }
    }

    builders.finish().map_err(|e| DecodeError::Parse(e.to_string()))
}

fn decode_traces_json(bytes: &[u8]) -> Result<RecordBatch, DecodeError> {
    let value: JsonValue = serde_json::from_slice(bytes)?;

    let resource_spans = value
        .get("resourceSpans")
        .or_else(|| value.get("resource_spans"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut builders = TraceBuilders::new();

    for rs in &resource_spans {
        let resource = rs.get("resource").unwrap_or(&JsonValue::Null);
        let scope_spans = rs
            .get("scopeSpans")
            .or_else(|| rs.get("scope_spans"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for ss in &scope_spans {
            let scope = ss.get("scope").unwrap_or(&JsonValue::Null);
            let spans = ss
                .get("spans")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for span in &spans {
                append_span_json(&mut builders, span, resource, scope)?;
            }
        }
    }

    builders.finish().map_err(|e| DecodeError::Parse(e.to_string()))
}

fn decode_logs_protobuf(bytes: &[u8]) -> Result<RecordBatch, DecodeError> {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use prost::Message;

    let request = ExportLogsServiceRequest::decode(bytes)?;
    let mut builders = LogBuilders::new();

    for resource_logs in request.resource_logs {
        let resource = resource_logs.resource.as_ref();
        for scope_logs in resource_logs.scope_logs {
            let scope = scope_logs.scope.as_ref();
            for log in scope_logs.log_records {
                builders.append_log(&log, resource, scope)?;
            }
        }
    }

    builders.finish().map_err(|e| DecodeError::Parse(e.to_string()))
}

fn decode_logs_json(bytes: &[u8]) -> Result<RecordBatch, DecodeError> {
    let value: JsonValue = serde_json::from_slice(bytes)?;

    let resource_logs = value
        .get("resourceLogs")
        .or_else(|| value.get("resource_logs"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut builders = LogBuilders::new();

    for rl in &resource_logs {
        let resource = rl.get("resource").unwrap_or(&JsonValue::Null);
        let scope_logs = rl
            .get("scopeLogs")
            .or_else(|| rl.get("scope_logs"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for sl in &scope_logs {
            let scope = sl.get("scope").unwrap_or(&JsonValue::Null);
            let logs = sl
                .get("logRecords")
                .or_else(|| sl.get("log_records"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for log in &logs {
                append_log_json(&mut builders, log, resource, scope)?;
            }
        }
    }

    builders.finish().map_err(|e| DecodeError::Parse(e.to_string()))
}

fn decode_metrics_protobuf(bytes: &[u8]) -> Result<MetricBatches, DecodeError> {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;
    use prost::Message;

    let request = ExportMetricsServiceRequest::decode(bytes)?;
    let mut gauge = GaugeBuilders::new();
    let mut sum = SumBuilders::new();
    let mut histogram = HistogramBuilders::new();
    let mut exp_histogram = ExpHistogramBuilders::new();
    let mut skipped = SkippedMetrics::default();

    for resource_metrics in request.resource_metrics {
        let resource = resource_metrics.resource.as_ref();
        for scope_metrics in resource_metrics.scope_metrics {
            let scope = scope_metrics.scope.as_ref();
            for metric in scope_metrics.metrics {
                let ctx = MetricContext {
                    name: &metric.name,
                    description: &metric.description,
                    unit: &metric.unit,
                    resource,
                    scope,
                };

                match metric.data {
                    Some(Data::Gauge(g)) => {
                        for point in g.data_points {
                            if !gauge.append(&point, &ctx)? {
                                track_skipped_value(&point.value, &mut skipped);
                            }
                        }
                    }
                    Some(Data::Sum(s)) => {
                        let temporality = s.aggregation_temporality;
                        let monotonic = s.is_monotonic;
                        for point in s.data_points {
                            if !sum.append(&point, &ctx, temporality, monotonic)? {
                                track_skipped_value(&point.value, &mut skipped);
                            }
                        }
                    }
                    Some(Data::Histogram(h)) => {
                        let temporality = h.aggregation_temporality;
                        for point in h.data_points {
                            histogram.append(&point, &ctx, temporality)?;
                        }
                    }
                    Some(Data::ExponentialHistogram(eh)) => {
                        let temporality = eh.aggregation_temporality;
                        for point in eh.data_points {
                            exp_histogram.append(&point, &ctx, temporality)?;
                        }
                    }
                    Some(Data::Summary(s)) => {
                        skipped.summaries += s.data_points.len();
                    }
                    None => {}
                }
            }
        }
    }

    Ok(MetricBatches {
        gauge: gauge.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        sum: sum.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        histogram: histogram.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        exp_histogram: exp_histogram.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        skipped,
    })
}

fn decode_metrics_json(bytes: &[u8]) -> Result<MetricBatches, DecodeError> {
    let value: JsonValue = serde_json::from_slice(bytes)?;

    let resource_metrics = value
        .get("resourceMetrics")
        .or_else(|| value.get("resource_metrics"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut gauge = GaugeBuilders::new();
    let mut sum = SumBuilders::new();
    let mut histogram = HistogramBuilders::new();
    let mut exp_histogram = ExpHistogramBuilders::new();
    let mut skipped = SkippedMetrics::default();

    for rm in &resource_metrics {
        let resource = rm.get("resource").unwrap_or(&JsonValue::Null);
        let scope_metrics = rm
            .get("scopeMetrics")
            .or_else(|| rm.get("scope_metrics"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for sm in &scope_metrics {
            let scope = sm.get("scope").unwrap_or(&JsonValue::Null);
            let metrics = sm
                .get("metrics")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for metric in &metrics {
                let name = metric.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let description = metric.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let unit = metric.get("unit").and_then(|v| v.as_str()).unwrap_or("");

                if let Some(g) = metric.get("gauge") {
                    append_gauge_json(&mut gauge, g, resource, scope, name, description, unit, &mut skipped)?;
                }
                if let Some(s) = metric.get("sum") {
                    append_sum_json(&mut sum, s, resource, scope, name, description, unit, &mut skipped)?;
                }
                if let Some(h) = metric.get("histogram") {
                    append_histogram_json(&mut histogram, h, resource, scope, name, description, unit)?;
                }
                if let Some(eh) = metric.get("exponentialHistogram") {
                    append_exp_histogram_json(&mut exp_histogram, eh, resource, scope, name, description, unit)?;
                }
                if metric.get("summary").is_some() {
                    if let Some(summary) = metric.get("summary") {
                        if let Some(points) = summary.get("dataPoints").or_else(|| summary.get("data_points")).and_then(|v| v.as_array()) {
                            skipped.summaries += points.len();
                        }
                    }
                }
            }
        }
    }

    Ok(MetricBatches {
        gauge: gauge.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        sum: sum.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        histogram: histogram.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        exp_histogram: exp_histogram.finish().map_err(|e| DecodeError::Parse(e.to_string()))?,
        skipped,
    })
}

fn track_skipped_value(
    value: &Option<opentelemetry_proto::tonic::metrics::v1::number_data_point::Value>,
    skipped: &mut SkippedMetrics,
) {
    match value {
        Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d)) if d.is_nan() => skipped.nan_values += 1,
        Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d)) if d.is_infinite() => skipped.infinity_values += 1,
        None => skipped.missing_values += 1,
        _ => {}
    }
}

// ============================================================================
// Schema
// ============================================================================

fn traces_schema() -> Schema {
    Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None), false),
        Field::new("end_timestamp", DataType::Int64, false),
        Field::new("duration", DataType::Int64, false),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("parent_span_id", DataType::Utf8, true),
        Field::new("trace_state", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("span_name", DataType::Utf8, false),
        Field::new("span_kind", DataType::Int32, false),
        Field::new("status_code", DataType::Int32, false),
        Field::new("status_message", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("span_attributes", DataType::Utf8, true),
        Field::new("events_json", DataType::Utf8, true),
        Field::new("links_json", DataType::Utf8, true),
        Field::new("dropped_attributes_count", DataType::Int32, true),
        Field::new("dropped_events_count", DataType::Int32, true),
        Field::new("dropped_links_count", DataType::Int32, true),
        Field::new("flags", DataType::Int32, true),
    ])
}

fn logs_schema() -> Schema {
    Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None), false),
        Field::new("observed_timestamp", DataType::Int64, false),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("severity_number", DataType::Int32, false),
        Field::new("severity_text", DataType::Utf8, false),
        Field::new("body", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("log_attributes", DataType::Utf8, true),
    ])
}

fn gauge_schema() -> Schema {
    Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None), false),
        Field::new("start_timestamp", DataType::Int64, false),
        Field::new("metric_name", DataType::Utf8, false),
        Field::new("metric_description", DataType::Utf8, false),
        Field::new("metric_unit", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("metric_attributes", DataType::Utf8, true),
        Field::new("flags", DataType::Int32, true),
        Field::new("exemplars_json", DataType::Utf8, true),
    ])
}

fn sum_schema() -> Schema {
    Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None), false),
        Field::new("start_timestamp", DataType::Int64, false),
        Field::new("metric_name", DataType::Utf8, false),
        Field::new("metric_description", DataType::Utf8, false),
        Field::new("metric_unit", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("metric_attributes", DataType::Utf8, true),
        Field::new("flags", DataType::Int32, true),
        Field::new("exemplars_json", DataType::Utf8, true),
        Field::new("aggregation_temporality", DataType::Int32, false),
        Field::new("is_monotonic", DataType::Boolean, false),
    ])
}

fn histogram_schema() -> Schema {
    Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None), false),
        Field::new("start_timestamp", DataType::Int64, false),
        Field::new("metric_name", DataType::Utf8, false),
        Field::new("metric_description", DataType::Utf8, false),
        Field::new("metric_unit", DataType::Utf8, false),
        Field::new("count", DataType::Int64, false),
        Field::new("sum", DataType::Float64, true),
        Field::new("min", DataType::Float64, true),
        Field::new("max", DataType::Float64, true),
        Field::new("bucket_counts", DataType::Utf8, false),
        Field::new("explicit_bounds", DataType::Utf8, false),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("metric_attributes", DataType::Utf8, true),
        Field::new("flags", DataType::Int32, true),
        Field::new("exemplars_json", DataType::Utf8, true),
        Field::new("aggregation_temporality", DataType::Int32, false),
    ])
}

fn exp_histogram_schema() -> Schema {
    Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None), false),
        Field::new("start_timestamp", DataType::Int64, false),
        Field::new("metric_name", DataType::Utf8, false),
        Field::new("metric_description", DataType::Utf8, false),
        Field::new("metric_unit", DataType::Utf8, false),
        Field::new("count", DataType::Int64, false),
        Field::new("sum", DataType::Float64, true),
        Field::new("min", DataType::Float64, true),
        Field::new("max", DataType::Float64, true),
        Field::new("scale", DataType::Int32, false),
        Field::new("zero_count", DataType::Int64, false),
        Field::new("zero_threshold", DataType::Float64, false),
        Field::new("positive_offset", DataType::Int32, true),
        Field::new("positive_bucket_counts", DataType::Utf8, false),
        Field::new("negative_offset", DataType::Int32, true),
        Field::new("negative_bucket_counts", DataType::Utf8, false),
        Field::new("service_name", DataType::Utf8, false),
        Field::new("service_namespace", DataType::Utf8, true),
        Field::new("service_instance_id", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        Field::new("scope_attributes", DataType::Utf8, true),
        Field::new("metric_attributes", DataType::Utf8, true),
        Field::new("flags", DataType::Int32, true),
        Field::new("exemplars_json", DataType::Utf8, true),
        Field::new("aggregation_temporality", DataType::Int32, false),
    ])
}

// ============================================================================
// JSON append
// ============================================================================

fn append_span_json(
    builders: &mut TraceBuilders,
    span: &JsonValue,
    resource: &JsonValue,
    scope: &JsonValue,
) -> Result<(), DecodeError> {
    let get_i64 = |obj: &JsonValue, key: &str| -> Option<i64> {
        json_i64(obj.get(key)?)
    };
    let get_str = |obj: &JsonValue, key: &str| -> Option<String> {
        obj.get(key)?.as_str().map(std::string::ToString::to_string)
    };

    let start = get_i64(span, "startTimeUnixNano")
        .or_else(|| get_i64(span, "start_time_unix_nano"))
        .unwrap_or(0);
    let end = get_i64(span, "endTimeUnixNano")
        .or_else(|| get_i64(span, "end_time_unix_nano"))
        .unwrap_or(0);

    builders.timestamp.append_value(start / 1000);
    builders.end_timestamp.append_value(end / 1_000_000);

    let dur = end.saturating_sub(start);
    if dur < 0 {
        return Err(DecodeError::Parse(
            "span end_time must be >= start_time (negative duration detected)".into(),
        ));
    }
    builders.duration.append_value(dur / 1_000_000);

    let trace_id = get_str(span, "traceId")
        .or_else(|| get_str(span, "trace_id"))
        .unwrap_or_default();
    let span_id = get_str(span, "spanId")
        .or_else(|| get_str(span, "span_id"))
        .unwrap_or_default();
    let parent_span_id = get_str(span, "parentSpanId")
        .or_else(|| get_str(span, "parent_span_id"))
        .unwrap_or_default();

    if trace_id.is_empty() {
        return Err(DecodeError::Parse(
            "trace_id is empty — every span must have a valid trace ID".into(),
        ));
    }
    if span_id.is_empty() {
        return Err(DecodeError::Parse(
            "span_id is empty — every span must have a valid span ID".into(),
        ));
    }

    builders.trace_id.append_value(&trace_id);
    builders.span_id.append_value(&span_id);
    if parent_span_id.is_empty() {
        builders.parent_span_id.append_null();
    } else {
        builders.parent_span_id.append_value(&parent_span_id);
    }

    let trace_state = get_str(span, "traceState")
        .or_else(|| get_str(span, "trace_state"))
        .unwrap_or_default();
    if trace_state.is_empty() {
        builders.trace_state.append_null();
    } else {
        builders.trace_state.append_value(&trace_state);
    }

    let service_name = extract_json_string_attr(resource, "service.name").unwrap_or_default();
    if service_name.is_empty()
        || service_name == "unknown"
        || service_name == "unknown_service"
        || service_name.starts_with("unknown_service:")
    {
        return Err(DecodeError::Parse(
            "span with unusable service_name detected — set OTEL_SERVICE_NAME (or service.name resource attribute) on the sending application".into(),
        ));
    }
    builders.service_name.append_value(&service_name);
    builders.service_namespace
        .append_option(extract_json_string_attr(resource, "service.namespace"));
    builders.service_instance_id
        .append_option(extract_json_string_attr(resource, "service.instance.id"));

    let name = get_str(span, "name").unwrap_or_default();
    if name.is_empty() {
        return Err(DecodeError::Parse(
            "span name is empty — every span must have a name describing the operation".into(),
        ));
    }
    builders.span_name.append_value(&name);

    let kind = get_i64(span, "kind").unwrap_or(0) as i32;
    builders.span_kind.append_value(kind);

    let status = span.get("status").unwrap_or(&JsonValue::Null);
    let code = get_i64(status, "code").unwrap_or(0) as i32;
    let message = get_str(status, "message").unwrap_or_default();
    builders.status_code.append_value(code);
    if message.is_empty() {
        builders.status_message.append_null();
    } else {
        builders.status_message.append_value(&message);
    }

    builders.resource_attributes
        .append_value(json_attrs_to_string(resource.get("attributes")));
    builders.scope_name
        .append_option(scope.get("name").and_then(|v| v.as_str()));
    builders.scope_version
        .append_option(scope.get("version").and_then(|v| v.as_str()));
    builders.scope_attributes
        .append_value(json_attrs_to_string(scope.get("attributes")));
    builders.span_attributes
        .append_value(json_attrs_to_string(span.get("attributes")));

    builders.events_json
        .append_value(json_events_to_string(span.get("events")));
    builders.links_json
        .append_value(json_links_to_string(span.get("links")));

    let dropped_attrs = get_i64(span, "droppedAttributesCount")
        .or_else(|| get_i64(span, "dropped_attributes_count"))
        .unwrap_or(0) as i32;
    let dropped_events = get_i64(span, "droppedEventsCount")
        .or_else(|| get_i64(span, "dropped_events_count"))
        .unwrap_or(0) as i32;
    let dropped_links = get_i64(span, "droppedLinksCount")
        .or_else(|| get_i64(span, "dropped_links_count"))
        .unwrap_or(0) as i32;
    let flags = get_i64(span, "flags").unwrap_or(0) as i32;

    builders.dropped_attributes_count.append_value(dropped_attrs);
    builders.dropped_events_count.append_value(dropped_events);
    builders.dropped_links_count.append_value(dropped_links);
    builders.flags.append_value(flags);

    Ok(())
}

fn append_log_json(
    builders: &mut LogBuilders,
    log: &JsonValue,
    resource: &JsonValue,
    scope: &JsonValue,
) -> Result<(), DecodeError> {
    let get_i64 = |obj: &JsonValue, key: &str| -> Option<i64> {
        json_i64(obj.get(key)?)
    };
    let get_str = |obj: &JsonValue, key: &str| -> Option<String> {
        obj.get(key)?.as_str().map(std::string::ToString::to_string)
    };

    let time = get_i64(log, "timeUnixNano")
        .or_else(|| get_i64(log, "time_unix_nano"))
        .unwrap_or(0);
    let observed = get_i64(log, "observedTimeUnixNano")
        .or_else(|| get_i64(log, "observed_time_unix_nano"))
        .unwrap_or(0);

    builders.timestamp.append_value(time / 1000);
    builders.observed_timestamp.append_value(observed / 1000);

    let trace_id = get_str(log, "traceId")
        .or_else(|| get_str(log, "trace_id"))
        .unwrap_or_default();
    let span_id = get_str(log, "spanId")
        .or_else(|| get_str(log, "span_id"))
        .unwrap_or_default();
    if trace_id.is_empty() {
        builders.trace_id.append_null();
    } else {
        builders.trace_id.append_value(&trace_id);
    }
    if span_id.is_empty() {
        builders.span_id.append_null();
    } else {
        builders.span_id.append_value(&span_id);
    }

    let service_name = extract_json_string_attr(resource, "service.name").unwrap_or_default();
    if service_name.is_empty()
        || service_name == "unknown"
        || service_name == "unknown_service"
        || service_name.starts_with("unknown_service:")
    {
        return Err(DecodeError::Parse(
            "log with unusable service_name detected — set OTEL_SERVICE_NAME (or service.name resource attribute) on the sending application".into(),
        ));
    }
    builders.service_name.append_value(&service_name);
    builders.service_namespace
        .append_option(extract_json_string_attr(resource, "service.namespace"));
    builders.service_instance_id
        .append_option(extract_json_string_attr(resource, "service.instance.id"));

    let severity_number = get_i64(log, "severityNumber")
        .or_else(|| get_i64(log, "severity_number"))
        .unwrap_or(0) as i32;
    let severity_text = get_str(log, "severityText")
        .or_else(|| get_str(log, "severity_text"))
        .unwrap_or_default();
    builders.severity_number.append_value(severity_number);
    builders.severity_text.append_value(&severity_text);

    // Body
    let body = log.get("body");
    let body_str = match body {
        Some(JsonValue::Object(map)) if map.contains_key("stringValue") || map.contains_key("string_value") => {
            map.get("stringValue")
                .or_else(|| map.get("string_value"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
        None => String::new(),
    };
    builders.body.append_value(&body_str);

    builders.resource_attributes
        .append_value(json_attrs_to_string(resource.get("attributes")));
    builders.scope_name
        .append_option(scope.get("name").and_then(|v| v.as_str()));
    builders.scope_version
        .append_option(scope.get("version").and_then(|v| v.as_str()));
    builders.scope_attributes
        .append_value(json_attrs_to_string(scope.get("attributes")));
    builders.log_attributes
        .append_value(json_attrs_to_string(log.get("attributes")));

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_gauge_json(
    builders: &mut GaugeBuilders,
    gauge: &JsonValue,
    resource: &JsonValue,
    scope: &JsonValue,
    name: &str,
    description: &str,
    unit: &str,
    skipped: &mut SkippedMetrics,
) -> Result<(), DecodeError> {
    let points = gauge
        .get("dataPoints")
        .or_else(|| gauge.get("data_points"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for point in &points {
        let time = point.get("timeUnixNano")
            .or_else(|| point.get("time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);
        let start = point.get("startTimeUnixNano")
            .or_else(|| point.get("start_time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);

        let val = if let Some(d) = point.get("asDouble").or_else(|| point.get("as_double")).and_then(json_f64) {
            if d.is_nan() || d.is_infinite() {
                skipped.nan_values += 1;
                continue;
            }
            d
        } else if let Some(i) = point.get("asInt").or_else(|| point.get("as_int")).and_then(json_i64) {
            i as f64
        } else {
            skipped.missing_values += 1;
            continue;
        };

        builders.timestamp.append_value(time / 1000);
        builders.start_timestamp.append_value(start / 1000);
        builders.metric_name.append_value(name);
        builders.metric_description.append_value(description);
        builders.metric_unit.append_value(unit);
        builders.value.append_value(val);

        let service_name = extract_json_string_attr(resource, "service.name").unwrap_or_default();
        builders.service_name.append_value(&service_name);
        builders.service_namespace
            .append_option(extract_json_string_attr(resource, "service.namespace"));
        builders.service_instance_id
            .append_option(extract_json_string_attr(resource, "service.instance.id"));

        builders.resource_attributes
            .append_value(json_attrs_to_string(resource.get("attributes")));
        builders.scope_name
            .append_option(scope.get("name").and_then(|v| v.as_str()));
        builders.scope_version
            .append_option(scope.get("version").and_then(|v| v.as_str()));
        builders.scope_attributes
            .append_value(json_attrs_to_string(scope.get("attributes")));
        builders.metric_attributes
            .append_value(json_attrs_to_string(point.get("attributes")));
        builders.flags.append_value(
            point.get("flags").and_then(json_i64).unwrap_or(0) as i32
        );
        builders.exemplars_json.append_value("[]");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_sum_json(
    builders: &mut SumBuilders,
    sum: &JsonValue,
    resource: &JsonValue,
    scope: &JsonValue,
    name: &str,
    description: &str,
    unit: &str,
    skipped: &mut SkippedMetrics,
) -> Result<(), DecodeError> {
    let temporality = sum
        .get("aggregationTemporality")
        .or_else(|| sum.get("aggregation_temporality"))
        .and_then(json_i64)
        .unwrap_or(0) as i32;
    let monotonic = sum
        .get("isMonotonic")
        .or_else(|| sum.get("is_monotonic"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let points = sum
        .get("dataPoints")
        .or_else(|| sum.get("data_points"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for point in &points {
        let time = point.get("timeUnixNano")
            .or_else(|| point.get("time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);
        let start = point.get("startTimeUnixNano")
            .or_else(|| point.get("start_time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);

        let val = if let Some(d) = point.get("asDouble").or_else(|| point.get("as_double")).and_then(json_f64) {
            if d.is_nan() || d.is_infinite() {
                skipped.nan_values += 1;
                continue;
            }
            d
        } else if let Some(i) = point.get("asInt").or_else(|| point.get("as_int")).and_then(json_i64) {
            i as f64
        } else {
            skipped.missing_values += 1;
            continue;
        };

        builders.timestamp.append_value(time / 1000);
        builders.start_timestamp.append_value(start / 1000);
        builders.metric_name.append_value(name);
        builders.metric_description.append_value(description);
        builders.metric_unit.append_value(unit);
        builders.value.append_value(val);

        let service_name = extract_json_string_attr(resource, "service.name").unwrap_or_default();
        builders.service_name.append_value(&service_name);
        builders.service_namespace
            .append_option(extract_json_string_attr(resource, "service.namespace"));
        builders.service_instance_id
            .append_option(extract_json_string_attr(resource, "service.instance.id"));

        builders.resource_attributes
            .append_value(json_attrs_to_string(resource.get("attributes")));
        builders.scope_name
            .append_option(scope.get("name").and_then(|v| v.as_str()));
        builders.scope_version
            .append_option(scope.get("version").and_then(|v| v.as_str()));
        builders.scope_attributes
            .append_value(json_attrs_to_string(scope.get("attributes")));
        builders.metric_attributes
            .append_value(json_attrs_to_string(point.get("attributes")));
        builders.flags.append_value(
            point.get("flags").and_then(json_i64).unwrap_or(0) as i32
        );
        builders.exemplars_json.append_value("[]");
        builders.aggregation_temporality.append_value(temporality);
        builders.is_monotonic.append_value(monotonic);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_histogram_json(
    builders: &mut HistogramBuilders,
    histogram: &JsonValue,
    resource: &JsonValue,
    scope: &JsonValue,
    name: &str,
    description: &str,
    unit: &str,
) -> Result<(), DecodeError> {
    let temporality = histogram
        .get("aggregationTemporality")
        .or_else(|| histogram.get("aggregation_temporality"))
        .and_then(json_i64)
        .unwrap_or(0) as i32;

    let points = histogram
        .get("dataPoints")
        .or_else(|| histogram.get("data_points"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for point in &points {
        let time = point.get("timeUnixNano")
            .or_else(|| point.get("time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);
        let start = point.get("startTimeUnixNano")
            .or_else(|| point.get("start_time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);

        let count = point.get("count").and_then(json_i64).unwrap_or(0);
        let sum = point.get("sum").and_then(json_f64);
        let min = point.get("min").and_then(json_f64);
        let max = point.get("max").and_then(json_f64);

        builders.timestamp.append_value(time / 1000);
        builders.start_timestamp.append_value(start / 1000);
        builders.metric_name.append_value(name);
        builders.metric_description.append_value(description);
        builders.metric_unit.append_value(unit);
        builders.count.append_value(count);

        if let Some(s) = sum { builders.sum.append_value(s); } else { builders.sum.append_null(); }
        if let Some(m) = min { builders.min.append_value(m); } else { builders.min.append_null(); }
        if let Some(m) = max { builders.max.append_value(m); } else { builders.max.append_null(); }

        let bucket_counts: Vec<u64> = point.get("bucketCounts")
            .or_else(|| point.get("bucket_counts"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(json_u64).collect())
            .unwrap_or_default();
        let explicit_bounds: Vec<f64> = point.get("explicitBounds")
            .or_else(|| point.get("explicit_bounds"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(serde_json::Value::as_f64).collect())
            .unwrap_or_default();

        builders.bucket_counts.append_value(serde_json::to_string(&bucket_counts).unwrap_or_else(|_| "[]".into()));
        builders.explicit_bounds.append_value(serde_json::to_string(&explicit_bounds).unwrap_or_else(|_| "[]".into()));

        let service_name = extract_json_string_attr(resource, "service.name").unwrap_or_default();
        builders.service_name.append_value(&service_name);
        builders.service_namespace
            .append_option(extract_json_string_attr(resource, "service.namespace"));
        builders.service_instance_id
            .append_option(extract_json_string_attr(resource, "service.instance.id"));

        builders.resource_attributes
            .append_value(json_attrs_to_string(resource.get("attributes")));
        builders.scope_name
            .append_option(scope.get("name").and_then(|v| v.as_str()));
        builders.scope_version
            .append_option(scope.get("version").and_then(|v| v.as_str()));
        builders.scope_attributes
            .append_value(json_attrs_to_string(scope.get("attributes")));
        builders.metric_attributes
            .append_value(json_attrs_to_string(point.get("attributes")));
        builders.flags.append_value(
            point.get("flags").and_then(json_i64).unwrap_or(0) as i32
        );
        builders.exemplars_json.append_value("[]");
        builders.aggregation_temporality.append_value(temporality);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_exp_histogram_json(
    builders: &mut ExpHistogramBuilders,
    exp_histogram: &JsonValue,
    resource: &JsonValue,
    scope: &JsonValue,
    name: &str,
    description: &str,
    unit: &str,
) -> Result<(), DecodeError> {
    let temporality = exp_histogram
        .get("aggregationTemporality")
        .or_else(|| exp_histogram.get("aggregation_temporality"))
        .and_then(json_i64)
        .unwrap_or(0) as i32;

    let points = exp_histogram
        .get("dataPoints")
        .or_else(|| exp_histogram.get("data_points"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for point in &points {
        let time = point.get("timeUnixNano")
            .or_else(|| point.get("time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);
        let start = point.get("startTimeUnixNano")
            .or_else(|| point.get("start_time_unix_nano"))
            .and_then(json_i64)
            .unwrap_or(0);

        let count = point.get("count").and_then(json_i64).unwrap_or(0);
        let sum = point.get("sum").and_then(json_f64);
        let min = point.get("min").and_then(json_f64);
        let max = point.get("max").and_then(json_f64);
        let scale = point.get("scale").and_then(json_i64).unwrap_or(0) as i32;
        let zero_count = point.get("zeroCount").or_else(|| point.get("zero_count")).and_then(json_i64).unwrap_or(0);
        let zero_threshold = point.get("zeroThreshold").or_else(|| point.get("zero_threshold")).and_then(json_f64).unwrap_or(0.0);

        builders.timestamp.append_value(time / 1000);
        builders.start_timestamp.append_value(start / 1000);
        builders.metric_name.append_value(name);
        builders.metric_description.append_value(description);
        builders.metric_unit.append_value(unit);
        builders.count.append_value(count);

        if let Some(s) = sum { builders.sum.append_value(s); } else { builders.sum.append_null(); }
        if let Some(m) = min { builders.min.append_value(m); } else { builders.min.append_null(); }
        if let Some(m) = max { builders.max.append_value(m); } else { builders.max.append_null(); }

        builders.scale.append_value(scale);
        builders.zero_count.append_value(zero_count);
        builders.zero_threshold.append_value(zero_threshold);

        if let Some(pos) = point.get("positive").and_then(|v| v.as_object()) {
            let offset = pos.get("offset").and_then(json_i64).unwrap_or(0) as i32;
            let bucket_counts: Vec<u64> = pos.get("bucketCounts")
                .or_else(|| pos.get("bucket_counts"))
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(json_u64).collect())
                .unwrap_or_default();
            builders.positive_offset.append_value(offset);
            builders.positive_bucket_counts.append_value(serde_json::to_string(&bucket_counts).unwrap_or_else(|_| "[]".into()));
        } else {
            builders.positive_offset.append_null();
            builders.positive_bucket_counts.append_value("[]");
        }

        if let Some(neg) = point.get("negative").and_then(|v| v.as_object()) {
            let offset = neg.get("offset").and_then(json_i64).unwrap_or(0) as i32;
            let bucket_counts: Vec<u64> = neg.get("bucketCounts")
                .or_else(|| neg.get("bucket_counts"))
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(json_u64).collect())
                .unwrap_or_default();
            builders.negative_offset.append_value(offset);
            builders.negative_bucket_counts.append_value(serde_json::to_string(&bucket_counts).unwrap_or_else(|_| "[]".into()));
        } else {
            builders.negative_offset.append_null();
            builders.negative_bucket_counts.append_value("[]");
        }

        let service_name = extract_json_string_attr(resource, "service.name").unwrap_or_default();
        builders.service_name.append_value(&service_name);
        builders.service_namespace
            .append_option(extract_json_string_attr(resource, "service.namespace"));
        builders.service_instance_id
            .append_option(extract_json_string_attr(resource, "service.instance.id"));

        builders.resource_attributes
            .append_value(json_attrs_to_string(resource.get("attributes")));
        builders.scope_name
            .append_option(scope.get("name").and_then(|v| v.as_str()));
        builders.scope_version
            .append_option(scope.get("version").and_then(|v| v.as_str()));
        builders.scope_attributes
            .append_value(json_attrs_to_string(scope.get("attributes")));
        builders.metric_attributes
            .append_value(json_attrs_to_string(point.get("attributes")));
        builders.flags.append_value(
            point.get("flags").and_then(json_i64).unwrap_or(0) as i32
        );
        builders.exemplars_json.append_value("[]");
        builders.aggregation_temporality.append_value(temporality);
    }

    Ok(())
}

// ============================================================================
// Helpers
// ============================================================================

/// Extract i64 from a JSON value, handling both numbers and strings.
/// OTLP JSON encodes uint64/sfixed64 values as strings.
fn json_i64(value: &JsonValue) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

/// Extract f64 from a JSON value, handling both numbers and strings.
fn json_f64(value: &JsonValue) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

/// Extract u64 from a JSON value, handling both numbers and strings.
fn json_u64(value: &JsonValue) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}



fn extract_string_attr(
    resource: Option<&opentelemetry_proto::tonic::resource::v1::Resource>,
    key: &str,
) -> Option<String> {
    resource
        .and_then(|r| {
            r.attributes.iter().find_map(|kv| {
                if kv.key == key {
                    kv.value.as_ref().and_then(|v| match &v.value {
                        Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s)) if !s.is_empty() => {
                            Some(s.clone())
                        }
                        _ => None,
                    })
                } else {
                    None
                }
            })
        })
}

fn extract_json_string_attr(resource: &JsonValue, key: &str) -> Option<String> {
    resource
        .get("attributes")
        .and_then(|attrs| attrs.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|attr| {
                let attr_key = attr.get("key")?.as_str()?;
                if attr_key == key {
                    attr.get("value")?
                        .get("stringValue")
                        .or_else(|| attr.get("value")?.get("string_value"))
                        .and_then(|v| v.as_str())
                        .map(std::string::ToString::to_string)
                        .filter(|s| !s.is_empty())
                } else {
                    None
                }
            })
        })
}

fn attrs_to_json(
    attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue],
) -> String {
    if attrs.is_empty() {
        return "{}".to_string();
    }
    let mut map = serde_json::Map::new();
    for kv in attrs {
        if let Some(val) = &kv.value {
            if let Some(json_val) = any_value_to_json(val) {
                map.insert(kv.key.clone(), json_val);
            }
        }
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

fn any_value_to_json(
    av: &opentelemetry_proto::tonic::common::v1::AnyValue,
) -> Option<JsonValue> {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    match &av.value {
        Some(Value::StringValue(s)) => Some(JsonValue::String(s.clone())),
        Some(Value::BoolValue(b)) => Some(JsonValue::Bool(*b)),
        Some(Value::IntValue(i)) => Some(JsonValue::Number((*i).into())),
        Some(Value::DoubleValue(d)) => serde_json::Number::from_f64(*d).map(JsonValue::Number),
        Some(Value::ArrayValue(arr)) => {
            let values: Vec<JsonValue> = arr
                .values
                .iter()
                .filter_map(any_value_to_json)
                .collect();
            Some(JsonValue::Array(values))
        }
        Some(Value::KvlistValue(kvlist)) => {
            let mut map = serde_json::Map::new();
            for kv in &kvlist.values {
                if let Some(val) = &kv.value {
                    if let Some(json_val) = any_value_to_json(val) {
                        map.insert(kv.key.clone(), json_val);
                    }
                }
            }
            Some(JsonValue::Object(map))
        }
        Some(Value::BytesValue(bytes)) => Some(JsonValue::String(hex::encode(bytes))),
        None => None,
    }
}

fn json_attrs_to_string(attrs: Option<&JsonValue>) -> String {
    match attrs {
        Some(JsonValue::Array(arr)) if !arr.is_empty() => {
            let mut map = serde_json::Map::new();
            for attr in arr {
                if let Some(key) = attr.get("key").and_then(|v| v.as_str()) {
                    if let Some(val) = attr.get("value") {
                        map.insert(key.to_string(), val.clone());
                    }
                }
            }
            serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
        }
        _ => "{}".to_string(),
    }
}

fn events_to_json(
    events: &[opentelemetry_proto::tonic::trace::v1::span::Event],
) -> String {
    if events.is_empty() {
        return "[]".to_string();
    }
    let arr: Vec<JsonValue> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "time_unix_nano": e.time_unix_nano,
                "name": e.name,
                "attributes": serde_json::from_str::<JsonValue>(&attrs_to_json(&e.attributes))
                    .unwrap_or(JsonValue::Object(serde_json::Map::new())),
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn links_to_json(
    links: &[opentelemetry_proto::tonic::trace::v1::span::Link],
) -> String {
    if links.is_empty() {
        return "[]".to_string();
    }
    let arr: Vec<JsonValue> = links
        .iter()
        .map(|l| {
            serde_json::json!({
                "trace_id": hex::encode(&l.trace_id),
                "span_id": hex::encode(&l.span_id),
                "trace_state": l.trace_state,
                "attributes": serde_json::from_str::<JsonValue>(&attrs_to_json(&l.attributes))
                    .unwrap_or(JsonValue::Object(serde_json::Map::new())),
            })
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn json_events_to_string(events: Option<&JsonValue>) -> String {
    match events {
        Some(JsonValue::Array(arr)) if !arr.is_empty() => {
            serde_json::to_string(arr).unwrap_or_else(|_| "[]".to_string())
        }
        _ => "[]".to_string(),
    }
}

fn json_links_to_string(links: Option<&JsonValue>) -> String {
    match links {
        Some(JsonValue::Array(arr)) if !arr.is_empty() => {
            serde_json::to_string(arr).unwrap_or_else(|_| "[]".to_string())
        }
        _ => "[]".to_string(),
    }
}

fn exemplars_to_json(
    exemplars: &[opentelemetry_proto::tonic::metrics::v1::Exemplar],
) -> String {
    if exemplars.is_empty() {
        return "[]".to_string();
    }
    let arr: Vec<JsonValue> = exemplars
        .iter()
        .map(|e| {
            let mut map = serde_json::Map::new();
            map.insert("trace_id".to_string(), JsonValue::String(hex::encode(&e.trace_id)));
            map.insert("span_id".to_string(), JsonValue::String(hex::encode(&e.span_id)));
            map.insert("time_unix_nano".to_string(), JsonValue::Number(e.time_unix_nano.into()));
            if let Some(filtered_attrs) = any_value_to_json(
                &opentelemetry_proto::tonic::common::v1::AnyValue {
                    value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::KvlistValue(
                        opentelemetry_proto::tonic::common::v1::KeyValueList {
                            values: e.filtered_attributes.clone(),
                        },
                    )),
                }
            ) {
                map.insert("filtered_attributes".to_string(), filtered_attrs);
            }
            let val = match &e.value {
                Some(opentelemetry_proto::tonic::metrics::v1::exemplar::Value::AsDouble(d)) => {
                    JsonValue::Number(serde_json::Number::from_f64(*d).unwrap_or(0.into()))
                }
                Some(opentelemetry_proto::tonic::metrics::v1::exemplar::Value::AsInt(i)) => {
                    JsonValue::Number((*i).into())
                }
                None => JsonValue::Null,
            };
            map.insert("value".to_string(), val);
            JsonValue::Object(map)
        })
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

// ============================================================================
// Log direct builders
// ============================================================================

/// Arrow builders for the flat log record schema.
pub struct LogBuilders {
    timestamp: TimestampMicrosecondBuilder,
    observed_timestamp: Int64Builder,
    trace_id: StringBuilder,
    span_id: StringBuilder,
    service_name: StringBuilder,
    service_namespace: StringBuilder,
    service_instance_id: StringBuilder,
    severity_number: Int32Builder,
    severity_text: StringBuilder,
    body: StringBuilder,
    resource_attributes: StringBuilder,
    scope_name: StringBuilder,
    scope_version: StringBuilder,
    scope_attributes: StringBuilder,
    log_attributes: StringBuilder,
}

impl LogBuilders {
    pub fn new() -> Self {
        Self {
            timestamp: TimestampMicrosecondBuilder::new(),
            observed_timestamp: Int64Builder::new(),
            trace_id: StringBuilder::new(),
            span_id: StringBuilder::new(),
            service_name: StringBuilder::new(),
            service_namespace: StringBuilder::new(),
            service_instance_id: StringBuilder::new(),
            severity_number: Int32Builder::new(),
            severity_text: StringBuilder::new(),
            body: StringBuilder::new(),
            resource_attributes: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            scope_version: StringBuilder::new(),
            scope_attributes: StringBuilder::new(),
            log_attributes: StringBuilder::new(),
        }
    }

    pub fn append_log(
        &mut self,
        log: &opentelemetry_proto::tonic::logs::v1::LogRecord,
        resource: Option<&opentelemetry_proto::tonic::resource::v1::Resource>,
        scope: Option<&opentelemetry_proto::tonic::common::v1::InstrumentationScope>,
    ) -> Result<(), DecodeError> {
        let time = i64::try_from(log.time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!(
                "timestamp overflow: time_unix_nano {} exceeds i64::MAX",
                log.time_unix_nano
            ))
        })?;
        let observed = i64::try_from(log.observed_time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!(
                "timestamp overflow: observed_time_unix_nano {} exceeds i64::MAX",
                log.observed_time_unix_nano
            ))
        })?;

        self.timestamp.append_value(time / 1000);
        self.observed_timestamp.append_value(observed / 1000);

        let trace_id = hex::encode(&log.trace_id);
        let span_id = hex::encode(&log.span_id);
        if trace_id.is_empty() {
            self.trace_id.append_null();
        } else {
            self.trace_id.append_value(&trace_id);
        }
        if span_id.is_empty() {
            self.span_id.append_null();
        } else {
            self.span_id.append_value(&span_id);
        }

        let service_name = extract_string_attr(resource, "service.name").unwrap_or_default();
        if service_name.is_empty()
            || service_name == "unknown"
            || service_name == "unknown_service"
            || service_name.starts_with("unknown_service:")
        {
            return Err(DecodeError::Parse(
                "log with unusable service_name detected — set OTEL_SERVICE_NAME (or service.name resource attribute) on the sending application".into(),
            ));
        }
        self.service_name.append_value(&service_name);
        self.service_namespace
            .append_option(extract_string_attr(resource, "service.namespace"));
        self.service_instance_id
            .append_option(extract_string_attr(resource, "service.instance.id"));

        self.severity_number.append_value(log.severity_number);
        if log.severity_text.is_empty() {
            self.severity_text.append_value("");
        } else {
            self.severity_text.append_value(&log.severity_text);
        }

        let body_str = match &log.body {
            Some(body) => body_to_string(body),
            None => String::new(),
        };
        self.body.append_value(&body_str);

        self.resource_attributes
            .append_value(attrs_to_json(resource.map(|r| &r.attributes[..]).unwrap_or(&[])));
        self.scope_name
            .append_option(scope.map(|s| s.name.as_str()));
        self.scope_version
            .append_option(scope.map(|s| s.version.as_str()));
        self.scope_attributes
            .append_value(attrs_to_json(scope.map(|s| &s.attributes[..]).unwrap_or(&[])));
        self.log_attributes
            .append_value(attrs_to_json(&log.attributes));

        Ok(())
    }

    pub fn finish(mut self) -> Result<RecordBatch, ArrowError> {
        let schema = logs_schema();
        let arrays: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(self.timestamp.finish()),
            Arc::new(self.observed_timestamp.finish()),
            Arc::new(self.trace_id.finish()),
            Arc::new(self.span_id.finish()),
            Arc::new(self.service_name.finish()),
            Arc::new(self.service_namespace.finish()),
            Arc::new(self.service_instance_id.finish()),
            Arc::new(self.severity_number.finish()),
            Arc::new(self.severity_text.finish()),
            Arc::new(self.body.finish()),
            Arc::new(self.resource_attributes.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.scope_version.finish()),
            Arc::new(self.scope_attributes.finish()),
            Arc::new(self.log_attributes.finish()),
        ];
        RecordBatch::try_new(Arc::new(schema), arrays)
    }
}

impl Default for LogBuilders {
    fn default() -> Self {
        Self::new()
    }
}

fn body_to_string(body: &opentelemetry_proto::tonic::common::v1::AnyValue) -> String {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    match &body.value {
        Some(Value::StringValue(s)) => s.clone(),
        Some(other) => {
            let av = opentelemetry_proto::tonic::common::v1::AnyValue {
                value: Some(other.clone()),
            };
            serde_json::to_string(&any_value_to_json(&av).unwrap_or(JsonValue::Null))
                .unwrap_or_default()
        }
        None => String::new(),
    }
}

// ============================================================================
// Metric direct builders
// ============================================================================

struct MetricContext<'a> {
    name: &'a str,
    description: &'a str,
    unit: &'a str,
    resource: Option<&'a opentelemetry_proto::tonic::resource::v1::Resource>,
    scope: Option<&'a opentelemetry_proto::tonic::common::v1::InstrumentationScope>,
}

struct GaugeBuilders {
    timestamp: TimestampMicrosecondBuilder,
    start_timestamp: Int64Builder,
    metric_name: StringBuilder,
    metric_description: StringBuilder,
    metric_unit: StringBuilder,
    value: Float64Builder,
    service_name: StringBuilder,
    service_namespace: StringBuilder,
    service_instance_id: StringBuilder,
    resource_attributes: StringBuilder,
    scope_name: StringBuilder,
    scope_version: StringBuilder,
    scope_attributes: StringBuilder,
    metric_attributes: StringBuilder,
    flags: Int32Builder,
    exemplars_json: StringBuilder,
}

impl GaugeBuilders {
    fn new() -> Self {
        Self {
            timestamp: TimestampMicrosecondBuilder::new(),
            start_timestamp: Int64Builder::new(),
            metric_name: StringBuilder::new(),
            metric_description: StringBuilder::new(),
            metric_unit: StringBuilder::new(),
            value: Float64Builder::new(),
            service_name: StringBuilder::new(),
            service_namespace: StringBuilder::new(),
            service_instance_id: StringBuilder::new(),
            resource_attributes: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            scope_version: StringBuilder::new(),
            scope_attributes: StringBuilder::new(),
            metric_attributes: StringBuilder::new(),
            flags: Int32Builder::new(),
            exemplars_json: StringBuilder::new(),
        }
    }

    fn append(
        &mut self,
        point: &opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
        ctx: &MetricContext<'_>,
    ) -> Result<bool, DecodeError> {
        let val = match &point.value {
            Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(i)) => *i as f64,
            Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d)) => {
                if d.is_nan() || d.is_infinite() {
                    return Ok(false);
                }
                *d
            }
            None => return Ok(false),
        };

        let time = i64::try_from(point.time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: time_unix_nano {}", point.time_unix_nano))
        })?;
        let start = i64::try_from(point.start_time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: start_time_unix_nano {}", point.start_time_unix_nano))
        })?;

        self.timestamp.append_value(time / 1000);
        self.start_timestamp.append_value(start / 1000);
        self.metric_name.append_value(ctx.name);
        self.metric_description.append_value(ctx.description);
        self.metric_unit.append_value(ctx.unit);
        self.value.append_value(val);

        let service_name = extract_string_attr(ctx.resource, "service.name").unwrap_or_default();
        self.service_name.append_value(&service_name);
        self.service_namespace
            .append_option(extract_string_attr(ctx.resource, "service.namespace"));
        self.service_instance_id
            .append_option(extract_string_attr(ctx.resource, "service.instance.id"));

        self.resource_attributes
            .append_value(attrs_to_json(ctx.resource.map(|r| &r.attributes[..]).unwrap_or(&[])));
        self.scope_name
            .append_option(ctx.scope.map(|s| s.name.as_str()));
        self.scope_version
            .append_option(ctx.scope.map(|s| s.version.as_str()));
        self.scope_attributes
            .append_value(attrs_to_json(ctx.scope.map(|s| &s.attributes[..]).unwrap_or(&[])));
        self.metric_attributes
            .append_value(attrs_to_json(&point.attributes));
        self.flags.append_value(point.flags as i32);
        self.exemplars_json.append_value(exemplars_to_json(&point.exemplars));

        Ok(true)
    }

    fn finish(mut self) -> Result<Option<RecordBatch>, ArrowError> {
        let metric_name = self.metric_name.finish();
        if metric_name.len() == 0 {
            return Ok(None);
        }
        let schema = gauge_schema();
        let arrays: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(self.timestamp.finish()),
            Arc::new(self.start_timestamp.finish()),
            Arc::new(metric_name),
            Arc::new(self.metric_description.finish()),
            Arc::new(self.metric_unit.finish()),
            Arc::new(self.value.finish()),
            Arc::new(self.service_name.finish()),
            Arc::new(self.service_namespace.finish()),
            Arc::new(self.service_instance_id.finish()),
            Arc::new(self.resource_attributes.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.scope_version.finish()),
            Arc::new(self.scope_attributes.finish()),
            Arc::new(self.metric_attributes.finish()),
            Arc::new(self.flags.finish()),
            Arc::new(self.exemplars_json.finish()),
        ];
        Ok(Some(RecordBatch::try_new(Arc::new(schema), arrays)?))
    }
}

struct SumBuilders {
    timestamp: TimestampMicrosecondBuilder,
    start_timestamp: Int64Builder,
    metric_name: StringBuilder,
    metric_description: StringBuilder,
    metric_unit: StringBuilder,
    value: Float64Builder,
    service_name: StringBuilder,
    service_namespace: StringBuilder,
    service_instance_id: StringBuilder,
    resource_attributes: StringBuilder,
    scope_name: StringBuilder,
    scope_version: StringBuilder,
    scope_attributes: StringBuilder,
    metric_attributes: StringBuilder,
    flags: Int32Builder,
    exemplars_json: StringBuilder,
    aggregation_temporality: Int32Builder,
    is_monotonic: arrow::array::BooleanBuilder,
}

impl SumBuilders {
    fn new() -> Self {
        Self {
            timestamp: TimestampMicrosecondBuilder::new(),
            start_timestamp: Int64Builder::new(),
            metric_name: StringBuilder::new(),
            metric_description: StringBuilder::new(),
            metric_unit: StringBuilder::new(),
            value: Float64Builder::new(),
            service_name: StringBuilder::new(),
            service_namespace: StringBuilder::new(),
            service_instance_id: StringBuilder::new(),
            resource_attributes: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            scope_version: StringBuilder::new(),
            scope_attributes: StringBuilder::new(),
            metric_attributes: StringBuilder::new(),
            flags: Int32Builder::new(),
            exemplars_json: StringBuilder::new(),
            aggregation_temporality: Int32Builder::new(),
            is_monotonic: arrow::array::BooleanBuilder::new(),
        }
    }

    fn append(
        &mut self,
        point: &opentelemetry_proto::tonic::metrics::v1::NumberDataPoint,
        ctx: &MetricContext<'_>,
        temporality: i32,
        monotonic: bool,
    ) -> Result<bool, DecodeError> {
        let val = match &point.value {
            Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(i)) => *i as f64,
            Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d)) => {
                if d.is_nan() || d.is_infinite() {
                    return Ok(false);
                }
                *d
            }
            None => return Ok(false),
        };

        let time = i64::try_from(point.time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: time_unix_nano {}", point.time_unix_nano))
        })?;
        let start = i64::try_from(point.start_time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: start_time_unix_nano {}", point.start_time_unix_nano))
        })?;

        self.timestamp.append_value(time / 1000);
        self.start_timestamp.append_value(start / 1000);
        self.metric_name.append_value(ctx.name);
        self.metric_description.append_value(ctx.description);
        self.metric_unit.append_value(ctx.unit);
        self.value.append_value(val);

        let service_name = extract_string_attr(ctx.resource, "service.name").unwrap_or_default();
        self.service_name.append_value(&service_name);
        self.service_namespace
            .append_option(extract_string_attr(ctx.resource, "service.namespace"));
        self.service_instance_id
            .append_option(extract_string_attr(ctx.resource, "service.instance.id"));

        self.resource_attributes
            .append_value(attrs_to_json(ctx.resource.map(|r| &r.attributes[..]).unwrap_or(&[])));
        self.scope_name
            .append_option(ctx.scope.map(|s| s.name.as_str()));
        self.scope_version
            .append_option(ctx.scope.map(|s| s.version.as_str()));
        self.scope_attributes
            .append_value(attrs_to_json(ctx.scope.map(|s| &s.attributes[..]).unwrap_or(&[])));
        self.metric_attributes
            .append_value(attrs_to_json(&point.attributes));
        self.flags.append_value(point.flags as i32);
        self.exemplars_json.append_value(exemplars_to_json(&point.exemplars));
        self.aggregation_temporality.append_value(temporality);
        self.is_monotonic.append_value(monotonic);

        Ok(true)
    }

    fn finish(mut self) -> Result<Option<RecordBatch>, ArrowError> {
        let metric_name = self.metric_name.finish();
        if metric_name.len() == 0 {
            return Ok(None);
        }
        let schema = sum_schema();
        let arrays: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(self.timestamp.finish()),
            Arc::new(self.start_timestamp.finish()),
            Arc::new(metric_name),
            Arc::new(self.metric_description.finish()),
            Arc::new(self.metric_unit.finish()),
            Arc::new(self.value.finish()),
            Arc::new(self.service_name.finish()),
            Arc::new(self.service_namespace.finish()),
            Arc::new(self.service_instance_id.finish()),
            Arc::new(self.resource_attributes.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.scope_version.finish()),
            Arc::new(self.scope_attributes.finish()),
            Arc::new(self.metric_attributes.finish()),
            Arc::new(self.flags.finish()),
            Arc::new(self.exemplars_json.finish()),
            Arc::new(self.aggregation_temporality.finish()),
            Arc::new(self.is_monotonic.finish()),
        ];
        Ok(Some(RecordBatch::try_new(Arc::new(schema), arrays)?))
    }
}

struct HistogramBuilders {
    timestamp: TimestampMicrosecondBuilder,
    start_timestamp: Int64Builder,
    metric_name: StringBuilder,
    metric_description: StringBuilder,
    metric_unit: StringBuilder,
    count: Int64Builder,
    sum: Float64Builder,
    min: Float64Builder,
    max: Float64Builder,
    bucket_counts: StringBuilder,
    explicit_bounds: StringBuilder,
    service_name: StringBuilder,
    service_namespace: StringBuilder,
    service_instance_id: StringBuilder,
    resource_attributes: StringBuilder,
    scope_name: StringBuilder,
    scope_version: StringBuilder,
    scope_attributes: StringBuilder,
    metric_attributes: StringBuilder,
    flags: Int32Builder,
    exemplars_json: StringBuilder,
    aggregation_temporality: Int32Builder,
}

impl HistogramBuilders {
    fn new() -> Self {
        Self {
            timestamp: TimestampMicrosecondBuilder::new(),
            start_timestamp: Int64Builder::new(),
            metric_name: StringBuilder::new(),
            metric_description: StringBuilder::new(),
            metric_unit: StringBuilder::new(),
            count: Int64Builder::new(),
            sum: Float64Builder::new(),
            min: Float64Builder::new(),
            max: Float64Builder::new(),
            bucket_counts: StringBuilder::new(),
            explicit_bounds: StringBuilder::new(),
            service_name: StringBuilder::new(),
            service_namespace: StringBuilder::new(),
            service_instance_id: StringBuilder::new(),
            resource_attributes: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            scope_version: StringBuilder::new(),
            scope_attributes: StringBuilder::new(),
            metric_attributes: StringBuilder::new(),
            flags: Int32Builder::new(),
            exemplars_json: StringBuilder::new(),
            aggregation_temporality: Int32Builder::new(),
        }
    }

    fn append(
        &mut self,
        point: &opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint,
        ctx: &MetricContext<'_>,
        temporality: i32,
    ) -> Result<(), DecodeError> {
        let time = i64::try_from(point.time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: time_unix_nano {}", point.time_unix_nano))
        })?;
        let start = i64::try_from(point.start_time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: start_time_unix_nano {}", point.start_time_unix_nano))
        })?;

        self.timestamp.append_value(time / 1000);
        self.start_timestamp.append_value(start / 1000);
        self.metric_name.append_value(ctx.name);
        self.metric_description.append_value(ctx.description);
        self.metric_unit.append_value(ctx.unit);
        self.count.append_value(point.count as i64);

        if let Some(s) = point.sum { self.sum.append_value(s); } else { self.sum.append_null(); }
        if let Some(m) = point.min { self.min.append_value(m); } else { self.min.append_null(); }
        if let Some(m) = point.max { self.max.append_value(m); } else { self.max.append_null(); }

        self.bucket_counts.append_value(serde_json::to_string(&point.bucket_counts).unwrap_or_else(|_| "[]".into()));
        self.explicit_bounds.append_value(serde_json::to_string(&point.explicit_bounds).unwrap_or_else(|_| "[]".into()));

        let service_name = extract_string_attr(ctx.resource, "service.name").unwrap_or_default();
        self.service_name.append_value(&service_name);
        self.service_namespace
            .append_option(extract_string_attr(ctx.resource, "service.namespace"));
        self.service_instance_id
            .append_option(extract_string_attr(ctx.resource, "service.instance.id"));

        self.resource_attributes
            .append_value(attrs_to_json(ctx.resource.map(|r| &r.attributes[..]).unwrap_or(&[])));
        self.scope_name
            .append_option(ctx.scope.map(|s| s.name.as_str()));
        self.scope_version
            .append_option(ctx.scope.map(|s| s.version.as_str()));
        self.scope_attributes
            .append_value(attrs_to_json(ctx.scope.map(|s| &s.attributes[..]).unwrap_or(&[])));
        self.metric_attributes
            .append_value(attrs_to_json(&point.attributes));
        self.flags.append_value(point.flags as i32);
        self.exemplars_json.append_value(exemplars_to_json(&point.exemplars));
        self.aggregation_temporality.append_value(temporality);

        Ok(())
    }

    fn finish(mut self) -> Result<Option<RecordBatch>, ArrowError> {
        let metric_name = self.metric_name.finish();
        if metric_name.len() == 0 {
            return Ok(None);
        }
        let schema = histogram_schema();
        let arrays: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(self.timestamp.finish()),
            Arc::new(self.start_timestamp.finish()),
            Arc::new(metric_name),
            Arc::new(self.metric_description.finish()),
            Arc::new(self.metric_unit.finish()),
            Arc::new(self.count.finish()),
            Arc::new(self.sum.finish()),
            Arc::new(self.min.finish()),
            Arc::new(self.max.finish()),
            Arc::new(self.bucket_counts.finish()),
            Arc::new(self.explicit_bounds.finish()),
            Arc::new(self.service_name.finish()),
            Arc::new(self.service_namespace.finish()),
            Arc::new(self.service_instance_id.finish()),
            Arc::new(self.resource_attributes.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.scope_version.finish()),
            Arc::new(self.scope_attributes.finish()),
            Arc::new(self.metric_attributes.finish()),
            Arc::new(self.flags.finish()),
            Arc::new(self.exemplars_json.finish()),
            Arc::new(self.aggregation_temporality.finish()),
        ];
        Ok(Some(RecordBatch::try_new(Arc::new(schema), arrays)?))
    }
}

struct ExpHistogramBuilders {
    timestamp: TimestampMicrosecondBuilder,
    start_timestamp: Int64Builder,
    metric_name: StringBuilder,
    metric_description: StringBuilder,
    metric_unit: StringBuilder,
    count: Int64Builder,
    sum: Float64Builder,
    min: Float64Builder,
    max: Float64Builder,
    scale: Int32Builder,
    zero_count: Int64Builder,
    zero_threshold: Float64Builder,
    positive_offset: Int32Builder,
    positive_bucket_counts: StringBuilder,
    negative_offset: Int32Builder,
    negative_bucket_counts: StringBuilder,
    service_name: StringBuilder,
    service_namespace: StringBuilder,
    service_instance_id: StringBuilder,
    resource_attributes: StringBuilder,
    scope_name: StringBuilder,
    scope_version: StringBuilder,
    scope_attributes: StringBuilder,
    metric_attributes: StringBuilder,
    flags: Int32Builder,
    exemplars_json: StringBuilder,
    aggregation_temporality: Int32Builder,
}

impl ExpHistogramBuilders {
    fn new() -> Self {
        Self {
            timestamp: TimestampMicrosecondBuilder::new(),
            start_timestamp: Int64Builder::new(),
            metric_name: StringBuilder::new(),
            metric_description: StringBuilder::new(),
            metric_unit: StringBuilder::new(),
            count: Int64Builder::new(),
            sum: Float64Builder::new(),
            min: Float64Builder::new(),
            max: Float64Builder::new(),
            scale: Int32Builder::new(),
            zero_count: Int64Builder::new(),
            zero_threshold: Float64Builder::new(),
            positive_offset: Int32Builder::new(),
            positive_bucket_counts: StringBuilder::new(),
            negative_offset: Int32Builder::new(),
            negative_bucket_counts: StringBuilder::new(),
            service_name: StringBuilder::new(),
            service_namespace: StringBuilder::new(),
            service_instance_id: StringBuilder::new(),
            resource_attributes: StringBuilder::new(),
            scope_name: StringBuilder::new(),
            scope_version: StringBuilder::new(),
            scope_attributes: StringBuilder::new(),
            metric_attributes: StringBuilder::new(),
            flags: Int32Builder::new(),
            exemplars_json: StringBuilder::new(),
            aggregation_temporality: Int32Builder::new(),
        }
    }

    fn append(
        &mut self,
        point: &opentelemetry_proto::tonic::metrics::v1::ExponentialHistogramDataPoint,
        ctx: &MetricContext<'_>,
        temporality: i32,
    ) -> Result<(), DecodeError> {
        let time = i64::try_from(point.time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: time_unix_nano {}", point.time_unix_nano))
        })?;
        let start = i64::try_from(point.start_time_unix_nano).map_err(|_| {
            DecodeError::Unsupported(format!("timestamp overflow: start_time_unix_nano {}", point.start_time_unix_nano))
        })?;

        self.timestamp.append_value(time / 1000);
        self.start_timestamp.append_value(start / 1000);
        self.metric_name.append_value(ctx.name);
        self.metric_description.append_value(ctx.description);
        self.metric_unit.append_value(ctx.unit);
        self.count.append_value(point.count as i64);

        if let Some(s) = point.sum { self.sum.append_value(s); } else { self.sum.append_null(); }
        if let Some(m) = point.min { self.min.append_value(m); } else { self.min.append_null(); }
        if let Some(m) = point.max { self.max.append_value(m); } else { self.max.append_null(); }

        self.scale.append_value(point.scale);
        self.zero_count.append_value(point.zero_count as i64);
        self.zero_threshold.append_value(point.zero_threshold);

        if let Some(pos) = &point.positive {
            self.positive_offset.append_value(pos.offset);
            self.positive_bucket_counts.append_value(serde_json::to_string(&pos.bucket_counts).unwrap_or_else(|_| "[]".into()));
        } else {
            self.positive_offset.append_null();
            self.positive_bucket_counts.append_value("[]");
        }

        if let Some(neg) = &point.negative {
            self.negative_offset.append_value(neg.offset);
            self.negative_bucket_counts.append_value(serde_json::to_string(&neg.bucket_counts).unwrap_or_else(|_| "[]".into()));
        } else {
            self.negative_offset.append_null();
            self.negative_bucket_counts.append_value("[]");
        }

        let service_name = extract_string_attr(ctx.resource, "service.name").unwrap_or_default();
        self.service_name.append_value(&service_name);
        self.service_namespace
            .append_option(extract_string_attr(ctx.resource, "service.namespace"));
        self.service_instance_id
            .append_option(extract_string_attr(ctx.resource, "service.instance.id"));

        self.resource_attributes
            .append_value(attrs_to_json(ctx.resource.map(|r| &r.attributes[..]).unwrap_or(&[])));
        self.scope_name
            .append_option(ctx.scope.map(|s| s.name.as_str()));
        self.scope_version
            .append_option(ctx.scope.map(|s| s.version.as_str()));
        self.scope_attributes
            .append_value(attrs_to_json(ctx.scope.map(|s| &s.attributes[..]).unwrap_or(&[])));
        self.metric_attributes
            .append_value(attrs_to_json(&point.attributes));
        self.flags.append_value(point.flags as i32);
        self.exemplars_json.append_value(exemplars_to_json(&point.exemplars));
        self.aggregation_temporality.append_value(temporality);

        Ok(())
    }

    fn finish(mut self) -> Result<Option<RecordBatch>, ArrowError> {
        let metric_name = self.metric_name.finish();
        if metric_name.len() == 0 {
            return Ok(None);
        }
        let schema = exp_histogram_schema();
        let arrays: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(self.timestamp.finish()),
            Arc::new(self.start_timestamp.finish()),
            Arc::new(metric_name),
            Arc::new(self.metric_description.finish()),
            Arc::new(self.metric_unit.finish()),
            Arc::new(self.count.finish()),
            Arc::new(self.sum.finish()),
            Arc::new(self.min.finish()),
            Arc::new(self.max.finish()),
            Arc::new(self.scale.finish()),
            Arc::new(self.zero_count.finish()),
            Arc::new(self.zero_threshold.finish()),
            Arc::new(self.positive_offset.finish()),
            Arc::new(self.positive_bucket_counts.finish()),
            Arc::new(self.negative_offset.finish()),
            Arc::new(self.negative_bucket_counts.finish()),
            Arc::new(self.service_name.finish()),
            Arc::new(self.service_namespace.finish()),
            Arc::new(self.service_instance_id.finish()),
            Arc::new(self.resource_attributes.finish()),
            Arc::new(self.scope_name.finish()),
            Arc::new(self.scope_version.finish()),
            Arc::new(self.scope_attributes.finish()),
            Arc::new(self.metric_attributes.finish()),
            Arc::new(self.flags.finish()),
            Arc::new(self.exemplars_json.finish()),
            Arc::new(self.aggregation_temporality.finish()),
        ];
        Ok(Some(RecordBatch::try_new(Arc::new(schema), arrays)?))
    }
}

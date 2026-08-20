pub mod analysis;

use anyhow::{Context, Result};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use itertools::Itertools;

use crate::types::{StatusCode, TimestampMicros};

// Re-export pure types and functions from analysis submodule.
pub use analysis::{
    Coverage, ServiceDependency, SignalCoverage, Window, compute_coverage, metric_bucket_interval,
    us_to_iso,
};

// ---------------------------------------------------------------------------
// DiagnoseRequest — API input (from_ts/to_ts mandatory)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema, validator::Validate)]
#[validate(schema(function = "validate_window", skip_on_field_errors = true))]
pub struct DiagnoseRequest {
    /// Single service to diagnose. Provide this OR `services` (call
    /// `GET /v1/services` first to discover valid names).
    // The example is published in /openapi.json and on the docs page. "checkout"
    // is the service every other example on the site uses; the previous value named
    // an unrelated internal deployment.
    #[schema(example = "checkout")]
    pub service: Option<String>,
    /// Multiple services in one call (max 10). Alternative to `service`.
    #[schema(example = json!(["checkout", "cart"]))]
    pub services: Option<Vec<String>>,
    /// Window start, **microseconds** since the Unix epoch (a 16-digit value,
    /// e.g. 1780000000000000). Seconds/millis/nanos are rejected with a hint.
    /// OPTIONAL: omit both `from_ts` and `to_ts` for the last 15 minutes, the
    /// same default `sigiro diagnose` uses. Providing `to_ts` without `from_ts`
    /// is rejected — a window that ends somewhere with no start is ambiguous,
    /// and guessing one silently would answer a question nobody asked.
    #[validate(custom(function = "validate_micros_opt"))]
    #[schema(example = 1780000000000000i64)]
    pub from_ts: Option<i64>,
    /// Window end, microseconds since the Unix epoch. Must be > `from_ts`.
    /// Optional: defaults to now when `from_ts` is given alone.
    #[validate(custom(function = "validate_micros_opt"))]
    #[schema(example = 1780001800000000i64)]
    pub to_ts: Option<i64>,
}

/// The default window when the caller supplies neither bound. Matches the CLI's
/// `--from` default so `sigiro diagnose svc` and `POST /v1/diagnose
/// {"service":"svc"}` describe the same slice of time.
const DEFAULT_WINDOW_MICROS: i64 = 15 * 60 * 1_000_000;

impl DiagnoseRequest {
    /// Resolve the optional bounds to a concrete window, once, so no downstream
    /// caller invents its own default. Assumes `validate_window` has run, which
    /// is what rejects `to_ts` without `from_ts`.
    pub fn window(&self, now_micros: i64) -> (i64, i64) {
        match (self.from_ts, self.to_ts) {
            (Some(from), Some(to)) => (from, to),
            (Some(from), None) => (from, now_micros),
            // Both absent: the documented 15-minute default.
            _ => (now_micros - DEFAULT_WINDOW_MICROS, now_micros),
        }
    }
}

// Timestamps are MICROSECONDS since the Unix epoch (matching the span/log
// `timestamp` column). Fail LOUD on the wrong unit: seconds/millis are still
// "positive" but resolve to ~1970, matching nothing — a false "no telemetry".
/// `validate_micros` for an optional field. `None` is legal (it means "use the
/// default"); `Some` still gets the full wrong-unit check, which is the whole
/// reason these fields were mandatory in the first place — a seconds value is
/// positive, resolves to ~1970, matches nothing, and reads as "no telemetry".
fn validate_micros_opt(ts: i64) -> Result<(), validator::ValidationError> {
    validate_micros(ts)
}

fn validate_micros(ts: i64) -> Result<(), validator::ValidationError> {
    const MICROS_MIN: i64 = 1_000_000_000_000_000; // ~2001-09 in µs — catches s / ms
    const MICROS_MAX: i64 = 10_000_000_000_000_000; // ~2286 in µs — catches ns
    if (MICROS_MIN..MICROS_MAX).contains(&ts) {
        return Ok(());
    }
    let hint = match ts {
        t if t <= 0 => "must be a positive Unix timestamp",
        t if t < 100_000_000_000 => "far too small to be a timestamp",
        t if t < 100_000_000_000_000 => "looks like SECONDS — multiply by 1_000_000 for µs",
        t if t < MICROS_MIN => "looks like MILLISECONDS — multiply by 1_000 for µs",
        _ => "looks like NANOSECONDS — divide by 1_000 for µs",
    };
    let mut err = validator::ValidationError::new("timestamp_micros");
    err.message = Some(
        format!(
            "not a plausible microsecond timestamp ({hint}); expected a 16-digit µs value since \
             the Unix epoch, e.g. 1780000000000000"
        )
        .into(),
    );
    Err(err)
}

fn validate_window(req: &DiagnoseRequest) -> Result<(), validator::ValidationError> {
    match (req.from_ts, req.to_ts) {
        // A window that ends somewhere but starts nowhere. Defaulting the start
        // would silently answer about a different span of time than the caller
        // named, so reject and say which field is missing.
        (None, Some(_)) => {
            let mut err = validator::ValidationError::new("window");
            err.message =
                Some("to_ts was provided without from_ts — supply both, or neither for the last 15 minutes".into());
            Err(err)
        }
        (Some(from), Some(to)) if to <= from => {
            let mut err = validator::ValidationError::new("window");
            err.message = Some("to_ts must be greater than from_ts".into());
            Err(err)
        }
        _ => Ok(()),
    }
}

const MAX_SERVICES: usize = 10;

// ---------------------------------------------------------------------------
// Response types — every section maps 1:1 to a CTE in the single query
// ---------------------------------------------------------------------------

/// One ranked, self-contained "what deserves attention" item — the first
/// thing an agent reads. Assembled in Rust from data the single query already
/// returned (zero extra SQL, zero extra memory).
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct Finding {
    /// "critical" | "warning" | "info" — findings are sorted by this.
    pub severity: String,
    /// Machine-readable category: "incident" (two or more shifts correlated
    /// in time — read `incident` for the members), "anomalous_operation"
    /// (a lone error_rate / latency_p95 shift, which also flags its
    /// operation summary), "anomalous_log_volume" /
    /// "anomalous_error_log_rate" / "anomalous_profile_cost" (lone
    /// standalone shifts — no operation to attach to),
    /// "errors_without_logs", "coverage_gap", "missing_resource_attributes",
    /// and the install-health kinds "duplicate_spans", "orphan_spans",
    /// "missing_client_spans" (telemetry-pipeline faults: is what you are
    /// reading even trustworthy?).
    pub kind: String,
    /// Human/agent-readable one-liner with the concrete numbers.
    pub summary: String,
    /// Ready-to-run /v1/query SQL for the evidence behind this finding,
    /// when one exists. Incident findings carry the drills on their member
    /// shifts instead.
    pub drill_down_sql: Option<String>,
    /// Present only when `kind` is "incident": the correlated member shifts
    /// (each with its own drill) and the suspect deploy, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident: Option<Incident>,
}

/// Cross-signal correlation of the anomaly subsystem: N regime shifts whose
/// `ts_shift` fall in the same or adjacent detection bucket (the scheduled
/// pass's own 5-minute resolution — see `crate::anomaly::correlate_shifts`
/// for why that window is structural, not a threshold) on one service are
/// ONE incident, not N alerts. Nothing is dropped: every member shift is
/// listed with its own before → after and drill query.
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct Incident {
    /// Earliest member `ts_shift` — the first detection bucket of the
    /// incident, µs since the Unix epoch.
    pub(crate) started_at: i64,
    /// `started_at` as an RFC3339 timestamp.
    pub(crate) started_at_iso: String,
    /// The correlated shifts, biggest first (by the same unit-free magnitude
    /// findings are ranked by).
    pub shifts: Vec<IncidentShift>,
    /// The deploy that landed inside the incident's bucket span or in the
    /// bucket immediately before its first shift — the prime suspect. None
    /// when no deploy correlates.
    pub deploy: Option<DerivedEvent>,
}

/// One member shift of an [`Incident`] — the same row the anomaly pass
/// persisted, with its entity, numbers, and drill kept intact.
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct IncidentShift {
    /// "error_rate" | "latency_p95" | "log_volume" | "error_log_rate" |
    /// "profile_cost".
    pub signal: String,
    /// The shifted entity, e.g. "operation 'POST /pay'" (span signals) or
    /// "service 'api'" (log/profile signals).
    pub entity: String,
    /// First bucket of the new regime, µs since the Unix epoch.
    pub(crate) ts_shift: i64,
    /// `ts_shift` as an RFC3339 timestamp.
    pub(crate) ts_shift_iso: String,
    /// Series mean before the shift (raw signal units).
    pub before: f64,
    /// Series mean from the shift onward.
    pub after: f64,
    /// The shift in prose, e.g. "error rate shifted at 14:30 (5.1% → 50.0%)".
    pub summary: String,
    /// Ready-to-run /v1/query SQL for the raw telemetry behind this shift
    /// (persisted by the anomaly pass: the new regime's spans/logs/samples).
    pub drill_down_sql: Option<String>,
}

/// Response envelope for a diagnosis: per-service blocks plus
/// per-service failures. Partial results by design — one broken service
/// must not discard the other nine (fail loud, per service).
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiagnoseResponse {
    /// Successful diagnosis blocks, keyed by service name.
    pub results: BTreeMap<String, DiagnosisBlock>,
    /// Services whose diagnosis query failed, keyed by service name,
    /// with the error message. Empty when everything succeeded.
    pub errors: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiagnosisBlock {
    /// Ranked list (critical first) of what deserves attention in this
    /// window. Anomalies are CORRELATED across signals first: shifts in the
    /// same or adjacent 5-minute detection bucket merge into one "incident"
    /// finding bundling the member shifts and the suspect deploy; lone
    /// shifts stay single-signal findings. Anomaly findings then rank by
    /// (correlated signal count, unit-free shift magnitude
    /// (after − before) / (after + before), raw |Δ|) descending — a
    /// multi-signal incident outranks a lone shift; the magnitude is
    /// comparable across error-rate, latency, log and profile shifts.
    /// Non-anomaly findings (error/log inconsistencies, actionable coverage
    /// gaps) follow. Read this before the raw sections below.
    pub findings: Vec<Finding>,
    pub service: String,
    pub window: Window,
    /// Distinct traces with an error span in the window — the headline
    /// "something's wrong" signal. Drill via the anomalous `operation_summaries`
    /// or the `spans` section, each of which carries a ready-to-run query.
    pub error_traces_total: i64,
    pub coverage: Coverage,
    pub context: ServiceContext,
    pub events: Vec<DerivedEvent>,
    pub operation_summaries: Vec<OperationSummary>,
    /// Per-(provider, model) LLM stats from OTel GenAI (`gen_ai.*`) span
    /// attributes, most-called first. Empty when the window has no LLM
    /// spans — services that never call a model simply don't have this.
    pub gen_ai: Vec<GenAiModelSummary>,
    pub metrics: Vec<MetricBucket>,
    /// Top spans (errors first, then slowest), as a self-describing sample:
    /// `total` vs the returned rows tells you how much was withheld, and
    /// `drill_all_sql` fetches the rest.
    pub spans: SampledSection<SpanRecord>,
    /// Log patterns (template-deduplicated, ordered by count). Each row carries
    /// a `drill_down_sql` for its raw logs.
    pub log_patterns: Vec<LogPattern>,
    /// Numbers the service logs about itself, promoted to statistics — the
    /// counters already embedded in log lines like
    /// `enqueued=1024 responses=24 translations=0`, one row per number.
    /// `pinned_zero` rows come first: a counter that never left zero while
    /// another number on the SAME line did move. That is the signature of a
    /// stage doing no work, which no baseline detector can find (a flat-zero
    /// series has nothing to deviate from) and which looks identical to
    /// "nothing to do".
    pub log_counters: Vec<LogCounter>,
    /// Raw logs, newest first — a self-describing sample (see `spans`).
    pub logs: SampledSection<LogRecord>,
    pub profiles: Vec<ProfileSummary>,
}

/// A bounded sample of a larger set that carries its own "there's more, and
/// here's how to get it" affordance, so an agent can decide to drill without
/// guessing. When `total > sample.len()`, run `drill_all_sql` via /v1/query.
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct SampledSection<T> {
    pub sample: Vec<T>,
    pub total: i64,
    /// How the sample was ordered/selected, e.g. "errors first, then slowest".
    pub sampled_by: String,
    /// Query returning the full set for this section, ready to paste into
    /// /v1/query.
    pub drill_all_sql: String,
}

#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct ServiceContext {
    pub service_name: String,
    pub(crate) first_seen: Option<String>,
    pub(crate) last_seen: Option<String>,
    pub namespace: Option<String>,
    pub deployment: Option<String>,
    /// F-26: distinct pod count when `service.instance.id` is present in
    /// resource attributes, else 0. Use `has_instance_id` to distinguish
    /// "no pods identified" from "service not seen".
    pub pod_count: i64,
    pub(crate) has_instance_id: bool,
    pub dependencies: Vec<ServiceDependency>,
    pub dependents: Vec<ServiceDependency>,
}

#[derive(Serialize, Deserialize, Debug, Clone, utoipa::ToSchema)]
pub struct DerivedEvent {
    pub(crate) detected_at: String,
    /// `detected_at` as µs since the Unix epoch — what incident correlation
    /// aligns deploys against.
    pub(crate) detected_at_us: i64,
    pub event_type: String,
    pub description: String,
}

/// One failure mode of an operation's error spans: the OTel `error.type`
/// dimension (stable semconv) with the number of error spans carrying it.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, utoipa::ToSchema)]
pub struct ErrorTypeCount {
    /// The `error.type` value, derived per span (see `error_type_expr`):
    /// the span's real `error.type` attribute when emitted; else the HTTP
    /// response status code (OTel's own rule for HTTP errors — "429",
    /// "500"); else "timeout" when `status_message` mentions one; else
    /// "_OTHER" (the semconv catch-all). Low-cardinality by construction.
    pub error_type: String,
    /// Error spans of this operation carrying this failure mode.
    pub count: i64,
}

#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct OperationSummary {
    /// The operation identity spans are grouped by. Usually equals
    /// `span_name`, but generic HTTP spans (span_name is a bare method like
    /// "POST", per OTel HTTP semconv) are disambiguated with
    /// `http.route`/`server.address` from span_attributes — e.g.
    /// "POST api.example.com" — so one giant "POST" bucket doesn't swallow
    /// every outbound call.
    pub operation: String,
    /// The raw OTel span name behind this operation (one representative
    /// value when the group was derived from attributes).
    pub span_name: String,
    pub count: i64,
    pub error_count: i64,
    pub error_pct: f64,
    /// Failure-mode composition of this operation's error spans — the OTel
    /// `error.type` dimension, biggest first (count desc, then name).
    /// Answers HOW the operation fails (timeout vs 429 vs 500), not just
    /// that it does — `error_pct` alone collapses 246 timeouts and 6 HTTP
    /// 500s into one number. Empty when the operation has no error spans.
    #[serde(default)]
    pub error_breakdown: Vec<ErrorTypeCount>,
    pub(crate) duration_p50_us: f64,
    pub(crate) duration_p95_us: f64,
    pub(crate) duration_p99_us: f64,
    pub exemplar_error_trace_id: Option<String>,
    pub baseline_error_pct: Option<f64>,
    pub baseline_p95_us: Option<f64>,
    pub baseline_count: Option<i64>,
    /// Whether the continuous anomaly pass (augurs BOCPD changepoints over
    /// this operation's persisted error-rate and p95-latency series — see
    /// `crate::anomaly`) found a worsening regime shift (mean after > mean
    /// before) whose `ts_shift` falls inside this diagnosis window. Read from the
    /// `sigiro_anomalies` table, never recomputed per request. Improvements
    /// do not flag, and a chronic-but-stable failure rate has no changepoint
    /// and is NOT anomalous — it IS the baseline; read `error_pct` for
    /// chronic health.
    ///
    /// Not present in the SQL payload (filled in Rust from the joined
    /// anomaly rows) — `default` keeps the flattened deserialize working.
    #[serde(default)]
    pub is_anomalous: bool,
    /// The detected shift(s) in prose, biggest shift (|after − before|)
    /// first when there are several, e.g.
    /// "error rate shifted at 14:32 (5.1% → 23.8%)" /
    /// "p95 shifted at 14:30 (2.1s → 240.0s)".
    pub anomaly_reason: Option<String>,
    /// Ready-to-run query for the raw spans behind this operation (errors
    /// first, then slowest). Sits next to `anomaly_reason` on purpose: when an
    /// operation looks wrong, this is exactly how to pull the evidence. Filled
    /// in Rust.
    pub drill_down_sql: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct MetricBucket {
    pub(crate) bucket: String,
    pub(crate) metric_name: String,
    pub(crate) p95: f64,
    pub(crate) avg: f64,
    pub(crate) count: i64,
}

/// A flat span record. Attributes and events are embedded as real JSON
/// (not escaped strings) so agents can read them directly.
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct SpanRecord {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    /// Empty string for root spans.
    pub(crate) parent_span_id: String,
    pub(crate) span_name: String,
    /// OTel span kind as stored (integer enum).
    pub(crate) span_kind: Option<i32>,
    pub(crate) status_code: i32,
    /// OTel status message — for spans that fail without emitting any log
    /// (e.g. HTTP client errors) this is often the only diagnostic string.
    pub status_message: Option<String>,
    pub is_error: bool,
    pub(crate) duration_us: i64,
    pub(crate) start_ts: i64,
    pub(crate) end_ts: i64,
    pub(crate) start_iso: String,
    #[schema(value_type = Object)]
    pub(crate) span_attributes: Option<serde_json::Value>,
    #[schema(value_type = Object)]
    pub(crate) events: Option<serde_json::Value>,
    /// Ready-to-run query for the full trace this span belongs to (every span
    /// sharing its `trace_id`, chronological). Filled in Rust.
    pub drill_down_sql: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct LogRecord {
    /// Microseconds since the Unix epoch.
    pub(crate) timestamp: i64,
    pub(crate) timestamp_iso: String,
    pub(crate) severity_text: String,
    pub(crate) body: String,
    pub(crate) trace_id: Option<String>,
    pub(crate) span_id: Option<String>,
    #[schema(value_type = Object)]
    pub(crate) attributes: Option<serde_json::Value>,
}

/// A log pattern with aggregated statistics.
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct LogPattern {
    /// Template or pattern string.
    pub pattern: String,
    /// Template ID (hash) for filtering raw logs. None if no template was extracted.
    pub template_id: Option<u64>,
    /// Pre-built SQL query the agent can run to fetch all raw logs matching
    /// this pattern in chronological order. Uses `template_id` for precise
    /// filtering. Filled in Rust (needs the caller's window + service).
    pub drill_down_sql: Option<String>,
    /// Example log body.
    pub example_body: Option<String>,
    /// Up to 5 distinct extracted-variable tuples (each a JSON array of the
    /// wildcard values for one log line) — enough to see what varies.
    #[schema(value_type = Object)]
    pub sample_values: Option<serde_json::Value>,
    pub(crate) severity_text: String,
    pub count: i64,
    pub(crate) error_count: i64,
    pub(crate) first_seen: Option<String>,
    pub(crate) last_seen: Option<String>,
}

/// One number a service logs about itself, aggregated over the window.
///
/// Free metrics: DRAIN already masks numbers out of every log body to build the
/// template and keeps the original token in `template_values`, so each numeric
/// variable position is a time series nobody had to instrument. The label is the
/// masked template token itself (`translations=<NUM>`), which is why this needs
/// no naming heuristic.
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct LogCounter {
    /// The log template this number appears in.
    pub pattern: String,
    /// The masked template token, e.g. `translations=<NUM>` — its own label.
    pub label: String,
    pub samples: i64,
    pub min_value: f64,
    pub max_value: f64,
    pub avg_value: f64,
    /// Never left zero in the whole window, while another number on the same
    /// log line did. Not ranked as an anomaly (a chronically-flat series is by
    /// definition not a deviation) — it is stated so the question "should this
    /// be zero?" gets asked. That question is the only thing that finds work
    /// which was switched off before the retention window opened.
    pub pinned_zero: bool,
}

/// One finish-reason bucket of a model's LLM calls — the OTel
/// `gen_ai.response.finish_reasons` dimension ("stop", "length",
/// "tool_calls", "content_filter", "error").
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, utoipa::ToSchema)]
pub struct FinishReasonCount {
    pub reason: String,
    /// LLM calls of this (system, model) that finished with this reason.
    pub count: i64,
}

/// Per-(provider, model) LLM call stats, read from the OTel GenAI semconv
/// (`gen_ai.*`, STABLE) span attributes. One row per (system, model) seen
/// in the window; a service that makes no LLM calls has none — the section
/// is simply empty. Threshold-free: ranked by call_count, nothing dropped.
#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct GenAiModelSummary {
    /// The GenAI provider (`gen_ai.system`, e.g. "openai", "anthropic");
    /// "unknown" when the SDK didn't emit it.
    pub system: String,
    /// The model (`gen_ai.request.model`, falling back to
    /// `gen_ai.response.model`).
    pub model: String,
    /// Distinct `gen_ai.operation.name` values seen for this model
    /// (e.g. "chat", "embeddings"), sorted. Empty when never emitted.
    pub operations: Vec<String>,
    /// LLM spans for this (system, model) in the window.
    pub call_count: i64,
    /// Percentage of those spans with OTel status ERROR.
    pub error_pct: f64,
    /// p95 span duration in microseconds.
    pub duration_p95_us: f64,
    /// Sum of `gen_ai.usage.input_tokens` across calls that reported it;
    /// None when no call did (distinct from a real 0).
    pub input_tokens: Option<i64>,
    /// Sum of `gen_ai.usage.output_tokens` (see `input_tokens`).
    pub output_tokens: Option<i64>,
    /// Finish-reason composition of this model's calls, biggest first —
    /// a rising "length"/"content_filter" share is a failure mode
    /// `error_pct` alone never shows. Empty when never emitted.
    pub finish_reasons: Vec<FinishReasonCount>,
}

#[derive(Serialize, Deserialize, Debug, utoipa::ToSchema)]
pub struct ProfileSummary {
    pub function: String,
    pub profile_type: String,
    pub total_value: i64,
    pub(crate) unit: String,
    pub(crate) sample_count: i64,
}

// ---------------------------------------------------------------------------
// SqlPayload — the exact JSON shape the single query produces
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SqlPayload {
    stats: SqlStats,
    context: ServiceContext,
    events: Vec<DerivedEvent>,
    operation_summaries: Vec<SqlOperationRow>,
    /// In-window anomalies whose entity is NOT a span operation (log_volume /
    /// error_log_rate / profile_cost — all keyed by the service) — surfaced
    /// as standalone findings, never joined to `operation_summaries`.
    service_anomalies: Vec<ServiceAnomalyHit>,
    gen_ai: Vec<GenAiModelSummary>,
    metrics: Vec<MetricBucket>,
    spans: Vec<SpanRecord>,
    log_patterns: Vec<LogPattern>,
    log_counters: Vec<LogCounter>,
    logs: Vec<LogRecord>,
    profiles: Vec<ProfileSummary>,
}

/// One operation row exactly as the SQL emits it: the public summary fields
/// plus the in-window `sigiro_anomalies` rows the query LEFT JOINed in,
/// which populate `is_anomalous`/`anomaly_reason` and are never returned raw.
#[derive(Deserialize)]
struct SqlOperationRow {
    #[serde(flatten)]
    summary: OperationSummary,
    #[serde(default)]
    anomalies: Vec<AnomalyHit>,
}

/// One in-window row from the `sigiro_anomalies` table (written by the
/// scheduled pass in `crate::anomaly`), as the diagnosis query emits it.
/// Per operation the hits arrive biggest shift first (the SQL orders by the
/// unit-free magnitude — see [`shift_magnitude`]), so `.first()` is the
/// shift that best represents the operation.
#[derive(Deserialize)]
struct AnomalyHit {
    signal: String,
    /// Bucket where the new regime starts, µs since epoch.
    ts_shift: i64,
    before: f64,
    after: f64,
    /// Dominant derived `error.type` of the shift's NEW regime — the most
    /// common failure mode among the operation's in-window error spans from
    /// `ts_shift` onward. error_rate shifts only; None for other signals or
    /// when the new regime has no error spans in the window. Joined at read
    /// time by the `shift_dominant` CTE — the scheduled pass never computes
    /// it (reader/writer decouple intact).
    #[serde(default)]
    dominant_error_type: Option<String>,
    /// The drill the scheduled pass persisted next to the shift (the new
    /// regime's raw telemetry) — carried into incident member shifts.
    #[serde(default)]
    drill_down_sql: Option<String>,
}

/// One in-window anomaly whose entity is not a span operation (log_volume /
/// error_log_rate / profile_cost), as the `service_anomalies` CTE emits it.
#[derive(Deserialize)]
struct ServiceAnomalyHit {
    /// The entity the series was keyed by — the service name for all three
    /// service-level signals (log_volume / error_log_rate / profile_cost).
    operation: String,
    #[serde(flatten)]
    hit: AnomalyHit,
}

/// UNIT-FREE shift magnitude — the symmetric relative change
/// |after − before| / (|before| + |after|), in (0, 1] for the worsening
/// shifts the pass persists (non-negative series, after > before). Raw
/// |after − before| is meaningless ACROSS signals — a 100000µs latency
/// shift would dwarf a 20-point error-rate shift — while this measure asks
/// the same question of every signal: how big is the jump relative to the
/// level itself? 1/3 ≈ doubled, 3/5 = quadrupled, 1.0 = appeared from a
/// zero baseline. Readers break the zero-baseline ties (everything that
/// appears from nothing scores 1.0) with |after − before|. Used ONLY to
/// rank (biggest first), never to filter: the subsystem stays
/// threshold-free. Mirrors the SQL `ORDER BY` in /v1/anomalies and the
/// `op_anomalies` CTE.
fn shift_magnitude(hit: &AnomalyHit) -> f64 {
    let denom = hit.before.abs() + hit.after.abs();
    if denom == 0.0 {
        return 0.0;
    }
    (hit.after - hit.before).abs() / denom
}

/// Rank key for one shift: the unit-free magnitude, then the raw delta as
/// the zero-baseline tie-break (see [`shift_magnitude`]).
fn shift_rank(hit: &AnomalyHit) -> (f64, f64) {
    (shift_magnitude(hit), (hit.after - hit.before).abs())
}

/// µs rendered for prose: "850µs" / "12.5ms" / "2.1s".
fn fmt_duration_us(us: f64) -> String {
    if us >= 1_000_000.0 {
        format!("{:.1}s", us / 1_000_000.0)
    } else if us >= 1_000.0 {
        format!("{:.1}ms", us / 1_000.0)
    } else {
        format!("{us:.0}µs")
    }
}

/// Prose for one detected shift, e.g.
/// "error rate shifted at 14:32 (5.1% → 23.8%), mostly 'timeout'" /
/// "p95 latency shifted at 14:30 (2.1s → 240.0s)".
fn anomaly_reason(hit: &AnomalyHit) -> String {
    let at = analysis::hhmm_utc(hit.ts_shift);
    match hit.signal.as_str() {
        "error_rate" => {
            let mostly = hit
                .dominant_error_type
                .as_deref()
                .map(|t| format!(", mostly '{t}'"))
                .unwrap_or_default();
            format!(
                "error rate shifted at {at} ({:.1}% → {:.1}%){mostly}",
                hit.before, hit.after
            )
        }
        "latency_p95" => format!(
            "p95 latency shifted at {at} ({} → {})",
            fmt_duration_us(hit.before),
            fmt_duration_us(hit.after)
        ),
        "log_volume" => format!(
            "log volume shifted at {at} ({:.0} → {:.0} logs per 5min)",
            hit.before, hit.after
        ),
        "error_log_rate" => format!(
            "error-log rate shifted at {at} ({:.1}% → {:.1}%)",
            hit.before, hit.after
        ),
        "profile_cost" => format!(
            "profile cost shifted at {at} ({:.0} → {:.0} per 5min)",
            hit.before, hit.after
        ),
        other => format!(
            "{other} shifted at {at} ({:.2} → {:.2})",
            hit.before, hit.after
        ),
    }
}

#[derive(Deserialize)]
struct SqlStats {
    spans_total: i64,
    error_traces_total: i64,
    logs_total: i64,
    log_template_count: i64,
    /// Logs with ERROR-class severity in the window — compared against
    /// `error_traces_total` to detect failures that emit no logs at all.
    error_logs_total: i64,
    /// Dominant derived `error.type` across ALL of the service's in-window
    /// error spans — names the failure mode in the `errors_without_logs`
    /// finding. None when the window has no error spans.
    #[serde(default)]
    dominant_error_type: Option<String>,
    metric_series_count: i64,
    profile_sample_count: i64,
    /// SERVER (2) and CLIENT (3) span counts in the window — a service that
    /// records what it receives but nothing it calls has no egress exporter,
    /// which truncates every trace at that hop.
    server_spans: i64,
    client_spans: i64,
    /// (trace_id, span_id) pairs seen more than once in the window.
    duplicate_span_pairs: i64,
    /// In-window spans whose named parent is absent from the span table.
    orphan_span_count: i64,
}

// ---------------------------------------------------------------------------
// Top-level diagnose orchestration
// ---------------------------------------------------------------------------

pub fn diagnose(
    conn: &Connection,
    req: &DiagnoseRequest,
    prefix: &str,
) -> Result<DiagnoseResponse> {
    let services = resolve_services(req)?;
    // Resolve the window ONCE, before the fan-out: every service in a
    // multi-service diagnosis must describe the same slice of time, and a
    // per-iteration `now` would give each a slightly different one.
    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or_default();
    let (from_ts, to_ts) = req.window(now_micros);
    let mut results = BTreeMap::new();
    let mut errors = BTreeMap::new();
    for service in &services {
        // Partial results: one broken service must not discard the others.
        // Each failure is still loud — logged here, surfaced per-service in
        // `errors`, and fatal when NOTHING succeeded.
        match diagnose_one(conn, service, from_ts, to_ts, prefix) {
            Ok(block) => {
                results.insert(service.clone(), block);
            }
            Err(e) => {
                tracing::error!(service = %service, error = %e,
                    "diagnosis query failed for service"
                );
                errors.insert(service.clone(), format!("{e:#}"));
            }
        }
    }
    if results.is_empty() && !errors.is_empty() {
        anyhow::bail!(
            "diagnosis failed for all services: {}",
            errors.iter().map(|(s, e)| format!("{s}: {e}")).join("; ")
        );
    }
    Ok(DiagnoseResponse { results, errors })
}

fn resolve_services(req: &DiagnoseRequest) -> Result<Vec<String>> {
    let services = match (&req.service, &req.services) {
        (Some(s), None) => vec![s.clone()],
        (None, Some(list)) => list.clone(),
        (Some(s), Some(list)) => std::iter::once(s.clone())
            .chain(list.iter().cloned())
            .unique()
            .collect(),
        (None, None) => anyhow::bail!("either 'service' or 'services' must be provided"),
    };
    anyhow::ensure!(!services.is_empty(), "services list must not be empty");
    anyhow::ensure!(
        services.len() <= MAX_SERVICES,
        "max {MAX_SERVICES} services per diagnosis, got {}",
        services.len()
    );
    Ok(services)
}

/// ONE DuckDB round-trip per service: a stack of CTEs (one per report
/// section) collapsed into a single JSON document, deserialized straight
/// into `DiagnosisBlock`.
#[tracing::instrument(skip(conn), fields(service = %service))]
fn diagnose_one(
    conn: &Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
    prefix: &str,
) -> Result<DiagnosisBlock> {
    let bucket = metric_bucket_interval(from_ts, to_ts);
    let window_us = to_ts - from_ts;
    // Baseline window: 4x the diagnosis window, ending where it starts.
    let baseline_from = from_ts.saturating_sub(window_us.saturating_mul(4));

    let sql = build_diagnosis_sql(prefix, bucket);
    let json: String = conn
        .query_row(
            &sql,
            duckdb::params![service, from_ts, to_ts, baseline_from, from_ts],
            |row| row.get(0),
        )
        .map_err(|e| anyhow::anyhow!("diagnosis query failed: {e}"))?;

    let payload: SqlPayload =
        serde_json::from_str(&json).context("failed to deserialize diagnosis JSON")?;

    let coverage = compute_coverage(
        payload.stats.spans_total,
        payload.stats.log_template_count,
        payload.stats.metric_series_count as usize,
        payload.stats.profile_sample_count,
    );

    // Every drill query is built here in Rust — it needs the caller's service +
    // window (safely quoted), which the SQL doesn't have handy — so it's pure
    // string work, no extra DB round-trip. Each targets /v1/query verbatim, so
    // an agent that spots a lead can execute it directly to go deeper.
    let err = StatusCode::Error.as_i32();
    let svc = service.replace('\'', "''");
    let window = format!("timestamp BETWEEN make_timestamp({from_ts}) AND make_timestamp({to_ts})");
    // Reproduce the win_spans filter exactly so a drilled count reconciles with
    // the `total` we reported (win_spans drops OTLP export self-spans).
    let span_scope = format!(
        "service_name = '{svc}' AND {window} AND span_name NOT LIKE '%TraceService/Export%'"
    );

    // #1: raw logs behind each pattern.
    let mut log_patterns = payload.log_patterns;
    for p in &mut log_patterns {
        p.drill_down_sql = p.template_id.map(|tid| {
            format!(
                "SELECT * FROM {prefix}sigiro_logs \
                 WHERE service_name = '{svc}' AND template_id = {tid} AND {window} \
                 ORDER BY timestamp ASC"
            )
        });
    }

    // Anomaly flags come from the persisted `sigiro_anomalies` table (the
    // continuous pass in `crate::anomaly` writes it) — the query already
    // LEFT JOINed the in-window operation-keyed rows (error_rate /
    // latency_p95) per operation; nothing is recomputed here. Prose is
    // assembled from the joined rows, biggest shift first (the SQL ordered
    // them by the unit-free magnitude), and each op's raw hits are kept
    // (parallel to `operation_summaries`) for the cross-signal correlation
    // below.
    let mut operation_summaries: Vec<OperationSummary> =
        Vec::with_capacity(payload.operation_summaries.len());
    let mut op_hits: Vec<Vec<AnomalyHit>> = Vec::with_capacity(payload.operation_summaries.len());
    for row in payload.operation_summaries {
        let mut op = row.summary;
        op.is_anomalous = !row.anomalies.is_empty();
        op.anomaly_reason = (!row.anomalies.is_empty())
            .then(|| row.anomalies.iter().map(anomaly_reason).join("; "));
        op_hits.push(row.anomalies);
        operation_summaries.push(op);
    }

    // #1: raw spans behind each operation (errors first, then slowest).
    for op in &mut operation_summaries {
        let filter = operation_drill_filter(&op.span_name, &op.operation);
        op.drill_down_sql = Some(format!(
            "SELECT * FROM {prefix}sigiro_spans \
             WHERE {span_scope} AND {filter} \
             ORDER BY CASE WHEN status_code = {err} THEN 0 ELSE 1 END, duration DESC"
        ));
    }

    // #1: the full trace each exemplar span belongs to.
    let mut span_rows = payload.spans;
    for s in &mut span_rows {
        let tid = s.trace_id.replace('\'', "''");
        s.drill_down_sql = Some(format!(
            "SELECT * FROM {prefix}sigiro_spans WHERE trace_id = '{tid}' ORDER BY timestamp ASC"
        ));
    }

    // #2: self-describing sampled sections — the returned sample, the true
    // total, how it was ordered, and the query for the full set.
    let spans = SampledSection {
        total: payload.stats.spans_total,
        sampled_by: "errors first, then longest duration".to_string(),
        drill_all_sql: format!(
            "SELECT * FROM {prefix}sigiro_spans \
             WHERE {span_scope} \
             ORDER BY CASE WHEN status_code = {err} THEN 0 ELSE 1 END, duration DESC"
        ),
        sample: span_rows,
    };
    let logs = SampledSection {
        total: payload.stats.logs_total,
        sampled_by: "most recent first".to_string(),
        drill_all_sql: format!(
            "SELECT * FROM {prefix}sigiro_logs \
             WHERE service_name = '{svc}' AND {window} ORDER BY timestamp DESC"
        ),
        sample: payload.logs,
    };

    // Top-level findings — ranked judgments assembled from data already in
    // hand. The one array an agent reads first. ALL detected anomalies
    // participate — operation-keyed shifts (error_rate / latency_p95, which
    // also flag their operation summary) and standalone service-keyed
    // shifts (log_volume / error_log_rate / profile_cost).
    //
    // CROSS-SIGNAL CORRELATION (P2): shifts in the same or adjacent
    // detection bucket are ONE incident (see
    // `crate::anomaly::correlate_shifts` for why that window is the
    // detector's own resolution, not a threshold), with the member shifts —
    // each keeping its signal, before → after, and drill — nested under one
    // "incident" finding, plus the deploy that landed in/just before the
    // incident (the prime suspect). Shifts that correlate with nothing stay
    // as their single-signal findings. Ranking: incidents and lone shifts
    // sort together by (member count, unit-free magnitude, raw |Δ|) — a
    // shift confirmed by more signals outranks a lone one; magnitude (see
    // `shift_magnitude`; raw |after − before| would be meaningless across
    // signals) breaks ties. A RANKING, never a filter: every detected shift
    // surfaces exactly once; the agent judges significance from the
    // before/after in the summary.
    struct Member {
        entity: String,
        lone_kind: String,
        lone_drill: Option<String>,
        hit: AnomalyHit,
    }
    let mut members: Vec<Member> = Vec::new();
    for (op, hits) in operation_summaries.iter().zip(op_hits) {
        for hit in hits {
            members.push(Member {
                entity: format!("operation '{}'", op.operation),
                lone_kind: "anomalous_operation".to_string(),
                lone_drill: op.drill_down_sql.clone(),
                hit,
            });
        }
    }
    for sa in payload.service_anomalies {
        members.push(Member {
            entity: format!("service '{}'", sa.operation),
            lone_kind: format!("anomalous_{}", sa.hit.signal),
            lone_drill: sa.hit.drill_down_sql.clone(),
            hit: sa.hit,
        });
    }

    let mut findings = Vec::new();
    let shift_ts: Vec<i64> = members.iter().map(|m| m.hit.ts_shift).collect();
    let mut ranked: Vec<((usize, f64, f64), Finding)> = Vec::new();
    for group in crate::anomaly::correlate_shifts(&shift_ts) {
        if let [lone] = group[..] {
            let m = &members[lone];
            let (magnitude, delta) = shift_rank(&m.hit);
            ranked.push((
                (1, magnitude, delta),
                Finding {
                    severity: "critical".to_string(),
                    kind: m.lone_kind.clone(),
                    summary: format!("{}: {}", m.entity, anomaly_reason(&m.hit)),
                    drill_down_sql: m.lone_drill.clone(),
                    incident: None,
                },
            ));
            continue;
        }
        // Members inside the incident go biggest first, matching how
        // everything else ranks.
        let mut by_rank = group.clone();
        by_rank.sort_by(|&a, &b| {
            let (ra, rb) = (shift_rank(&members[a].hit), shift_rank(&members[b].hit));
            rb.0.total_cmp(&ra.0).then(rb.1.total_cmp(&ra.1))
        });
        let started_at = group
            .iter()
            .map(|&i| members[i].hit.ts_shift)
            .min()
            .expect("non-empty");
        let last_shift = group
            .iter()
            .map(|&i| members[i].hit.ts_shift)
            .max()
            .expect("non-empty");
        let mut shifts = Vec::with_capacity(by_rank.len());
        for &i in &by_rank {
            let m = &members[i];
            shifts.push(IncidentShift {
                signal: m.hit.signal.clone(),
                entity: m.entity.clone(),
                ts_shift: m.hit.ts_shift,
                ts_shift_iso: us_to_iso(m.hit.ts_shift)?,
                before: m.hit.before,
                after: m.hit.after,
                summary: anomaly_reason(&m.hit),
                drill_down_sql: m.hit.drill_down_sql.clone(),
            });
        }
        // The suspect deploy: same resolution argument as the member
        // grouping — a deploy is correlated when its bucket falls inside
        // the incident's bucket span or the bucket immediately before the
        // first shift (a deploy at 14:29 causing a shift detected in the
        // 14:30 bucket is one bucket earlier by construction). Closest to
        // the incident start wins when several qualify.
        let bucket_us = crate::anomaly::DETECTION_BUCKET_US;
        let (first_bucket, last_bucket) = (
            started_at.div_euclid(bucket_us),
            last_shift.div_euclid(bucket_us),
        );
        let deploy = payload
            .events
            .iter()
            .filter(|e| e.event_type == "deploy")
            .filter(|e| {
                let b = e.detected_at_us.div_euclid(bucket_us);
                b >= first_bucket - 1 && b <= last_bucket
            })
            .min_by_key(|e| (e.detected_at_us - started_at).abs())
            .cloned();
        let mut summary = format!(
            "incident ~{} on service '{service}': {}",
            analysis::hhmm_utc(started_at),
            shifts
                .iter()
                .map(|s| format!("{}: {}", s.entity, s.summary))
                .join("; ")
        );
        if let Some(d) = &deploy {
            summary.push_str(&format!(
                "; suspect deploy: {} at {}",
                d.description,
                analysis::hhmm_utc(d.detected_at_us)
            ));
        }
        let (magnitude, delta) = shift_rank(&members[by_rank[0]].hit);
        ranked.push((
            (group.len(), magnitude, delta),
            Finding {
                severity: "critical".to_string(),
                kind: "incident".to_string(),
                summary,
                drill_down_sql: None,
                incident: Some(Incident {
                    started_at,
                    started_at_iso: us_to_iso(started_at)?,
                    shifts,
                    deploy,
                }),
            },
        ));
    }
    ranked.sort_by(|a, b| {
        b.0.0
            .cmp(&a.0.0)
            .then(b.0.1.total_cmp(&a.0.1))
            .then(b.0.2.total_cmp(&a.0.2))
    });
    findings.extend(ranked.into_iter().map(|(_, f)| f));
    if payload.stats.error_traces_total > 0 && payload.stats.error_logs_total == 0 {
        // Name the dominant failure mode, and let the drill carry the
        // per-span derived error.type — for log-less failures the span is
        // the ONLY evidence, so the drill should already answer "which
        // failure mode is each of these?".
        let mostly = payload
            .stats
            .dominant_error_type
            .as_deref()
            .map(|t| format!(" (mostly '{t}')"))
            .unwrap_or_default();
        findings.push(Finding {
            severity: "warning".to_string(),
            kind: "errors_without_logs".to_string(),
            summary: format!(
                "{} error trace(s){mostly} but zero ERROR-severity logs in this window — \
                 these failures are only visible via span status/status_message; check \
                 that exceptions are logged and exported",
                payload.stats.error_traces_total
            ),
            drill_down_sql: Some(format!(
                "SELECT {error_type} AS error_type, * FROM {prefix}sigiro_spans \
                 WHERE {span_scope} AND status_code = {err} ORDER BY timestamp DESC",
                error_type = error_type_expr("")
            )),
            incident: None,
        });
    }
    if coverage.has_spans && !coverage.has_logs {
        findings.push(Finding {
            severity: "warning".to_string(),
            kind: "coverage_gap".to_string(),
            summary: "spans present but no logs in this window — export OTLP logs so \
                      errors can be correlated with their messages"
                .to_string(),
            drill_down_sql: None,
            incident: None,
        });
    }
    findings.extend(install_health_findings(&payload.stats, prefix, &span_scope));
    if coverage.has_spans
        && payload.context.namespace.is_none()
        && payload.context.deployment.is_none()
    {
        findings.push(Finding {
            severity: "info".to_string(),
            kind: "missing_resource_attributes".to_string(),
            summary: "no k8s resource attributes (k8s.namespace.name / k8s.deployment.name) \
                      on spans — the context section is empty; enable OTel resource \
                      detectors to fill it"
                .to_string(),
            drill_down_sql: None,
            incident: None,
        });
    }
    findings.sort_by_key(|f| match f.severity.as_str() {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    });

    Ok(DiagnosisBlock {
        findings,
        service: service.to_string(),
        window: Window::new(
            TimestampMicros::from_micros(from_ts),
            TimestampMicros::from_micros(to_ts),
        )?,
        error_traces_total: payload.stats.error_traces_total,
        coverage,
        context: payload.context,
        events: payload.events,
        operation_summaries,
        gen_ai: payload.gen_ai,
        metrics: payload.metrics,
        spans,
        log_patterns,
        log_counters: payload.log_counters,
        logs,
        profiles: payload.profiles,
    })
}

/// Telemetry-pipeline faults, as ranked findings — the question "is what I am
/// reading even trustworthy?", which no other section of the block answers.
/// A duplicated span inflates every count derived from it, an orphaned one
/// severs the trace tree, and a service with SERVER but no CLIENT spans hides
/// a whole downstream hop. Each is a plain count the single query already
/// returned; the drills re-derive the evidence through /v1/query.
///
/// Migrated from the deleted `sigiro diagnose` CLI report, which was the only
/// place these were checked.
fn install_health_findings(stats: &SqlStats, prefix: &str, span_scope: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    if stats.duplicate_span_pairs > 0 {
        findings.push(Finding {
            severity: "warning".to_string(),
            kind: "duplicate_spans".to_string(),
            summary: format!(
                "{} (trace_id, span_id) pair(s) arrived more than once in this window — an \
                 OTLP exporter retry storm or colliding span ids; counts derived from these \
                 traces are inflated",
                stats.duplicate_span_pairs
            ),
            drill_down_sql: Some(format!(
                "SELECT trace_id, span_id, count(*) AS copies FROM {prefix}sigiro_spans \
                 WHERE {span_scope} GROUP BY trace_id, span_id HAVING count(*) > 1 \
                 ORDER BY copies DESC"
            )),
            incident: None,
        });
    }
    if stats.orphan_span_count > 0 {
        findings.push(Finding {
            severity: "warning".to_string(),
            kind: "orphan_spans".to_string(),
            summary: format!(
                "{} span(s) reference a parent that never arrived — a service in these traces \
                 stopped exporting (rollout, network policy, broken init); the trace tree is \
                 severed above them",
                stats.orphan_span_count
            ),
            drill_down_sql: Some(format!(
                "SELECT trace_id, span_id, parent_span_id, span_name FROM {prefix}sigiro_spans s \
                 WHERE {span_scope} AND parent_span_id IS NOT NULL AND parent_span_id != '' \
                 AND NOT EXISTS (SELECT 1 FROM {prefix}sigiro_spans p \
                 WHERE p.trace_id = s.trace_id AND p.span_id = s.parent_span_id) \
                 ORDER BY timestamp DESC"
            )),
            incident: None,
        });
    }
    if stats.server_spans > 0 && stats.client_spans == 0 {
        findings.push(Finding {
            severity: "warning".to_string(),
            kind: "missing_client_spans".to_string(),
            summary: format!(
                "{} SERVER span(s) and zero CLIENT spans — this service records what it \
                 receives but nothing it calls; every trace is truncated at this hop. Check \
                 that the outbound HTTP/DB client is instrumented",
                stats.server_spans
            ),
            drill_down_sql: Some(format!(
                "SELECT span_kind, count(*) AS spans FROM {prefix}sigiro_spans \
                 WHERE {span_scope} GROUP BY span_kind ORDER BY spans DESC"
            )),
            incident: None,
        });
    }
    findings
}

// ---------------------------------------------------------------------------
// The single query
// ---------------------------------------------------------------------------

/// Span names that are bare HTTP methods (OTel HTTP semconv names client
/// spans this way) — their real identity lives in span_attributes.
const HTTP_METHOD_SPAN_NAMES: &[&str] =
    &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

/// The ONE derived-operation expression, shared verbatim between the
/// diagnosis query (grouping) and the per-operation `drill_down_sql`
/// (filtering) so drilled rows always reconcile with the reported counts.
/// `q` is the column qualifier ("s." inside the query, "" in drill SQL).
///
/// A bare HTTP-method span name gets ` <http.route>` (server spans) or
/// ` <server.address>` (client spans) appended when present — NOT url.path,
/// which is unbounded-cardinality (ids in paths). Uses only functions on the
/// /v1/query scalar allowlist (json_extract_string, nullif, coalesce) so the
/// drill SQL passes `validate_read_only`. Shared with the scheduled anomaly
/// pass (`crate::anomaly`) so table rows join back on the SAME identity.
pub(crate) fn operation_key_expr(q: &str) -> String {
    format!(
        "CASE WHEN {q}span_name IN ('GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS') \
         THEN {q}span_name || COALESCE(' ' || COALESCE(\
NULLIF(json_extract_string({q}span_attributes, 'http.route'), ''), \
NULLIF(json_extract_string({q}span_attributes, 'server.address'), '')), '') \
         ELSE {q}span_name END"
    )
}

/// The ONE "this span failed" predicate. `q` is the column qualifier ("s."
/// inside the query, "" in drill SQL).
///
/// Span status is the primary signal but CANNOT be the only one. An SDK that
/// returns a 5xx as a normal success value produces STATUS_CODE_OK spans for
/// real server errors — Effect's `HttpMiddleware.tracer` derives span status
/// from the handler's exit, so a handler returning a 503 response object ends
/// OK. Keying error_rate on span status alone therefore reads every such 5xx as
/// a success and no signal ever shifts (a benchapp endpoint 503'd for a
/// month unnoticed). OTel semconv already treats an `http.response.status_code`
/// of 500 or above as an error, and the tracer records that attribute even when
/// it gets span status wrong — so trust it too rather than depending on every
/// app getting instrumentation right.
///
/// Uses only /v1/query-allowlisted scalars (json_extract_string, nullif,
/// try_cast) so drill SQL still passes `validate_read_only`.
pub(crate) fn is_error_expr(q: &str) -> String {
    let err = StatusCode::Error.as_i32();
    format!(
        "({q}status_code = {err} OR TRY_CAST(NULLIF(json_extract_string(\
{q}span_attributes, 'http.response.status_code'), '') AS INTEGER) >= 500)"
    )
}

/// The ONE derived OTel `error.type` expression (semconv "recording
/// errors", STABLE) — the failure-mode dimension of an error span. Only
/// meaningful for error spans: callers gate on `status_code = 2` themselves
/// (a CASE in `win_spans`, a WHERE in drill SQL). Priority per span:
/// 1. the span's real `error.type` attribute — the standard, wins outright;
/// 2. `http.response.status_code` — OTel's own rule for HTTP status errors:
///    `error.type` IS the code string ("429", "500");
/// 3. 'timeout' when `status_message` mentions one — most SDKs today (e.g.
///    shopfront's) emit only free-text status;
/// 4. '_OTHER' — the semconv catch-all bucket.
///
/// Low-cardinality by construction (attr value | status code | timeout |
/// _OTHER) — a clean grouping key, no thresholds. Uses only /v1/query
/// allowlisted scalars (json_extract_string, nullif, coalesce, lower,
/// contains; CASE is an expression, not a function) so drill SQL embedding
/// it passes `validate_read_only` verbatim. `q` is the column qualifier,
/// exactly as in `operation_key_expr`.
fn error_type_expr(q: &str) -> String {
    format!(
        "COALESCE(\
NULLIF(json_extract_string({q}span_attributes, 'error.type'), ''), \
NULLIF(json_extract_string({q}span_attributes, 'http.response.status_code'), ''), \
CASE WHEN contains(lower(COALESCE({q}status_message, '')), 'timeout') THEN 'timeout' END, \
'_OTHER')"
    )
}

/// The one WHERE fragment that selects exactly one operation's raw spans.
/// Bare-HTTP-method groups must filter on the SAME derived expression the
/// queries grouped by — `span_name = 'POST'` would also match POST spans
/// that were bucketed under "POST some.host" — so drilled rows reconcile
/// exactly with the reported counts. Shared by the per-operation
/// `drill_down_sql` here and by the anomaly pass (`crate::anomaly`).
pub(crate) fn operation_drill_filter(span_name: &str, operation: &str) -> String {
    if HTTP_METHOD_SPAN_NAMES.contains(&span_name) {
        let operation = operation.replace('\'', "''");
        format!("{} = '{operation}'", operation_key_expr(""))
    } else {
        format!("span_name = '{}'", span_name.replace('\'', "''"))
    }
}

/// Builds the one-shot diagnosis query. Five bind parameters, in order:
/// service, from_ts (µs), to_ts (µs), baseline_from (µs), baseline_to (µs).
///
/// Every section is a CTE; the final SELECT packs them into one JSON object
/// via scalar subqueries, so the whole diagnosis is a single round-trip.
pub(crate) fn build_diagnosis_sql(prefix: &str, bucket: &str) -> String {
    let err = StatusCode::Error.as_i32();
    let is_err_s = is_error_expr("s.");
    let op_key = operation_key_expr("s.");
    let error_type = error_type_expr("s.");
    // The in-window span filter, repeated by the CTEs that need columns
    // win_spans deliberately does not carry (see win_spans comment).
    // Duplication marked for later review: SQL has no way to share a
    // predicate without also sharing a projection.
    let span_win = "s.service_name = a.service \
      AND s.timestamp BETWEEN a.from_ts AND a.to_ts \
      AND s.span_name NOT LIKE '%TraceService/Export%'";

    format!(
        r#"
WITH args AS (
    SELECT ?::VARCHAR AS service,
           make_timestamp(?::BIGINT) AS from_ts,
           make_timestamp(?::BIGINT) AS to_ts,
           make_timestamp(?::BIGINT) AS bl_from,
           make_timestamp(?::BIGINT) AS bl_to
),
-- Deliberately NARROW: this CTE is referenced by several sections, so DuckDB
-- materializes it. The wide VARCHAR columns (span_attributes, events_json,
-- resource_attributes) would dominate peak memory; sections that need them
-- (top_spans, deploy_events, svc_meta) hit the base table with the same
-- window predicate instead. `error_type` is DERIVED here at scan time for
-- the same reason: the failure-mode CTEs below get a short low-cardinality
-- string per ERROR span (NULL otherwise) without span_attributes /
-- status_message ever materializing.
win_spans AS (
    SELECT s.trace_id, s.span_name, s.span_kind, s.status_code, s.duration,
           s.timestamp AS ts,
           {op_key} AS operation,
           -- span_attributes is NOT projected here, so derive the error flag
           -- once and let every downstream CTE read `is_error`.
           {is_err_s} AS is_error,
           CASE WHEN {is_err_s} THEN {error_type} END AS error_type
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win}
),
win_logs AS (
    SELECT l.*
    FROM {prefix}sigiro_logs l, args a
    WHERE l.service_name = a.service
      AND l.timestamp BETWEEN a.from_ts AND a.to_ts
),
span_stats AS (
    SELECT count(*) AS spans_total,
           count(DISTINCT CASE WHEN is_error THEN trace_id END) AS error_traces_total,
           -- Span-kind mix, for the missing_client_spans install-health
           -- finding. Free: win_spans already carries span_kind.
           count(CASE WHEN span_kind = 2 THEN 1 END) AS server_spans,
           count(CASE WHEN span_kind = 3 THEN 1 END) AS client_spans
    FROM win_spans
),
-- Install health: the same (trace_id, span_id) arriving more than once — an
-- OTLP exporter retry storm, or an SDK minting colliding ids. Either way the
-- trace is unreliable and every count derived from it is inflated. Hits the
-- base table rather than widening win_spans, which several sections
-- materialize (see its comment).
dup_spans AS (
    SELECT count(*) AS duplicate_span_pairs
    FROM (
        SELECT s.trace_id, s.span_id
        FROM {prefix}sigiro_spans s, args a
        WHERE {span_win}
        GROUP BY s.trace_id, s.span_id
        HAVING count(*) > 1
    )
),
-- Install health: in-window spans naming a parent that never arrived — a
-- service in the trace stopped exporting (rollout, network policy, broken
-- init), leaving the tree severed. The anti-join is over ALL services (the
-- parent is usually a DIFFERENT service; scoping it to this one would call
-- every normal cross-service child an orphan) and bounded to the window ±1h,
-- for the same reason the dependency CTEs are: traces do not span hours, and
-- an unbounded probe scans the full span history on every diagnosis.
orphan_spans AS (
    SELECT count(*) AS orphan_span_count
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win}
      AND s.parent_span_id IS NOT NULL AND s.parent_span_id != ''
      AND NOT EXISTS (
          SELECT 1 FROM {prefix}sigiro_spans p
          WHERE p.trace_id = s.trace_id
            AND p.span_id = s.parent_span_id
            AND p.timestamp BETWEEN a.from_ts - INTERVAL '1 hour'
                                AND a.to_ts + INTERVAL '1 hour'
      )
),
log_stats AS (
    SELECT count(*) AS logs_total,
           -- template_id when DRAIN classified the log, hash(body) otherwise:
           -- keeps the count correct for untemplated logs without holding
           -- every distinct body string in memory.
           count(DISTINCT COALESCE(template_id, hash(body))) AS log_template_count,
           count(CASE WHEN severity_text IN ('ERROR', 'ERR', 'FATAL', 'CRITICAL') THEN 1 END) AS error_logs_total
    FROM win_logs
),
profile_stats AS (
    SELECT count(*) AS profile_sample_count
    FROM {prefix}sigiro_profiles p, args a
    WHERE p.service_name = a.service
      AND p.timestamp BETWEEN a.from_ts AND a.to_ts
),
metric_points AS (
    SELECT m.timestamp, m.metric_name, m.value AS p95_src, m.value AS avg_src, 1::BIGINT AS cnt
    FROM {prefix}sigiro_metrics_gauge m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
    UNION ALL
    SELECT m.timestamp, m.metric_name, m.value, m.value, 1::BIGINT
    FROM {prefix}sigiro_metrics_sum m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
    UNION ALL
    SELECT m.timestamp, m.metric_name, m.max,
           CASE WHEN m.count > 0 THEN m.sum / m.count ELSE 0 END, m.count::BIGINT
    FROM {prefix}sigiro_metrics_histogram m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
    UNION ALL
    SELECT m.timestamp, m.metric_name, m.max,
           CASE WHEN m.count > 0 THEN m.sum / m.count ELSE 0 END, m.count::BIGINT
    FROM {prefix}sigiro_metrics_exp_histogram m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
),
metrics AS (
    SELECT time_bucket(INTERVAL '{bucket}', timestamp)::VARCHAR AS bucket,
           metric_name,
           approx_quantile(p95_src, 0.95) AS p95,
           avg(avg_src) AS avg,
           sum(cnt)::BIGINT AS n
    FROM metric_points
    GROUP BY bucket, metric_name
),
ops_current AS (
    SELECT operation,
           any_value(span_name) AS span_name,
           count(*) AS cnt,
           count(CASE WHEN is_error THEN 1 END) AS err_cnt,
           count(CASE WHEN is_error THEN 1 END) * 100.0 / count(*) AS err_pct,
           approx_quantile(duration, 0.50) AS p50,
           approx_quantile(duration, 0.95) AS p95,
           approx_quantile(duration, 0.99) AS p99,
           any_value(CASE WHEN is_error THEN trace_id END) AS exemplar_error_trace_id
    FROM win_spans
    GROUP BY operation
),
ops_baseline AS (
    SELECT {op_key} AS operation,
           count(*) AS bl_cnt,
           count(CASE WHEN {is_err_s} THEN 1 END) * 100.0 / count(*) AS bl_err_pct,
           approx_quantile(s.duration, 0.95) AS bl_p95
    FROM {prefix}sigiro_spans s, args a
    WHERE s.service_name = a.service
      AND s.timestamp BETWEEN a.bl_from AND a.bl_to
      AND s.span_name NOT LIKE '%TraceService/Export%'
    GROUP BY operation
),
-- Failure-mode composition (OTel `error.type`, derived per span in
-- win_spans) of each operation's ERROR spans. Threshold-free: a breakdown
-- ordered by count, never a cutoff.
op_error_types AS (
    SELECT operation, error_type, count(*) AS cnt
    FROM win_spans
    WHERE error_type IS NOT NULL
    GROUP BY operation, error_type
),
op_error_breakdown AS (
    SELECT operation,
           to_json(list(json_object('error_type', error_type, 'count', cnt)
                        ORDER BY cnt DESC, error_type)) AS error_breakdown
    FROM op_error_types
    GROUP BY operation
),
-- LLM calls per the OTel GenAI semconv (`gen_ai.*` span attributes,
-- STABLE): one row per span carrying a model. Derived at scan time for the
-- same reason win_spans derives error_type — the downstream CTEs get a few
-- short columns per LLM span and span_attributes never materializes.
-- Spans without a model (the non-LLM overwhelming majority) are dropped
-- here, so the whole section costs nothing and is empty for non-LLM
-- services. Extraction uses only /v1/query-allowlisted scalars
-- (json_extract_string, json_extract, coalesce, nullif) plus TRY_CAST —
-- token counts arrive as JSON strings, TRY_CAST turns garbage into NULL
-- instead of failing the whole diagnosis.
gen_ai_spans AS (
    SELECT COALESCE(NULLIF(json_extract_string(s.span_attributes, 'gen_ai.system'), ''),
                    'unknown') AS system,
           COALESCE(
               NULLIF(json_extract_string(s.span_attributes, 'gen_ai.request.model'), ''),
               NULLIF(json_extract_string(s.span_attributes, 'gen_ai.response.model'), '')
           ) AS model,
           NULLIF(json_extract_string(s.span_attributes, 'gen_ai.operation.name'), '') AS operation,
           s.status_code, s.duration,
           TRY_CAST(json_extract_string(s.span_attributes, 'gen_ai.usage.input_tokens') AS BIGINT) AS input_tokens,
           TRY_CAST(json_extract_string(s.span_attributes, 'gen_ai.usage.output_tokens') AS BIGINT) AS output_tokens,
           -- Semconv says finish_reasons is a string ARRAY; tolerate SDKs
           -- that emit a plain string (the TRY_CAST is NULL then, and the
           -- fallback wraps the scalar). Missing → [NULL], filtered below.
           COALESCE(
               TRY_CAST(json_extract(s.span_attributes, 'gen_ai.response.finish_reasons') AS VARCHAR[]),
               [NULLIF(json_extract_string(s.span_attributes, 'gen_ai.response.finish_reasons'), '')]
           ) AS finish_reasons
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win} AND model IS NOT NULL
),
gen_ai_models AS (
    SELECT system, model,
           count(*) AS call_count,
           count(CASE WHEN status_code = {err} THEN 1 END) * 100.0 / count(*) AS error_pct,
           approx_quantile(duration, 0.95) AS p95,
           -- sum skips NULLs and is NULL when every call lacked the attr —
           -- "not reported" stays distinct from a real 0.
           sum(input_tokens) AS input_tokens,
           sum(output_tokens) AS output_tokens
    FROM gen_ai_spans
    GROUP BY system, model
),
gen_ai_operations AS (
    SELECT system, model, list(operation ORDER BY operation) AS operations
    FROM (SELECT DISTINCT system, model, operation FROM gen_ai_spans WHERE operation IS NOT NULL)
    GROUP BY system, model
),
gen_ai_finish AS (
    SELECT system, model,
           to_json(list(json_object('reason', reason, 'count', cnt)
                        ORDER BY cnt DESC, reason)) AS finish_reasons
    FROM (
        SELECT system, model, reason, count(*) AS cnt
        FROM (SELECT system, model, unnest(finish_reasons) AS reason FROM gen_ai_spans)
        WHERE reason IS NOT NULL
        GROUP BY system, model, reason
    )
    GROUP BY system, model
),
-- Dominant failure mode of each in-window error_rate shift's NEW regime:
-- the most common derived error.type among that operation's error spans
-- from ts_shift onward. Computed at READ time from spans win_spans already
-- scanned — the scheduled pass (src/anomaly.rs) never computes or persists
-- it, so the reader/writer decouple stays intact.
shift_dominant AS (
    SELECT operation, ts_shift,
           first(error_type ORDER BY cnt DESC, error_type) AS dominant_error_type
    FROM (
        SELECT an.operation, an.ts_shift, w.error_type, count(*) AS cnt
        FROM {prefix}sigiro_anomalies an
        JOIN args a ON an.service_name = a.service
        JOIN win_spans w ON w.operation = an.operation AND w.ts >= an.ts_shift
        WHERE an.signal = 'error_rate'
          AND an.ts_shift BETWEEN a.from_ts AND a.to_ts
          AND w.error_type IS NOT NULL
        GROUP BY an.operation, an.ts_shift, w.error_type
    )
    GROUP BY operation, ts_shift
),
-- Precomputed OPERATION-KEYED anomalies (error_rate / latency_p95 — the
-- signals whose entity is a span operation) for this service whose regime
-- shift falls inside the diagnosis window, grouped per operation,
-- biggest shift first by the unit-free symmetric relative change
-- (after − before) / (after + before), |after − before| breaking ties —
-- ranking, not a threshold; nothing is dropped. The `sigiro_anomalies`
-- table is written by the scheduled pass (src/anomaly.rs) — diagnose
-- only READS it; no changepoint detection runs per request.
op_anomalies AS (
    SELECT an.operation,
           to_json(list(json_object(
               'signal', an.signal, 'kind', an.kind,
               'ts_shift', epoch_us(an.ts_shift),
               'before', an."before", 'after', an."after",
               'dominant_error_type', sd.dominant_error_type,
               'drill_down_sql', an.drill_down_sql
           ) ORDER BY abs(an."after" - an."before")
                          / nullif(abs(an."before") + abs(an."after"), 0) DESC,
                      abs(an."after" - an."before") DESC)) AS anomalies
    FROM {prefix}sigiro_anomalies an
    LEFT JOIN shift_dominant sd
        ON sd.operation = an.operation AND sd.ts_shift = an.ts_shift, args a
    WHERE an.service_name = a.service
      AND an.signal IN ('error_rate', 'latency_p95')
      AND an.ts_shift BETWEEN a.from_ts AND a.to_ts
    GROUP BY an.operation
),
-- The NON-operation-keyed anomalies for this service in the same window:
-- log_volume / error_log_rate / profile_cost, all keyed by the service
-- (carried in `operation`). No operation summary to attach to — they
-- surface as standalone top-level findings, ranked in Rust together with
-- the operation anomalies by the same unit-free magnitude.
service_anomalies AS (
    SELECT an.operation, an.signal, epoch_us(an.ts_shift) AS ts_shift,
           an."before", an."after", an.drill_down_sql
    FROM {prefix}sigiro_anomalies an, args a
    WHERE an.service_name = a.service
      AND an.signal NOT IN ('error_rate', 'latency_p95')
      AND an.ts_shift BETWEEN a.from_ts AND a.to_ts
),
-- is_anomalous / anomaly_reason are assembled in Rust from the joined
-- `anomalies` rows. This CTE only gathers the evidence: current aggregates,
-- baseline aggregates, and the in-window anomaly rows.
operation_summaries AS (
    SELECT c.operation, c.span_name, c.cnt, c.err_cnt, c.err_pct,
           c.p50, c.p95, c.p99, c.exemplar_error_trace_id,
           b.bl_err_pct, b.bl_p95, b.bl_cnt,
           COALESCE(eb.error_breakdown, '[]'::JSON) AS error_breakdown,
           COALESCE(oa.anomalies, '[]'::JSON) AS anomalies
    FROM ops_current c
    LEFT JOIN ops_baseline b USING (operation)
    LEFT JOIN op_error_breakdown eb USING (operation)
    LEFT JOIN op_anomalies oa USING (operation)
    ORDER BY c.err_cnt DESC, c.cnt DESC
    LIMIT 20
),
top_spans AS (
    SELECT s.trace_id, s.span_id, COALESCE(s.parent_span_id, '') AS parent_span_id,
           s.span_name, s.span_kind, s.status_code, s.status_message,
           s.duration AS duration_us,
           epoch_us(s.timestamp) AS start_ts,
           s.end_timestamp AS end_ts,
           s.timestamp::VARCHAR AS start_iso,
           s.span_attributes, s.events_json
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win}
    ORDER BY CASE WHEN s.status_code = {err} THEN 0 ELSE 1 END, s.duration DESC
    LIMIT 25
),
raw_logs AS (
    SELECT epoch_us(timestamp) AS ts_us,
           timestamp::VARCHAR AS timestamp_iso,
           COALESCE(severity_text, 'INFO') AS severity_text,
           body, trace_id, span_id, log_attributes
    FROM win_logs
    ORDER BY timestamp DESC
    LIMIT 50
),
log_patterns AS (
    SELECT COALESCE(NULLIF(t.template_text, ''), l.body) AS pattern,
           any_value(l.template_id) AS template_id,
           any_value(l.body) AS example_body,
           l.severity_text,
           count(*) AS cnt,
           count(CASE WHEN l.severity_text IN ('ERROR', 'ERR', 'FATAL', 'CRITICAL') THEN 1 END) AS error_count,
           min(l.timestamp)::VARCHAR AS first_seen,
           max(l.timestamp)::VARCHAR AS last_seen,
           list_slice(
               list(DISTINCT l.template_values)
                   FILTER (WHERE l.template_values IS NOT NULL AND l.template_values != '[]'),
               1, 5) AS sample_values
    FROM win_logs l
    LEFT JOIN {prefix}sigiro_log_templates t ON l.template_id = t.template_id
    GROUP BY pattern, l.severity_text
    ORDER BY cnt DESC
    LIMIT 50
),
-- Every number the service logs about itself, promoted to a statistic.
-- `template_values` holds the ORIGINAL token at each variable position of the
-- template, in order, so zipping the template's variable tokens against it
-- pairs each value with its own label. Two `unnest`es in one projection expand
-- row-wise in DuckDB, which is the zip.
log_counter_pairs AS (
    SELECT COALESCE(NULLIF(t.template_text, ''), l.body) AS pattern,
           unnest(list_filter(
               str_split(COALESCE(NULLIF(t.template_text, ''), l.body), ' '),
               x -> x = '<*>' OR x LIKE '%<NUM>%' OR x LIKE '%<IP>%'
                    OR x LIKE '%<UUID>%' OR x LIKE '%<HEX>%'
           )) AS label,
           unnest(from_json(l.template_values, '["VARCHAR"]')) AS raw_value
    FROM win_logs l
    LEFT JOIN {prefix}sigiro_log_templates t ON l.template_id = t.template_id
    WHERE l.template_values IS NOT NULL AND l.template_values != '[]'
),
log_counters AS (
    SELECT pattern, label, samples, min_value, max_value, avg_value,
           -- A zero that stayed zero while a SIBLING number on the same line
           -- moved. Self-inclusion is harmless: when max_value = 0 any
           -- partition max above zero can only have come from a sibling.
           (max_value = 0 AND max(max_value) OVER (PARTITION BY pattern) > 0) AS pinned_zero
    FROM (
        SELECT pattern, label,
               count(*) AS samples,
               min(v) AS min_value,
               max(v) AS max_value,
               round(avg(v), 3) AS avg_value
        FROM (
            -- Only <NUM> positions are counters, and the value token carries its
            -- label too (`translations=0`), so the number has to come back out.
            --
            -- The TEMPLATE is the authority on where the number starts and ends,
            -- never a rescan of the raw token. Masking already decided whether a
            -- leading `-` belongs to the number or to the identifier, and only
            -- the template records that verdict: `code=-1` masks to `code=<NUM>`
            -- (minus inside the mask → negative) while `shard-1` masks to
            -- `shard-<NUM>` (minus survives as a constant → positive). A
            -- `-?\d+` rescan cannot tell those apart and read shard-5 as -5.
            -- Slicing off the template's own literal prefix/suffix cannot:
            -- `latency=<NUM>ms` over `latency=42.5ms` yields exactly `42.5`.
            --
            -- Exactly ONE <NUM> per token, which is also what makes the slice
            -- well-defined: a version-like identifier masks to
            -- `command-a-<NUM>-<NUM>`, where there is no single number to take,
            -- so drop it rather than invent one. A model name is not a counter.
            -- A length mismatch slices to '' and TRY_CAST drops the row — the
            -- surprising case yields no counter instead of a wrong one.
            SELECT pattern, label,
                   TRY_CAST(substring(
                       raw_value,
                       len(split_part(label, '<NUM>', 1)) + 1,
                       len(raw_value) - len(split_part(label, '<NUM>', 1))
                                      - len(split_part(label, '<NUM>', 2))
                   ) AS DOUBLE) AS v
            FROM log_counter_pairs
            WHERE len(str_split(label, '<NUM>')) = 2
        )
        WHERE v IS NOT NULL
        GROUP BY pattern, label
    )
    ORDER BY pinned_zero DESC, samples DESC
    LIMIT 40
),
deploy_events AS (
    -- Anti-join against the baseline window: a version already running
    -- before from_ts is steady state, not a deploy — without this, every
    -- diagnosis fabricated a "deploy" event at window start, exactly
    -- the false lead an agent seizes on.
    SELECT min(detected_at) AS detected_at,
           min(detected_at_us) AS detected_at_us,
           'deploy' AS event_type,
           'version ' || version || ' first seen' AS description
    FROM (
        SELECT s.timestamp::VARCHAR AS detected_at,
               epoch_us(s.timestamp) AS detected_at_us,
               s.resource_attributes::JSON->>'service.version' AS version
        FROM {prefix}sigiro_spans s, args a
        WHERE {span_win}
    )
    WHERE version IS NOT NULL
      AND version NOT IN (
          SELECT (s.resource_attributes::JSON->>'service.version')
          FROM {prefix}sigiro_spans s, args a
          WHERE s.service_name = a.service
            AND s.timestamp BETWEEN a.bl_from AND a.bl_to
            AND (s.resource_attributes::JSON->>'service.version') IS NOT NULL
      )
    GROUP BY version
),
k8s_events AS (
    SELECT timestamp::VARCHAR AS detected_at,
           epoch_us(timestamp) AS detected_at_us,
           'k8s_event' AS event_type,
           body AS description
    FROM win_logs
    WHERE body LIKE '%OOMKilled%' OR body LIKE '%Unhealthy%'
       OR body LIKE '%BackOff%' OR body LIKE '%FailedScheduling%'
       OR body LIKE '%ScalingReplicaSet%'
    LIMIT 20
),
events AS (
    SELECT * FROM deploy_events
    UNION ALL
    SELECT * FROM k8s_events
),
profiles AS (
    SELECT p.stack_trace[len(p.stack_trace)] AS function,
           COALESCE(p.profile_type, 'unknown') AS profile_type,
           sum(p.value)::BIGINT AS total_value,
           COALESCE(any_value(p.unit), '') AS unit,
           count(*) AS sample_count
    FROM {prefix}sigiro_profiles p, args a
    WHERE p.service_name = a.service
      AND p.timestamp BETWEEN a.from_ts AND a.to_ts
      AND len(p.stack_trace) > 0
    GROUP BY function, profile_type
    ORDER BY total_value DESC
    LIMIT 20
),
svc_meta AS (
    SELECT any_value(NULLIF(s.resource_attributes::JSON->>'k8s.namespace.name', '')) AS namespace,
           any_value(NULLIF(s.resource_attributes::JSON->>'k8s.deployment.name', '')) AS deployment,
           count(DISTINCT s.service_instance_id) AS pod_count,
           count(DISTINCT s.service_instance_id) > 0 AS has_instance_id,
           min(s.timestamp)::VARCHAR AS first_seen,
           max(s.timestamp)::VARCHAR AS last_seen
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win}
),
-- Dependency CTEs join on pre-DISTINCTed trace ids: (a) each foreign span is
-- counted exactly once (the old span-to-span join multiplied counts by the
-- number of our spans in the trace), and (b) the foreign side is bounded to
-- the window ±1h — traces don't span hours, and the old unbounded join
-- scanned the FULL span history on every diagnosis.
client_traces AS (
    SELECT DISTINCT trace_id FROM win_spans WHERE span_kind IN (3, 4)
),
svc_traces AS (
    SELECT DISTINCT trace_id FROM win_spans
),
deps AS (
    SELECT s2.service_name,
           count(*) AS span_count,
           count(CASE WHEN s2.status_code = {err} THEN 1 END) AS error_count,
           count(CASE WHEN s2.status_code = {err} THEN 1 END) * 100.0 / count(*) AS error_rate
    FROM {prefix}sigiro_spans s2
    JOIN client_traces ct ON s2.trace_id = ct.trace_id, args a
    WHERE s2.service_name != a.service
      AND s2.timestamp BETWEEN a.from_ts - INTERVAL '1 hour' AND a.to_ts + INTERVAL '1 hour'
    GROUP BY s2.service_name
    ORDER BY span_count DESC
    LIMIT 20
),
dependents AS (
    SELECT s1.service_name,
           count(*) AS span_count,
           count(CASE WHEN s1.status_code = {err} THEN 1 END) AS error_count,
           count(CASE WHEN s1.status_code = {err} THEN 1 END) * 100.0 / count(*) AS error_rate
    FROM {prefix}sigiro_spans s1
    JOIN svc_traces st ON s1.trace_id = st.trace_id, args a
    WHERE s1.service_name != a.service
      AND s1.span_kind IN (3, 4)
      AND s1.span_name NOT LIKE '%TraceService/Export%'
      AND s1.timestamp BETWEEN a.from_ts - INTERVAL '1 hour' AND a.to_ts + INTERVAL '1 hour'
    GROUP BY s1.service_name
    ORDER BY span_count DESC
    LIMIT 20
)
SELECT json_object(
    'stats', json_object(
        'spans_total', (SELECT spans_total FROM span_stats),
        'error_traces_total', (SELECT error_traces_total FROM span_stats),
        'logs_total', (SELECT logs_total FROM log_stats),
        'log_template_count', (SELECT log_template_count FROM log_stats),
        'error_logs_total', (SELECT error_logs_total FROM log_stats),
        'dominant_error_type', (SELECT first(error_type ORDER BY cnt DESC, error_type)
            FROM (SELECT error_type, sum(cnt) AS cnt FROM op_error_types GROUP BY error_type)),
        'metric_series_count', (SELECT count(DISTINCT metric_name) FROM metric_points),
        'profile_sample_count', (SELECT profile_sample_count FROM profile_stats),
        'server_spans', (SELECT server_spans FROM span_stats),
        'client_spans', (SELECT client_spans FROM span_stats),
        'duplicate_span_pairs', (SELECT duplicate_span_pairs FROM dup_spans),
        'orphan_span_count', (SELECT orphan_span_count FROM orphan_spans)
    ),
    'context', json_object(
        'service_name', (SELECT service FROM args),
        'first_seen', (SELECT first_seen FROM svc_meta),
        'last_seen', (SELECT last_seen FROM svc_meta),
        'namespace', (SELECT namespace FROM svc_meta),
        'deployment', (SELECT deployment FROM svc_meta),
        'pod_count', (SELECT pod_count FROM svc_meta),
        'has_instance_id', (SELECT has_instance_id FROM svc_meta),
        'dependencies', (SELECT COALESCE(to_json(list(json_object(
            'service_name', service_name, 'span_count', span_count,
            'error_count', error_count, 'error_rate', error_rate
        ) ORDER BY span_count DESC)), '[]'::JSON) FROM deps),
        'dependents', (SELECT COALESCE(to_json(list(json_object(
            'service_name', service_name, 'span_count', span_count,
            'error_count', error_count, 'error_rate', error_rate
        ) ORDER BY span_count DESC)), '[]'::JSON) FROM dependents)
    ),
    'events', (SELECT COALESCE(to_json(list(json_object(
        'detected_at', detected_at, 'detected_at_us', detected_at_us,
        'event_type', event_type, 'description', description
    ) ORDER BY detected_at)), '[]'::JSON) FROM events),
    'operation_summaries', (SELECT COALESCE(to_json(list(json_object(
        'operation', operation,
        'span_name', span_name, 'count', cnt, 'error_count', err_cnt, 'error_pct', err_pct,
        'error_breakdown', error_breakdown,
        'duration_p50_us', p50, 'duration_p95_us', p95, 'duration_p99_us', p99,
        'exemplar_error_trace_id', exemplar_error_trace_id,
        'baseline_error_pct', bl_err_pct, 'baseline_p95_us', bl_p95, 'baseline_count', bl_cnt,
        'anomalies', anomalies
    ) ORDER BY err_cnt DESC, cnt DESC)), '[]'::JSON) FROM operation_summaries),
    'service_anomalies', (SELECT COALESCE(to_json(list(json_object(
        'operation', operation, 'signal', signal, 'ts_shift', ts_shift,
        'before', "before", 'after', "after", 'drill_down_sql', drill_down_sql
    ))), '[]'::JSON) FROM service_anomalies),
    'gen_ai', (SELECT COALESCE(to_json(list(json_object(
        'system', m.system, 'model', m.model,
        'operations', COALESCE(o.operations, []::VARCHAR[]),
        'call_count', m.call_count, 'error_pct', m.error_pct,
        'duration_p95_us', m.p95,
        'input_tokens', m.input_tokens, 'output_tokens', m.output_tokens,
        'finish_reasons', COALESCE(f.finish_reasons, '[]'::JSON)
    ) ORDER BY m.call_count DESC, m.system, m.model)), '[]'::JSON)
        FROM gen_ai_models m
        LEFT JOIN gen_ai_operations o USING (system, model)
        LEFT JOIN gen_ai_finish f USING (system, model)),
    'metrics', (SELECT COALESCE(to_json(list(json_object(
        'bucket', bucket, 'metric_name', metric_name, 'p95', p95, 'avg', avg, 'count', n
    ) ORDER BY bucket, metric_name)), '[]'::JSON) FROM metrics),
    'spans', (SELECT COALESCE(to_json(list(json_object(
        'trace_id', trace_id, 'span_id', span_id, 'parent_span_id', parent_span_id,
        'span_name', span_name, 'span_kind', span_kind, 'status_code', status_code,
        'status_message', status_message,
        'is_error', status_code = {err}, 'duration_us', duration_us,
        'start_ts', start_ts, 'end_ts', end_ts, 'start_iso', start_iso,
        'span_attributes', TRY_CAST(span_attributes AS JSON),
        'events', TRY_CAST(events_json AS JSON)
    ) ORDER BY CASE WHEN status_code = {err} THEN 0 ELSE 1 END, duration_us DESC)),
        '[]'::JSON) FROM top_spans),
    'log_patterns', (SELECT COALESCE(to_json(list(json_object(
        'pattern', pattern, 'template_id', template_id, 'example_body', example_body,
        'severity_text', severity_text, 'count', cnt, 'error_count', error_count,
        'first_seen', first_seen, 'last_seen', last_seen,
        'sample_values', TRY_CAST(sample_values AS JSON[])
    ) ORDER BY cnt DESC)), '[]'::JSON) FROM log_patterns),
    'log_counters', (SELECT COALESCE(to_json(list(json_object(
        'pattern', pattern, 'label', label, 'samples', samples,
        'min_value', min_value, 'max_value', max_value, 'avg_value', avg_value,
        'pinned_zero', pinned_zero
    ) ORDER BY pinned_zero DESC, samples DESC)), '[]'::JSON) FROM log_counters),
    'logs', (SELECT COALESCE(to_json(list(json_object(
        'timestamp', ts_us, 'timestamp_iso', timestamp_iso, 'severity_text', severity_text,
        'body', body, 'trace_id', trace_id, 'span_id', span_id,
        'attributes', TRY_CAST(log_attributes AS JSON)
    ) ORDER BY ts_us DESC)), '[]'::JSON) FROM raw_logs),
    'profiles', (SELECT COALESCE(to_json(list(json_object(
        'function', function, 'profile_type', profile_type, 'total_value', total_value,
        'unit', unit, 'sample_count', sample_count
    ) ORDER BY total_value DESC)), '[]'::JSON) FROM profiles)
)::VARCHAR AS diagnosis
"#
    )
}

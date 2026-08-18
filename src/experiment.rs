//! Controlled inference experiment comparison.
//!
//! AIPerf owns workload replay, SLO goodput, search, and confidence intervals.
//! Sigiro verifies that every declared period has matching tenant-scoped OTLP
//! evidence, then applies one fixed confidence-interval comparison rule.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::anyhow;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

const SUPPORTED_AIPERF_SCHEMA: &str = "1.0";
const SUPPORTED_AIPERF_VERSION: &str = "0.12.0";
const AIPERF_GOODPUT_METRIC: &str = "goodput_avg";
const AIPERF_REQUEST_COUNT_METRIC: &str = "request_count_avg";
const AIPERF_GOOD_REQUEST_COUNT_METRIC: &str = "good_request_count_avg";
const AIPERF_REQUEST_ERROR_RATE_METRIC: &str = "request_error_rate_avg";
const AIPERF_ERROR_REQUEST_COUNT_METRIC: &str = "error_request_count_avg";
const REQUIRED_METRICS: [&str; 4] = [
    "gen_ai.client.operation.duration",
    "gen_ai.client.operation.time_to_first_chunk",
    "gen_ai.client.operation.time_per_output_chunk",
    "gen_ai.client.token.usage",
];
const MICROS_MIN: i64 = 1_000_000_000_000_000;
const MICROS_MAX: i64 = 10_000_000_000_000_000;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExperimentError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ExperimentError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// One complete A/A calibration followed by an A/B/B/A reversal experiment.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema, validator::Validate)]
#[validate(schema(function = "validate_experiment_request", skip_on_field_errors = true))]
pub struct ExperimentComparisonRequest {
    /// Stable experiment identifier carried on every AIPerf OTLP metric.
    #[schema(example = "exp-2026-08-16-001")]
    pub experiment_id: String,
    /// SHA-256 of the exact model and weights.
    pub model_fingerprint: String,
    /// SHA-256 of the GPU, driver, topology, and host description.
    pub hardware_fingerprint: String,
    /// SHA-256 of AIPerf's replayed `inputs.json`.
    pub workload_fingerprint: String,
    /// SHA-256 of the pinned AIPerf command and version.
    pub benchmark_fingerprint: String,
    /// SHA-256 of the control runtime configuration.
    pub control_config_fingerprint: String,
    /// SHA-256 of the candidate runtime configuration.
    pub candidate_config_fingerprint: String,
    /// Two identical control periods that establish the restart noise floor.
    pub calibration_control: [ExperimentPeriod; 2],
    /// Control immediately before treatment.
    pub control_before: ExperimentPeriod,
    /// Two candidate periods; both must agree.
    pub candidate: [ExperimentPeriod; 2],
    /// Restored control after treatment.
    pub control_after: ExperimentPeriod,
}

/// One process lifecycle and its AIPerf aggregate confidence artifact.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ExperimentPeriod {
    /// Unique run identifier carried on every AIPerf OTLP metric.
    pub run_id: String,
    /// Inclusive window start, in microseconds since the Unix epoch.
    #[schema(example = 1780000000000000i64)]
    pub from_ts: i64,
    /// Inclusive window end, in microseconds since the Unix epoch.
    #[schema(example = 1780000300000000i64)]
    pub to_ts: i64,
    /// Unmodified `profile_export_aiperf_aggregate.json` content.
    pub aiperf: AiperfAggregateSummary,
}

/// Supported subset of AIPerf's aggregate confidence export.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AiperfAggregateSummary {
    pub schema_version: String,
    pub aiperf_version: String,
    pub metadata: AiperfAggregateMetadata,
    /// AIPerf metrics required by this comparison contract. Additive metrics are ignored.
    pub metrics: AiperfAggregateMetrics,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AiperfAggregateMetrics {
    #[schema(required = true, nullable = false)]
    pub goodput_avg: Option<AiperfConfidenceMetric>,
    #[schema(required = true, nullable = false)]
    pub request_count_avg: Option<AiperfConfidenceMetric>,
    #[schema(required = true, nullable = false)]
    pub good_request_count_avg: Option<AiperfConfidenceMetric>,
    /// AIPerf omits both error metrics on clean runs.
    pub request_error_rate_avg: Option<AiperfConfidenceMetric>,
    /// AIPerf omits this `ERROR_ONLY` metric on clean runs.
    pub error_request_count_avg: Option<AiperfConfidenceMetric>,
    /// Future AIPerf metrics do not break this pinned comparison contract.
    #[serde(flatten)]
    #[schema(ignore)]
    pub additional: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AiperfAggregateMetadata {
    pub aggregation_type: String,
    pub num_profile_runs: u32,
    pub num_successful_runs: u32,
    /// Must be empty. Details remain owned by the source AIPerf artifact.
    #[schema(value_type = Vec<Object>)]
    pub failed_runs: Vec<Value>,
}

/// One aggregate confidence metric from AIPerf 0.12.0.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct AiperfConfidenceMetric {
    pub mean: f64,
    pub std: f64,
    pub min: f64,
    pub max: f64,
    /// AIPerf writes `null` when the mean is zero and CV is mathematically infinite.
    pub cv: Option<f64>,
    pub se: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub t_critical: f64,
    pub unit: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentOutcome {
    Confirmed,
    Rejected,
    Inconclusive,
    DriftDetected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentWinner {
    Control,
    Candidate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentPeriodRole {
    CalibrationControlOne,
    CalibrationControlTwo,
    ControlBefore,
    CandidateOne,
    CandidateTwo,
    ControlAfter,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ExperimentComparisonResponse {
    pub experiment_id: String,
    pub outcome: ExperimentOutcome,
    pub winner: Option<ExperimentWinner>,
    pub reason: String,
    /// Fixed in this API version. AIPerf computes it under the declared SLO.
    pub objective: String,
    pub unit: String,
    pub periods: Vec<ComparedPeriod>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ComparedPeriod {
    pub role: ExperimentPeriodRole,
    pub run_id: String,
    pub goodput_mean: f64,
    pub goodput_ci_low: f64,
    pub goodput_ci_high: f64,
    pub telemetry_rows: i64,
    pub metric_names: Vec<String>,
    pub observed_from_ts: i64,
    pub observed_to_ts: i64,
    /// A directly runnable `/v1/query` statement for this period's evidence.
    pub evidence_sql: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ExperimentEvidence {
    periods: BTreeMap<String, PeriodEvidence>,
}

#[derive(Clone, Debug)]
struct PeriodEvidence {
    rows: i64,
    metric_names: BTreeSet<String>,
    observed_from_ts: i64,
    observed_to_ts: i64,
}

#[derive(Clone, Copy)]
struct PeriodSpec<'a> {
    role: ExperimentPeriodRole,
    arm: &'static str,
    config_fingerprint: &'a str,
    period: &'a ExperimentPeriod,
}

impl ExperimentComparisonRequest {
    fn period_specs(&self) -> [PeriodSpec<'_>; 6] {
        [
            PeriodSpec {
                role: ExperimentPeriodRole::CalibrationControlOne,
                arm: "control",
                config_fingerprint: &self.control_config_fingerprint,
                period: &self.calibration_control[0],
            },
            PeriodSpec {
                role: ExperimentPeriodRole::CalibrationControlTwo,
                arm: "control",
                config_fingerprint: &self.control_config_fingerprint,
                period: &self.calibration_control[1],
            },
            PeriodSpec {
                role: ExperimentPeriodRole::ControlBefore,
                arm: "control",
                config_fingerprint: &self.control_config_fingerprint,
                period: &self.control_before,
            },
            PeriodSpec {
                role: ExperimentPeriodRole::CandidateOne,
                arm: "candidate",
                config_fingerprint: &self.candidate_config_fingerprint,
                period: &self.candidate[0],
            },
            PeriodSpec {
                role: ExperimentPeriodRole::CandidateTwo,
                arm: "candidate",
                config_fingerprint: &self.candidate_config_fingerprint,
                period: &self.candidate[1],
            },
            PeriodSpec {
                role: ExperimentPeriodRole::ControlAfter,
                arm: "control",
                config_fingerprint: &self.control_config_fingerprint,
                period: &self.control_after,
            },
        ]
    }
}

fn validation_error(message: impl Into<String>) -> validator::ValidationError {
    let mut error = validator::ValidationError::new("experiment_comparison");
    error.message = Some(message.into().into());
    error
}

fn validate_experiment_request(
    request: &ExperimentComparisonRequest,
) -> Result<(), validator::ValidationError> {
    validate_identifier("experiment_id", &request.experiment_id)?;
    for (name, fingerprint) in [
        ("model_fingerprint", &request.model_fingerprint),
        ("hardware_fingerprint", &request.hardware_fingerprint),
        ("workload_fingerprint", &request.workload_fingerprint),
        ("benchmark_fingerprint", &request.benchmark_fingerprint),
        (
            "control_config_fingerprint",
            &request.control_config_fingerprint,
        ),
        (
            "candidate_config_fingerprint",
            &request.candidate_config_fingerprint,
        ),
    ] {
        validate_fingerprint(name, fingerprint)?;
    }
    if request.control_config_fingerprint == request.candidate_config_fingerprint {
        return Err(validation_error(
            "control_config_fingerprint and candidate_config_fingerprint must differ",
        ));
    }

    let specs = request.period_specs();
    let mut run_ids = BTreeSet::new();
    let mut expected_unit: Option<String> = None;
    for spec in &specs {
        validate_identifier("run_id", &spec.period.run_id)?;
        if !run_ids.insert(spec.period.run_id.as_str()) {
            return Err(validation_error(format!(
                "duplicate run_id: {}",
                spec.period.run_id
            )));
        }
        validate_window(spec.period)?;
        let unit = validate_aiperf_summary(&spec.period.run_id, &spec.period.aiperf)?;
        if let Some(expected) = expected_unit.as_deref() {
            if expected != unit {
                return Err(validation_error(format!(
                    "goodput unit mismatch for run {}: expected {expected}, got {unit}",
                    spec.period.run_id
                )));
            }
        } else {
            expected_unit = Some(unit.to_string());
        }
    }
    for pair in specs.windows(2) {
        if pair[0].period.to_ts > pair[1].period.from_ts {
            return Err(validation_error(format!(
                "experiment windows overlap or are out of order: {} then {}",
                pair[0].period.run_id, pair[1].period.run_id
            )));
        }
    }
    Ok(())
}

fn validate_identifier(name: &str, value: &str) -> Result<(), validator::ValidationError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(validation_error(format!(
            "{name} must be 1-128 ASCII letters, digits, dots, underscores, or hyphens"
        )));
    }
    Ok(())
}

fn validate_fingerprint(name: &str, value: &str) -> Result<(), validator::ValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(validation_error(format!(
            "{name} must be a 64-character lowercase SHA-256 hex digest"
        )));
    }
    Ok(())
}

fn validate_window(period: &ExperimentPeriod) -> Result<(), validator::ValidationError> {
    for (name, timestamp) in [("from_ts", period.from_ts), ("to_ts", period.to_ts)] {
        if !(MICROS_MIN..MICROS_MAX).contains(&timestamp) {
            return Err(validation_error(format!(
                "{} {name} is not a plausible 16-digit microsecond timestamp",
                period.run_id
            )));
        }
    }
    if period.to_ts <= period.from_ts {
        return Err(validation_error(format!(
            "{} to_ts must be greater than from_ts",
            period.run_id
        )));
    }
    Ok(())
}

fn validate_aiperf_summary(
    run_id: &str,
    summary: &AiperfAggregateSummary,
) -> Result<String, validator::ValidationError> {
    if summary.schema_version != SUPPORTED_AIPERF_SCHEMA {
        return Err(validation_error(format!(
            "{run_id} uses unsupported AIPerf aggregate schema {}; expected {SUPPORTED_AIPERF_SCHEMA}",
            summary.schema_version
        )));
    }
    if summary.aiperf_version != SUPPORTED_AIPERF_VERSION {
        return Err(validation_error(format!(
            "{run_id} uses unsupported AIPerf version {}; expected {SUPPORTED_AIPERF_VERSION}",
            summary.aiperf_version
        )));
    }
    let metadata = &summary.metadata;
    if metadata.aggregation_type != "confidence" {
        return Err(validation_error(format!(
            "{run_id} aggregation_type must be confidence"
        )));
    }
    if metadata.num_profile_runs < 2
        || metadata.num_successful_runs != metadata.num_profile_runs
        || !metadata.failed_runs.is_empty()
    {
        return Err(validation_error(format!(
            "{run_id} must contain at least two successful AIPerf profile runs and no failed runs"
        )));
    }

    let goodput = required_metric(summary, run_id, AIPERF_GOODPUT_METRIC)?;
    let requests = required_metric(summary, run_id, AIPERF_REQUEST_COUNT_METRIC)?;
    if requests.min <= 0.0 {
        return Err(validation_error(format!(
            "{run_id} request_count_avg.min must be greater than zero"
        )));
    }
    required_metric(summary, run_id, AIPERF_GOOD_REQUEST_COUNT_METRIC)?;
    if summary.metrics.request_error_rate_avg.is_some() {
        let error_rate = required_metric(summary, run_id, AIPERF_REQUEST_ERROR_RATE_METRIC)?;
        require_zero_distribution(&error_rate, run_id, AIPERF_REQUEST_ERROR_RATE_METRIC)?;
    }
    if summary.metrics.error_request_count_avg.is_some() {
        let errors = required_metric(summary, run_id, AIPERF_ERROR_REQUEST_COUNT_METRIC)?;
        require_zero_distribution(&errors, run_id, AIPERF_ERROR_REQUEST_COUNT_METRIC)?;
    }
    Ok(goodput.unit)
}

fn require_zero_distribution(
    metric: &AiperfConfidenceMetric,
    run_id: &str,
    name: &str,
) -> Result<(), validator::ValidationError> {
    if [
        metric.min,
        metric.mean,
        metric.max,
        metric.std,
        metric.se,
        metric.ci_low,
        metric.ci_high,
    ]
    .iter()
    .any(|value| *value != 0.0)
    {
        return Err(validation_error(format!(
            "{run_id} {name} distribution must be exactly zero"
        )));
    }
    Ok(())
}

fn required_metric(
    summary: &AiperfAggregateSummary,
    run_id: &str,
    name: &str,
) -> Result<AiperfConfidenceMetric, validator::ValidationError> {
    let metric = match name {
        AIPERF_GOODPUT_METRIC => summary.metrics.goodput_avg.clone(),
        AIPERF_REQUEST_COUNT_METRIC => summary.metrics.request_count_avg.clone(),
        AIPERF_GOOD_REQUEST_COUNT_METRIC => summary.metrics.good_request_count_avg.clone(),
        AIPERF_REQUEST_ERROR_RATE_METRIC => summary.metrics.request_error_rate_avg.clone(),
        AIPERF_ERROR_REQUEST_COUNT_METRIC => summary.metrics.error_request_count_avg.clone(),
        _ => None,
    }
    .ok_or_else(|| validation_error(format!("{run_id} is missing AIPerf metric {name}")))?;
    let numbers = [
        metric.mean,
        metric.std,
        metric.min,
        metric.max,
        metric.se,
        metric.ci_low,
        metric.ci_high,
        metric.t_critical,
    ];
    if numbers.iter().any(|number| !number.is_finite()) {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} contains a non-finite number"
        )));
    }
    if let Some(cv) = metric.cv {
        if !cv.is_finite() {
            return Err(validation_error(format!(
                "{run_id} AIPerf {name} contains a non-finite cv"
            )));
        }
    } else if metric.mean != 0.0 || metric.std != 0.0 {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} omits cv despite a nonzero mean or std"
        )));
    }
    if metric.std < 0.0 || metric.se < 0.0 || metric.t_critical <= 0.0 {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} requires std >= 0, se >= 0, and t_critical > 0"
        )));
    }
    if metric.min > metric.mean || metric.mean > metric.max {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} requires min <= mean <= max"
        )));
    }
    if metric.ci_low > metric.ci_high {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} requires ci_low <= ci_high"
        )));
    }
    if metric.ci_low > metric.mean || metric.mean > metric.ci_high {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} confidence interval does not contain its mean"
        )));
    }
    // AIPerf does not clamp its t-distribution confidence interval to the
    // metric's physical floor, so ci_low can legitimately be negative for a
    // nonnegative metric with high variance. The observed min/mean/max cannot.
    if [metric.min, metric.mean, metric.max]
        .iter()
        .any(|value| *value < 0.0)
    {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} observed count/goodput values cannot be negative"
        )));
    }
    if metric.unit.trim().is_empty() {
        return Err(validation_error(format!(
            "{run_id} AIPerf {name} unit is empty"
        )));
    }
    Ok(metric)
}

/// Fetch every required AIPerf histogram for the experiment in one bounded query,
/// then verify each row's run, arm, fingerprints, and exact period window.
pub(crate) fn load_experiment_evidence(
    connection: &Connection,
    request: &ExperimentComparisonRequest,
    prefix: &str,
) -> Result<ExperimentEvidence, ExperimentError> {
    let specs = request.period_specs();
    let global_from = specs[0].period.from_ts;
    let global_to = specs[5].period.to_ts;
    let table = format!("{prefix}sigiro_metrics_histogram");
    let sql = format!(
        "SELECT metric_name, epoch_us(timestamp), resource_attributes \
         FROM {table} \
         WHERE epoch_us(timestamp) BETWEEN ?1 AND ?2 \
           AND json_extract_string(resource_attributes, '$.\"optimization.experiment.id\"') = ?3 \
           AND metric_name IN (?4, ?5, ?6, ?7) \
         ORDER BY timestamp, metric_name"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| ExperimentError::Internal(anyhow!(error)))?;
    let mut rows = statement
        .query(params![
            global_from,
            global_to,
            request.experiment_id,
            REQUIRED_METRICS[0],
            REQUIRED_METRICS[1],
            REQUIRED_METRICS[2],
            REQUIRED_METRICS[3],
        ])
        .map_err(|error| ExperimentError::Internal(anyhow!(error)))?;

    let spec_by_run: BTreeMap<&str, PeriodSpec<'_>> = specs
        .iter()
        .copied()
        .map(|spec| (spec.period.run_id.as_str(), spec))
        .collect();
    let mut evidence: BTreeMap<String, PeriodEvidence> = BTreeMap::new();

    while let Some(row) = rows
        .next()
        .map_err(|error| ExperimentError::Internal(anyhow!(error)))?
    {
        let metric_name: String = row
            .get(0)
            .map_err(|error| ExperimentError::Internal(anyhow!(error)))?;
        let timestamp: i64 = row
            .get(1)
            .map_err(|error| ExperimentError::Internal(anyhow!(error)))?;
        let raw_attributes: Option<String> = row
            .get(2)
            .map_err(|error| ExperimentError::Internal(anyhow!(error)))?;
        let raw_attributes = raw_attributes.ok_or_else(|| {
            ExperimentError::invalid(format!(
                "AIPerf metric {metric_name} at {timestamp} has no resource_attributes"
            ))
        })?;
        let attributes: Value = serde_json::from_str(&raw_attributes).map_err(|error| {
            ExperimentError::invalid(format!(
                "AIPerf metric {metric_name} at {timestamp} has invalid resource_attributes: {error}"
            ))
        })?;
        let run_id = attribute(&attributes, "optimization.run.id")?;
        let spec = spec_by_run.get(run_id).ok_or_else(|| {
            ExperimentError::invalid(format!(
                "Sigiro evidence contains unknown run_id {run_id} for experiment {}",
                request.experiment_id
            ))
        })?;
        if timestamp < spec.period.from_ts || timestamp > spec.period.to_ts {
            return Err(ExperimentError::invalid(format!(
                "Sigiro evidence for run {run_id} at {timestamp} falls outside its declared window {}..{}",
                spec.period.from_ts, spec.period.to_ts
            )));
        }
        for (key, expected) in [
            ("optimization.experiment.id", request.experiment_id.as_str()),
            ("optimization.run.id", run_id),
            ("optimization.arm", spec.arm),
            ("optimization.config.sha256", spec.config_fingerprint),
            (
                "optimization.model.sha256",
                request.model_fingerprint.as_str(),
            ),
            (
                "optimization.hardware.sha256",
                request.hardware_fingerprint.as_str(),
            ),
            (
                "optimization.workload.sha256",
                request.workload_fingerprint.as_str(),
            ),
            (
                "optimization.benchmark.sha256",
                request.benchmark_fingerprint.as_str(),
            ),
        ] {
            let observed = attribute(&attributes, key)?;
            if observed != expected {
                return Err(ExperimentError::invalid(format!(
                    "Sigiro evidence for run {run_id} has {key}={observed}; expected {expected}"
                )));
            }
        }

        let entry = evidence
            .entry(run_id.to_string())
            .or_insert_with(|| PeriodEvidence {
                rows: 0,
                metric_names: BTreeSet::new(),
                observed_from_ts: timestamp,
                observed_to_ts: timestamp,
            });
        entry.rows += 1;
        entry.metric_names.insert(metric_name);
        entry.observed_from_ts = entry.observed_from_ts.min(timestamp);
        entry.observed_to_ts = entry.observed_to_ts.max(timestamp);
    }

    let required: BTreeSet<String> = REQUIRED_METRICS.iter().map(ToString::to_string).collect();
    for spec in specs {
        let period = evidence.get(&spec.period.run_id).ok_or_else(|| {
            ExperimentError::invalid(format!(
                "no Sigiro AIPerf evidence found for run {}",
                spec.period.run_id
            ))
        })?;
        let missing: Vec<&str> = required
            .difference(&period.metric_names)
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(ExperimentError::invalid(format!(
                "Sigiro evidence for run {} is missing metrics: {}",
                spec.period.run_id,
                missing.join(", ")
            )));
        }
    }

    Ok(ExperimentEvidence { periods: evidence })
}

fn attribute<'a>(attributes: &'a Value, key: &str) -> Result<&'a str, ExperimentError> {
    attributes
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .ok_or_else(|| ExperimentError::invalid(format!("resource_attributes is missing {key}")))
}

/// Apply the fixed reversal and confidence-interval rule to already-verified evidence.
pub(crate) fn compare_experiment(
    request: &ExperimentComparisonRequest,
    evidence: &ExperimentEvidence,
    prefix: &str,
) -> Result<ExperimentComparisonResponse, ExperimentError> {
    let specs = request.period_specs();
    let goodputs: Vec<AiperfConfidenceMetric> = specs
        .iter()
        .map(|spec| {
            required_metric(
                &spec.period.aiperf,
                &spec.period.run_id,
                AIPERF_GOODPUT_METRIC,
            )
            .map_err(|error| ExperimentError::invalid(error.to_string()))
        })
        .collect::<Result<_, _>>()?;

    let drift_pair = [(0, 1), (2, 5), (3, 4)]
        .into_iter()
        .find(|(left, right)| !intervals_overlap(&goodputs[*left], &goodputs[*right]));

    let (outcome, winner, reason) = if let Some((left, right)) = drift_pair {
        (
            ExperimentOutcome::DriftDetected,
            None,
            format!(
                "same-configuration confidence intervals do not overlap for {} and {}",
                specs[left].period.run_id, specs[right].period.run_id
            ),
        )
    } else {
        let control_upper = goodputs[2].ci_high.max(goodputs[5].ci_high);
        let control_lower = goodputs[2].ci_low.min(goodputs[5].ci_low);
        let candidate_lower = goodputs[3].ci_low.min(goodputs[4].ci_low);
        let candidate_upper = goodputs[3].ci_high.max(goodputs[4].ci_high);
        if candidate_lower > control_upper {
            (
                ExperimentOutcome::Confirmed,
                Some(ExperimentWinner::Candidate),
                "both candidate lower confidence bounds exceed both control upper bounds"
                    .to_string(),
            )
        } else if control_lower > candidate_upper {
            (
                ExperimentOutcome::Rejected,
                Some(ExperimentWinner::Control),
                "both control lower confidence bounds exceed both candidate upper bounds"
                    .to_string(),
            )
        } else {
            (
                ExperimentOutcome::Inconclusive,
                None,
                "control and candidate goodput confidence intervals overlap".to_string(),
            )
        }
    };

    let mut periods = Vec::with_capacity(specs.len());
    for (spec, goodput) in specs.iter().zip(&goodputs) {
        let period_evidence = evidence.periods.get(&spec.period.run_id).ok_or_else(|| {
            ExperimentError::invalid(format!(
                "no verified evidence for run {}",
                spec.period.run_id
            ))
        })?;
        periods.push(ComparedPeriod {
            role: spec.role,
            run_id: spec.period.run_id.clone(),
            goodput_mean: goodput.mean,
            goodput_ci_low: goodput.ci_low,
            goodput_ci_high: goodput.ci_high,
            telemetry_rows: period_evidence.rows,
            metric_names: period_evidence.metric_names.iter().cloned().collect(),
            observed_from_ts: period_evidence.observed_from_ts,
            observed_to_ts: period_evidence.observed_to_ts,
            evidence_sql: evidence_sql(request, *spec, prefix),
        });
    }

    Ok(ExperimentComparisonResponse {
        experiment_id: request.experiment_id.clone(),
        outcome,
        winner,
        reason,
        objective: "goodput".to_string(),
        unit: goodputs[0].unit.clone(),
        periods,
    })
}

fn intervals_overlap(left: &AiperfConfidenceMetric, right: &AiperfConfidenceMetric) -> bool {
    left.ci_low <= right.ci_high && right.ci_low <= left.ci_high
}

fn evidence_sql(
    request: &ExperimentComparisonRequest,
    spec: PeriodSpec<'_>,
    prefix: &str,
) -> String {
    format!(
        "SELECT timestamp, metric_name, count, sum, min, max, resource_attributes, metric_attributes \
         FROM {prefix}sigiro_metrics_histogram \
         WHERE timestamp BETWEEN make_timestamp({}) AND make_timestamp({}) \
           AND json_extract_string(resource_attributes, '$.\"optimization.experiment.id\"') = '{}' \
           AND json_extract_string(resource_attributes, '$.\"optimization.run.id\"') = '{}' \
           AND metric_name IN ('{}', '{}', '{}', '{}') \
         ORDER BY timestamp, metric_name",
        spec.period.from_ts,
        spec.period.to_ts,
        request.experiment_id,
        spec.period.run_id,
        REQUIRED_METRICS[0],
        REQUIRED_METRICS[1],
        REQUIRED_METRICS[2],
        REQUIRED_METRICS[3],
    )
}

//! End-to-end contract for AIPerf experiment comparison.
//!
//! The test sends real OTLP/HTTP histogram payloads through the production
//! transformer and flusher, then calls the authenticated comparison endpoint.

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use axum_test::TestServer;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::metrics::v1::{
    AggregationTemporality, Histogram, HistogramDataPoint, Metric, ResourceMetrics, ScopeMetrics,
    metric,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use parking_lot::Mutex;
use prost::Message;
use rusqlite::Connection;
use serde_json::{Value, json};
use sigiro::admin;
use sigiro::auth::{AuthMode, ControlPlane};
use sigiro::inserter::MsgRouter;
use sigiro::pool::ProjectPool;
use sigiro::server::{AppState, build_http_router};
use tempfile::TempDir;

const BASE_TS: i64 = 1_780_000_000_000_000;
const REQUIRED_METRICS: [&str; 4] = [
    "gen_ai.client.operation.duration",
    "gen_ai.client.operation.time_to_first_chunk",
    "gen_ai.client.operation.time_per_output_chunk",
    "gen_ai.client.token.usage",
];

struct TestContext {
    state: Arc<AppState>,
    key_a: String,
    key_b: String,
    _data_dir: TempDir,
}

fn build_context() -> TestContext {
    let data_dir = TempDir::new().expect("create temporary data directory");
    let data_path = camino::Utf8PathBuf::from_path_buf(data_dir.path().to_path_buf())
        .expect("temporary path is UTF-8");
    unsafe {
        std::env::set_var("SIGIRO_EXTENSION_DIR", "/tmp/sigiro-test-extensions");
    }
    sigiro::storage::prewarm_extensions(&data_path).expect("prewarm test extensions");

    let control = Arc::new(ControlPlane::open_in_memory_for_tests().expect("control plane"));
    control.create_tenant("tenant_a").expect("create tenant A");
    control.create_tenant("tenant_b").expect("create tenant B");
    let key_a = control.mint_key("tenant_a").expect("key A").key;
    let key_b = control.mint_key("tenant_b").expect("key B").key;

    let pool = Arc::new(ProjectPool::new(
        data_path.clone(),
        4,
        sigiro::storage::StorageMode::LocalFile,
    ));
    let router = Arc::new(MsgRouter::new(Arc::clone(&pool)));
    router.start();

    let admin_conn = Connection::open_in_memory().expect("admin database");
    admin::create_instance_status_table(&admin_conn).expect("admin schema");
    let state = Arc::new(AppState {
        pool,
        admin_conn: Arc::new(Mutex::new(admin_conn)),
        http_client: reqwest::Client::new(),
        router,
        auth_mode: AuthMode::TenantKeys { control },
        data_dir: data_path,
        init_complete: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        query_surface: sigiro::config::QuerySurface::None,
    });

    TestContext {
        state,
        key_a,
        key_b,
        _data_dir: data_dir,
    }
}

fn fingerprint(character: char) -> String {
    character.to_string().repeat(64)
}

fn confidence_metric(mean: f64, low: f64, high: f64, unit: &str) -> Value {
    json!({
        "mean": mean,
        "std": (high - low) / 4.0,
        "min": low,
        "max": high,
        "cv": 0.01,
        "se": 0.5,
        "ci_low": low,
        "ci_high": high,
        "t_critical": 2.0,
        "unit": unit
    })
}

fn constant_confidence_metric(value: f64, unit: &str) -> Value {
    let mut metric = confidence_metric(value, value, value, unit);
    metric["cv"] = if value == 0.0 {
        Value::Null
    } else {
        json!(0.0)
    };
    metric["se"] = json!(0.0);
    metric
}

fn zero_confidence_metric(unit: &str) -> Value {
    constant_confidence_metric(0.0, unit)
}

fn aiperf_summary(mean: f64, low: f64, high: f64) -> Value {
    json!({
        "schema_version": "1.0",
        "aiperf_version": "0.12.0",
        "metadata": {
            "aggregation_type": "confidence",
            "num_profile_runs": 3,
            "num_successful_runs": 3,
            "failed_runs": []
        },
        "metrics": {
            "goodput_avg": confidence_metric(mean, low, high, "requests/s"),
            "request_count_avg": confidence_metric(100.0, 99.0, 101.0, "requests"),
            "good_request_count_avg": confidence_metric(100.0, 99.0, 101.0, "requests"),
            "future_additive_metric": {"shape": "ignored"}
        },
        "future_additive_field": true
    })
}

fn period(run_id: &str, index: usize, mean: f64, low: f64, high: f64) -> Value {
    let timestamp = BASE_TS + index as i64 * 2_000_000;
    json!({
        "run_id": run_id,
        "from_ts": timestamp - 500_000,
        "to_ts": timestamp + 500_000,
        "aiperf": aiperf_summary(mean, low, high)
    })
}

fn comparison_request(control: (f64, f64, f64), candidate: (f64, f64, f64)) -> Value {
    json!({
        "experiment_id": "exp-e2e-001",
        "model_fingerprint": fingerprint('c'),
        "hardware_fingerprint": fingerprint('d'),
        "workload_fingerprint": fingerprint('e'),
        "benchmark_fingerprint": fingerprint('f'),
        "control_config_fingerprint": fingerprint('a'),
        "candidate_config_fingerprint": fingerprint('b'),
        "calibration_control": [
            period("A0-1", 0, control.0, control.1, control.2),
            period("A0-2", 1, control.0, control.1, control.2)
        ],
        "control_before": period("A1", 2, control.0, control.1, control.2),
        "candidate": [
            period("B1", 3, candidate.0, candidate.1, candidate.2),
            period("B2", 4, candidate.0, candidate.1, candidate.2)
        ],
        "control_after": period("A2", 5, control.0, control.1, control.2)
    })
}

fn resource_attributes(run_id: &str, arm: &str, config: &str) -> Vec<KeyValue> {
    vec![
        ("service.name", "aiperf".to_string()),
        ("optimization.experiment.id", "exp-e2e-001".to_string()),
        ("optimization.run.id", run_id.to_string()),
        ("optimization.arm", arm.to_string()),
        ("optimization.config.sha256", config.to_string()),
        ("optimization.model.sha256", fingerprint('c')),
        ("optimization.hardware.sha256", fingerprint('d')),
        ("optimization.workload.sha256", fingerprint('e')),
        ("optimization.benchmark.sha256", fingerprint('f')),
    ]
    .into_iter()
    .map(|(key, value)| KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value)),
        }),
    })
    .collect()
}

fn metrics_payload(run_id: &str, index: usize, arm: &str, config: &str) -> Vec<u8> {
    let time_unix_nano = ((BASE_TS + index as i64 * 2_000_000) * 1_000) as u64;
    let metrics: Vec<Metric> = REQUIRED_METRICS
        .iter()
        .map(|name| Metric {
            name: (*name).to_string(),
            description: String::new(),
            unit: "s".to_string(),
            metadata: Vec::new(),
            data: Some(metric::Data::Histogram(Histogram {
                aggregation_temporality: AggregationTemporality::Cumulative as i32,
                data_points: vec![HistogramDataPoint {
                    attributes: Vec::new(),
                    start_time_unix_nano: time_unix_nano - 1_000,
                    time_unix_nano,
                    count: 1,
                    sum: Some(0.01),
                    bucket_counts: vec![1, 0],
                    explicit_bounds: vec![0.1],
                    exemplars: Vec::new(),
                    flags: 0,
                    min: Some(0.01),
                    max: Some(0.01),
                }],
            })),
        })
        .collect();
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: resource_attributes(run_id, arm, config),
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
    .encode_to_vec()
}

async fn post_comparison(server: &TestServer, key: &str, body: &Value) -> axum_test::TestResponse {
    server
        .post("/v1/experiments/compare")
        .add_header("Authorization", format!("Bearer {key}"))
        .json(body)
        .await
}

#[tokio::test]
async fn experiment_comparison_validates_real_otlp_evidence_and_all_outcomes() {
    let context = build_context();
    let otlp = sigiro::ingest::otlp_http_router(
        Arc::clone(&context.state.router),
        context.state.auth_mode.clone(),
        8_388_608,
    );
    let app = build_http_router(Arc::clone(&context.state)).merge(otlp);
    let server = TestServer::new(app).expect("build merged API server");

    for (run_id, index, arm, config) in [
        ("A0-1", 0, "control", fingerprint('a')),
        ("A0-2", 1, "control", fingerprint('a')),
        ("A1", 2, "control", fingerprint('a')),
        ("B1", 3, "candidate", fingerprint('b')),
        ("B2", 4, "candidate", fingerprint('b')),
        ("A2", 5, "control", fingerprint('a')),
    ] {
        let response = server
            .post("/v1/metrics")
            .add_header("Authorization", format!("Bearer {}", context.key_a))
            .bytes(metrics_payload(run_id, index, arm, &config).into())
            .content_type("application/x-protobuf")
            .await;
        response.assert_status_ok();
    }
    context
        .state
        .router
        .shutdown_and_wait()
        .await
        .expect("persist AIPerf OTLP metrics");
    {
        let readers = context
            .state
            .pool
            .get_project("tenant_a")
            .expect("tenant A project");
        let connection = readers.get();
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM lake.sigiro_metrics_histogram",
                [],
                |row| row.get(0),
            )
            .expect("histogram count");
        let attributes: String = connection
            .query_row(
                "SELECT resource_attributes FROM lake.sigiro_metrics_histogram LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("histogram attributes");
        assert_eq!(
            count, 24,
            "all AIPerf histograms must persist: {attributes}"
        );
        assert!(
            attributes.contains("optimization.experiment.id"),
            "OTLP transformation must preserve experiment attributes: {attributes}"
        );
    }

    for (control, candidate, expected) in [
        ((100.0, 95.0, 105.0), (130.0, 125.0, 135.0), "confirmed"),
        ((130.0, 125.0, 135.0), (100.0, 95.0, 105.0), "rejected"),
        ((100.0, 90.0, 110.0), (105.0, 95.0, 115.0), "inconclusive"),
    ] {
        let response = post_comparison(
            &server,
            &context.key_a,
            &comparison_request(control, candidate),
        )
        .await;
        response.assert_status_ok();
        let body: Value = serde_json::from_str(&response.text()).expect("comparison JSON");
        assert_eq!(body["outcome"], expected);
        assert_eq!(body["periods"].as_array().map(Vec::len), Some(6));
        assert!(
            body["periods"][0]["evidence_sql"]
                .as_str()
                .unwrap_or_default()
                .contains("sigiro_metrics_histogram")
        );
        if expected == "confirmed" {
            let evidence_sql = body["periods"][0]["evidence_sql"]
                .as_str()
                .expect("evidence SQL");
            server
                .post("/v1/query")
                .add_header("Authorization", format!("Bearer {}", context.key_a))
                .text(evidence_sql)
                .await
                .assert_status_ok();
        }
    }

    // A nonempty candidate can complete requests while every request misses
    // its SLO. That is a valid zero-goodput result, not an invalid artifact.
    let mut zero_goodput = comparison_request((100.0, 95.0, 105.0), (0.0, 0.0, 0.0));
    for candidate in zero_goodput["candidate"]
        .as_array_mut()
        .expect("candidate periods")
    {
        candidate["aiperf"]["metrics"]["goodput_avg"] = zero_confidence_metric("requests/s");
        candidate["aiperf"]["metrics"]["good_request_count_avg"] =
            zero_confidence_metric("requests");
    }
    let response = post_comparison(&server, &context.key_a, &zero_goodput).await;
    response.assert_status_ok();
    let body: Value = serde_json::from_str(&response.text()).expect("zero-goodput JSON");
    assert_eq!(body["outcome"], "rejected");
    assert_eq!(body["winner"], "control");

    // AIPerf's t-distribution CI is not clamped to a metric's physical floor.
    // A high-variance nonnegative metric can therefore have a negative ci_low
    // while its observed min/mean/max remain valid.
    let mut negative_ci = comparison_request((100.0, 95.0, 105.0), (130.0, 125.0, 135.0));
    let wide_goodput = &mut negative_ci["candidate"][0]["aiperf"]["metrics"]["goodput_avg"];
    wide_goodput["min"] = json!(0.0);
    wide_goodput["max"] = json!(260.0);
    wide_goodput["std"] = json!(130.0);
    wide_goodput["cv"] = json!(1.0);
    wide_goodput["se"] = json!(75.0555);
    wide_goodput["t_critical"] = json!(4.3027);
    wide_goodput["ci_low"] = json!(-192.95);
    wide_goodput["ci_high"] = json!(452.95);
    let response = post_comparison(&server, &context.key_a, &negative_ci).await;
    response.assert_status_ok();
    let body: Value = serde_json::from_str(&response.text()).expect("negative-CI JSON");
    assert_eq!(body["outcome"], "inconclusive");

    let mut drift = comparison_request((100.0, 95.0, 105.0), (130.0, 125.0, 135.0));
    drift["calibration_control"][1]["aiperf"] = aiperf_summary(150.0, 145.0, 155.0);
    let response = post_comparison(&server, &context.key_a, &drift).await;
    response.assert_status_ok();
    let body: Value = serde_json::from_str(&response.text()).expect("drift JSON");
    assert_eq!(body["outcome"], "drift_detected");

    let valid = comparison_request((100.0, 95.0, 105.0), (130.0, 125.0, 135.0));
    let clean_metrics = valid["candidate"][0]["aiperf"]["metrics"]
        .as_object()
        .expect("clean metrics");
    assert!(!clean_metrics.contains_key("request_error_rate_avg"));
    assert!(!clean_metrics.contains_key("error_request_count_avg"));
    let mut missing_summary = valid.clone();
    missing_summary["candidate"][0]["aiperf"]["metrics"]
        .as_object_mut()
        .expect("metrics object")
        .remove("goodput_avg");
    post_comparison(&server, &context.key_a, &missing_summary)
        .await
        .assert_status(axum::http::StatusCode::BAD_REQUEST);
    let mut present_zero_errors = valid.clone();
    present_zero_errors["candidate"][0]["aiperf"]["metrics"]["request_error_rate_avg"] =
        zero_confidence_metric("%");
    present_zero_errors["candidate"][0]["aiperf"]["metrics"]["error_request_count_avg"] =
        zero_confidence_metric("requests");
    post_comparison(&server, &context.key_a, &present_zero_errors)
        .await
        .assert_status_ok();

    let mut impossible_bounds = valid.clone();
    impossible_bounds["candidate"][0]["aiperf"]["metrics"]["goodput_avg"]["min"] = json!(131.0);
    let mut negative_std = valid.clone();
    negative_std["candidate"][0]["aiperf"]["metrics"]["goodput_avg"]["std"] = json!(-1.0);
    let mut reversed_ci = valid.clone();
    reversed_ci["candidate"][0]["aiperf"]["metrics"]["goodput_avg"]["ci_low"] = json!(140.0);
    reversed_ci["candidate"][0]["aiperf"]["metrics"]["goodput_avg"]["ci_high"] = json!(120.0);
    let mut negative_goodput = valid.clone();
    negative_goodput["candidate"][0]["aiperf"]["metrics"]["goodput_avg"]["min"] = json!(-1.0);
    let mut empty_benchmark = valid.clone();
    empty_benchmark["candidate"][0]["aiperf"]["metrics"]["request_count_avg"] =
        zero_confidence_metric("requests");
    let mut request_error_rate = valid.clone();
    request_error_rate["candidate"][0]["aiperf"]["metrics"]["request_error_rate_avg"] =
        constant_confidence_metric(1.0, "%");
    let mut request_errors = valid.clone();
    request_errors["candidate"][0]["aiperf"]["metrics"]["error_request_count_avg"] =
        constant_confidence_metric(1.0, "requests");
    let mut empty_goodput_unit = valid.clone();
    empty_goodput_unit["candidate"][0]["aiperf"]["metrics"]["goodput_avg"]["unit"] = json!("");
    let mut empty_request_unit = valid.clone();
    empty_request_unit["candidate"][0]["aiperf"]["metrics"]["good_request_count_avg"]["unit"] =
        json!("");

    for (request, expected_detail) in [
        (impossible_bounds, "min <= mean <= max"),
        (negative_std, "std >= 0"),
        (reversed_ci, "ci_low <= ci_high"),
        (negative_goodput, "cannot be negative"),
        (
            empty_benchmark,
            "request_count_avg.min must be greater than zero",
        ),
        (
            request_error_rate,
            "request_error_rate_avg distribution must be exactly zero",
        ),
        (
            request_errors,
            "error_request_count_avg distribution must be exactly zero",
        ),
        (empty_goodput_unit, "goodput_avg unit is empty"),
        (empty_request_unit, "good_request_count_avg unit is empty"),
    ] {
        let response = post_comparison(&server, &context.key_a, &request).await;
        response.assert_status(axum::http::StatusCode::BAD_REQUEST);
        assert!(
            response.text().contains(expected_detail),
            "expected {expected_detail:?} in fail-loud response: {}",
            response.text()
        );
    }

    let mut wrong_fingerprint = valid.clone();
    wrong_fingerprint["candidate_config_fingerprint"] = json!(fingerprint('9'));
    post_comparison(&server, &context.key_a, &wrong_fingerprint)
        .await
        .assert_status(axum::http::StatusCode::BAD_REQUEST);

    let mut missing_run = valid.clone();
    missing_run["candidate"][1]["run_id"] = json!("missing-B2");
    post_comparison(&server, &context.key_a, &missing_run)
        .await
        .assert_status(axum::http::StatusCode::BAD_REQUEST);

    // The same request authenticated as tenant B cannot see tenant A evidence.
    post_comparison(&server, &context.key_b, &valid)
        .await
        .assert_status(axum::http::StatusCode::BAD_REQUEST);

    let spec: Value =
        serde_json::from_str(&server.get("/openapi.json").await.text()).expect("OpenAPI JSON");
    let operation = &spec["paths"]["/v1/experiments/compare"]["post"];
    assert!(operation["requestBody"].is_object());
    assert!(
        operation["description"]
            .as_str()
            .unwrap_or_default()
            .contains("microseconds")
    );
    assert!(spec["components"]["schemas"]["ExperimentComparisonResponse"].is_object());
    let aggregate_properties =
        &spec["components"]["schemas"]["AiperfAggregateMetrics"]["properties"];
    assert!(aggregate_properties["request_count_avg"].is_object());
    assert!(aggregate_properties["request_error_rate_avg"].is_object());
    assert!(aggregate_properties["error_request_count_avg"].is_object());
    let aggregate_required = spec["components"]["schemas"]["AiperfAggregateMetrics"]["required"]
        .as_array()
        .expect("required aggregate metrics");
    assert!(
        aggregate_required
            .iter()
            .any(|name| name == "request_count_avg")
    );
    assert!(
        !aggregate_required
            .iter()
            .any(|name| name == "request_error_rate_avg")
    );
    assert!(
        !aggregate_required
            .iter()
            .any(|name| name == "error_request_count_avg")
    );
    assert!(operation["security"].is_array());
}

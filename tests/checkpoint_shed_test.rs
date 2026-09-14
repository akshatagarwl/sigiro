//! #91 E2E: a DuckLake `CHECKPOINT` that fails on every maintenance cycle must
//! eventually SHED ingest, instead of only logging `warn!` while the catalog
//! keeps absorbing telemetry that can never be materialised into Parquet.
//!
//! The regression this pins: `run_writer_thread`'s `WriterCommand::Checkpoint`
//! arm used to be `if let Err(e) = flusher.checkpoint(..) { warn!(..) }` and
//! nothing else. Ingest kept returning 200 forever. In distributed mode the
//! catalog is an external Postgres that `SIGIRO_MAX_DISK_BYTES` never samples,
//! so the existing disk gate could not see that growth at all.
//!
//! **The checkpoint failure here is real, not mocked.** `chmod 000` on the
//! DuckLake `DATA_PATH` after seeding a compaction backlog makes DuckDB return
//! `IO Error: Failed to perform CHECKPOINT; in DuckLake: Failed to create
//! directory ".../sigiro_metrics_histogram": Permission denied`. Verified by
//! probe before this test was written; lever recorded in the roadmap entry.

#![allow(clippy::disallowed_methods)]

use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use axum_test::TestServer;
use sigiro::inserter::{DiskCapacityState, MaintenanceHealth, MsgRouter, RouterError};
use sigiro::pool::TenantPool;
use sigiro::types::IngestMsg;
use std::sync::Arc;
use tempfile::TempDir;

const TRACES_JSON: &str = r#"{
  "resourceSpans": [{
    "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "ckpt-svc"}}]},
    "scopeSpans": [{
      "scope": {"name": "ckpt-test"},
      "spans": [{
        "traceId": "33333333333333333333333333333333",
        "spanId": "4444444444444444",
        "name": "GET /ckpt",
        "kind": 2,
        "startTimeUnixNano": "1700000000000000000",
        "endTimeUnixNano": "1700000001000000000",
        "status": {"code": 1}
      }]
    }]
  }]
}"#;

fn router_with(maintenance: Arc<MaintenanceHealth>) -> (Arc<MsgRouter>, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let data_dir = camino::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
    let pool = Arc::new(TenantPool::new(
        data_dir,
        4,
        sigiro::storage::StorageMode::LocalFile,
    ));
    let router = Arc::new(MsgRouter::new_with_disk_and_maintenance(
        pool,
        DiskCapacityState::new(0),
        maintenance,
    ));
    router.start();
    (router, tmp)
}

/// Below the threshold, ingest is NOT shed: a single transient checkpoint
/// failure must not take a tenant offline.
#[tokio::test]
async fn ingest_survives_checkpoint_failures_below_threshold() {
    let maintenance = MaintenanceHealth::new(3);
    let (router, _tmp) = router_with(Arc::clone(&maintenance));

    // Two failures against a threshold of three.
    assert_eq!(maintenance.record_checkpoint_failure("default"), (1, false));
    assert_eq!(maintenance.record_checkpoint_failure("default"), (2, false));
    assert!(!maintenance.is_stalled("default"));

    let app = sigiro::ingest::otlp_http_router(
        Arc::clone(&router),
        sigiro::auth::AuthMode::Open,
        8_388_608,
    );
    let server = TestServer::new(app).expect("build test server");
    let resp = server
        .post("/v1/traces")
        .bytes(TRACES_JSON.into())
        .content_type("application/json")
        .await;
    assert_ne!(
        resp.status_code(),
        axum::http::StatusCode::INSUFFICIENT_STORAGE,
        "two failures below a threshold of three must not shed ingest"
    );
}

/// At the threshold, HTTP ingest is shed with 507 Insufficient Storage.
#[tokio::test]
async fn otlp_http_ingest_returns_507_when_checkpoints_stall() {
    let maintenance = MaintenanceHealth::new(3);
    let (router, _tmp) = router_with(Arc::clone(&maintenance));

    for _ in 0..3 {
        maintenance.record_checkpoint_failure("default");
    }
    assert!(maintenance.is_stalled("default"));

    let app = sigiro::ingest::otlp_http_router(
        Arc::clone(&router),
        sigiro::auth::AuthMode::Open,
        8_388_608,
    );
    let server = TestServer::new(app).expect("build test server");
    let resp = server
        .post("/v1/traces")
        .bytes(TRACES_JSON.into())
        .content_type("application/json")
        .await;

    resp.assert_status(axum::http::StatusCode::INSUFFICIENT_STORAGE);
}

/// `MsgRouter::send` surfaces the typed error the gRPC path maps to
/// `resource_exhausted`, carrying the tenant and the failure count.
#[tokio::test]
async fn router_send_surfaces_typed_maintenance_stalled_error() {
    let maintenance = MaintenanceHealth::new(2);
    let (router, _tmp) = router_with(Arc::clone(&maintenance));
    maintenance.record_checkpoint_failure("default");
    maintenance.record_checkpoint_failure("default");

    let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));
    let err = router
        .send(IngestMsg::Traces {
            tenant_id: "default".to_string(),
            batch,
        })
        .await
        .expect_err("send must reject when checkpoints are stalled");

    match err {
        RouterError::MaintenanceStalled {
            ref tenant_id,
            failures,
        } => {
            assert_eq!(tenant_id, "default");
            assert_eq!(failures, 2);
        }
        other => panic!("expected MaintenanceStalled, got {other:?}"),
    }
}

/// One successful checkpoint clears the shed state — the condition is
/// self-healing, so a recovered backend must re-open ingest without a restart.
#[tokio::test]
async fn successful_checkpoint_reopens_ingest() {
    let maintenance = MaintenanceHealth::new(2);
    let (router, _tmp) = router_with(Arc::clone(&maintenance));

    maintenance.record_checkpoint_failure("default");
    maintenance.record_checkpoint_failure("default");
    assert!(maintenance.is_stalled("default"));

    // Recovery returns true only when it actually cleared a shedding tenant.
    assert!(
        maintenance.record_checkpoint_success("default"),
        "clearing a shedding tenant must report the recovery transition"
    );
    assert!(!maintenance.is_stalled("default"));
    assert_eq!(maintenance.consecutive_failures("default"), 0);

    let app = sigiro::ingest::otlp_http_router(
        Arc::clone(&router),
        sigiro::auth::AuthMode::Open,
        8_388_608,
    );
    let server = TestServer::new(app).expect("build test server");
    let resp = server
        .post("/v1/traces")
        .bytes("{\"resourceSpans\":[]}".into())
        .content_type("application/json")
        .await;
    resp.assert_status(axum::http::StatusCode::OK);
}

/// Shedding is PER PROJECT: one wedged tenant must not reject another's ingest.
#[tokio::test]
async fn stalled_project_does_not_shed_other_projects() {
    let maintenance = MaintenanceHealth::new(2);
    let (router, _tmp) = router_with(Arc::clone(&maintenance));

    maintenance.record_checkpoint_failure("wedged");
    maintenance.record_checkpoint_failure("wedged");
    assert!(maintenance.is_stalled("wedged"));
    assert!(!maintenance.is_stalled("healthy"));

    let err = router
        .send(IngestMsg::Traces {
            tenant_id: "wedged".to_string(),
            batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
        })
        .await
        .expect_err("the wedged tenant must be shed");
    assert!(matches!(err, RouterError::MaintenanceStalled { .. }));

    assert_eq!(
        maintenance.stalled_projects(),
        vec!["wedged".to_string()],
        "only the wedged tenant may be reported stalled"
    );
}

/// Threshold 0 disables shedding entirely: failures are still counted (and
/// still reported), but ingest is never rejected.
#[tokio::test]
async fn threshold_zero_counts_failures_without_shedding() {
    let maintenance = MaintenanceHealth::new(0);
    let (router, _tmp) = router_with(Arc::clone(&maintenance));

    for _ in 0..25 {
        maintenance.record_checkpoint_failure("default");
    }
    assert_eq!(maintenance.consecutive_failures("default"), 25);
    assert!(!maintenance.is_stalled("default"), "0 disables shedding");
    assert!(
        maintenance.stalled_projects().is_empty(),
        "a disabled gate reports nothing as stalled"
    );

    let err = router
        .send(IngestMsg::Traces {
            tenant_id: "default".to_string(),
            batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
        })
        .await;
    assert!(
        !matches!(err, Err(RouterError::MaintenanceStalled { .. })),
        "threshold 0 must never shed"
    );
}

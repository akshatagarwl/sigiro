//! Disk-capacity gate E2E: when the box is over its `SIGIRO_MAX_DISK_BYTES`
//! cap, ingest is rejected. HTTP ingest returns 507 Insufficient Storage, and
//! `MsgRouter::send` surfaces the typed `RouterError::DiskCapacityExceeded`
//! that the gRPC path maps to `resource_exhausted`.
//!
//! The gate short-circuits at the very top of `router.send`, before any
//! flusher/DuckDB work, so these tests need no project connection or ingest
//! plumbing — just an armed, tripped `DiskCapacityState`.

#![allow(clippy::disallowed_methods)]

use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use axum_test::TestServer;
use sigiro::inserter::{DiskCapacityState, MsgRouter, RouterError};
use sigiro::pool::ProjectPool;
use sigiro::types::IngestMsg;
use std::sync::Arc;
use tempfile::TempDir;

/// A single-span OTLP traces payload (JSON) that transforms to one row, so the
/// handler actually reaches `router.send` rather than short-circuiting on an
/// empty batch.
const TRACES_JSON: &str = r#"{
  "resourceSpans": [{
    "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "cap-svc"}}]},
    "scopeSpans": [{
      "scope": {"name": "cap-test"},
      "spans": [{
        "traceId": "11111111111111111111111111111111",
        "spanId": "2222222222222222",
        "name": "GET /cap",
        "kind": 2,
        "startTimeUnixNano": "1700000000000000000",
        "endTimeUnixNano": "1700000001000000000",
        "status": {"code": 1}
      }]
    }]
  }]
}"#;

fn tripped_router() -> (Arc<MsgRouter>, Arc<DiskCapacityState>, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let data_dir = camino::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
    let pool = Arc::new(ProjectPool::new(
        data_dir,
        4,
        sigiro::storage::StorageMode::LocalFile,
    ));

    // Arm a tiny cap and trip it (bytes >= cap => over).
    let disk = DiskCapacityState::new(1);
    assert_eq!(disk.apply_sample(1_000_000, true), Some(true));
    assert!(disk.is_over_cap());

    let router = Arc::new(MsgRouter::new_with_disk(
        Arc::clone(&pool),
        Arc::clone(&disk),
    ));
    router.start();
    (router, disk, tmp)
}

#[tokio::test]
async fn otlp_http_ingest_returns_507_when_over_disk_cap() {
    let (router, _disk, _tmp) = tripped_router();

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

    // 507 Insufficient Storage — NOT a 500, and NOT a 200.
    resp.assert_status(axum::http::StatusCode::INSUFFICIENT_STORAGE);
}

#[tokio::test]
async fn router_send_surfaces_typed_disk_capacity_error() {
    let (router, _disk, _tmp) = tripped_router();

    // The batch is never inspected — the gate rejects before touching it.
    let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));
    let err = router
        .send(IngestMsg::Traces {
            project_id: "default".to_string(),
            batch,
        })
        .await
        .expect_err("send must reject when over disk cap");

    assert!(
        matches!(err, RouterError::DiskCapacityExceeded),
        "expected DiskCapacityExceeded (maps to gRPC resource_exhausted), got {err:?}"
    );
}

#[tokio::test]
async fn otlp_http_ingest_ok_when_under_disk_cap() {
    // Cap armed but not tripped: ingest must NOT be gated (proves the gate is
    // the trip state, not merely "cap configured").
    let tmp = TempDir::new().expect("temp dir");
    let data_dir = camino::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
    let pool = Arc::new(ProjectPool::new(
        data_dir,
        4,
        sigiro::storage::StorageMode::LocalFile,
    ));
    let disk = DiskCapacityState::new(1_000_000_000);
    assert!(!disk.is_over_cap());
    let router = Arc::new(MsgRouter::new_with_disk(Arc::clone(&pool), disk));
    router.start();

    let app = sigiro::ingest::otlp_http_router(
        Arc::clone(&router),
        sigiro::auth::AuthMode::Open,
        8_388_608,
    );
    let server = TestServer::new(app).expect("build test server");

    // Empty payload transforms to zero rows and returns 200 without hitting the
    // flusher/DuckDB — enough to prove the gate does not reject when under cap.
    let resp = server
        .post("/v1/traces")
        .bytes("{\"resourceSpans\":[]}".into())
        .content_type("application/json")
        .await;
    resp.assert_status(axum::http::StatusCode::OK);
}

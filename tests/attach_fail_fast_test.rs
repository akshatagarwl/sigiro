//! E2E: a PERMANENT DuckLake attach failure (a data_path / storage-backend
//! mismatch) must FAIL FAST, not wedge the connection pool in its 30x blocking
//! retry loop — the exact failure mode behind the 2026-07 hosted outage, where a
//! catalog created against S3 could never re-attach under a local-Parquet config
//! and every request piled up behind ~30s of `thread::sleep`.
//!
//! `#[ignore]`d — needs Docker (Postgres). Run via:
//!   docker compose -f docker-compose.dev.yml up -d --wait postgres
//!   cargo test --test attach_fail_fast_test -- --ignored --nocapture
//!   docker compose -f docker-compose.dev.yml down -v

use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use sigiro::pool::ProjectPool;
use sigiro::storage::{self, StorageConfig, StorageMode};

// Matches docker-compose.dev.yml.
const CATALOG_URL: &str = "postgresql://sigiro:sigiro@127.0.0.1:5432/ducklake";
const CATALOG_LIBPQ: &str = "dbname=ducklake host=127.0.0.1 user=sigiro password=sigiro port=5432";
const TENANT: &str = "faulttest";
const SCHEMA: &str = "tenant_faulttest";

fn local_config() -> StorageConfig {
    StorageConfig {
        catalog_url: CATALOG_URL.to_string(),
        s3_bucket: String::new(), // local Parquet — the config that can't match an S3 catalog
        s3_region: String::new(),
        s3_endpoint: String::new(),
        s3_key_id: String::new(),
        s3_secret: String::new(),
        s3_use_ssl: true,
    }
}

fn pg_conn() -> duckdb::Connection {
    let conn = duckdb::Connection::open_in_memory().expect("open in-memory duckdb");
    conn.execute_batch("INSTALL ducklake; LOAD ducklake; INSTALL postgres; LOAD postgres;")
        .expect("load ducklake+postgres extensions");
    conn
}

fn drop_schema() {
    let conn = pg_conn();
    conn.execute_batch(&format!("ATTACH '{CATALOG_LIBPQ}' AS pg (TYPE postgres)"))
        .expect("attach pg for cleanup");
    conn.execute_batch(&format!("DROP SCHEMA IF EXISTS pg.{SCHEMA} CASCADE"))
        .expect("drop tenant schema");
}

/// Recreate the prod poison: a catalog whose recorded `data_path` is an S3 URL
/// the current local-Parquet config can never match. Built with a valid local
/// DATA_PATH, then the stored value is rewritten to S3 — the state a
/// since-removed S3 deploy left the prod catalog in.
fn poison_with_s3_data_path() {
    let conn = pg_conn();
    conn.execute_batch(&format!(
        "ATTACH 'ducklake:postgres:{CATALOG_LIBPQ}' AS lake \
         (METADATA_SCHEMA '{SCHEMA}', DATA_PATH '/tmp/sigiro-faulttest-orig', CREATE_IF_NOT_EXISTS true)"
    ))
    .expect("create tenant catalog");
    conn.execute_batch("DETACH lake").expect("detach lake");
    conn.execute_batch(&format!("ATTACH '{CATALOG_LIBPQ}' AS pg (TYPE postgres)"))
        .expect("attach pg to poison");
    conn.execute_batch(&format!(
        "UPDATE pg.{SCHEMA}.ducklake_metadata SET value = 's3://sigiro-poison/{TENANT}/' \
         WHERE key = 'data_path'"
    ))
    .expect("rewrite stored data_path to S3");
}

#[test]
#[ignore = "needs Docker Postgres (docker-compose.dev.yml)"]
fn poisoned_catalog_fails_fast_not_wedged() {
    let tmp = tempfile::tempdir().expect("create temp data dir");
    let data_dir = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 data dir");

    // Warm the pool's extension cache so the TIMED attach below measures retry-loop
    // time, not a cold extension download.
    storage::prewarm_extensions(&data_dir).expect("prewarm extensions");

    drop_schema();
    poison_with_s3_data_path();

    let pool = ProjectPool::new(
        data_dir.clone(),
        4,
        StorageMode::Distributed(local_config()),
    );

    let start = Instant::now();
    let result = pool.create_project(TENANT);
    let elapsed = start.elapsed();

    drop_schema(); // cleanup regardless of the assertions below

    let err = match result {
        Ok(_) => panic!("S3-pinned catalog + local config must fail to attach"),
        Err(e) => format!("{e:#}"),
    };

    // Definitive proof of the short-circuit: the fatal branch reports "not
    // retrying"; the old wedge would instead have run the loop dry and reported
    // "exhausted 30 ... attempts". This holds regardless of timing.
    assert!(
        err.contains("not retrying"),
        "permanent failure must short-circuit the retry loop; got: {err}"
    );
    assert!(
        !err.contains("exhausted"),
        "must NOT have run the full retry loop; got: {err}"
    );
    assert!(
        err.contains("data_path"),
        "failure must be classified as the data_path (backend mismatch) category; got: {err}"
    );
    // Secondary guard: one attempt is sub-second; the old wedge was
    // ATTACH_ATTEMPTS(30) x ATTACH_BACKOFF(1s) ≈ 30s.
    assert!(
        elapsed < Duration::from_secs(10),
        "fatal attach must return promptly; took {elapsed:?} (retry loop not short-circuited?)"
    );
}

//! Boot-failure tests spawn a real `sigiro serve` process and assert it
//! refuses to boot (non-zero exit + a clear stderr message) for incoherent
//! storage configuration or a failed local default-project initialization.
//!
//! Deliberately NOT `#[ignore]`d and needs NO Docker: these cases fail
//! before any Postgres/S3 connection is attempted.

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Spawn `sigiro serve` with a clean env, a throwaway data dir and empty
/// OTEL endpoint, then require it to exit before accepting traffic.
fn run_serve_expecting_fast_exit(extra_env: &[(&str, &str)]) -> (bool, String) {
    let data_dir = tempfile::tempdir().expect("failed to create temp data dir");
    run_serve_with_data_dir(data_dir.path(), extra_env)
}

fn run_serve_with_data_dir(data_dir: &Path, extra_env: &[(&str, &str)]) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigiro"));
    cmd.arg("serve")
        .env_clear()
        // Preserve PATH/HOME so the binary and any platform dirs resolve.
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("SIGIRO_DATA_DIR", data_dir)
        // Empty (not unset) so an ambient endpoint can't redirect telemetry.
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("failed to spawn `sigiro serve`");

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait().expect("failed to poll child status") {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    child.kill().ok();
                    child.wait().ok();
                    panic!(
                        "`sigiro serve` did not exit within 30s on an incoherent storage config \
                         — the boot-time coherence gate must fail fast, not hang"
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };

    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr).ok();
    }
    (status.success(), stderr)
}

#[test]
fn local_default_project_init_failure_refuses_to_serve() {
    let data_dir = tempfile::tempdir().expect("failed to create temp data dir");
    // A regular file at the project path makes initialization fail without
    // depending on extension downloads or filesystem permissions.
    std::fs::write(data_dir.path().join("default"), b"not a directory")
        .expect("failed to block default project directory");

    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve test HTTP port");
    let port = listener.local_addr().expect("test HTTP address").port();
    drop(listener);
    let port_str = port.to_string();
    let (success, stderr) = run_serve_with_data_dir(
        data_dir.path(),
        &[
            ("SIGIRO_HTTP_PORT", &port_str),
            ("SIGIRO_OTLP_HTTP_PORT", "0"),
            ("SIGIRO_GRPC_PORT", "0"),
        ],
    );
    assert!(
        !success,
        "serve must exit non-zero on default project init failure"
    );
    assert!(
        stderr.contains("Error: default project init failed"),
        "boot error must identify default project init; got stderr:\n{stderr}"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "HTTP listener must not be ready after failed initialization"
    );
}

#[test]
fn serve_refuses_to_boot_with_s3_bucket_but_no_catalog_url() {
    // Incoherent axis: S3 data storage requested (bucket set) with no
    // distributed PostgreSQL catalog. Must fail loud, not silently drop to
    // local-file mode and ignore the bucket.
    let (success, stderr) = run_serve_expecting_fast_exit(&[("SIGIRO_S3_BUCKET", "sigiro-data")]);
    assert!(
        !success,
        "serve must exit non-zero when SIGIRO_S3_BUCKET is set without SIGIRO_CATALOG_URL"
    );
    assert!(
        stderr.contains("SIGIRO_S3_BUCKET") && stderr.contains("SIGIRO_CATALOG_URL"),
        "boot error must name both incoherent vars; got stderr:\n{stderr}"
    );
}

#[test]
fn serve_refuses_to_boot_with_empty_catalog_url() {
    // Malformed/partial config: the catalog URL is present but empty. Must fail
    // loud rather than being treated as "unset" (local-file mode).
    let (success, stderr) = run_serve_expecting_fast_exit(&[("SIGIRO_CATALOG_URL", "")]);
    assert!(
        !success,
        "serve must exit non-zero when SIGIRO_CATALOG_URL is set but empty"
    );
    assert!(
        stderr.contains("SIGIRO_CATALOG_URL") && stderr.contains("empty"),
        "boot error must explain the empty catalog URL; got stderr:\n{stderr}"
    );
}

#[test]
fn serve_refuses_legacy_host_only_s3_endpoint() {
    let (success, stderr) = run_serve_expecting_fast_exit(&[
        (
            "SIGIRO_CATALOG_URL",
            "postgresql://sigiro:***@127.0.0.1:5432/ducklake",
        ),
        ("SIGIRO_S3_BUCKET", "sigiro-data"),
        ("SIGIRO_S3_REGION", "us-east-1"),
        ("SIGIRO_S3_ENDPOINT", "127.0.0.1:9000"),
        ("SIGIRO_S3_KEY_ID", "key"),
        ("SIGIRO_S3_SECRET", "secret"),
    ]);
    assert!(!success, "serve must reject a host-only S3 endpoint");
    assert!(
        stderr.contains("SIGIRO_S3_ENDPOINT")
            && stderr.contains("http://")
            && stderr.contains("https://"),
        "boot error must explain the absolute endpoint URL contract; got stderr:\n{stderr}"
    );
}

#[test]
fn serve_refuses_s3_fields_without_bucket() {
    let (success, stderr) = run_serve_expecting_fast_exit(&[
        (
            "SIGIRO_CATALOG_URL",
            "postgresql://sigiro:***@127.0.0.1:5432/ducklake",
        ),
        ("SIGIRO_S3_REGION", "us-east-1"),
    ]);
    assert!(!success, "serve must reject S3 fields without a bucket");
    assert!(
        stderr.contains("SIGIRO_S3_REGION") && stderr.contains("SIGIRO_S3_BUCKET"),
        "boot error must name the orphaned field and required bucket; got stderr:\n{stderr}"
    );
}

#[test]
fn serve_refuses_s3_fields_without_catalog() {
    let (success, stderr) =
        run_serve_expecting_fast_exit(&[("SIGIRO_S3_ENDPOINT", "https://s3.example.com")]);
    assert!(!success, "serve must reject S3 fields without a catalog");
    assert!(
        stderr.contains("SIGIRO_S3_ENDPOINT") && stderr.contains("SIGIRO_CATALOG_URL"),
        "boot error must name the orphaned field and required catalog; got stderr:\n{stderr}"
    );
}

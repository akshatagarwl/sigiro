//! Boot-failure tests for the storage config axis. These spawn a real
//! `sigiro serve` child process and assert it REFUSES TO BOOT (non-zero exit +
//! a clear stderr message) on an incoherent storage configuration, proving the
//! `StorageMode::from_env` coherence gate is actually wired into startup — not
//! just unit-tested in isolation.
//!
//! Deliberately NOT `#[ignore]`d and needs NO Docker: every case here fails at
//! config-resolution time, BEFORE any Postgres/S3 connection is attempted, so
//! it runs in the normal `cargo test` gate.

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Spawn `sigiro serve` with a clean, fully-specified env (only the vars passed
/// in, plus a throwaway data dir and empty OTEL endpoint), wait for it to exit
/// within `deadline`, and return `(exited_success, stderr)`. Fail-loud if it
/// does not exit on its own — a boot-failure case that HANGS is itself a bug.
fn run_serve_expecting_fast_exit(extra_env: &[(&str, &str)]) -> (bool, String) {
    let data_dir = tempfile::tempdir().expect("failed to create temp data dir");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigiro"));
    cmd.arg("serve")
        .env_clear()
        // Preserve PATH/HOME so the binary and any platform dirs resolve.
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("SIGIRO_DATA_DIR", data_dir.path())
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

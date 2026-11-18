//! Runtime retention check. Retention runs inside `sigiro serve`, whose DuckDB
//! is locked down and can NOT autoload the `icu` extension — so the retention
//! SQL must avoid every timezone operation. A unit test can't catch this
//! (its DuckDB has icu available); only booting the real server does. This
//! guards against the regression where `now() - INTERVAL` / `AT TIME ZONE` /
//! `epoch(now())` silently failed retention on every tick in production.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn retention_runs_without_timezone_extension_at_runtime() {
    let data_dir = tempfile::tempdir().expect("temp data dir");
    let log = tempfile::NamedTempFile::new().expect("temp log");
    let log_path = log.path().to_path_buf();

    // RETENTION_DAYS=1 arms retention; the maintenance task's first interval
    // tick fires immediately at boot, so the cleanup runs within seconds.
    let mut child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
        .arg("serve")
        .env("SIGIRO_DATA_DIR", data_dir.path())
        // Shared, stable extension cache across runs (same path api_integration_test
        // uses). Without it each spawn cold-downloads every DuckDB extension into the
        // temp data dir — now including icu — which can push serve boot past the 90s
        // deadline below and flake the test. Read-only after the first download.
        .env("SIGIRO_EXTENSION_DIR", "/tmp/sigiro-test-extensions")
        .env("SIGIRO_HTTP_PORT", "42101")
        .env("SIGIRO_OTLP_HTTP_PORT", "42102")
        .env("SIGIRO_GRPC_PORT", "42103")
        .env("SIGIRO_RETENTION_DAYS", "1")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(log.reopen().expect("reopen log for child stderr"))
        .spawn()
        .expect("failed to spawn `sigiro serve`");

    let deadline = Instant::now() + Duration::from_secs(90);
    let outcome = loop {
        let mut buf = String::new();
        std::fs::File::open(&log_path)
            .and_then(|mut f| f.read_to_string(&mut buf))
            .ok();
        if buf.contains("retention cleanup completed") {
            break Ok(());
        }
        if buf.contains("retention cleanup failed") {
            let line = buf
                .lines()
                .find(|l| l.contains("retention cleanup failed"))
                .unwrap_or("")
                .to_string();
            break Err(line);
        }
        if Instant::now() >= deadline {
            break Err("retention never ran within 90s".to_string());
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    // SAFETY: SIGTERM only this test's child.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    child.wait().ok();

    outcome.expect("retention must succeed inside the locked-down `sigiro serve` runtime");
}

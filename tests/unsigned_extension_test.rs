//! Can sigiro load a locally built (unsigned) DuckLake extension?
//!
//! sigiro#317 is fixed upstream, but no rebuilt DuckLake is published for the
//! pinned DuckDB version, so the fix can only ship as a locally built binary —
//! and DuckDB refuses unsigned extensions unless `allow_unsigned_extensions` is
//! set at Config time.
//!
//! An earlier deploy failed in production because the extension was verified
//! under a test that set that flag explicitly, while the service never did:
//!
//!   Extension "…/ducklake.duckdb_extension" could not be loaded because its
//!   signature is either missing or invalid and unsigned extensions are
//!   disabled by configuration (allow_unsigned_extensions)
//!
//! Both halves of the contract are pinned here: default config must refuse, and
//! the opt-in must accept.
//!
//! `Envar::on_startup` caches on FIRST ACCESS for the life of the process, so
//! the two cases cannot share one process — setting the variable after another
//! test has already read it has no effect. Each case therefore re-executes this
//! test binary as a child with the environment established up front.

// Driving the env var IS the contract under test here, so the production ban on
// direct env access does not apply — same exemption the other test files take.
#![allow(clippy::disallowed_methods)]

use sigiro::pool::open_duckdb_connection;

const VAR: &str = "SIGIRO_ALLOW_UNSIGNED_EXTENSIONS";
const CHILD: &str = "SIGIRO_UNSIGNED_CHILD";

fn built_extension() -> Option<String> {
    let path = std::env::var("SIGIRO_TEST_DUCKLAKE").ok()?;
    std::path::Path::new(&path).exists().then_some(path)
}

/// Re-run this test binary as a child with `VAR` set per `allow`.
fn run_child(test_name: &str, allow: bool) -> String {
    let exe = std::env::current_exe().expect("test binary path");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([test_name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1");
    if allow {
        cmd.env(VAR, "true");
    } else {
        cmd.env_remove(VAR);
    }
    let out = cmd.output().expect("run child test process");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The load attempt itself. Runs only in the child; as a parent it is a no-op so
/// the harness never executes it with an unknown environment.
#[test]
fn load_unsigned_extension_child() {
    if std::env::var(CHILD).is_err() {
        return;
    }
    let Some(ext) = built_extension() else {
        println!("CHILD_RESULT: SKIPPED");
        return;
    };
    let conn = open_duckdb_connection().expect("open duckdb");
    match conn.execute_batch(&format!("LOAD '{ext}';")) {
        Ok(()) => {
            let version: String = conn
                .query_row(
                    "SELECT coalesce(extension_version,'?') FROM duckdb_extensions() \
                     WHERE extension_name='ducklake' AND loaded",
                    [],
                    |r| r.get(0),
                )
                .expect("extension must actually be loaded");
            println!("CHILD_RESULT: LOADED {version}");
        }
        Err(e) => println!(
            "CHILD_RESULT: REFUSED {}",
            e.to_string().lines().next().unwrap_or("")
        ),
    }
}

#[test]
fn unsigned_extension_is_refused_by_default() {
    if std::env::var(CHILD).is_ok() || built_extension().is_none() {
        return;
    }
    let out = run_child("load_unsigned_extension_child", false);
    assert!(
        out.contains("CHILD_RESULT: REFUSED") && out.contains("signature"),
        "default config must refuse an unsigned extension; got:\n{out}"
    );
    eprintln!("default config correctly refused the unsigned extension");
}

#[test]
fn unsigned_extension_loads_with_the_opt_in() {
    if std::env::var(CHILD).is_ok() || built_extension().is_none() {
        return;
    }
    let out = run_child("load_unsigned_extension_child", true);
    assert!(
        out.contains("CHILD_RESULT: LOADED"),
        "SIGIRO_ALLOW_UNSIGNED_EXTENSIONS=true must allow the load; got:\n{out}"
    );
    let line = out
        .lines()
        .find(|l| l.contains("CHILD_RESULT: LOADED"))
        .unwrap_or_default();
    eprintln!("opt-in loaded the extension — {}", line.trim());
}

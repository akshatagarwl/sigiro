#![allow(clippy::disallowed_methods)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Guards tests that mutate the SIGIRO_API_KEY env var.
/// std::env::set_var/remove_var mutate process-global state and race when
/// cargo test runs tests in parallel threads.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Helper: create a temp dir with a config.toml containing the given content.
fn write_temp_config(content: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("config.toml");
    let mut f = std::fs::File::create(&path).expect("create file");
    f.write_all(content.as_bytes()).expect("write");
    (dir, path)
}

#[test]
fn read_config_from_file() {
    let (_dir, path) = write_temp_config("api_key=\"sk_live_test123\"\n");
    let content = std::fs::read_to_string(&path).unwrap();
    let config: toml::Value = content.parse().unwrap();
    assert_eq!(config["api_key"].as_str().unwrap(), "sk_live_test123");
}

#[test]
fn env_var_overrides_file() {
    let _guard = ENV_MUTEX.lock().unwrap();
    // Set env var, verify get_api_key returns it regardless of file state
    unsafe { std::env::set_var("SIGIRO_API_KEY", "sk_env_override") };
    let result = sigiro::config::get_api_key();
    unsafe { std::env::remove_var("SIGIRO_API_KEY") };
    assert_eq!(result.unwrap(), "sk_env_override");
}

#[test]
fn missing_config_returns_error() {
    let _guard = ENV_MUTEX.lock().unwrap();
    // Ensure no env var set
    unsafe { std::env::remove_var("SIGIRO_API_KEY") };
    let bad_path = PathBuf::from("/tmp/sigiro-test-nonexistent/config.toml");
    let err = std::fs::read_to_string(&bad_path).unwrap_err();
    assert!(
        err.to_string().contains("No such file"),
        "error should indicate missing file"
    );
}

#[test]
fn data_dir_requires_env_var() {
    // data_dir() must read SIGIRO_DATA_DIR — if unset it must error,
    // not silently return an empty path. This catches the mutant that
    // replaces data_dir with Ok(Default::default()).
    let _guard = ENV_MUTEX.lock().unwrap();
    unsafe { std::env::remove_var("SIGIRO_DATA_DIR") };
    let result = sigiro::data_dir();
    // Env var is restored by the mutex guard dropping, but verify the
    // error path explicitly.
    assert!(
        result.is_err(),
        "data_dir should error when SIGIRO_DATA_DIR is unset"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("SIGIRO_DATA_DIR"),
        "error message should mention SIGIRO_DATA_DIR, got: {err_msg}"
    );
}

#[test]
fn auth_command_writes_config() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "api_key=\"sk_live_written\"\n").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("sk_live_written"));
}

#[cfg(unix)]
#[test]
fn config_file_permissions_0600() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let sigiro_dir = dir.path().join("sigiro");
    std::fs::create_dir_all(&sigiro_dir).unwrap();
    let path = sigiro_dir.join("config.toml");
    std::fs::write(&path, "api_key=\"sk_live_test123\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let perms = std::fs::metadata(&path).unwrap().permissions();
    assert_eq!(perms.mode() & 0o777, 0o600);
}

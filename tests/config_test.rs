#![allow(clippy::disallowed_methods)]

use sigiro::config::QueryRowCap;

#[test]
fn query_row_cap_rejects_zero_and_preserves_numeric_config_format() {
    assert!(QueryRowCap::try_new(0).is_err());
    assert!("0".parse::<QueryRowCap>().is_err());

    let cap = "1000000"
        .parse::<QueryRowCap>()
        .expect("positive row cap must retain the existing numeric config format");
    assert_eq!(cap.as_usize(), 1_000_000);
}

#[test]
fn data_dir_uses_platform_default_when_unset() {
    // data_dir() falls back to the platform data directory when SIGIRO_DATA_DIR
    // is unset (~/.local/share/sigiro on Unix, %LOCALAPPDATA%\sigiro on Windows).
    unsafe { std::env::remove_var("SIGIRO_DATA_DIR") };
    let result = sigiro::data_dir();
    assert!(
        result.is_ok(),
        "data_dir should return platform default when SIGIRO_DATA_DIR is unset"
    );
    let path = result.unwrap();
    assert!(
        path.as_str().ends_with("sigiro"),
        "data_dir should end with 'sigiro', got: {path}"
    );
}

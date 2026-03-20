use sigiro::{format_bytes, parse_window};

#[test]
fn parse_window_minutes() {
    assert_eq!(parse_window("15m").unwrap(), 15 * 60 * 1_000_000);
    assert_eq!(parse_window("1m").unwrap(), 60 * 1_000_000);
}

#[test]
fn parse_window_hours() {
    assert_eq!(parse_window("1h").unwrap(), 3600 * 1_000_000);
    assert_eq!(parse_window("2h").unwrap(), 2 * 3600 * 1_000_000);
}

#[test]
fn parse_window_trims() {
    assert_eq!(parse_window("  15m  ").unwrap(), 15 * 60 * 1_000_000);
}

#[test]
fn parse_window_rejects_invalid() {
    assert!(parse_window("15").is_err());
    assert!(parse_window("abc").is_err());
    assert!(parse_window("15d").is_err());
}

#[test]
fn format_bytes_units() {
    assert_eq!(format_bytes(500), "500 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
}

#[test]
fn format_bytes_fractional() {
    assert_eq!(format_bytes(1536), "1.5 KB");
}

use sigiro::format_bytes;

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

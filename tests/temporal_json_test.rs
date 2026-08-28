//! Temporal values must render self-describing over `/v1/query`.
//!
//! These drive real DuckDB columns of each temporal type through the real
//! converter, so the `ValueRef` variants and units are the ones production
//! actually receives rather than hand-built fixtures.
//!
//! The bug being locked out: the previous `Timestamp(_, us)` arm discarded the
//! unit and emitted raw ticks, so a `TIMESTAMP_S` column and a `TIMESTAMP_NS`
//! column produced bare integers 10^9 apart with nothing to tell them apart.
//! INTERVAL had no arm at all and reached callers as a Rust debug string.

use duckdb::Connection;
use sigiro::sql::duckdb_value_to_json;

/// One row, one column, through the production converter.
fn render(conn: &Connection, expr: &str) -> serde_json::Value {
    let sql = format!("SELECT {expr} AS v");
    let mut stmt = conn.prepare(&sql).expect("prepare");
    let mut rows = stmt.query([]).expect("query");
    let row = rows.next().expect("row").expect("one row");
    duckdb_value_to_json(row.get_ref(0).expect("get_ref"))
}

#[test]
fn every_timestamp_unit_renders_the_same_instant() {
    let conn = Connection::open_in_memory().expect("open");

    // The SAME instant stored at four different precisions. Before the fix
    // these produced 1787668085, 1787668085309, 1787668085308912 and
    // 1787668085308912987: four different bare integers for one moment.
    let s = render(&conn, "'2026-08-25 14:28:05.308912987'::TIMESTAMP_S");
    let ms = render(&conn, "'2026-08-25 14:28:05.308912987'::TIMESTAMP_MS");
    let us = render(&conn, "'2026-08-25 14:28:05.308912987'::TIMESTAMP");
    let ns = render(&conn, "'2026-08-25 14:28:05.308912987'::TIMESTAMP_NS");

    for v in [&s, &ms, &us, &ns] {
        let text = v
            .as_str()
            .unwrap_or_else(|| panic!("expected string, got {v}"));
        assert!(
            text.starts_with("2026-08-25T14:28:05"),
            "every precision must render the same instant, got {text}"
        );
        assert!(text.ends_with('Z'), "must be UTC RFC 3339, got {text}");
    }

    // Precision is preserved, not normalised away.
    assert_eq!(us.as_str().unwrap(), "2026-08-25T14:28:05.308912Z");
    assert_eq!(ns.as_str().unwrap(), "2026-08-25T14:28:05.308912987Z");
    assert_eq!(ms.as_str().unwrap(), "2026-08-25T14:28:05.309Z");
    assert_eq!(s.as_str().unwrap(), "2026-08-25T14:28:05Z");
}

#[test]
fn timestamptz_and_naive_timestamp_render_identically() {
    // They are indistinguishable in ValueRef (both Timestamp(Microsecond, ..)),
    // so this asserts the documented consequence rather than a nicety: every
    // timestamp renders as UTC.
    let conn = Connection::open_in_memory().expect("open");
    let naive = render(&conn, "'2026-08-25 14:28:05.308912'::TIMESTAMP");
    let tz = render(&conn, "'2026-08-25 14:28:05.308912+00'::TIMESTAMPTZ");
    assert_eq!(naive, tz);
    assert_eq!(tz.as_str().unwrap(), "2026-08-25T14:28:05.308912Z");
}

#[test]
fn interval_is_not_a_rust_debug_string() {
    let conn = Connection::open_in_memory().expect("open");

    // The exact production reproduction from the bug report.
    let zero = render(&conn, "'2026-08-25'::TIMESTAMP - '2026-08-25'::TIMESTAMP");
    let text = zero.as_str().expect("string");
    assert!(
        !text.contains("Interval {"),
        "a Rust debug struct reached the caller: {text}"
    );
    assert_eq!(text, "PT0S");

    assert_eq!(
        render(&conn, "INTERVAL 362081 MICROSECOND")
            .as_str()
            .unwrap(),
        "PT0.362081S"
    );
    assert_eq!(
        render(&conn, "INTERVAL 90 SECOND").as_str().unwrap(),
        "PT90S"
    );
    // Calendar parts stay separate from the time part, because their length
    // depends on when they are applied.
    assert_eq!(render(&conn, "INTERVAL 14 MONTH").as_str().unwrap(), "P14M");
}

#[test]
fn date_and_time_render_as_text() {
    let conn = Connection::open_in_memory().expect("open");
    assert_eq!(
        render(&conn, "'2026-08-25'::DATE").as_str().unwrap(),
        "2026-08-25"
    );
    assert_eq!(
        render(&conn, "'14:28:05.308912'::TIME").as_str().unwrap(),
        "14:28:05.308912"
    );
}

#[test]
fn non_temporal_types_are_untouched() {
    // The converter is shared by every /v1/query response, so the change must
    // not disturb the numeric arms the API already returns.
    let conn = Connection::open_in_memory().expect("open");
    assert_eq!(render(&conn, "42::BIGINT"), serde_json::json!(42));
    assert_eq!(render(&conn, "1.5::DOUBLE"), serde_json::json!(1.5));
    assert_eq!(render(&conn, "'hi'"), serde_json::json!("hi"));
    assert_eq!(render(&conn, "NULL::BIGINT"), serde_json::Value::Null);
    assert_eq!(render(&conn, "true"), serde_json::json!(true));
}

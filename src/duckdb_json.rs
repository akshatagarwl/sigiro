//! The DuckDB→JSON value converter shared by `/v1/query` and the federated
//! query coordinator. Native-only: it reads `duckdb-rs` rows.

/// Read and convert one DuckDB result cell. DuckDB v2 emits a dedicated
/// Arrow `Null` array for an untyped SQL `NULL`; duckdb-rs 1.x cannot turn
/// that array into `ValueRef`, so handle it before calling `Row::get_ref`.
pub fn duckdb_row_value_to_json(
    row: &duckdb::Row<'_>,
    idx: usize,
    data_type: &arrow::datatypes::DataType,
) -> duckdb::Result<serde_json::Value> {
    if matches!(data_type, arrow::datatypes::DataType::Null) {
        return Ok(serde_json::Value::Null);
    }
    row.get_ref(idx).map(duckdb_value_to_json)
}

/// Convert one DuckDB column value to a `serde_json::Value`. The ONE
/// canonical value converter — shared by the `/v1/query` HTTP path
/// (`execute_sql` above) and the federated query coordinator
/// (`crate::federate::run_federated_query`), which reads rows off its own
/// local coordinator connection the same way this handler reads rows off a
/// project reader connection. Public so tests can exercise it directly too.
pub fn duckdb_value_to_json(val: duckdb::types::ValueRef<'_>) -> serde_json::Value {
    use duckdb::types::ValueRef;
    match val {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Boolean(b) => serde_json::Value::Bool(b),
        ValueRef::TinyInt(b) => serde_json::json!(b),
        ValueRef::SmallInt(b) => serde_json::json!(b),
        ValueRef::Int(b) => serde_json::json!(b),
        ValueRef::BigInt(b) => serde_json::json!(b),
        // HUGEINT (int128) — produced e.g. by SUM over BIGINT (the federated
        // fold's sum(count)/sum(sum)). Emit as a JSON number when it fits i64,
        // else as a string to avoid silent precision loss. Without this arm it
        // fell through to the Debug catch-all ("HugeInt(3)").
        ValueRef::HugeInt(b) => match i64::try_from(b) {
            Ok(n) => serde_json::json!(n),
            Err(_) => serde_json::Value::String(b.to_string()),
        },
        // Unsigned integer variants — a passthrough `SELECT *` over a
        // sigiro_* table (e.g. a UBIGINT column) used to fall through to the
        // `{:?}` catch-all below and print e.g. `"UBigInt(5)"` instead of `5`.
        ValueRef::UTinyInt(b) => serde_json::json!(b),
        ValueRef::USmallInt(b) => serde_json::json!(b),
        ValueRef::UInt(b) => serde_json::json!(b),
        ValueRef::UBigInt(b) => serde_json::json!(b),
        ValueRef::Float(b) => serde_json::json!(b),
        ValueRef::Double(b) => serde_json::json!(b),
        // DECIMAL — emit a JSON number when the value round-trips through f64
        // exactly (checked by reformatting at the same scale and comparing
        // text, not by comparing floats), else a decimal string so a
        // high-precision DECIMAL never silently loses digits.
        ValueRef::Decimal(d) => decimal_to_json(&d.to_string()),
        // Time (#8, #236): a point in time is microseconds since the Unix
        // epoch whatever the column's unit, matching the µs integers the rest
        // of the API uses; a length of time (INTERVAL, such as
        // `end_timestamp - timestamp`) is microseconds too, like `duration`.
        // A DATE or TIME of day has no epoch, so it is ISO 8601 text.
        ValueRef::Timestamp(unit, value) => serde_json::json!(unit.to_micros(value)),
        ValueRef::Date32(days) => date_to_json(days),
        ValueRef::Time64(unit, value) => time_of_day_to_json(unit.to_micros(value)),
        ValueRef::Interval {
            months,
            days,
            nanos,
        } => serde_json::json!(interval_micros(months, days, nanos)),
        ValueRef::Text(b) => serde_json::Value::String(String::from_utf8_lossy(b).to_string()),
        ValueRef::Blob(b) => serde_json::Value::String(format!("[blob {} bytes]", b.len())),
        // Containers (LIST, STRUCT, MAP, ...) convert through the owned value.
        _ => duckdb_owned_value_to_json(val.to_owned()),
    }
}

fn duckdb_owned_value_to_json(val: duckdb::types::Value) -> serde_json::Value {
    use duckdb::types::Value;

    match val {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(v) => serde_json::json!(v),
        Value::TinyInt(v) => serde_json::json!(v),
        Value::SmallInt(v) => serde_json::json!(v),
        Value::Int(v) => serde_json::json!(v),
        Value::BigInt(v) => serde_json::json!(v),
        Value::HugeInt(v) => match i64::try_from(v) {
            Ok(n) => serde_json::json!(n),
            Err(_) => serde_json::Value::String(v.to_string()),
        },
        Value::UHugeInt(v) => match u64::try_from(v) {
            Ok(n) => serde_json::json!(n),
            Err(_) => serde_json::Value::String(v.to_string()),
        },
        Value::UTinyInt(v) => serde_json::json!(v),
        Value::USmallInt(v) => serde_json::json!(v),
        Value::UInt(v) => serde_json::json!(v),
        Value::UBigInt(v) => serde_json::json!(v),
        Value::Float(v) => serde_json::json!(v),
        Value::Double(v) => serde_json::json!(v),
        Value::Decimal(v) => decimal_to_json(&v.to_string()),
        Value::Timestamp(unit, v) => serde_json::json!(unit.to_micros(v)),
        Value::Text(v) | Value::Enum(v) => serde_json::Value::String(v),
        Value::Blob(v) | Value::Geometry(v) => {
            serde_json::Value::String(format!("[blob {} bytes]", v.len()))
        }
        Value::Date32(v) => date_to_json(v),
        Value::Time64(unit, v) => time_of_day_to_json(unit.to_micros(v)),
        Value::Interval {
            months,
            days,
            nanos,
        } => serde_json::json!(interval_micros(months, days, nanos)),
        Value::Struct(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), duckdb_owned_value_to_json(value.clone())))
                .collect(),
        ),
        // A MAP's keys need not be strings, so it is a list of pairs.
        Value::Map(entries) => serde_json::Value::Array(
            entries
                .iter()
                .map(|(key, value)| {
                    serde_json::json!({
                        "key": duckdb_owned_value_to_json(key.clone()),
                        "value": duckdb_owned_value_to_json(value.clone()),
                    })
                })
                .collect(),
        ),
        Value::Union(value) => duckdb_owned_value_to_json(*value),
        Value::List(values) | Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(duckdb_owned_value_to_json).collect())
        }
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

/// A DATE as `YYYY-MM-DD`; a day outside `time`'s range falls back to the day
/// count so a value is never lost.
fn date_to_json(days_since_epoch: i32) -> serde_json::Value {
    time::Date::from_julian_day(UNIX_EPOCH_JULIAN_DAY + days_since_epoch).map_or_else(
        |_| serde_json::json!(days_since_epoch),
        |date| date.to_string().into(),
    )
}

/// A TIME of day as `HH:MM:SS.ffffff`.
fn time_of_day_to_json(micros: i64) -> serde_json::Value {
    let secs = micros.div_euclid(1_000_000);
    let frac = micros.rem_euclid(1_000_000);
    format!(
        "{:02}:{:02}:{:02}.{frac:06}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
    .into()
}

/// An INTERVAL in microseconds, counting a month as 30 days the way DuckDB's
/// own `epoch_us(INTERVAL)` does.
fn interval_micros(months: i32, days: i32, nanos: i64) -> i64 {
    const MICROS_PER_DAY: i64 = 86_400_000_000;
    (i64::from(months) * 30 + i64::from(days)) * MICROS_PER_DAY + nanos / 1_000
}

/// `time::Date::from_julian_day(UNIX_EPOCH_JULIAN_DAY)` is 1970-01-01.
const UNIX_EPOCH_JULIAN_DAY: i32 = 2_440_588;

/// Render a DuckDB DECIMAL's exact text (`d.to_string()`, e.g. `"15.50"`) as a
/// JSON number when converting it to `f64` and back at the same scale
/// reproduces the identical text (i.e. no precision was lost), else as a
/// string. Pure text/parse round-trip — never trusts float equality.
fn decimal_to_json(exact: &str) -> serde_json::Value {
    let Ok(f) = exact.parse::<f64>() else {
        return serde_json::Value::String(exact.to_string());
    };
    let scale = exact.split_once('.').map_or(0, |(_, frac)| frac.len());
    if format!("{f:.scale$}") == exact {
        serde_json::json!(f)
    } else {
        serde_json::Value::String(exact.to_string())
    }
}

#[cfg(test)]
mod tests {
    /// Run one single-row query and convert each cell the way `/v1/query` does.
    fn row(sql: &str) -> Vec<serde_json::Value> {
        let conn = crate::pool::open_duckdb().unwrap();
        let mut stmt = conn.prepare(sql).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let stmt = row.as_ref();
        (0..stmt.column_count())
            .map(|idx| super::duckdb_row_value_to_json(row, idx, &stmt.column_type(idx)).unwrap())
            .collect()
    }

    #[test]
    fn every_timestamp_unit_comes_back_as_microseconds() {
        let at = "'2026-11-25 11:00:00.123456'";
        assert_eq!(
            row(&format!(
                "SELECT {at}::TIMESTAMP, '2026-11-25 11:00:00.123456+00'::TIMESTAMPTZ, {at}::TIMESTAMP_MS, \
                 {at}::TIMESTAMP_S, '2026-11-25 11:00:00.123456789'::TIMESTAMP_NS"
            )),
            [
                1_795_604_400_123_456_i64,
                1_795_604_400_123_456,
                1_795_604_400_123_000,
                1_795_604_400_000_000,
                1_795_604_400_123_456,
            ]
            .map(serde_json::Value::from)
        );
    }

    #[test]
    fn intervals_are_microseconds_and_dates_and_times_are_iso_text() {
        assert_eq!(
            row(
                "SELECT TIMESTAMP '2026-11-25 11:00:01.5' - TIMESTAMP '2026-11-24 11:00:00', \
                 INTERVAL 1 MONTH, DATE '2026-11-25', TIME '13:05:46.414'"
            ),
            [
                serde_json::json!(86_401_500_000_i64),
                serde_json::json!(30 * 86_400_000_000_i64),
                serde_json::json!("2026-11-25"),
                serde_json::json!("13:05:46.414000"),
            ]
        );
    }

    #[test]
    fn structs_and_maps_are_json_not_debug_text() {
        assert_eq!(
            row("SELECT {'a': 1, 'b': [TIMESTAMP '1970-01-01 00:00:01']}, MAP {'k': 2}"),
            [
                serde_json::json!({"a": 1, "b": [1_000_000]}),
                serde_json::json!([{"key": "k", "value": 2}]),
            ]
        );
    }
}

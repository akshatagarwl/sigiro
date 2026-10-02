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
        // Timestamp/Date32/Time64 — raw ticks, not a Debug-formatted enum
        // tuple.
        ValueRef::Timestamp(_, us) => serde_json::json!(us),
        ValueRef::Date32(d) => serde_json::json!(d),
        ValueRef::Time64(_, t) => serde_json::json!(t),
        ValueRef::Text(b) => serde_json::Value::String(String::from_utf8_lossy(b).to_string()),
        ValueRef::Blob(b) => serde_json::Value::String(format!("[blob {} bytes]", b.len())),
        ValueRef::List(..) => duckdb_owned_value_to_json(val.to_owned()),
        _ => serde_json::Value::String(format!("{:?}", val)),
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
        Value::Timestamp(_, v) => serde_json::json!(v),
        Value::Text(v) | Value::Enum(v) => serde_json::Value::String(v),
        Value::Blob(v) | Value::Geometry(v) => {
            serde_json::Value::String(format!("[blob {} bytes]", v.len()))
        }
        Value::Date32(v) => serde_json::json!(v),
        Value::Time64(_, v) => serde_json::json!(v),
        Value::List(values) | Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(duckdb_owned_value_to_json).collect())
        }
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

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

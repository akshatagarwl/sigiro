//! The read-only SQL firewall and the DuckDB→JSON value converter.
//!
//! Pure logic: every item here is a function of its arguments alone — no
//! connection, no filesystem, no clock. It lives in its own module because
//! four modules need it and three of them used to reach back into
//! `crate::server` to get it, which made `server` a dependency of the code
//! it serves (`quack -> server`, `federate -> server`).
//!
//! `validate_read_only*` is the fail-closed AST firewall applied in EVERY
//! storage mode — see `BLOCKED_TABLE_FUNCTIONS` for how the denylist,
//! the positive scalar-function allowlist and the table allowlist combine.

use sqlparser::ast::{
    Expr, ObjectName, Query as SqlQuery, SetExpr, Statement, TableFactor, Visit, Visitor,
};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};
use std::ops::ControlFlow;
/// Escape a string for safe use inside a SQL single-quoted string literal.
pub(crate) fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

/// DuckDB file-reading table functions blocked even in SELECT queries
/// (e.g., read_csv('/etc/passwd') is a valid SELECT but reads arbitrary files).
/// This SQL-layer denylist is defense-in-depth, not the primary guard, in
/// EITHER storage mode: in local mode the engine-level filesystem lockdown
/// (pool::lock_filesystem, `enable_external_access = false`) additionally
/// blocks these at the engine level regardless of function name; in
/// distributed/S3 mode (where DuckLake needs network access, so the lockdown
/// is skipped — see `pool::lock_filesystem`'s doc comment) this denylist plus
/// the positive scalar-function allowlist (`SCALAR_FUNCTIONS`) below and the
/// table allowlist (`ALLOWED_TABLES`) ARE the real guard: together they are
/// `validate_read_only`, the SAME fail-closed AST-based firewall in every
/// mode. A denylist alone can never be complete against DuckDB's reader
/// family, which is why it is only one layer of `validate_read_only`, not
/// the whole of it.
const BLOCKED_TABLE_FUNCTIONS: &[&str] = &[
    "DROP",
    "READ_CSV",
    "READ_CSV_AUTO",
    "READ_PARQUET",
    "PARQUET_SCAN",
    "READ_JSON",
    "READ_JSON_AUTO",
    "READ_NDJSON",
    "READ_NDJSON_AUTO",
    "READ_TEXT",
    "READ_BLOB",
    "GLOB",
    "SNIFF_CSV",
    "CSV_SCAN",
    "DUCKDB_SECRETS",
    "DUCKDB_SETTINGS",
];

/// DuckLake time-travel qualifier, e.g. `AT (VERSION => 3)` or
/// `AT (TIMESTAMP => '2026-07-09 19:55:00')`. sqlparser's DuckDB dialect can't
/// parse the `=>` form yet, so it is stripped before the SELECT-only structural
/// check. The keyword denylist below still scans the ORIGINAL sql, so this
/// widens reads without weakening the write/DDL/file-access guards.
///
/// Captures the clause BODY (group 1) so `validate_read_only` can check it
/// against `TIME_TRAVEL_AT_LITERAL` before ever stripping/parsing anything —
/// see that constant's doc comment for why the body must be validated
/// separately from the surrounding SELECT.
static TIME_TRAVEL_AT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\bAT\s*\(([^)]*=>[^)]*)\)").expect("valid time-travel regex")
});

/// The ONLY AT (...) bodies `validate_read_only` accepts: a bare
/// `VERSION => <integer literal>` or `TIMESTAMP => '<string literal>'`,
/// anchored start-to-end so nothing can trail a valid-looking prefix.
///
/// Why this exists: DuckDB executes the ORIGINAL sql, AT body included —
/// `validate_read_only` strips the AT clause only so sqlparser (which can't
/// parse the `=>` syntax) can parse the surrounding SELECT. Before this
/// check, that meant validated-SQL != executed-SQL for anything hidden
/// inside `AT (...)`: a function call, a subquery, a column reference, or
/// any other expression there ran with NONE of the scalar/relation
/// allowlists in this file ever seeing it — e.g. `AT (TIMESTAMP =>
/// CURRENT_TIMESTAMP)` or `AT (VERSION => some_column)` used to parse and
/// validate cleanly despite never having been AST-checked at all. A
/// positive check (accept only these two literal shapes) closes that gap
/// outright rather than trying to deny specific dangerous expression shapes
/// — DuckDB's expression grammar is large and a denylist here would always
/// be one shape behind, same reasoning as `SCALAR_FUNCTIONS` above.
static TIME_TRAVEL_AT_LITERAL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)^\s*(?:VERSION\s*=>\s*-?[0-9]+|TIMESTAMP\s*=>\s*'(?:[^']|'')*')\s*$")
        .expect("valid time-travel literal regex")
});

/// The entire allowed FROM-target surface for user queries: every table
/// sigiro itself creates (src/schema.rs CREATE TABLE names), including
/// `sigiro_anomalies` — the table the scheduled anomaly pass
/// (`crate::anomaly`) writes and /v1/diagnose + /v1/anomalies read.
/// Matched case-insensitively against the FINAL identifier — see
/// `is_allowed_table` for how the schema-qualifier prefix (`lake.` /
/// `main.`) is handled.
pub(crate) const ALLOWED_TABLES: &[&str] = &[
    "SIGIRO_SPANS",
    "SIGIRO_LOGS",
    "SIGIRO_LOG_TEMPLATES",
    "SIGIRO_METRICS_GAUGE",
    "SIGIRO_METRICS_SUM",
    "SIGIRO_METRICS_HISTOGRAM",
    "SIGIRO_METRICS_EXP_HISTOGRAM",
    "SIGIRO_PROFILES",
    "SIGIRO_ANOMALIES",
];

/// Table-valued / pragma functions reachable through this same `/v1/query`
/// guard — allowlist EXACTLY these, sourced from real call sites:
/// - `PRAGMA_DATABASE_SIZE` — the DuckDB memory-limit/usage self-check
///   (`SELECT memory_limit, memory_usage FROM pragma_database_size()`), the
///   one way an operator can read storage-engine headroom over the public
///   query surface. Verified by tests/server_test.rs.
/// - `RANGE` — tests/api_integration_test.rs
///   `test_query_midstream_error_never_returns_partial_rows` (`FROM range(5) t(i)`,
///   a pure row generator with no filesystem/network access).
const SAFE_FUNCTIONS: &[&str] = &["PRAGMA_DATABASE_SIZE", "RANGE"];

/// Case-insensitive SCALAR function names allowed anywhere a plain
/// `Expr::Function` can appear — SELECT-list, WHERE, HAVING, GROUP BY,
/// ORDER BY, CASE branches, aggregate/function arguments, subqueries.
/// This is a DIFFERENT concern from `SAFE_FUNCTIONS`/`is_safe_function`
/// above (relation-position table-valued functions in a `FROM`) — a
/// function can need to be on one list, the other, both, or neither.
///
/// This closes the scalar-function hole: `AllowlistVisitor` used to
/// allowlist relations but not scalar calls, so `SELECT getenv('AWS_...')`
/// or `SELECT current_setting('extension_directory')` parsed as a
/// harmless-looking `SELECT` with no relation to reject and no blocked
/// keyword for the tokenizer scan — leaking process env / DuckDB config.
/// A POSITIVE allowlist (reject anything not explicitly named) because
/// DuckDB ships dozens of settings/env/introspection scalars and a
/// denylist is always one function behind.
///
/// Sourced from:
/// - Real `/v1/query` traffic sigiro's own CLI sends today (via
///   `SigiroClient::query`, e.g. `sigiro status`'s recent-ingest block):
///   `count`, `coalesce`, `nullif`, `now`, `quantile_cont`.
/// - The documented `/v1/query` examples (`www/content/docs/hosted-onboarding.md`,
///   `getting-started.md`, `guides/self-host-edge-federation.md`): `count`,
///   `approx_quantile`, `now`, `avg`, `sum`, `min`, `max`, `json_extract`.
/// - Sigiro's own internal analytics SQL (`src/diagnose/mod.rs`,
///   `src/schema.rs`) — never itself routed through this gate (it runs
///   directly against `ctx.conn`), but real DuckDB usage this system
///   already relies on and representative of what a legitimate ad-hoc
///   analytics query needs: `any_value`, `first`, `make_timestamp`,
///   `time_bucket`, `len`.
/// - Standard read-only observability/analytics scalars and aggregates
///   with no filesystem/network/settings access (string/date/math/
///   conditional helpers).
///
/// Deliberately EXCLUDED, even though real: `current_setting` — the obvious
/// way to read the `threads` setting, but the SAME function reads
/// `extension_directory`, `home_directory`, and every other DuckDB setting,
/// with no way to restrict it to one argument at the AST level. Sigiro's own
/// storage-headroom query was written to use `pragma_database_size()` only,
/// rather than reopening this hole for one number. `getenv` is the other half
/// of the vulnerability this allowlist closes and is never on this list either.
const SCALAR_FUNCTIONS: &[&str] = &[
    // Aggregates
    "COUNT",
    "SUM",
    "MIN",
    "MAX",
    "AVG",
    "APPROX_QUANTILE",
    "QUANTILE_CONT",
    "STDDEV",
    "MEDIAN",
    "ANY_VALUE",
    "FIRST",
    // JSON
    "JSON_EXTRACT",
    "JSON_EXTRACT_STRING",
    // Date/time
    "NOW",
    "CURRENT_TIMESTAMP",
    "DATE_TRUNC",
    "DATE_PART",
    "DATEPART",
    "EPOCH",
    "EPOCH_MS",
    "STRFTIME",
    "STRPTIME",
    "TO_TIMESTAMP",
    "MAKE_TIMESTAMP",
    "TIME_BUCKET",
    // String
    "LOWER",
    "UPPER",
    "SUBSTRING",
    "SUBSTR",
    "CONCAT",
    "CONCAT_WS",
    "LENGTH",
    "LEN",
    "TRIM",
    "LTRIM",
    "RTRIM",
    "REPLACE",
    "SPLIT_PART",
    "REGEXP_MATCHES",
    "REGEXP_EXTRACT",
    "REGEXP_REPLACE",
    "STARTS_WITH",
    "ENDS_WITH",
    "CONTAINS",
    "FORMAT",
    // Math
    "ABS",
    "ROUND",
    "FLOOR",
    "CEIL",
    "CEILING",
    "SQRT",
    "POW",
    "POWER",
    "EXP",
    "LN",
    "LOG",
    "GREATEST",
    "LEAST",
    "MOD",
    "SIGN",
    // Conditional
    "COALESCE",
    "NULLIF",
    "IFNULL",
    "NVL",
    // List / array.
    //
    // `sigiro_profiles.stack_trace` is a VARCHAR[] (src/schema.rs), so WITHOUT
    // these sigiro can ingest and store profiles it cannot meaningfully query:
    // no aggregation by frame, no "which function is hot", no flamegraph. That
    // is the whole point of the profiles signal, and it was unreachable through
    // the public API. Found by profiling sigiro with a second sigiro and then
    // being unable to group the result by stack frame.
    //
    // These read a value the caller can already SELECT in full. They touch no
    // filesystem, no network and no settings, so they add no surface the table
    // allowlist does not already bound. `LEN` was on this list already, for the
    // same array type.
    //
    // `LIST_FILTER`/`LIST_TRANSFORM` take a lambda. A lambda body is an
    // ordinary expression, so it is walked by the same visitor and every
    // function call inside it faces this identical allowlist — a lambda cannot
    // smuggle in `getenv` or `current_setting`.
    "LIST_FILTER",
    "LIST_TRANSFORM",
    "LIST_CONTAINS",
    "LIST_HAS_ANY",
    "LIST_POSITION",
    "LIST_EXTRACT",
    "LIST_ELEMENT",
    "LIST_SLICE",
    "LIST_SORT",
    "LIST_DISTINCT",
    "LIST_CONCAT",
    "LIST_AGGREGATE",
    "ARRAY_TO_STRING",
    "ARRAY_LENGTH",
    "UNNEST",
];

/// Schema qualifiers accepted ahead of an allowed table name. DuckLake attaches
/// its catalog `AS lake` (src/storage.rs); the same data reads as schema `main`
/// under a local-file catalog and `lake` under a PostgreSQL-backed one (see
/// tests/e2e/docker/run.sh's `PROFILE_SCHEMA` detection). `information_schema`
/// is DuckDB's static, file-system-free catalog metadata — inherently safe to
/// expose in full (used by the same e2e script to detect the schema above).
fn is_allowed_table(name: &ObjectName, allow_information_schema: bool) -> bool {
    let idents = &name.0;
    match idents.len() {
        1 => ALLOWED_TABLES.contains(&idents[0].value.to_uppercase().as_str()),
        2 => {
            let schema = idents[0].value.to_uppercase();
            let table = idents[1].value.to_uppercase();
            if schema == "INFORMATION_SCHEMA" {
                return allow_information_schema;
            }
            matches!(schema.as_str(), "LAKE" | "MAIN") && ALLOWED_TABLES.contains(&table.as_str())
        }
        // Fail closed: no legitimate sigiro query uses 3+-part relation names.
        _ => false,
    }
}

fn is_safe_function(name: &ObjectName) -> bool {
    name.0.last().is_some_and(|ident| {
        let upper = ident.value.to_uppercase();
        SAFE_FUNCTIONS.contains(&upper.as_str())
    })
}

/// A SCALAR function call (`Expr::Function`, appearing anywhere in the
/// expression tree — not a relation) is safe only if its (possibly
/// qualified) name is on `SCALAR_FUNCTIONS`, matched on the final
/// identifier so `main.count(*)` and `count(*)` are treated the same.
fn is_allowed_scalar_function(name: &ObjectName) -> bool {
    name.0
        .last()
        .is_some_and(|ident| SCALAR_FUNCTIONS.contains(&ident.value.to_uppercase().as_str()))
}

/// Reject the Postgres/DuckDB `TABLE <name>` query-shorthand (equivalent to
/// `SELECT * FROM <name>`). It is a distinct `SetExpr::Table` AST node whose
/// `table_name`/`schema_name` fields are plain `String`s, not an `ObjectName`
/// routed through sqlparser's relation/table-factor visitor hooks — so
/// without this explicit check it would bypass `AllowlistVisitor` entirely.
/// Not used anywhere in sigiro's own queries or tests, so fail closed.
fn reject_table_shorthand(expr: &SetExpr) -> ControlFlow<String> {
    match expr {
        SetExpr::SetOperation { left, right, .. } => match reject_table_shorthand(left) {
            ControlFlow::Continue(()) => reject_table_shorthand(right),
            brk => brk,
        },
        SetExpr::Table(_) => {
            ControlFlow::Break("query rejected: TABLE shorthand not allowed".to_string())
        }
        _ => ControlFlow::Continue(()),
    }
}

/// Walks the full AST (via sqlparser's `visitor` feature — CTEs, set
/// operations, every FROM/JOIN, derived tables, and subqueries nested in
/// expressions/WHERE/scalar positions are all covered by the crate's own
/// recursive Visit derive, not hand-enumerated here) and rejects any
/// relation that is not an allowed sigiro table or safe function.
///
/// This closes the replacement-scan hole: `SELECT * FROM 's3://...'` or
/// `FROM '/etc/passwd'` parses as `TableFactor::Table` with a
/// single-element, unquoted-string `ObjectName` — it is not on
/// `ALLOWED_TABLES`, so `is_allowed_table` rejects it regardless of what
/// DuckDB's replacement-scan would have done with it.
/// CTE-name allowlisting used to be handled here via a per-`Query` scope
/// stack (push CTE aliases in `pre_visit_query`, pop in `post_visit_query`,
/// treat a bare name matching an in-scope alias as safe). That design was
/// abandoned after two subtle bypasses: a cross-scope leak from sqlparser
/// visiting a `Select`'s `projection` before its `from`, and a
/// forward-reference where a later CTE named `evil` shadows nothing and
/// `FROM evil` inside an earlier CTE resolves to the *physical* table
/// (`WITH a AS (SELECT * FROM evil), evil AS (SELECT 1) SELECT * FROM a`).
/// CTE scoping is too easy to get subtly wrong for a security boundary, so
/// `WITH` is rejected outright in `pre_visit_query` below instead of being
/// analyzed. Subqueries (`SELECT * FROM (SELECT ... ) t`) cover the same
/// legitimate use cases and remain fully supported.
struct AllowlistVisitor {
    /// When false (hosted mode), `information_schema` enumeration is rejected —
    /// see `validate_read_only_with`.
    allow_information_schema: bool,
}

impl Visitor for AllowlistVisitor {
    type Break = String;

    fn pre_visit_query(&mut self, query: &SqlQuery) -> ControlFlow<String> {
        if query.with.is_some() {
            return ControlFlow::Break(
                "query rejected: WITH/CTE clauses are not allowed".to_string(),
            );
        }
        reject_table_shorthand(&query.body)
    }

    fn pre_visit_table_factor(&mut self, table_factor: &TableFactor) -> ControlFlow<String> {
        match table_factor {
            TableFactor::Table { name, args, .. } => {
                if let Some(args) = args {
                    if !is_safe_function(name) {
                        return ControlFlow::Break(format!("query rejected: {name} not allowed"));
                    }
                    let _ = args;
                    return ControlFlow::Continue(());
                }
                if is_allowed_table(name, self.allow_information_schema) {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(format!("query rejected: {name} not allowed"))
                }
            }
            // Subqueries / parenthesized joins: no relation of their own,
            // recurse into their children (handled automatically by the
            // derived Visit impl once we return Continue here).
            TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => {
                ControlFlow::Continue(())
            }
            TableFactor::Function { name, args, .. } => {
                if !is_safe_function(name) {
                    return ControlFlow::Break(format!(
                        "query rejected: function {name} not allowed"
                    ));
                }
                let _ = args;
                ControlFlow::Continue(())
            }
            TableFactor::TableFunction { expr, .. } => match expr {
                Expr::Function(f) if is_safe_function(&f.name) => ControlFlow::Continue(()),
                _ => ControlFlow::Break("query rejected: table function not allowed".to_string()),
            },
            // UNNEST, PIVOT, UNPIVOT, JSON_TABLE, OPENJSON, MATCH_RECOGNIZE, etc.
            // — fail closed, none of sigiro's own queries or tests use them.
            other => ControlFlow::Break(format!(
                "query rejected: relation kind not allowed: {other}"
            )),
        }
    }

    /// Every scalar function call anywhere in the expression tree — the
    /// derived `Visit` for `Expr` fires this for the projection list, WHERE,
    /// HAVING, GROUP BY, ORDER BY, CASE branches, aggregate/function
    /// arguments, and subqueries alike (any `Expr` field recurses through
    /// the same `pre_visit_expr` hook; see `sqlparser::ast::visitor`'s own
    /// doc example, where a scalar subquery's inner expressions are visited
    /// too). This is what closes the `getenv`/`current_setting` scalar hole
    /// `pre_visit_table_factor` never covered — see `SCALAR_FUNCTIONS`.
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<String> {
        if let Expr::Function(f) = expr
            && !is_allowed_scalar_function(&f.name)
        {
            return ControlFlow::Break(format!("query rejected: function {} not allowed", f.name));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_statement(&mut self, _statement: &Statement) -> ControlFlow<String> {
        // The only Statement this visitor ever walks into is the top-level
        // Statement::Query we matched on before calling `.visit()` — any
        // OTHER Statement node reachable here means one snuck in as a nested
        // AST node (e.g. a data-modifying CTE arm parsed as
        // `SetExpr::Insert(Statement)`/`SetExpr::Update(Statement)`). Reject
        // it; a read-only query tree never legitimately contains one.
        ControlFlow::Break("query rejected: nested statement not allowed".to_string())
    }
}

/// Permissive validation (local / self-host / quack edge): `information_schema`
/// is allowed because the catalog holds only this node's own data. This is the
/// wrapper the quack edge gate and all firewall unit tests use.
pub fn validate_read_only(sql: &str) -> Result<(), String> {
    validate_read_only_with(sql, true)
}

/// `allow_information_schema = false` (HOSTED mode) additionally rejects
/// `information_schema` enumeration. With a shared PostgreSQL catalog, the
/// DuckLake attach exposes the WHOLE database's `information_schema` to a
/// tenant connection, letting it read the NAMES of the control plane
/// (`sigiro_control.*`) and sibling tenant schemas. Data reads are already
/// blocked by the table allowlist; this closes the structure disclosure.
/// The `/v1/query` handler passes `!hosted`; local/edge callers pass `true`.
pub fn validate_read_only_with(sql: &str, allow_information_schema: bool) -> Result<(), String> {
    // Validate the syntax users actually execute. GenericDialect rejects
    // DuckDB's standard aggregate `FILTER (WHERE ...)` clause before the
    // allowlist visitor can inspect it.
    let dialect = DuckDbDialect {};

    // Every AT (...) clause body must be a literal BEFORE anything else runs:
    // DuckDB executes the clause verbatim, so a non-literal body (function
    // call, subquery, column reference, ...) would run unvalidated by every
    // guard below. See `TIME_TRAVEL_AT_LITERAL`'s doc comment.
    for caps in TIME_TRAVEL_AT.captures_iter(sql) {
        let body = &caps[1];
        if !TIME_TRAVEL_AT_LITERAL.is_match(body) {
            return Err(format!(
                "query rejected: AT (...) time-travel clause must be a literal \
                 `VERSION => <integer>` or `TIMESTAMP => '<string>'`, found: AT ({body})"
            ));
        }
    }

    let parseable = TIME_TRAVEL_AT.replace_all(sql, " ");
    let statements = Parser::parse_sql(&dialect, &parseable)
        .map_err(|e| format!("query rejected: parse error: {e}"))?;

    // A firewall admits exactly one statement: zero (empty/whitespace-only
    // SQL) is not a query to run, and DuckDB's own semicolon-separated batch
    // form would let a second, unvalidated-by-this-loop statement ride along
    // — `execute_sql`/`conn.prepare()` only ever runs one statement anyway
    // (see this module's doc comment), so this matches the real contract.
    let [stmt] = statements.as_slice() else {
        return Err(format!(
            "query rejected: exactly one SQL statement is required, found {}",
            statements.len()
        ));
    };

    let Statement::Query(query) = stmt else {
        return Err("query rejected: only SELECT queries are allowed".to_string());
    };
    if let ControlFlow::Break(msg) = query.visit(&mut AllowlistVisitor {
        allow_information_schema,
    }) {
        return Err(msg);
    }

    let tokens = Tokenizer::new(&dialect, sql)
        .tokenize()
        .map_err(|e| format!("query rejected: tokenize error: {e}"))?;
    for token in &tokens {
        if let Token::Word(w) = token {
            let word = w.value.to_uppercase();
            if BLOCKED_TABLE_FUNCTIONS.contains(&word.as_str()) {
                return Err(format!("query rejected: {word} not allowed"));
            }
        }
    }
    Ok(())
}
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

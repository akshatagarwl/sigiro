use anyhow::{Context, Result};
use axum::{Extension, Json, Router, extract::State, http::HeaderMap, middleware as axum_mw};
use axum_anyhow::{ApiResult, IntoApiError, ResultExt, bad_request};
use axum_extra::routing::RouterExt;
use axum_server::{Handle, tls_rustls::RustlsConfig};
use parking_lot::Mutex;
use rusqlite::Connection;
use sqlparser::ast::{
    Expr, ObjectName, Query as SqlQuery, SetExpr, Statement, TableFactor, Visit, Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};
use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tonic::transport::{Identity, Server as TonicServer, ServerTlsConfig};
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::KeyExtractor};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::admin;
use crate::auth::{self, AuthContext};
use crate::ingest::OtlpGrpcService;
use crate::inserter::{DiskCapacityState, MsgRouter};
use crate::investigate;
use crate::pool::{ProjectPool, ReaderPool};
use crate::responses::*;
use crate::types::*;
use tracing_attributes::instrument;

#[derive(OpenApi)]
#[openapi(
    paths(
        query_handler,
        services_handler,
        trace_detail_handler,
        investigate_handler,
        admin_status,
        create_tenant_handler,
        create_key_handler,
        revoke_key_handler,
            ),
    components(
        schemas(
            ServicesResponse,
            ServiceEntry,
            TraceDetailResponse,
            SpanDetail,
            ServiceBreakdownEntry,
            CreateKeyResponse,
            RevokeKeyResponse,
            CreateTenantRequest,
            CreateTenantResponse,
            CreateKeyRequest,
            AdminStatusResponse,
        )
    ),
    tags(
        (name = "query", description = "SQL query endpoints"),
        (name = "admin", description = "Admin endpoints"),
            )
)]
pub struct ApiDoc;

/// Pre-auth rate-limit key. This must not key on the unverified Authorization
/// header: an attacker could rotate bogus tokens to evade the limiter and grow
/// its key map without bound. ConnectInfo is populated by `serve_http_app`.
#[derive(Clone)]
pub(crate) struct SourceRateLimitKeyExtractor;

impl KeyExtractor for SourceRateLimitKeyExtractor {
    type Key = String;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        let ip = req
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip());
        Ok(match ip {
            Some(ip) => format!("ip:{ip}"),
            None => "ip:unknown".to_string(),
        })
    }
}

/// Post-auth rate-limit key. Auth middleware inserts this trusted tenant
/// context before the limiter runs, so one tenant cannot consume another's
/// query allowance and raw API keys never live in the governor key map.
#[derive(Clone)]
struct TenantRateLimitKeyExtractor;

impl KeyExtractor for TenantRateLimitKeyExtractor {
    type Key = String;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        req.extensions()
            .get::<AuthContext>()
            .map(|ctx| format!("tenant:{}", ctx.tenant_id))
            .ok_or(tower_governor::GovernorError::UnableToExtractKey)
    }
}

/// Render a `GovernorError` as a JSON body matching the shape of every other
/// API error (`{status, title, detail}`) so a rate-limited client cannot
/// mistake the plain-text default ("Too Many Requests!") for query output.
pub(crate) fn rate_limit_error_response(
    err: tower_governor::GovernorError,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use tower_governor::GovernorError;

    let (status, detail, extra_headers) = match err {
        GovernorError::TooManyRequests { wait_time, headers } => (
            StatusCode::TOO_MANY_REQUESTS,
            format!("rate limit exceeded — retry after {wait_time}s"),
            headers,
        ),
        GovernorError::UnableToExtractKey => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "unable to extract rate-limit key".to_string(),
            None,
        ),
        GovernorError::Other { code, msg, headers } => (
            code,
            msg.unwrap_or_else(|| "rate limit error".to_string()),
            headers,
        ),
    };
    let body = serde_json::json!({
        "status": status.as_u16(),
        "title": "rate limited",
        "detail": detail,
    });
    let mut response = (status, Json(body)).into_response();
    if let Some(headers) = extra_headers {
        response.headers_mut().extend(headers);
    }
    response
}

pub struct AppState {
    pub pool: Arc<ProjectPool>,
    /// Admin SQLite for instance status (and, historically, worker registry).
    pub admin_conn: Arc<Mutex<Connection>>,
    pub http_client: reqwest::Client,
    pub router: Arc<crate::inserter::MsgRouter>,
    /// Explicit auth mode — Open or Key. Never implicit.
    pub auth_mode: crate::auth::AuthMode,
    /// Resolved data directory.
    /// --- DI pattern: injected at construction; never read from env at request time. ---
    /// The previous design re-derived data_dir from the SIGIRO_DATA_DIR env var
    /// on every health check, which raced with parallel tests (each test
    /// called `std::env::set_var` to set up its own data dir, and the last
    /// writer won for all concurrent tests). Carrying data_dir through
    /// AppState makes the value request-scoped by construction — there is
    /// no shared mutable state to race on.
    pub data_dir: camino::Utf8PathBuf,
    /// True after `run_serve` finishes init (DuckLake attached, schema
    /// created). Replaces the previous `.ready` sentinel file — typed
    /// in-memory state is checked at request time instead of a file
    /// existence check, which is faster and impossible to forget to set.
    /// --- DI pattern: production code calls `mark_init_complete()`; tests call `.store(true)` directly. ---
    pub init_complete: Arc<AtomicBool>,
    /// Which cross-project query surface (if any) this node exposes.
    /// Currently inert — set from `SIGIRO_QUERY_SURFACE` at startup, read by
    /// a future feature (see `crate::config::QuerySurface`).
    pub query_surface: crate::config::QuerySurface,
}

impl AppState {
    /// Mark init as complete. Called by `run_serve` at the tail of init.
    /// Idempotent — safe to call more than once.
    pub fn mark_init_complete(&self) {
        self.init_complete.store(true, Ordering::Release);
    }
}

// DuckDB's access_mode is database-level, not connection-level:
// - `SET access_mode = 'read_only'` fails with "Cannot change access_mode setting while database is running"
// - `Connection::open_with_flags(path, ReadOnly)` creates a separate database instance that
//   doesn't share in-memory state with the write connection (ingested data invisible)
//   Therefore we use AST-based SQL validation (sqlparser) + `prepare()` single-statement guarantee.
//   `conn.prepare()` only handles one statement — multi-statement injection via semicolons
//   is already prevented by DuckDB itself.

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
/// `AT (TIMESTAMP => '2026-07-09 19:55:00')`. sqlparser's GenericDialect can't
/// parse the `=>` form, so it is stripped before the SELECT-only structural
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
/// sigiro itself creates (src/schema.rs CREATE TABLE names) plus the
/// read-only `sigiro_anomalies` view it also creates there. Matched
/// case-insensitively against the FINAL identifier — see `is_allowed_table`
/// for how the schema-qualifier prefix (`lake.` / `main.`) is handled.
const ALLOWED_TABLES: &[&str] = &[
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

/// Table-valued / pragma functions sigiro's OWN code calls through this same
/// `/v1/query` guard — allowlist EXACTLY these, sourced from real call sites:
/// - `PRAGMA_DATABASE_SIZE` — src/diagnose.rs `check_duckdb_health` (DuckDB
///   memory-limit/usage self-check via `FROM pragma_database_size()`).
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
/// - Real `/v1/query` traffic sigiro's own CLI sends today
///   (`src/diagnose.rs`, via `SigiroClient::query`): `count`, `coalesce`,
///   `nullif`, `now`, `quantile_cont` — these MUST keep working or
///   `sigiro diagnose` breaks against its own server.
/// - The documented `/v1/query` examples (`www/content/docs/hosted-onboarding.md`,
///   `getting-started.md`, `guides/self-host-edge-federation.md`): `count`,
///   `approx_quantile`, `now`, `avg`, `sum`, `min`, `max`, `json_extract`.
/// - Sigiro's own internal analytics SQL (`src/investigate/mod.rs`,
///   `src/schema.rs`) — never itself routed through this gate (it runs
///   directly against `ctx.conn`), but real DuckDB usage this system
///   already relies on and representative of what a legitimate ad-hoc
///   analytics query needs: `any_value`, `first`, `make_timestamp`,
///   `time_bucket`, `len`.
/// - Standard read-only observability/analytics scalars and aggregates
///   with no filesystem/network/settings access (string/date/math/
///   conditional helpers).
///
/// Deliberately EXCLUDED, even though real: `current_setting` — used
/// internally by `src/diagnose.rs` `check_duckdb_health` to read the
/// `threads` setting, but the same function reads `extension_directory`,
/// `home_directory`, and every other DuckDB setting, with no way to
/// restrict it to one argument at the AST level. `check_duckdb_health` was
/// changed to stop calling it (see its doc comment) rather than reopening
/// this hole. `getenv` is the other half of the vulnerability this
/// allowlist closes and is never on this list either.
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

/// A table-valued-function / pivot-style relation is safe only if its
/// (possibly qualified) name is on `SAFE_FUNCTIONS` — matched on the final
/// identifier so `main.pragma_database_size()` and `pragma_database_size()`
/// are treated the same.
fn is_safe_function(name: &ObjectName) -> bool {
    name.0
        .last()
        .is_some_and(|ident| SAFE_FUNCTIONS.contains(&ident.value.to_uppercase().as_str()))
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
                let allowed = if args.is_some() {
                    is_safe_function(name)
                } else {
                    is_allowed_table(name, self.allow_information_schema)
                };
                if allowed {
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
            TableFactor::Function { name, .. } => {
                if is_safe_function(name) {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(format!("query rejected: function {name} not allowed"))
                }
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
        if let Expr::Function(f) = expr {
            if !is_allowed_scalar_function(&f.name) {
                return ControlFlow::Break(format!(
                    "query rejected: function {} not allowed",
                    f.name
                ));
            }
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
    let dialect = GenericDialect {};

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

/// Build the HTTP Router for the main API (port 9999).
/// Extracted from `run_servers` so integration tests can build the same
/// router with test state without starting TCP listeners.
pub fn build_http_router(app_state: Arc<AppState>) -> Router {
    let rate_limit_v1 = app_state.auth_mode.requires_auth();

    // v1 API routes. In hosted mode the source limiter runs before auth to
    // bound bogus-token work, then auth injects a trusted AuthContext, then the
    // per-tenant limiter runs before the handler.
    let v1 = Router::new()
        .typed_post(query_handler)
        .typed_post(investigate_handler)
        .typed_get(services_handler)
        .typed_get(trace_detail_handler);

    let v1 = if rate_limit_v1 {
        let tenant_governor = GovernorConfigBuilder::default()
            .key_extractor(TenantRateLimitKeyExtractor)
            .per_millisecond(200)
            .burst_size(20)
            .finish()
            .expect("valid per-tenant governor config");
        let source_governor = GovernorConfigBuilder::default()
            .key_extractor(SourceRateLimitKeyExtractor)
            .per_millisecond(20)
            .burst_size(100)
            .finish()
            .expect("valid pre-auth source governor config");
        v1.layer(GovernorLayer::new(tenant_governor).error_handler(rate_limit_error_response))
            .layer(axum_mw::from_fn_with_state(
                app_state.clone(),
                auth::auth_middleware,
            ))
            .layer(GovernorLayer::new(source_governor).error_handler(rate_limit_error_response))
    } else {
        v1.layer(axum_mw::from_fn_with_state(
            app_state.clone(),
            auth::auth_middleware,
        ))
    };

    // All /admin routes, including status, are admin-token gated by their
    // handlers (see require_admin_token).
    let admin = Router::new()
        .typed_get(admin_status)
        .typed_post(create_tenant_handler)
        .typed_post(create_key_handler)
        .typed_delete(revoke_key_handler);

    // Public routes + v1 API + admin + Swagger UI
    Router::new()
        .typed_get(health)
        .typed_get(v1_health)
        .typed_get(healthz)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .nest("/v1", v1)
        .nest("/admin", admin)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(app_state)
}

/// Server port configuration — all configurable via environment variables.
/// Uses typed-env for fail-fast parsing: malformed values error at startup.
#[derive(Clone)]
pub struct ServerPorts {
    pub http: u16,
    pub grpc: u16,
    pub otlp_http: u16,
}

impl ServerPorts {
    /// Load ports from environment. Fails fast on malformed values.
    /// Unset vars default to well-known OTLP ports.
    pub fn load() -> anyhow::Result<Self> {
        use crate::config;
        Ok(Self {
            http: config::HTTP_PORT.value().map_err(anyhow::Error::from)?,
            grpc: config::GRPC_PORT.value().map_err(anyhow::Error::from)?,
            otlp_http: config::OTLP_HTTP_PORT
                .value()
                .map_err(anyhow::Error::from)?,
        })
    }
}

/// Reject a public/wildcard bind for the quack edge's mandatory tailnet
/// perimeter. In edge mode (`query_surface == Quack`), auth is Open and every
/// listener — quack itself plus, per `run_servers`, HTTP/OTLP-HTTP/gRPC too —
/// binds this SAME address, so it has to actually be a private/loopback
/// interface: quack speaks plain HTTP with no working TLS of its own, and an
/// Open-auth surface on a wildcard bind is reachable from anywhere. Parse the
/// address and reject any `is_unspecified()` (all-interfaces) form rather than
/// string-matching the two obvious spellings — the wildcard has many equivalent
/// textual forms (`0.0.0.0`, `::`, `[::]`, `0::0`, `0:0:0:0:0:0:0:0`,
/// `::ffff:0.0.0.0`) that all bind every interface, and matching only the
/// literals would leave exactly the same hole open under a different spelling.
pub fn validate_quack_bind_address(bind: &str) -> Result<()> {
    let normalized = bind
        .strip_prefix('[')
        .and_then(|b| b.strip_suffix(']'))
        .unwrap_or(bind);
    let ip: std::net::IpAddr = normalized
        .parse()
        .with_context(|| format!("SIGIRO_QUACK_BIND {bind:?} is not a valid IP address"))?;
    let wildcard = match ip {
        std::net::IpAddr::V4(v4) => v4.is_unspecified(),
        std::net::IpAddr::V6(v6) => {
            v6.is_unspecified() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_unspecified())
        }
    };
    anyhow::ensure!(
        !wildcard,
        "SIGIRO_QUACK_BIND must not be a wildcard address ({bind:?}) — quack has no working \
         TLS of its own; bind an explicit loopback/private interface and rely on a network \
         perimeter (e.g. Tailscale) for anything wider"
    );
    Ok(())
}

/// Resolve the bind IPs `run_servers` uses for its HTTP/OTLP-HTTP listener and
/// its gRPC listener, given the already-validated edge bind (`Some`, a quack
/// edge — see `validate_quack_bind_address`) or `None` (hosted mode). Pure
/// and unit-testable independent of `run_servers`'s side effects (real
/// listeners, DB pools, ...).
///
/// Hosted mode's pair is BYTE-IDENTICAL to before this fix: `0.0.0.0` for
/// HTTP/OTLP (kamal-proxy sits on the docker network — a loopback bind would
/// break proxy access) and `::` for gRPC (that listener's pre-existing,
/// unrelated default). Edge mode returns the SAME address for both, so
/// HTTP/OTLP-HTTP/gRPC all end up bound exactly where quack itself is.
pub fn resolve_listener_bind_ips(
    edge_bind: Option<&str>,
) -> Result<(std::net::IpAddr, std::net::IpAddr)> {
    match edge_bind {
        Some(bind) => {
            let normalized = bind
                .strip_prefix('[')
                .and_then(|b| b.strip_suffix(']'))
                .unwrap_or(bind);
            let ip: std::net::IpAddr = normalized
                .parse()
                .with_context(|| format!("SIGIRO_QUACK_BIND {bind:?} is not a valid IP address"))?;
            Ok((ip, ip))
        }
        None => Ok((
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        )),
    }
}

pub async fn run_servers(
    pool: Arc<ProjectPool>,
    admin_conn: Arc<Mutex<Connection>>,
    router: Arc<MsgRouter>,
    data_dir: camino::Utf8PathBuf,
    init_complete: Arc<AtomicBool>,
) -> Result<()> {
    let ports = ServerPorts::load()?;
    let max_ingest_body_bytes = crate::config::max_ingest_body_bytes()?;

    let query_surface = crate::config::query_surface()?;
    let auth_mode =
        crate::auth::AuthMode::from_runtime_config(&data_dir, query_surface, pool.storage_mode())?;

    // The bind address enforced for EVERY listener (HTTP, OTLP-HTTP, gRPC, and
    // quack itself) when this node is a quack edge — resolved once here so
    // the gRPC spawn below (which happens before the quack section further
    // down) already has it. `None` in hosted mode: every listener below then
    // falls back to today's wildcard bind, unchanged. See
    // `validate_quack_bind_address` for why a wildcard is rejected outright.
    let edge_bind = if query_surface == crate::config::QuerySurface::Quack {
        let bind = crate::config::QUACK_BIND
            .value()
            .map_err(anyhow::Error::from)?;
        validate_quack_bind_address(&bind)?;
        Some(bind)
    } else {
        None
    };
    let (http_bind_ip, grpc_bind_ip) = resolve_listener_bind_ips(edge_bind.as_deref())?;

    // TLS configuration
    let (otlp_http_tls_config, grpc_tls_config) = match (
        crate::config::TLS_CERT.value(),
        crate::config::TLS_KEY.value(),
    ) {
        (Ok(cert), Ok(key)) => {
            tracing::info!("direct-ingest TLS enabled — cert={cert}, key={key}");
            ensure_rustls_provider()?;
            let http_tls_config = RustlsConfig::from_pem_file(&cert, &key)
                .await
                .context("failed to load HTTP TLS config")?;
            let cert_pem = tokio::fs::read(&cert)
                .await
                .with_context(|| format!("failed to read TLS certificate {cert}"))?;
            let key_pem = tokio::fs::read(&key)
                .await
                .with_context(|| format!("failed to read TLS key {key}"))?;
            let grpc_tls_config = ServerTlsConfig::new()
                .identity(Identity::from_pem(cert_pem, key_pem))
                .timeout(Duration::from_secs(5));
            (Some(http_tls_config), Some(grpc_tls_config))
        }
        (Ok(_), Err(_)) => {
            anyhow::bail!("SIGIRO_TLS_CERT is set but SIGIRO_TLS_KEY is missing");
        }
        (Err(_), Ok(_)) => {
            anyhow::bail!("SIGIRO_TLS_KEY is set but SIGIRO_TLS_CERT is missing");
        }
        (Err(_), Err(_)) => {
            tracing::info!("TLS not configured — serving plain TCP");
            (None, None)
        }
    };

    // gRPC server — authenticated via Bearer token in metadata
    let grpc_server = TonicServer::builder()
        .concurrency_limit_per_connection(16)
        .load_shed(true)
        .timeout(Duration::from_secs(15));
    let mut grpc_server = match grpc_tls_config {
        Some(tls_config) => grpc_server
            .tls_config(tls_config)
            .context("failed to configure gRPC TLS")?,
        None => grpc_server,
    };
    let grpc_router = Arc::clone(&router);
    let grpc_auth_mode = auth_mode.clone();
    let grpc_port = ports.grpc;
    let mut servers = JoinSet::new();
    let (grpc_shutdown_tx, grpc_shutdown_rx) = oneshot::channel();
    servers.spawn(async move {
        let addr = SocketAddr::new(grpc_bind_ip, grpc_port);
        let svc = OtlpGrpcService::new(grpc_router, grpc_auth_mode, max_ingest_body_bytes);
        tracing::info!("gRPC listening on {addr}");
        let result = grpc_server
            .add_service(svc.trace_service())
            .add_service(svc.logs_service())
            .add_service(svc.metrics_service())
            .add_service(svc.profiles_service())
            .serve_with_shutdown(addr, async move {
                if grpc_shutdown_rx.await.is_err() {
                    tracing::debug!("gRPC shutdown sender dropped");
                }
            })
            .await
            .context("gRPC server failed");
        ("gRPC", result)
    });

    // Clone before `data_dir` moves into AppState — the storage-maintenance
    // task samples this same directory for the disk cap.
    let maintenance_data_dir = data_dir.clone();

    let app_state = Arc::new(AppState {
        pool: Arc::clone(&pool),
        admin_conn: Arc::clone(&admin_conn),
        http_client: reqwest::Client::new(),
        router: Arc::clone(&router),
        auth_mode: auth_mode.clone(),
        data_dir,
        init_complete: Arc::clone(&init_complete),
        query_surface,
    });

    // Edge query-serving surface: served in-process, on a connection cloned
    // from the SAME DuckDB instance ingest writes to (a DuckDB-file catalog
    // is one-writer-XOR-many-readers, so a separate process cannot share the
    // lock — see crate::quack module docs). `query_surface == None` (the
    // default) changes nothing: no extension load, no server, no new port.
    //
    // Serving over an S3-backed distributed "default" project (SIGIRO_CATALOG_URL
    // set + SIGIRO_S3_BUCKET set) is supported here with NO extra guard: the
    // filesystem lockdown (`pool::lock_filesystem`) is what's skipped in that
    // mode, not the security boundary — `validate_read_only` (SELECT-only +
    // sigiro_* table allowlist + reader-function denylist + positive
    // scalar-function allowlist + WITH-ban) IS the boundary, in every storage
    // mode, and does not depend on the filesystem lockdown at all (see
    // `crate::quack::install_authorization_gate`, which wraps this same
    // validator). `enable_external_access` staying on in S3 mode is required
    // for DuckLake itself to reach S3, not a hole left open for user queries.
    let quack_shutdown_tx = if let Some(bind) = edge_bind.clone() {
        let port = crate::config::QUACK_PORT
            .value()
            .map_err(anyhow::Error::from)?;
        let token = crate::config::QUACK_TOKEN
            .value()
            .map_err(anyhow::Error::from)?;
        let token = if token.is_empty() { None } else { Some(token) };

        let quack_conn = pool.clone_connection("default").context(
            "failed to open the quack query-surface connection — run_serve should have \
             pre-created the single-edge \"default\" project before run_servers starts \
             (see main.rs run_serve's quack pre-create, which fails loud on its own \
             create_project error rather than deferring the failure to here)",
        )?;
        crate::quack::load_and_pin(&quack_conn)?;
        crate::quack::install_authorization_gate(&quack_conn)?;
        let listen = crate::quack::start_server(&quack_conn, &bind, port, token.as_deref())?;
        tracing::info!(
            bind,
            port,
            listen_uri = %listen.listen_uri,
            "quack query surface serving (in-process, read-only)"
        );

        let (tx, rx) = oneshot::channel::<()>();
        servers.spawn_blocking(move || {
            if rx.blocking_recv().is_err() {
                tracing::debug!("quack shutdown sender dropped");
            }
            let result = crate::quack::stop_server(&quack_conn, &bind, port)
                .context("failed to stop the quack query surface cleanly");
            if let Err(ref error) = result {
                tracing::warn!("{error:#}");
            }
            drop(quack_conn);
            ("quack", result)
        });
        Some(tx)
    } else {
        None
    };

    // OTLP HTTP ingest sub-router (POST /v1/traces|logs|metrics|profiles + the
    // pyroscope paths) — a composable Router with its own auth/DefaultBodyLimit/
    // state. RequestDecompressionLayer is applied to THIS sub-router only, not
    // the whole :9999 app, matching what the dedicated :4318 listener already
    // does. Merged into the :9999 app below so there is one public port; the
    // :4318/:4317 listeners keep running too (unexposed), reusing the same
    // router (axum's Router is cheap to clone — Arc-backed).
    let otlp_ingest_router =
        crate::ingest::otlp_http_router(Arc::clone(&router), auth_mode, max_ingest_body_bytes)
            .layer(tower_http::decompression::RequestDecompressionLayer::new());

    let http_app = build_http_router(app_state).merge(otlp_ingest_router.clone());

    let http_port = ports.http;
    let http_handle = Handle::new();
    let http_server_handle = http_handle.clone();
    servers.spawn(async move {
        let result = serve_http_app(
            "HTTP",
            http_bind_ip,
            http_port,
            http_app,
            // The main HTTP API stays plaintext inside the deployment network;
            // kamal-proxy terminates public TLS before forwarding to this port.
            None,
            http_server_handle,
        )
        .await;
        ("HTTP", result)
    });

    // OTLP HTTP — authenticated via Authorization header
    let otlp_http_app = otlp_ingest_router;
    let otlp_http_port = ports.otlp_http;
    let otlp_tls_config = otlp_http_tls_config;
    let otlp_http_handle = Handle::new();
    let otlp_http_server_handle = otlp_http_handle.clone();
    servers.spawn(async move {
        let result = serve_http_app(
            "OTLP HTTP",
            http_bind_ip,
            otlp_http_port,
            otlp_http_app,
            otlp_tls_config,
            otlp_http_server_handle,
        )
        .await;
        ("OTLP HTTP", result)
    });

    // Graceful shutdown: wait for SIGTERM or SIGINT, then drain.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

    // Storage-maintenance background task: routine retention cleanup plus the
    // disk-byte cap sampler/gate (runs if either is configured). The disk gate
    // shares the router's `DiskCapacityState` so `send` rejects when tripped.
    let retention_handle = spawn_storage_maintenance_task(
        Arc::clone(&router),
        maintenance_data_dir,
        router.disk_state(),
    );

    let server_error = tokio::select! {
        result = servers.join_next() => {
            let error = match result {
                Some(Ok((name, Ok(())))) => anyhow::anyhow!("{name} server exited unexpectedly"),
                Some(Ok((name, Err(error)))) => error.context(format!("{name} server exited")),
                Some(Err(error)) => anyhow::Error::new(error).context("server task panicked"),
                None => anyhow::anyhow!("server task set became empty unexpectedly"),
            };
            tracing::error!("{error:#}");
            Some(error)
        },
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM received — starting graceful shutdown");
            None
        },
        _ = sigint.recv() => {
            tracing::info!("SIGINT received — starting graceful shutdown");
            None
        },
    };

    http_handle.graceful_shutdown(None);
    otlp_http_handle.graceful_shutdown(None);
    let shutdown_send = grpc_shutdown_tx
        .send(())
        .map_err(|_| anyhow::anyhow!("gRPC shutdown receiver dropped unexpectedly"));
    let quack_shutdown_send = quack_shutdown_tx.map(|tx| {
        tx.send(())
            .map_err(|_| anyhow::anyhow!("quack shutdown receiver dropped unexpectedly"))
    });
    let server_cleanup = match tokio::time::timeout(Duration::from_secs(10), async {
        let mut first_error = None;
        while let Some(result) = servers.join_next().await {
            match result {
                Ok((_name, Ok(()))) => {}
                Ok((name, Err(error))) => {
                    tracing::error!("{name} server failed while stopping: {error:#}");
                    first_error.get_or_insert_with(|| {
                        error.context(format!("{name} server failed while stopping"))
                    });
                }
                Err(error) => {
                    tracing::error!("server task failed while stopping: {error}");
                    first_error.get_or_insert_with(|| {
                        anyhow::Error::new(error).context("server task failed while stopping")
                    });
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            servers.abort_all();
            while servers.join_next().await.is_some() {}
            Err(anyhow::anyhow!(
                "server connections did not drain within 10 seconds"
            ))
        }
    };

    let retention_cleanup = if let Some(handle) = retention_handle {
        handle.abort();
        match handle.await {
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(anyhow::Error::new(error).context("retention task failed to stop")),
            Ok(()) => Ok(()),
        }
    } else {
        Ok(())
    };

    // Network tasks are gone, so no request can create a new flusher while the
    // existing channels are closed and drained.
    tracing::info!("draining flusher tasks...");
    let router_cleanup = router
        .shutdown_and_wait()
        .await
        .context("failed to drain flusher tasks");

    if let Some(error) = server_error {
        return Err(error);
    }
    shutdown_send?;
    if let Some(result) = quack_shutdown_send {
        result?;
    }
    server_cleanup?;
    retention_cleanup?;
    router_cleanup?;

    Ok(())
}

fn ensure_rustls_provider() -> Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return Ok(());
    }

    match rustls::crypto::aws_lc_rs::default_provider().install_default() {
        Ok(()) => Ok(()),
        Err(_) if rustls::crypto::CryptoProvider::get_default().is_some() => Ok(()),
        Err(_) => anyhow::bail!("failed to install Rustls AWS-LC crypto provider"),
    }
}

/// `bind_ip`: `run_servers` passes the resolved edge/hosted bind — a quack
/// edge binds its own private address (`SIGIRO_QUACK_BIND`), hosted mode
/// keeps the unchanged `0.0.0.0` (it sits behind kamal-proxy on the docker
/// network, where a loopback bind would break proxy access).
async fn serve_http_app(
    name: &'static str,
    bind_ip: std::net::IpAddr,
    port: u16,
    app: Router,
    tls_config: Option<RustlsConfig>,
    handle: Handle<SocketAddr>,
) -> Result<()> {
    let addr = SocketAddr::new(bind_ip, port);
    if let Some(config) = tls_config {
        tracing::info!(server = name, protocol = "HTTPS", "listening on {addr}");
        axum_server::bind_rustls(addr, config)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .with_context(|| format!("{name} HTTPS server failed"))
    } else {
        tracing::info!(server = name, protocol = "HTTP", "listening on {addr}");
        axum_server::bind(addr)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .with_context(|| format!("{name} HTTP server failed"))
    }
}

/// Total bytes of regular files under `path` (recursive), plus whether the walk
/// was COMPLETE. Symlinks are not followed and are not counted (only regular
/// files). The bool is `false` if any entry/stat failed — the total is then an
/// UNDER-count, and the caller must not use it to CLEAR the cap (an
/// under-count must never re-open ingest while real usage may still be over);
/// an under-count that still exceeds the cap is safe to trip on.
fn dir_size_bytes(path: &std::path::Path) -> (u64, bool) {
    let mut total: u64 = 0;
    let mut reliable = true;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        match entry {
            Ok(e) if e.file_type().is_file() => match e.metadata() {
                Ok(meta) => total += meta.len(),
                Err(err) => {
                    reliable = false;
                    tracing::warn!("disk sampler: stat failed for {:?}: {err}", e.path());
                }
            },
            Ok(_) => {}
            Err(err) => {
                reliable = false;
                tracing::warn!("disk sampler: walk entry failed: {err}");
            }
        }
    }
    (total, reliable)
}

/// Resets `DiskCapacityState::cleanup_in_flight` to `false` on drop so the
/// single-flight self-heal flag is released on every exit of the spawned
/// cleanup task — normal completion, error, OR panic-unwind — instead of only
/// on the success path. Without this a panic in `run_retention_cleanup` would
/// leave the flag stuck `true` and disable immediate self-heal forever.
struct CleanupInFlightGuard(Arc<DiskCapacityState>);

impl Drop for CleanupInFlightGuard {
    fn drop(&mut self) {
        self.0.cleanup_in_flight.store(false, Ordering::Release);
    }
}

/// Spawn the storage-maintenance background task. Runs when retention is on
/// (`SIGIRO_RETENTION_DAYS > 0`) OR the disk cap is armed (`cap > 0`); returns
/// `None` when neither is configured. Drives two independent timers:
/// - every `SIGIRO_DISK_SAMPLE_SECONDS`: sample `DATA_DIR` bytes and, if the
///   cap is armed, trip/clear the gate with hysteresis. On the OFF→ON trip,
///   single-flight an immediate retention cleanup (self-heal); the next sample
///   tick re-measures and clears the gate — clearing stays on this one sampler.
/// - hourly: run the routine retention cleanup (as before).
fn spawn_storage_maintenance_task(
    router: Arc<MsgRouter>,
    data_dir: camino::Utf8PathBuf,
    disk: Arc<DiskCapacityState>,
) -> Option<tokio::task::JoinHandle<()>> {
    let retention_days = match crate::config::RETENTION_DAYS.value() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("invalid SIGIRO_RETENTION_DAYS, disabling retention: {e:#}");
            0
        }
    };
    let cap = disk.cap;
    if retention_days == 0 && cap == 0 {
        return None;
    }

    let sample_secs = match crate::config::disk_sample_seconds() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("invalid SIGIRO_DISK_SAMPLE_SECONDS, using 30s: {e:#}");
            30
        }
    };

    tracing::info!(
        retention_days,
        disk_cap_bytes = cap,
        sample_secs,
        "storage maintenance enabled"
    );
    if cap > 0
        && !crate::config::S3_BUCKET
            .value()
            .unwrap_or_default()
            .is_empty()
    {
        tracing::warn!(
            "SIGIRO_MAX_DISK_BYTES caps only the LOCAL catalog/inline/temp footprint in S3 mode — cold Parquet in the bucket is not counted"
        );
    }

    Some(tokio::spawn(async move {
        let mut sample_interval = tokio::time::interval(Duration::from_secs(sample_secs));
        let mut retention_interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tokio::select! {
                _ = sample_interval.tick() => {
                    let dir = data_dir.clone();
                    let (bytes, reliable) = match tokio::task::spawn_blocking(move || dir_size_bytes(dir.as_std_path())).await {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!("disk sampler task failed to join: {e}");
                            continue;
                        }
                    };
                    // `apply_sample` is only ever called here (one serialized
                    // sampler) — no concurrent caller can race the load/store,
                    // and `over_cap` is re-evaluated on the NEXT tick after any
                    // self-heal cleanup frees space, so we never resample off-loop.
                    match disk.apply_sample(bytes, reliable) {
                        Some(true) => {
                            tracing::warn!(
                                sampled_bytes = bytes,
                                cap,
                                "disk cap tripped — REJECTING ingest until usage falls below 90% of cap"
                            );
                            // Self-heal: kick one retention cleanup, single-flighted.
                            // The next sample tick re-measures and clears the gate.
                            if retention_days > 0
                                && disk
                                    .cleanup_in_flight
                                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                    .is_ok()
                            {
                                let r = Arc::clone(&router);
                                let ds = Arc::clone(&disk);
                                tokio::spawn(async move {
                                    // RAII: reset the single-flight flag on EVERY exit —
                                    // normal, error, or panic — so a panicking cleanup
                                    // can't wedge self-heal off forever.
                                    let _reset = CleanupInFlightGuard(ds);
                                    if let Err(e) = r.run_retention_cleanup(retention_days).await {
                                        tracing::error!("disk-cap self-heal retention cleanup failed: {e:#}");
                                    }
                                });
                            }
                        }
                        Some(false) => {
                            tracing::info!(
                                sampled_bytes = bytes,
                                cap,
                                "disk usage back under 90% of cap — ingest re-enabled"
                            );
                        }
                        None => {}
                    }
                }

                _ = retention_interval.tick() => {
                    if retention_days > 0
                        && let Err(e) = router.run_retention_cleanup(retention_days).await
                    {
                        tracing::error!("retention cleanup failed: {e:#}");
                    }
                }
            }
        }
    }))
}

#[instrument(skip(state))]
async fn health(
    _path: HealthPath,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    // 1. Flusher health — most critical. A dead flusher silently drops
    //    telemetry, so we must report degraded even if init completed.
    if state.router.has_dead_flushers() {
        tracing::warn!("health check: dead flushers detected");
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse::Degraded {
                reason: "dead flushers".to_string(),
            }),
        );
    }

    // 2. Init completeness — `AppState::init_complete` is flipped at the
    //    tail of `run_serve`. While the HTTP listener is up before init
    //    finishes, the server is not actually serving traffic yet, so
    //    probes (k8s readiness, docker healthcheck) must see 503.
    if !state.init_complete.load(Ordering::Acquire) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse::Starting),
        );
    }

    tracing::info!("health check called");
    (
        axum::http::StatusCode::OK,
        axum::Json(HealthResponse::Ready),
    )
}

#[instrument(skip(state))]
async fn v1_health(
    _path: V1HealthPath,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    health(HealthPath, State(state)).await
}

#[instrument(skip(state))]
async fn healthz(
    _path: HealthzPath,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    health(HealthPath, State(state)).await
}

/// Get the per-project DuckDB reader pool using the AuthContext.
fn project_reader_pool(state: &AppState, auth: &AuthContext) -> ApiResult<ReaderPool> {
    // Explicit DI: get_project returns only existing projects.
    // Callers must call create_project() first to initialize a project.
    state
        .pool
        .get_project(&auth.tenant_id)
        .map_err(|e| anyhow::anyhow!(e).context_internal("Project DB", "connection not found"))
}

/// Escape a string for safe use inside a SQL single-quoted string literal.
fn escape_sql_string(s: &str) -> String {
    s.replace("'", "''")
}

/// Execute SQL against the local DuckDB project connection. Returns the
/// result rows plus whether the row cap (`SIGIRO_MAX_RESULT_ROWS`) truncated
/// them.
async fn execute_sql(
    state: &AppState,
    auth: &AuthContext,
    sql: &str,
) -> anyhow::Result<(Vec<serde_json::Value>, bool)> {
    let readers = project_reader_pool(state, auth)
        .map_err(|e: axum_anyhow::ApiError| anyhow::anyhow!("{:?}", e))?;
    let sql_clone = sql.to_string();
    let timeout_secs = crate::config::query_timeout_secs()?;
    let max_rows = crate::config::max_result_rows()?;
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel::<Arc<duckdb::InterruptHandle>>();

    let mut query_task = tokio::task::spawn_blocking(move || {
        let conn = readers.get();
        // Sent before `prepare`/`query` so the timeout branch below can
        // interrupt this exact connection even if it never gets a chance to
        // run (e.g. still waiting on `readers.get()` for a free connection).
        // An error here just means the async side gave up waiting — nothing
        // to interrupt for, so there is nothing to report.
        if handle_tx.send(conn.interrupt_handle()).is_err() {
            tracing::debug!("query timeout watchdog receiver dropped before send");
        }

        let mut stmt = conn
            .prepare(&sql_clone)
            .map_err(|e| anyhow::anyhow!("failed to prepare query: {e}"))?;
        // Execute ONCE. Column names come from the live statement behind the
        // Rows handle (owned Vec, so the borrow ends before we iterate).
        let mut rows = stmt
            .query([])
            .map_err(|e| anyhow::anyhow!("failed to execute query: {e}"))?;
        let column_names: Vec<String> = rows
            .as_ref()
            .map(duckdb::Statement::column_names)
            .unwrap_or_default();
        let mut results = Vec::new();
        let mut truncated = false;
        // Propagate a mid-stream error instead of silently truncating the
        // result and returning partial rows as HTTP 200. Row-cap truncation
        // (below) is the one deliberate, reported exception.
        while let Some(row) = rows
            .next()
            .map_err(|e| anyhow::anyhow!("row read failed: {e}"))?
        {
            if results.len() >= max_rows {
                truncated = true;
                break;
            }
            let mut obj = serde_json::Map::new();
            for (i, name) in column_names.iter().enumerate() {
                let val = row
                    .get_ref(i)
                    .map_err(|e| anyhow::anyhow!("column read failed: {e}"))?;
                obj.insert(name.clone(), duckdb_value_to_json(val));
            }
            results.push(serde_json::Value::Object(obj));
        }
        Ok((results, truncated))
    });

    // ponytail: timeout enforcement polls a shared oneshot handle rather than
    // tracking per-connection "busy" state, so there's a narrow window where
    // the deadline fires just as the query finishes and the now-idle pooled
    // connection gets a spurious interrupt(). DuckDB (this bundled version)
    // has no `statement_timeout` setting to enforce this engine-side instead;
    // revisit if spurious interrupts on adjacent queries are ever observed.
    tokio::select! {
        joined = &mut query_task => {
            joined
                .map_err(|e| anyhow::anyhow!("query task failed: {e}"))?
        }
        () = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            if let Ok(handle) = handle_rx.await {
                tracing::warn!(timeout_secs, "query exceeded timeout — interrupting");
                handle.interrupt();
            }
            (&mut query_task)
                .await
                .map_err(|e| anyhow::anyhow!("query task failed: {e}"))?
        }
    }
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
        _ => serde_json::Value::String(format!("{:?}", val)),
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

/// `x-sigiro-truncated` header + JSON rows — signals `SIGIRO_MAX_RESULT_ROWS`
/// truncation without changing the response body shape (still a bare array,
/// so existing callers like `SigiroClient::query` keep working unchanged).
type QueryResponse = (
    [(&'static str, &'static str); 1],
    Json<Vec<serde_json::Value>>,
);

#[utoipa::path(
    post,
    path = "/v1/query",
    request_body = String,
    responses(
        (status = 200, description = "Query results", body = Vec<serde_json::Value>)
    ),
    tag = "query"
)]
#[instrument(skip_all, fields(sql_len = body.len()))]
async fn query_handler(
    _path: QueryPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    body: String,
) -> ApiResult<QueryResponse> {
    // Truncated SQL preview so every query outcome (reject / fail / complete)
    // leaves a server-side trace — a silently-throttled or errored query used
    // to leave none, which is what let a false "data loss" RCA take hold.
    let sql_preview: String = body.chars().take(200).collect();
    // In hosted mode (shared PostgreSQL catalog) block information_schema
    // enumeration — it would expose the control plane + sibling tenant schema
    // names. Local/self-host keeps it (single-catalog, safe).
    let allow_information_schema = !state.pool.storage_mode().is_distributed();
    if let Err(e) = validate_read_only_with(&body, allow_information_schema) {
        tracing::warn!(sql = %sql_preview, "query rejected (validation): {e}");
        return Err(bad_request("query rejected", &e));
    }
    match execute_sql(&state, &auth, &body).await {
        Ok((results, truncated)) => {
            tracing::info!(
                rows = results.len(),
                truncated,
                sql = %sql_preview,
                "query completed"
            );
            // SIGIRO_MAX_RESULT_ROWS was hit — the client must not mistake a
            // capped result for the complete answer.
            let truncated_header = [(
                "x-sigiro-truncated",
                if truncated { "true" } else { "false" },
            )];
            Ok((truncated_header, Json(results)))
        }
        Err(e) => {
            tracing::warn!(sql = %sql_preview, "query failed: {e:#}");
            Err(bad_request("query rejected", &format!("{e:#}")))
        }
    }
}

/// GET /v1/services — discover what services are sending telemetry.
/// Agents call this before /investigate to know what services exist.
#[utoipa::path(
    get,
    path = "/v1/services",
    responses(
        (status = 200, description = "List of services", body = ServicesResponse)
    ),
    tag = "query"
)]
#[instrument(skip_all)]
async fn services_handler(
    _path: ServicesPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<ServicesResponse>> {
    let prefix = ProjectPool::prefix();
    let sql = format!(
        "SELECT service_name, \
                min(timestamp)::VARCHAR AS first_seen, \
                max(timestamp)::VARCHAR AS last_seen, \
                count(*) AS span_count \
         FROM {}sigiro_spans \
         GROUP BY service_name \
         ORDER BY span_count DESC",
        prefix
    );
    let (results, _truncated) = execute_sql(&state, &auth, &sql)
        .await
        .map_err(|e| bad_request("services query failed", &format!("{e:#}")))?;

    let mut services = Vec::new();
    for row in results {
        let obj = row.as_object().ok_or_else(|| {
            bad_request("services query failed", "expected object row from query")
        })?;
        let name = obj
            .get("service_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let first_seen = obj
            .get("first_seen")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);
        let last_seen = obj
            .get("last_seen")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);
        let span_count = obj
            .get("span_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        services.push(ServiceEntry {
            name,
            first_seen,
            last_seen,
            span_count,
        });
    }

    Ok(Json(ServicesResponse { services }))
}

/// GET /v1/traces/{trace_id} -- return a single trace with full detail including critical path and service breakdown.
#[utoipa::path(
     get,
     path = "/v1/traces/{trace_id}",
     params(
         ("trace_id" = String, Path, description = "Trace ID")
     ),
     responses(
         (status = 200, description = "Trace detail", body = TraceDetailResponse)
     ),
     tag = "query"
 )]
#[instrument(skip_all)]
async fn trace_detail_handler(
    _path: TraceDetailPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<TraceDetailResponse>> {
    let prefix = ProjectPool::prefix();
    let trace_id = _path.trace_id.clone();
    let safe_trace_id = escape_sql_string(&trace_id);

    let sql = format!(
        "SELECT
            trace_id, span_id, parent_span_id, span_name, duration,
            status_code, service_name, span_kind,
            span_attributes, resource_attributes,
            timestamp::VARCHAR AS start_timestamp,
            end_timestamp,
            events_json, links_json,
            scope_name, scope_version,
            trace_state
         FROM {}sigiro_spans
         WHERE trace_id = '{}'
         ORDER BY timestamp",
        prefix, safe_trace_id
    );
    let (results, _truncated) = execute_sql(&state, &auth, &sql)
        .await
        .map_err(|e| bad_request("trace detail query failed", &format!("{e:#}")))?;

    let mut spans: Vec<SpanDetail> = Vec::new();
    let mut span_map: std::collections::HashMap<String, SpanDetail> =
        std::collections::HashMap::new();
    let mut parent_to_children: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut root_span_id: Option<String> = None;
    let mut total_duration_us: i64 = 0;
    let mut error_count: i64 = 0;
    let mut services_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut max_duration_span: Option<SpanDetail> = None;

    for row in results {
        let obj = row.as_object().ok_or_else(|| {
            bad_request(
                "trace detail query failed",
                "expected object row from query",
            )
        })?;

        let span_id = obj
            .get("span_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let parent_span_id = obj
            .get("parent_span_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let duration_us = obj
            .get("duration")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let status_code = obj
            .get("status_code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0) as i32;
        let service_name = obj
            .get("service_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let span = SpanDetail {
            trace_id: obj
                .get("trace_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            span_id: span_id.clone(),
            parent_span_id: if parent_span_id.is_empty() {
                None
            } else {
                Some(parent_span_id.clone())
            },
            span_name: obj
                .get("span_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            duration_us,
            status_code,
            service_name: service_name.clone(),
            timestamp: obj
                .get("start_timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            span_kind: obj
                .get("span_kind")
                .and_then(serde_json::Value::as_i64)
                .map(|n| n as i32),
            span_attributes: obj
                .get("span_attributes")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            resource_attributes: obj
                .get("resource_attributes")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            events_json: obj
                .get("events_json")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            links_json: obj
                .get("links_json")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            scope_name: obj
                .get("scope_name")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            scope_version: obj
                .get("scope_version")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            trace_state: obj
                .get("trace_state")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
        };
        total_duration_us = total_duration_us.saturating_add(duration_us);
        if status_code == 2 {
            error_count += 1;
        }
        services_set.insert(service_name);

        if parent_span_id.is_empty() {
            root_span_id = Some(span_id.clone());
        } else {
            parent_to_children
                .entry(parent_span_id)
                .or_default()
                .push(span_id.clone());
        }

        let is_longer = max_duration_span
            .as_ref()
            .map(|s| duration_us > s.duration_us)
            .unwrap_or(true);
        if is_longer {
            max_duration_span = Some(span.clone());
        }

        span_map.insert(span_id.clone(), span.clone());
        spans.push(span);
    }

    if spans.is_empty() {
        return Err(axum_anyhow::not_found(
            "Trace Not Found",
            &format!("No trace found with id: {}", trace_id),
        ));
    }

    // Determine root span
    let root_span = if let Some(root_id) = root_span_id {
        span_map
            .get(&root_id)
            .cloned()
            .unwrap_or_else(|| max_duration_span.clone().unwrap())
    } else {
        max_duration_span.unwrap()
    };

    // Compute critical path (longest path from root to leaf by duration sum)
    let critical_path = compute_critical_path(&root_span.span_id, &parent_to_children, &span_map);

    // Compute service breakdown
    let service_breakdown = compute_service_breakdown(&spans, total_duration_us);

    Ok(Json(TraceDetailResponse {
        trace_id,
        total_duration_us,
        root_span,
        span_count: spans.len() as i64,
        error_count,
        services: services_set.into_iter().collect(),
        spans,
        critical_path,
        service_breakdown,
    }))
}

/// Compute the critical path: longest duration-sum path from root to any leaf.
fn compute_critical_path(
    root_id: &str,
    parent_to_children: &std::collections::HashMap<String, Vec<String>>,
    span_map: &std::collections::HashMap<String, SpanDetail>,
) -> Vec<String> {
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        span_id: &str,
        parent_to_children: &std::collections::HashMap<String, Vec<String>>,
        span_map: &std::collections::HashMap<String, SpanDetail>,
        current_path: &[String],
        current_duration: i64,
        best_path: &mut Vec<String>,
        best_duration: &mut i64,
    ) {
        let span = match span_map.get(span_id) {
            Some(s) => s,
            None => return,
        };
        let mut path = current_path.to_vec();
        path.push(span_id.to_string());
        let new_duration = current_duration.saturating_add(span.duration_us);

        // Leaf node: check if this path is the critical path
        if let Some(children) = parent_to_children.get(span_id) {
            for child_id in children {
                dfs(
                    child_id,
                    parent_to_children,
                    span_map,
                    &path,
                    new_duration,
                    best_path,
                    best_duration,
                );
            }
        } else {
            // Leaf node
            if new_duration > *best_duration {
                *best_duration = new_duration;
                *best_path = path;
            }
        }
    }

    let mut best_path = Vec::new();
    let mut best_duration = i64::MIN;
    dfs(
        root_id,
        parent_to_children,
        span_map,
        &[],
        0,
        &mut best_path,
        &mut best_duration,
    );
    best_path
}

/// Compute service duration breakdown with percentages.
fn compute_service_breakdown(
    spans: &[SpanDetail],
    total_duration_us: i64,
) -> Vec<ServiceBreakdownEntry> {
    let mut service_durations: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for span in spans {
        *service_durations
            .entry(span.service_name.clone())
            .or_insert(0) += span.duration_us;
    }

    let total = if total_duration_us > 0 {
        total_duration_us as f64
    } else {
        1.0
    };
    let mut breakdown: Vec<ServiceBreakdownEntry> = service_durations
        .into_iter()
        .map(|(service, duration_us)| ServiceBreakdownEntry {
            service,
            duration_us,
            percentage: (duration_us as f64 / total) * 100.0,
        })
        .collect();
    breakdown.sort_by(|a, b| b.duration_us.cmp(&a.duration_us));
    breakdown
}

#[utoipa::path(
    post,
    path = "/v1/investigate",
    responses(
        (status = 200, description = "Investigation results")
    ),
    tag = "query"
)]
#[instrument(skip_all)]
async fn investigate_handler(
    _path: InvestigatePath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> ApiResult<Json<std::collections::BTreeMap<String, investigate::InvestigationBlock>>> {
    // Validate mandatory timestamp parameters.
    if req.from_ts == 0 {
        return Err(bad_request(
            "Invalid Request",
            "from_ts is required and must be a positive Unix timestamp in microseconds",
        ));
    }
    if req.to_ts == 0 {
        return Err(bad_request(
            "Invalid Request",
            "to_ts is required and must be a positive Unix timestamp in microseconds",
        ));
    }
    if req.to_ts <= req.from_ts {
        return Err(bad_request(
            "Invalid Request",
            "to_ts must be greater than from_ts",
        ));
    }

    let data_dir = crate::data_dir().context_internal("Investigate", "SIGIRO_DATA_DIR not set")?;
    let project_data_path = if state.pool.storage_mode().is_distributed() {
        crate::storage::tenant_data_dir(&data_dir, &auth.tenant_id)
            .context_internal("Investigate", "invalid tenant id")?
    } else {
        data_dir.join(&auth.tenant_id)
    };
    if !project_data_path.exists()
        || project_data_path
            .read_dir()
            .map(|mut i| i.next().is_none())
            .unwrap_or(true)
    {
        return Err(axum_anyhow::not_found(
            "No Data Found",
            &format!(
                "No data found for project {}. Please ingest data before calling investigate.",
                auth.tenant_id
            ),
        ));
    }
    let readers = project_reader_pool(&state, &auth)?;
    let blocks = tokio::task::spawn_blocking(move || {
        let conn = readers.get();
        investigate::investigate(&conn, &req, ProjectPool::prefix())
    })
    .await
    .map_err(|e| anyhow::anyhow!("investigation task failed: {e}"))
    .context_internal("Investigate", "investigation task failed")?
    .context_internal("Investigate", "investigation failed")?;
    Ok(Json(blocks))
}

// --- Admin endpoints ---
// All endpoints, including status, are admin-token gated (see require_admin_token).

/// GET /admin/status — returns all instance_status rows as JSON object.
#[utoipa::path(
    get,
    path = "/admin/status",
    responses(
        (status = 200, description = "Server status", body = AdminStatusResponse)
    ),
    tag = "admin"
)]
async fn admin_status(
    _path: AdminStatusPath,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<AdminStatusResponse>> {
    require_admin_token(&headers)?;
    let guard = state.admin_conn.lock();
    let mut map =
        admin::read_all_status(&guard).context_internal("Admin", "failed to read status")?;
    map.insert(
        "ingest_dropped_messages_total".to_string(),
        state.router.dropped_messages().to_string(),
    );
    Ok(Json(AdminStatusResponse { status: map }))
}

fn require_admin_token(headers: &HeaderMap) -> Result<(), axum_anyhow::ApiError> {
    let expected = crate::config::ADMIN_TOKEN.value().map_err(|_| {
        axum_anyhow::unauthorized(
            "Missing Admin Token",
            "SIGIRO_ADMIN_TOKEN is not configured",
        )
    })?;
    if expected.is_empty() {
        return Err(axum_anyhow::unauthorized(
            "Missing Admin Token",
            "SIGIRO_ADMIN_TOKEN is empty",
        ));
    }
    let token = auth::extract_bearer_token(headers)?;
    if token == expected {
        Ok(())
    } else {
        Err(axum_anyhow::unauthorized(
            "Invalid Admin Token",
            "The provided admin bearer token is invalid",
        ))
    }
}

fn control_plane_from_state(state: &AppState) -> ApiResult<Arc<auth::ControlPlane>> {
    state.auth_mode.control_plane().ok_or_else(|| {
        bad_request(
            "distributed auth required",
            "tenant provisioning requires SIGIRO_CATALOG_URL distributed mode",
        )
    })
}

/// POST /admin/tenants — create or reactivate a tenant in the distributed control plane.
#[utoipa::path(
    post,
    path = "/admin/tenants",
    request_body = CreateTenantRequest,
    responses(
        (status = 200, description = "Tenant created", body = CreateTenantResponse)
    ),
    tag = "admin"
)]
async fn create_tenant_handler(
    _path: AdminTenantsPath,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateTenantRequest>,
) -> ApiResult<Json<CreateTenantResponse>> {
    require_admin_token(&headers)?;
    let control = control_plane_from_state(&state)?;
    control
        .create_tenant(&req.id)
        .map_err(auth::ControlPlaneError::into_api_error)?;
    Ok(Json(CreateTenantResponse {
        id: req.id,
        status: "active".to_string(),
    }))
}

/// POST /admin/keys — mint an API key for an existing tenant.
#[utoipa::path(
    post,
    path = "/admin/keys",
    request_body = CreateKeyRequest,
    responses(
        (status = 200, description = "API key created", body = CreateKeyResponse)
    ),
    tag = "admin"
)]
async fn create_key_handler(
    _path: AdminKeysPath,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateKeyRequest>,
) -> ApiResult<Json<CreateKeyResponse>> {
    require_admin_token(&headers)?;
    let control = control_plane_from_state(&state)?;
    let minted = control
        .mint_key(&req.tenant_id)
        .map_err(auth::ControlPlaneError::into_api_error)?;
    Ok(Json(CreateKeyResponse {
        key: minted.key,
        key_hash: minted.key_hash,
        tenant_id: minted.tenant_id,
    }))
}

/// DELETE /admin/keys/{short_token} — revoke an API key by hash prefix.
#[utoipa::path(
    delete,
    path = "/admin/keys/{short_token}",
    params(
        ("short_token" = String, Path, description = "8-64 character key hash prefix")
    ),
    responses(
        (status = 200, description = "API key revoked", body = RevokeKeyResponse)
    ),
    tag = "admin"
)]
async fn revoke_key_handler(
    AdminKeyPath { short_token }: AdminKeyPath,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<RevokeKeyResponse>> {
    require_admin_token(&headers)?;
    let control = control_plane_from_state(&state)?;
    let revoked = control
        .revoke_key_hash_prefix(&short_token)
        .map_err(auth::ControlPlaneError::into_api_error)?;
    Ok(Json(RevokeKeyResponse {
        revoked,
        short_token,
    }))
}

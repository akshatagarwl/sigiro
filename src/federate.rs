//! Phase C: the federated query CLI coordinator (`sigiro query "<SQL>"`).
//!
//! Phase A/B built the EDGE side: each self-hosted node serves its own
//! DuckLake read-only over the DuckDB `quack` wire protocol (see
//! `crate::quack`). This module builds the CLIENT side — a coordinator that
//! queries MULTIPLE edges and stitches the results into one answer, while
//! each edge's data stays put.
//!
//! # Empirically-verified design decisions (do not re-derive; see the Phase C
//! task brief for the measurements)
//!
//! - `quack_query('quack://host:port', '<sub-sql>', token := ...)` pushes the
//!   sub-SQL to the edge and runs it there, with full DuckLake partition/
//!   min-max pruning. Plain `ATTACH ... (TYPE quack)` does NOT push
//!   predicates (pulls whole tables) — never used here.
//! - Naive aggregate folding is wrong (avg-of-avgs measured 13% off). Every
//!   foldable aggregate is split into an additive PARTIAL computed at each
//!   edge, unioned, then folded exactly once at the coordinator (see
//!   `partial_cols`/`final_expr`).
//! - DuckDB TEMP tables are connection-local — even two `Connection`s
//!   `try_clone()`'d from the same in-memory database do NOT see each
//!   other's temp tables (verified directly: a table created on one clone
//!   raised `Catalog Error` on the other). So each edge is fetched on its own
//!   independent, throwaway in-memory connection, and the fetched data is
//!   bridged into ONE coordinator connection as real Arrow `RecordBatch`es —
//!   not by sharing a connection or session.
//! - The bridge uses DuckDB's own Arrow interop (`duckdb::vtab::arrow::ArrowVTab`
//!   registered as `arrow(?, ?)`, fed via `arrow_recordbatch_to_query_params`)
//!   so DuckDB infers the target table's column types itself — no hand-rolled
//!   Arrow→DuckDB type mapping. Verified end-to-end against hand-computed
//!   ground truth (including `sum(BIGINT)` promoting to `DECIMAL(38,0)`).
//! - The query is parsed with DuckDB's OWN parser via `json_serialize_sql`,
//!   not a hand-rolled SQL parser — the JSON AST shape used below (node
//!   classes, `group_sets`, modifier types, comparison operator names) was
//!   verified directly against this crate's pinned DuckDB build, not guessed.

use anyhow::{Context, Result};
use arrow::record_batch::RecordBatch;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Edge resolution
// ---------------------------------------------------------------------------

/// One query-able edge: a self-hosted sigiro node serving `quack` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) port: u16,
}

/// Resolve the edge list. `cli_edges` are `--edge name=host:port` flags.
///
/// Precedence: if `SIGIRO_EDGES` (comma-separated `name=host:port`) is set,
/// or `--edge` was passed, those are used VERBATIM and tailnet discovery is
/// skipped entirely. Otherwise, peers tagged `tag:sigiro-edge` are
/// discovered via the local `tailscaled` (`tailscale status --json`).
///
/// Fails loud if zero edges resolve either way — a federated query coordinator
/// silently running against nothing is a worse failure mode than refusing to run.
pub(crate) fn resolve_edges(cli_edges: &[String]) -> Result<Vec<Edge>> {
    let env_edges = crate::config::EDGES
        .value()
        .context("failed to read SIGIRO_EDGES")?;
    let mut specs: Vec<String> = env_edges
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    specs.extend(cli_edges.iter().cloned());

    let edges = if specs.is_empty() {
        discover_tailnet_edges()?
    } else {
        specs
            .iter()
            .map(|s| parse_edge_spec(s))
            .collect::<Result<Vec<_>>>()?
    };

    anyhow::ensure!(
        !edges.is_empty(),
        "no edges resolved for the federated query: no --edge/SIGIRO_EDGES override was \
         given, and tailnet discovery found no peer tagged tag:sigiro-edge. Refusing to run \
         a federated query against zero edges — pass --edge name=host:port, set SIGIRO_EDGES, \
         or tag a tailnet peer tag:sigiro-edge."
    );

    // ponytail: best-effort textual normalization only (lowercase +
    // loopback-alias folding) — does not resolve DNS or compare IPs
    // numerically, so e.g. a hostname that merely resolves to the same box
    // as another entry still slips through as a "different" edge.
    let mut seen: std::collections::HashMap<(String, u16), &str> = std::collections::HashMap::new();
    for edge in &edges {
        if let Some(prev_name) =
            seen.insert((normalize_host(&edge.host), edge.port), edge.name.as_str())
        {
            anyhow::bail!(
                "duplicate edge {}:{} listed twice (as {prev_name:?} and {:?}) — federating the \
                 same edge twice would silently double-count every count/sum/avg; remove the \
                 duplicate --edge/SIGIRO_EDGES entry",
                edge.host,
                edge.port,
                edge.name
            );
        }
    }

    Ok(edges)
}

/// Normalize a host for edge-duplicate detection: lowercase, and fold the
/// three common loopback spellings (`localhost`, `127.0.0.1`, `::1`) to one
/// canonical form so the SAME machine listed under two different loopback
/// names is caught as a duplicate edge.
fn normalize_host(host: &str) -> String {
    match host.to_lowercase().as_str() {
        "localhost" | "127.0.0.1" | "::1" => "127.0.0.1".to_string(),
        other => other.to_string(),
    }
}

fn parse_edge_spec(spec: &str) -> Result<Edge> {
    let (name, addr) = spec
        .split_once('=')
        .with_context(|| format!("invalid edge spec {spec:?}: expected name=host:port"))?;
    let (host, port) = addr
        .rsplit_once(':')
        .with_context(|| format!("invalid edge spec {spec:?}: expected name=host:port"))?;
    let port: u16 = port
        .parse()
        .with_context(|| format!("invalid edge spec {spec:?}: {port:?} is not a valid port"))?;
    anyhow::ensure!(
        !name.is_empty() && !host.is_empty(),
        "invalid edge spec {spec:?}: name and host must not be empty"
    );
    Ok(Edge {
        name: name.to_string(),
        host: host.to_string(),
        port,
    })
}

/// Discover edges from the tailnet: peers (and self) tagged `tag:sigiro-edge`,
/// using their first Tailscale IP and the configured default quack port
/// (`SIGIRO_QUACK_PORT`, default 7777 — the coordinator has no way to know an
/// individual edge's actual bound port, so all discovered edges are assumed
/// to use the same default).
fn discover_tailnet_edges() -> Result<Vec<Edge>> {
    let output = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .context(
            "failed to run `tailscale status --json` — is tailscale installed and running? \
             (use --edge/SIGIRO_EDGES to bypass tailnet discovery for testing)",
        )?;
    anyhow::ensure!(
        output.status.success(),
        "`tailscale status --json` exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let status: Value = serde_json::from_slice(&output.stdout)
        .context("failed to parse `tailscale status --json` output as JSON")?;
    let default_port = crate::config::QUACK_PORT.value()?;

    let mut candidates: Vec<&Value> = Vec::new();
    if let Some(self_node) = status.get("Self") {
        candidates.push(self_node);
    }
    if let Some(peers) = status.get("Peer").and_then(Value::as_object) {
        candidates.extend(peers.values());
    }

    let mut edges = Vec::new();
    for node in candidates {
        let tagged = node
            .get("Tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| tags.iter().any(|t| t.as_str() == Some("tag:sigiro-edge")));
        if !tagged {
            continue;
        }
        let host = node
            .get("TailscaleIPs")
            .and_then(Value::as_array)
            .and_then(|ips| ips.first())
            .and_then(Value::as_str)
            .with_context(|| {
                format!("tailnet peer tagged tag:sigiro-edge has no TailscaleIPs: {node}")
            })?
            .to_string();
        let name = node
            .get("HostName")
            .and_then(Value::as_str)
            .unwrap_or(&host)
            .to_string();
        edges.push(Edge {
            name,
            host,
            port: default_port,
        });
    }
    Ok(edges)
}

// ---------------------------------------------------------------------------
// Query planning: DuckDB's own parser (json_serialize_sql) -> a validated,
// narrow AST -> per-edge PARTIAL SQL + coordinator FINAL fold SQL.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AggKind {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone)]
struct AggregateItem {
    kind: AggKind,
    /// Column argument. `None` only for `count(*)`.
    arg: Option<String>,
    alias: String,
}

/// A plain-column SELECT item. `name` is the resolved real column (never an
/// alias); `output` is the JSON key the caller sees — the user's `AS` alias
/// when given, else `name` itself.
#[derive(Debug, Clone)]
struct ColumnItem {
    name: String,
    output: String,
}

#[derive(Debug, Clone)]
enum SelectItem {
    Star,
    Column(ColumnItem),
    Aggregate(AggregateItem),
}

/// `NULLS FIRST`/`NULLS LAST` on an ORDER BY key. `Default` means the user
/// wrote neither — omit an explicit NULLS clause and let both the per-edge
/// push and the coordinator fold use DuckDB's own (matching) default, rather
/// than guessing what that default is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NullOrder {
    Default,
    First,
    Last,
}

impl NullOrder {
    fn to_sql(self) -> &'static str {
        match self {
            NullOrder::Default => "",
            NullOrder::First => " NULLS FIRST",
            NullOrder::Last => " NULLS LAST",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedQuery {
    table: String,
    where_sql: Option<String>,
    has_time_bound: bool,
    group_keys: Vec<String>,
    select_items: Vec<SelectItem>,
    order_by: Vec<(String, bool, NullOrder)>,
    limit: Option<i64>,
}

/// True if any projected item is an aggregate (`count(*)`, `sum(x)`, …).
fn has_aggregate(items: &[SelectItem]) -> bool {
    items.iter().any(|i| matches!(i, SelectItem::Aggregate(_)))
}

impl ParsedQuery {
    fn is_aggregate(&self) -> bool {
        has_aggregate(&self.select_items)
    }
}

/// Parse `sql` via `json_serialize_sql` on `conn` and validate it against the
/// MVP federatable grammar: a single `SELECT` over one `sigiro_*` table,
/// optional WHERE (column/literal comparisons + AND/OR only), optional plain
/// GROUP BY, aggregates limited to the foldable whitelist
/// (count/sum/min/max/avg, no DISTINCT), optional ORDER BY a column/alias,
/// optional LIMIT. Fails loud with a specific reason on anything outside
/// that shape — never silently degrades (e.g. never falls back to
/// avg-of-avgs for an unsupported case).
fn plan_query(conn: &duckdb::Connection, sql: &str) -> Result<ParsedQuery> {
    let escaped = crate::storage::escape_sql_string(sql);
    let json_text: String = conn
        .query_row(
            &format!("SELECT json_serialize_sql('{escaped}')"),
            [],
            |r| r.get(0),
        )
        .context("failed to call json_serialize_sql — is the json extension loaded?")?;
    let ast: Value =
        serde_json::from_str(&json_text).context("failed to parse json_serialize_sql output")?;

    if ast.get("error").and_then(Value::as_bool).unwrap_or(true) {
        let message = ast
            .get("error_message")
            .and_then(Value::as_str)
            .unwrap_or("query rejected by DuckDB's parser (not a plain SELECT, or a syntax error)");
        anyhow::bail!("federated query not federatable: {message}");
    }

    let statements = ast
        .get("statements")
        .and_then(Value::as_array)
        .context("json_serialize_sql output missing statements array")?;
    anyhow::ensure!(
        statements.len() == 1,
        "federated queries support exactly one SELECT statement; got {} \
         (multi-statement batches are not federatable)",
        statements.len()
    );
    let node = field(&statements[0], "node")?;
    anyhow::ensure!(
        field_str(node, "type")? == "SELECT_NODE",
        "federated queries must be a plain SELECT (got a {} node — UNION/INTERSECT/EXCEPT \
         and other set operations are not federatable)",
        field_str(node, "type")?
    );

    let cte_count = node
        .get("cte_map")
        .and_then(|m| m.get("map"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    anyhow::ensure!(
        cte_count == 0,
        "CTEs (WITH ...) are not supported in a federated query"
    );
    anyhow::ensure!(
        is_null_field(node, "having"),
        "HAVING is not supported in a federated query"
    );
    anyhow::ensure!(
        is_null_field(node, "qualify"),
        "QUALIFY is not supported in a federated query"
    );
    anyhow::ensure!(
        is_null_field(node, "sample"),
        "SAMPLE/TABLESAMPLE is not supported in a federated query"
    );

    let from_table = field(node, "from_table")?;
    anyhow::ensure!(
        field_str(from_table, "type")? == "BASE_TABLE",
        "federated queries support exactly one base table in FROM (no JOIN, subquery, \
         CTE reference, or table function); got a {} relation",
        field_str(from_table, "type")?
    );
    anyhow::ensure!(
        is_null_field(from_table, "at_clause"),
        "DuckLake time-travel (AT (...)) is not supported in a federated query"
    );
    anyhow::ensure!(
        is_null_field(from_table, "sample"),
        "SAMPLE/TABLESAMPLE is not supported in a federated query"
    );
    let catalog_name = from_table
        .get("catalog_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    anyhow::ensure!(
        catalog_name.is_empty(),
        "cross-catalog table references are not supported in a federated query"
    );
    let schema_name = from_table
        .get("schema_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    anyhow::ensure!(
        schema_name.is_empty()
            || schema_name.eq_ignore_ascii_case("lake")
            || schema_name.eq_ignore_ascii_case("main"),
        "unexpected schema qualifier {schema_name:?} on the FROM table — expected unqualified, lake., or main."
    );
    let table = field_str(from_table, "table_name")?.to_string();
    anyhow::ensure!(
        table.to_lowercase().starts_with("sigiro_"),
        "federated queries may only target sigiro_* tables; got {table:?}"
    );
    let table_alias = from_table
        .get("alias")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // GROUP BY: plain grouping only (reject GROUPING SETS/ROLLUP/CUBE).
    let group_expressions = node
        .get("group_expressions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let group_sets = node
        .get("group_sets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !group_sets.is_empty() {
        let plain: Vec<i64> = (0..group_expressions.len() as i64).collect();
        let is_plain = group_sets.len() == 1
            && group_sets[0]
                .as_array()
                .map(|set| {
                    set.iter()
                        .filter_map(Value::as_i64)
                        .eq(plain.iter().copied())
                })
                .unwrap_or(false);
        anyhow::ensure!(
            is_plain,
            "GROUPING SETS / ROLLUP / CUBE are not federatable — only a plain GROUP BY is supported"
        );
    }
    let mut group_keys = Vec::new();
    for expr in &group_expressions {
        anyhow::ensure!(
            field_str(expr, "class")? == "COLUMN_REF",
            "GROUP BY keys must be plain columns in a federated query"
        );
        group_keys.push(column_ref_name(expr, &table, &table_alias)?);
    }

    // Modifiers: ORDER BY (column/alias only) and LIMIT (no OFFSET) only.
    let mut order_by = Vec::new();
    let mut limit = None;
    for modifier in node
        .get("modifiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match field_str(modifier, "type")? {
            "ORDER_MODIFIER" => {
                anyhow::ensure!(
                    order_by.is_empty(),
                    "multiple ORDER BY clauses are not supported"
                );
                for order in modifier
                    .get("orders")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let expression = field(order, "expression")?;
                    anyhow::ensure!(
                        field_str(expression, "class")? == "COLUMN_REF",
                        "ORDER BY must reference a plain column or a SELECT-list alias in a \
                         federated query (not an expression)"
                    );
                    let desc = field_str(order, "type")? == "DESCENDING";
                    let null_order = match field_str(order, "null_order")? {
                        "ORDER_DEFAULT" => NullOrder::Default,
                        "NULLS FIRST" => NullOrder::First,
                        "NULLS LAST" => NullOrder::Last,
                        other => anyhow::bail!(
                            "unsupported ORDER BY null-ordering in a federated query: {other}"
                        ),
                    };
                    order_by.push((
                        column_ref_name(expression, &table, &table_alias)?,
                        desc,
                        null_order,
                    ));
                }
            }
            "LIMIT_MODIFIER" => {
                anyhow::ensure!(limit.is_none(), "multiple LIMIT clauses are not supported");
                anyhow::ensure!(
                    is_null_field(modifier, "offset"),
                    "OFFSET is not supported in a federated query"
                );
                let limit_expr = field(modifier, "limit")?;
                anyhow::ensure!(
                    field_str(limit_expr, "class")? == "CONSTANT",
                    "LIMIT must be a literal integer in a federated query"
                );
                let n = limit_expr
                    .get("value")
                    .and_then(|v| v.get("value"))
                    .and_then(Value::as_i64)
                    .context("LIMIT literal is not an integer")?;
                limit = Some(n);
            }
            other => anyhow::bail!("unsupported query modifier in a federated query: {other}"),
        }
    }

    // SELECT list.
    let select_list = node
        .get("select_list")
        .and_then(Value::as_array)
        .context("SELECT_NODE missing select_list")?;
    let mut select_items = Vec::new();
    for item in select_list {
        match field_str(item, "class")? {
            "STAR" => select_items.push(SelectItem::Star),
            "COLUMN_REF" => {
                let name = column_ref_name(item, &table, &table_alias)?;
                let output = item
                    .get("alias")
                    .and_then(Value::as_str)
                    .filter(|a| !a.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| name.clone());
                select_items.push(SelectItem::Column(ColumnItem { name, output }));
            }
            "FUNCTION" => {
                anyhow::ensure!(
                    !item
                        .get("distinct")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    "DISTINCT aggregates (e.g. count(DISTINCT ...)) are not federatable across \
                     edges; run per-edge or narrow the query"
                );
                anyhow::ensure!(
                    is_null_field(item, "filter"),
                    "FILTER (WHERE ...) on an aggregate is not federatable"
                );
                let has_order_bys = item
                    .get("order_bys")
                    .and_then(|o| o.get("orders"))
                    .and_then(Value::as_array)
                    .is_some_and(|o| !o.is_empty());
                anyhow::ensure!(!has_order_bys, "ordered aggregates are not federatable");
                let function_name = field_str(item, "function_name")?.to_lowercase();
                let children = item
                    .get("children")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let (kind, arg) = match function_name.as_str() {
                    "count_star" => (AggKind::Count, None),
                    "count" => (
                        AggKind::Count,
                        Some(single_column_arg(&children, &table, &table_alias)?),
                    ),
                    "sum" => (
                        AggKind::Sum,
                        Some(single_column_arg(&children, &table, &table_alias)?),
                    ),
                    "min" => (
                        AggKind::Min,
                        Some(single_column_arg(&children, &table, &table_alias)?),
                    ),
                    "max" => (
                        AggKind::Max,
                        Some(single_column_arg(&children, &table, &table_alias)?),
                    ),
                    "avg" => (
                        AggKind::Avg,
                        Some(single_column_arg(&children, &table, &table_alias)?),
                    ),
                    other => anyhow::bail!(
                        "aggregate {other}(...) is not federatable across edges; only \
                         count/sum/min/max/avg are foldable. Run per-edge or narrow the query."
                    ),
                };
                let alias = item
                    .get("alias")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                anyhow::ensure!(
                    !alias.is_empty(),
                    "aggregate {function_name}(...) needs an explicit alias (e.g. AS total) to \
                     be usable in a federated query"
                );
                select_items.push(SelectItem::Aggregate(AggregateItem { kind, arg, alias }));
            }
            other => anyhow::bail!(
                "unsupported SELECT expression in a federated query (only plain columns, *, and \
                 count/sum/min/max/avg aggregates are supported): {other}"
            ),
        }
    }

    let is_star = select_items.iter().any(|i| matches!(i, SelectItem::Star));
    if is_star {
        anyhow::ensure!(
            select_items.len() == 1,
            "SELECT * cannot be combined with other select-list items in a federated query"
        );
        anyhow::ensure!(
            group_keys.is_empty(),
            "SELECT * cannot be combined with GROUP BY in a federated query"
        );
    }
    let is_aggregate_query = has_aggregate(&select_items);
    if is_aggregate_query {
        for item in &select_items {
            if let SelectItem::Column(col) = item {
                anyhow::ensure!(
                    group_keys.iter().any(|k| k.eq_ignore_ascii_case(&col.name)),
                    "column {:?} is selected alongside an aggregate but does not appear in \
                     GROUP BY — not federatable",
                    col.name
                );
            }
        }
    }

    // WHERE: deparse a narrow expression grammar, then check for a bound on
    // "timestamp" anywhere in the tree (the required time-bound rule).
    let where_clause = node.get("where_clause").filter(|v| !v.is_null());
    let where_sql = where_clause
        .map(|w| deparse_expr(w, &table, &table_alias))
        .transpose()?;
    let has_time_bound = where_clause.is_some_and(|w| contains_column_named(w, "timestamp"));

    // ORDER BY may reference a SELECT-list alias (e.g. `SELECT duration AS d
    // ... ORDER BY d`) rather than the real column — `json_serialize_sql`
    // runs pre-bind, so an alias reference and a real column reference parse
    // identically as a bare COLUMN_REF (same as the struct-field ambiguity
    // `column_ref_name` already documents). Rewrite any ORDER BY key that
    // matches a Column alias back to the real underlying column name: the
    // real name is the only thing that is ever a genuine column at the
    // per-edge push and in the coordinator fold's FROM scope — the alias is
    // applied exactly once, in the final SELECT list.
    let alias_to_col: std::collections::HashMap<String, String> = select_items
        .iter()
        .filter_map(|item| match item {
            SelectItem::Column(c) if !c.output.eq_ignore_ascii_case(&c.name) => {
                Some((c.output.to_lowercase(), c.name.clone()))
            }
            _ => None,
        })
        .collect();
    for (col, _, _) in &mut order_by {
        if let Some(real) = alias_to_col.get(&col.to_lowercase()) {
            *col = real.clone();
        }
    }

    Ok(ParsedQuery {
        table,
        where_sql,
        has_time_bound,
        group_keys,
        select_items,
        order_by,
        limit,
    })
}

fn single_column_arg(children: &[Value], table: &str, table_alias: &str) -> Result<String> {
    anyhow::ensure!(
        children.len() == 1,
        "aggregate argument must be a single plain column in a federated query"
    );
    anyhow::ensure!(
        field_str(&children[0], "class")? == "COLUMN_REF",
        "aggregate argument must be a plain column (not an expression) in a federated query"
    );
    column_ref_name(&children[0], table, table_alias)
}

/// Resolve a `COLUMN_REF` node's column name. `json_serialize_sql` represents
/// both `alias.column` (a genuine table qualifier) and `structcol.field`
/// (struct field access) identically as a multi-part `column_names` array —
/// it runs pre-bind, so it cannot tell them apart itself. Silently taking the
/// last part (as this used to) rebinds a struct access like
/// `attributes.status` to an unrelated top-level `status` column. So: a
/// multi-part reference is only accepted when the first part is the FROM
/// table's own name or alias (a real qualifier we can safely strip); anything
/// else — including genuine struct/nested field access — fails loud rather
/// than risk resolving to the wrong column.
fn column_ref_name(v: &Value, table: &str, table_alias: &str) -> Result<String> {
    let names = v
        .get("column_names")
        .and_then(Value::as_array)
        .context("COLUMN_REF missing column_names")?;
    anyhow::ensure!(!names.is_empty(), "COLUMN_REF has empty column_names");
    let names = names
        .iter()
        .map(|n| {
            n.as_str()
                .context("COLUMN_REF column_names entry is not a string")
        })
        .collect::<Result<Vec<&str>>>()?;
    if names.len() > 1 {
        let qualifier = names[0];
        let is_from_table_qualifier = qualifier.eq_ignore_ascii_case(table)
            || (!table_alias.is_empty() && qualifier.eq_ignore_ascii_case(table_alias));
        anyhow::ensure!(
            is_from_table_qualifier,
            "qualified/struct column reference {:?} is not federatable: {qualifier:?} is neither \
             the FROM table ({table:?}) nor its alias — struct field access and unrecognized \
             table qualifiers cannot be safely resolved before the query is bound",
            names.join(".")
        );
    }
    Ok((*names.last().expect("checked non-empty above")).to_string())
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value> {
    v.get(key)
        .with_context(|| format!("AST node missing expected field {key:?}"))
}

fn field_str<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    field(v, key)?
        .as_str()
        .with_context(|| format!("AST field {key:?} is not a string"))
}

fn is_null_field(v: &Value, key: &str) -> bool {
    v.get(key).is_none_or(Value::is_null)
}

/// A DELIBERATELY narrow WHERE-clause deparser: only column references,
/// literal constants, comparisons, and AND/OR conjunctions. Anything else
/// (function calls, CAST, IN, BETWEEN, arithmetic) fails loud rather than
/// risk mis-deparsing an expression this planner cannot prove correct.
fn deparse_expr(v: &Value, table: &str, table_alias: &str) -> Result<String> {
    match field_str(v, "class")? {
        "COLUMN_REF" => Ok(quote_ident(&column_ref_name(v, table, table_alias)?)),
        "CONSTANT" => deparse_constant(field(v, "value")?),
        "COMPARISON" => {
            let op = comparison_op(field_str(v, "type")?)?;
            let left = deparse_expr(field(v, "left")?, table, table_alias)?;
            let right = deparse_expr(field(v, "right")?, table, table_alias)?;
            Ok(format!("({left} {op} {right})"))
        }
        "CONJUNCTION" => {
            let op = match field_str(v, "type")? {
                "CONJUNCTION_AND" => "AND",
                "CONJUNCTION_OR" => "OR",
                other => anyhow::bail!("unsupported conjunction in WHERE clause: {other}"),
            };
            let children = v
                .get("children")
                .and_then(Value::as_array)
                .context("CONJUNCTION missing children")?;
            anyhow::ensure!(!children.is_empty(), "CONJUNCTION with no children");
            let parts = children
                .iter()
                .map(|c| deparse_expr(c, table, table_alias))
                .collect::<Result<Vec<_>>>()?;
            Ok(format!("({})", parts.join(&format!(" {op} "))))
        }
        other => anyhow::bail!(
            "unsupported WHERE clause expression in a federated query (only column/literal \
             comparisons combined with AND/OR are supported — no function calls, CAST, IN, \
             BETWEEN, or arithmetic): {other}"
        ),
    }
}

fn comparison_op(t: &str) -> Result<&'static str> {
    Ok(match t {
        "COMPARE_GREATERTHAN" => ">",
        "COMPARE_GREATERTHANOREQUALTO" => ">=",
        "COMPARE_LESSTHAN" => "<",
        "COMPARE_LESSTHANOREQUALTO" => "<=",
        "COMPARE_EQUAL" => "=",
        "COMPARE_NOTEQUAL" => "<>",
        other => anyhow::bail!("unsupported comparison operator in WHERE clause: {other}"),
    })
}

fn deparse_constant(value: &Value) -> Result<String> {
    if value
        .get("is_null")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok("NULL".to_string());
    }
    let type_obj = value.get("type").context("CONSTANT missing type")?;
    let type_id = type_obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("<missing type id>");
    let raw = value.get("value").context("CONSTANT missing value")?;
    match type_id {
        "VARCHAR" => {
            let s = raw.as_str().context("VARCHAR constant is not a string")?;
            Ok(format!("'{}'", crate::storage::escape_sql_string(s)))
        }
        "BOOLEAN" => Ok(if raw.as_bool().unwrap_or(false) {
            "TRUE"
        } else {
            "FALSE"
        }
        .to_string()),
        "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "UTINYINT" | "USMALLINT" | "UINTEGER"
        | "UBIGINT" | "DOUBLE" | "FLOAT" => Ok(raw.to_string()),
        // DECIMAL is serialized as an unscaled mantissa (`value`) + a separate
        // `type_info.scale` — e.g. the literal `1.5` comes back as
        // `{"type_info":{"scale":1,...},"value":15}`. Emitting the raw
        // mantissa (as the old code did) reproduces `15`, ten times too big.
        // Reconstruct the actual decimal text from mantissa + scale using
        // integer string manipulation only (no float involved, so no
        // rounding risk).
        "DECIMAL" => {
            let type_info = type_obj
                .get("type_info")
                .context("DECIMAL constant missing type_info")?;
            deparse_decimal(raw, type_info)
        }
        // HUGEINT/UHUGEINT (128-bit) constants are serialized as a
        // `{"upper":..,"lower":..}` object, not a JSON number — splicing that
        // object's Display/to_string() into SQL text (as the old code did via
        // the shared `raw.to_string()` arm) produces garbage SQL, not a wrong
        // number. Fail loud instead of ever doing that.
        "HUGEINT" | "UHUGEINT" => anyhow::bail!(
            "unsupported WHERE literal type {type_id}: 128-bit integer literals are serialized \
             by DuckDB as a {{upper,lower}} object rather than a plain number and cannot be \
             safely deparsed into SQL text by this planner — narrow the literal to fit a 64-bit \
             integer"
        ),
        other => anyhow::bail!("unsupported WHERE literal type: {other}"),
    }
}

/// Reconstruct a DECIMAL literal's exact text from its unscaled mantissa and
/// scale (e.g. mantissa `155`, scale `1` -> `"15.5"`). Pure integer/string
/// arithmetic — never round-trips through a float, so it cannot introduce the
/// rounding error a `mantissa as f64 / 10f64.powi(scale)` approach could.
fn deparse_decimal(raw: &Value, type_info: &Value) -> Result<String> {
    let scale = type_info
        .get("scale")
        .and_then(Value::as_i64)
        .context("DECIMAL type_info missing scale")?;
    anyhow::ensure!(
        scale >= 0,
        "DECIMAL literal has an unsupported negative scale {scale}"
    );
    let mantissa = raw.as_i64().with_context(|| {
        format!(
            "DECIMAL literal mantissa {raw:?} does not fit in a 64-bit integer — not federatable"
        )
    })?;
    if scale == 0 {
        return Ok(mantissa.to_string());
    }
    let scale = scale as usize;
    let negative = mantissa < 0;
    let digits = mantissa.unsigned_abs().to_string();
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (int_part, frac_part) = digits.split_at(digits.len() - scale);
    Ok(format!(
        "{}{int_part}.{frac_part}",
        if negative { "-" } else { "" }
    ))
}

/// Generic recursive scan of the (already-validated) WHERE AST for any
/// `COLUMN_REF` naming `target` (case-insensitive), anywhere in the tree.
/// Intentionally not tied to the narrow `deparse_expr` grammar — the required
/// time-bound check should hold regardless of how it's expressed.
fn contains_column_named(v: &Value, target: &str) -> bool {
    match v {
        Value::Object(map) => {
            if map.get("class").and_then(Value::as_str) == Some("COLUMN_REF")
                && map
                    .get("column_names")
                    .and_then(Value::as_array)
                    .is_some_and(|names| {
                        names
                            .iter()
                            .any(|n| n.as_str().is_some_and(|s| s.eq_ignore_ascii_case(target)))
                    })
            {
                return true;
            }
            map.values().any(|val| contains_column_named(val, target))
        }
        Value::Array(arr) => arr.iter().any(|val| contains_column_named(val, target)),
        _ => false,
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn sanitize_ident(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

// ---------------------------------------------------------------------------
// PARTIAL (per-edge) / FINAL (coordinator fold) SQL construction.
// ---------------------------------------------------------------------------

/// Per-edge additive partial columns for one aggregate, keyed by its
/// position (`idx`) in the SELECT list so names never collide.
fn partial_cols(idx: usize, agg: &AggregateItem) -> Vec<(String, String)> {
    let arg = agg.arg.as_deref().map(quote_ident);
    match agg.kind {
        AggKind::Count => {
            let expr = match &arg {
                Some(a) => format!("count({a})"),
                None => "count(*)".to_string(),
            };
            vec![(format!("_p{idx}_c"), expr)]
        }
        AggKind::Sum => vec![(
            format!("_p{idx}_s"),
            format!("sum({})", arg.expect("sum has an arg")),
        )],
        AggKind::Min => vec![(
            format!("_p{idx}_mn"),
            format!("min({})", arg.expect("min has an arg")),
        )],
        AggKind::Max => vec![(
            format!("_p{idx}_mx"),
            format!("max({})", arg.expect("max has an arg")),
        )],
        AggKind::Avg => {
            let a = arg.expect("avg has an arg");
            vec![
                (format!("_p{idx}_s"), format!("sum({a})")),
                (format!("_p{idx}_n"), format!("count({a})")),
            ]
        }
    }
}

/// Coordinator-side fold of the additive partials back into the user's
/// original aggregate, under the user's original alias. `avg` is
/// `sum(partial sums) / sum(partial counts)` — never avg-of-avgs.
fn final_expr(idx: usize, agg: &AggregateItem) -> String {
    let alias = quote_ident(&agg.alias);
    match agg.kind {
        AggKind::Count => format!("sum({}) AS {alias}", quote_ident(&format!("_p{idx}_c"))),
        AggKind::Sum => format!("sum({}) AS {alias}", quote_ident(&format!("_p{idx}_s"))),
        AggKind::Min => format!("min({}) AS {alias}", quote_ident(&format!("_p{idx}_mn"))),
        AggKind::Max => format!("max({}) AS {alias}", quote_ident(&format!("_p{idx}_mx"))),
        AggKind::Avg => format!(
            "sum({})::DOUBLE / NULLIF(sum({}), 0) AS {alias}",
            quote_ident(&format!("_p{idx}_s")),
            quote_ident(&format!("_p{idx}_n"))
        ),
    }
}

/// Render an `order_by` list as the SQL fragment that follows `ORDER BY `
/// (no leading keyword), shared by the per-edge push (`build_partial_sql`)
/// and the coordinator's final fold (`build_final_sql`) so both places order
/// identically — including `NULLS FIRST`/`NULLS LAST` when the user wrote it.
fn order_by_sql(order_by: &[(String, bool, NullOrder)]) -> String {
    order_by
        .iter()
        .map(|(col, desc, nulls)| {
            format!(
                "{} {}{}",
                quote_ident(col),
                if *desc { "DESC" } else { "ASC" },
                nulls.to_sql()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// For a non-aggregate passthrough, a user `LIMIT n` beyond `row_cap` gets
/// pushed to each edge as `LIMIT min(n, row_cap)` (see `effective_edge_cap`)
/// — which means the edge can never return more than `row_cap` rows, so the
/// `cap+1` overflow sentinel (see `build_partial_sql`/`fetch_edge`) can never
/// trip. Left unchecked, that silently returns `row_cap` rows instead of the
/// `n` the user asked for. Fail loud instead, before that pushdown happens.
fn check_limit_within_cap(parsed: &ParsedQuery, row_cap: usize) -> Result<()> {
    if parsed.group_keys.is_empty() && !parsed.is_aggregate() {
        if let Some(n) = parsed.limit {
            anyhow::ensure!(
                n <= row_cap as i64,
                "LIMIT {n} exceeds SIGIRO_QUERY_ROW_CAP = {row_cap}; narrow the query or raise \
                 the cap"
            );
        }
    }
    Ok(())
}

/// The per-edge row cap actually enforced for this query. For a pure
/// passthrough SELECT (no GROUP BY, no aggregate) with a user LIMIT n, the
/// edge is capped at `min(n, row_cap)` — the user's own LIMIT already bounds
/// it. Every other shape (GROUP BY complete groups, full-table aggregates,
/// top-N before the fold) is capped at the full `row_cap` safety net, since
/// no per-edge LIMIT can be pushed down without breaking correctness.
fn effective_edge_cap(parsed: &ParsedQuery, row_cap: usize) -> i64 {
    let cap = row_cap as i64;
    if parsed.group_keys.is_empty() && !parsed.is_aggregate() {
        parsed.limit.map_or(cap, |n| n.min(cap))
    } else {
        cap
    }
}

/// For a passthrough top-N (`ORDER BY ... LIMIT n`, no GROUP BY/aggregate),
/// an ORDER BY key that isn't itself in the SELECT list still needs to reach
/// the coordinator so the fold can sort by it (e.g. `SELECT trace_id ...
/// ORDER BY duration`) — the coordinator only ever sees columns the per-edge
/// partial actually projected. Returns the (deduplicated) extra real column
/// names to add to that projection; the final SELECT list is built purely
/// from `parsed.select_items`, so these never leak into the query's output.
fn passthrough_extra_order_cols(parsed: &ParsedQuery) -> Vec<String> {
    if !parsed.group_keys.is_empty() || parsed.is_aggregate() {
        return Vec::new();
    }
    if parsed
        .select_items
        .iter()
        .any(|i| matches!(i, SelectItem::Star))
    {
        return Vec::new();
    }
    let selected: std::collections::HashSet<String> = parsed
        .select_items
        .iter()
        .filter_map(|i| match i {
            SelectItem::Column(c) => Some(c.name.to_lowercase()),
            _ => None,
        })
        .collect();
    let mut extra = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, _, _) in &parsed.order_by {
        let key = name.to_lowercase();
        if !selected.contains(&key) && seen.insert(key) {
            extra.push(name.clone());
        }
    }
    extra
}

/// Build the SQL sent to each edge via `quack_query`. Always wrapped in an
/// outer `LIMIT {cap}+1` so a result of exactly `cap+1` rows is a
/// distinguishable "this edge blew past the cap" signal (see `fetch_edge`).
fn build_partial_sql(parsed: &ParsedQuery, row_cap: usize) -> String {
    let effective_cap = effective_edge_cap(parsed, row_cap);
    let mut select_parts = Vec::new();
    for (idx, item) in parsed.select_items.iter().enumerate() {
        match item {
            SelectItem::Star => select_parts.push("*".to_string()),
            SelectItem::Column(col) => select_parts.push(quote_ident(&col.name)),
            SelectItem::Aggregate(agg) => {
                for (col, expr) in partial_cols(idx, agg) {
                    select_parts.push(format!("{expr} AS {}", quote_ident(&col)));
                }
            }
        }
    }
    for col in passthrough_extra_order_cols(parsed) {
        select_parts.push(quote_ident(&col));
    }
    // Qualify the table with the DuckLake catalog (`lake`). The edge's quack
    // server runs each request in a fresh connection with NO `USE lake`
    // default catalog, so an unqualified name fails at the edge ("Table
    // sigiro_spans does not exist! Did you mean lake.sigiro_spans?"). The
    // final fold (build_final_sql) runs on local materialized tables, so it
    // is not qualified.
    let mut inner = format!(
        "SELECT {} FROM {}.{}",
        select_parts.join(", "),
        quote_ident("lake"),
        quote_ident(&parsed.table)
    );
    if let Some(where_sql) = &parsed.where_sql {
        inner.push_str(&format!(" WHERE {where_sql}"));
    }
    if !parsed.group_keys.is_empty() {
        let keys: Vec<String> = parsed.group_keys.iter().map(|k| quote_ident(k)).collect();
        inner.push_str(&format!(" GROUP BY {}", keys.join(", ")));
    }
    // Passthrough with a user ORDER BY / LIMIT: push both down. A passthrough
    // top-N (`ORDER BY ... LIMIT n`) needs a per-edge ORDER BY too, not just
    // the LIMIT — without it, each edge's LIMIT n keeps n ARBITRARY
    // scan-order rows, and the coordinator's final sort-then-limit is then
    // sorting garbage, not the true global top-N. GROUP BY/aggregate shapes
    // need COMPLETE per-edge groups, so neither is pushed for those — the
    // user's ORDER BY/LIMIT is applied once, after the fold, in
    // `build_final_sql`.
    if parsed.group_keys.is_empty() && !parsed.is_aggregate() {
        if !parsed.order_by.is_empty() {
            inner.push_str(&format!(" ORDER BY {}", order_by_sql(&parsed.order_by)));
        }
        if let Some(n) = parsed.limit {
            inner.push_str(&format!(" LIMIT {}", n.min(effective_cap)));
        }
    }
    format!(
        "SELECT * FROM ({inner}) sigiro_partial LIMIT {}",
        effective_cap + 1
    )
}

/// Build the coordinator's fold query over the unioned per-edge partial
/// tables: re-GROUP BY the same keys, fold each aggregate exactly once, then
/// apply the user's original ORDER BY / LIMIT AFTER the fold (never before —
/// that's what makes top-N correct across edges).
fn build_final_sql(parsed: &ParsedQuery, part_tables: &[String]) -> String {
    let union_sql: Vec<String> = part_tables
        .iter()
        .map(|t| format!("SELECT * FROM {}", quote_ident(t)))
        .collect();
    let mut select_parts = Vec::new();
    for (idx, item) in parsed.select_items.iter().enumerate() {
        match item {
            SelectItem::Star => select_parts.push("*".to_string()),
            SelectItem::Column(col) => select_parts.push(format!(
                "{} AS {}",
                quote_ident(&col.name),
                quote_ident(&col.output)
            )),
            SelectItem::Aggregate(agg) => select_parts.push(final_expr(idx, agg)),
        }
    }
    let mut sql = format!(
        "SELECT {} FROM ({}) sigiro_union",
        select_parts.join(", "),
        union_sql.join(" UNION ALL ")
    );
    if !parsed.group_keys.is_empty() {
        let keys: Vec<String> = parsed.group_keys.iter().map(|k| quote_ident(k)).collect();
        sql.push_str(&format!(" GROUP BY {}", keys.join(", ")));
    }
    if !parsed.order_by.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order_by_sql(&parsed.order_by)));
    }
    if let Some(n) = parsed.limit {
        sql.push_str(&format!(" LIMIT {n}"));
    }
    sql
}

// ---------------------------------------------------------------------------
// Scatter / gather orchestration.
// ---------------------------------------------------------------------------

/// Wrap `inner_sql` in the `quack_query(...)` remote-exec call for `edge`.
/// Mirrors `tests/quack_query_surface_test.rs`'s `wrap_quack_query` — the
/// documented client pattern (`quack_query`, not `ATTACH`; see module docs).
fn wrap_quack_query(edge: &Edge, inner_sql: &str, token: Option<&str>) -> String {
    let mut sql = format!(
        "SELECT * FROM quack_query('quack://{}:{}', '{}'",
        edge.host,
        edge.port,
        crate::storage::escape_sql_string(inner_sql)
    );
    if let Some(t) = token {
        sql.push_str(&format!(
            ", token := '{}'",
            crate::storage::escape_sql_string(t)
        ));
    }
    sql.push_str(", disable_ssl := true);");
    sql
}

/// Fetch one edge's partial result. Runs on its own throwaway in-memory
/// DuckDB connection (never shared/cloned — see module docs on why TEMP
/// tables can't be the transport). Returns `Ok((edge_name, None))` when the
/// edge legitimately contributed zero rows (e.g. a GROUP BY partial with no
/// matching groups returns zero Arrow batches — verified directly — so
/// there is no schema to bridge and nothing to materialize).
async fn fetch_edge(
    edge: Edge,
    partial_sql: String,
    token: Option<String>,
    timeout: Duration,
    effective_cap: i64,
) -> Result<(String, Option<RecordBatch>), (String, String)> {
    let wrapped = wrap_quack_query(&edge, &partial_sql, token.as_deref());
    let edge_name = edge.name;
    let log_name = edge_name.clone();
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel::<Arc<duckdb::InterruptHandle>>();

    let mut task = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<RecordBatch>> {
        let conn = duckdb::Connection::open_in_memory()
            .context("failed to open embedded DuckDB connection for edge fetch")?;
        conn.execute_batch("LOAD quack;")
            .context("failed to load the quack extension for the edge fetch connection")?;
        if handle_tx.send(conn.interrupt_handle()).is_err() {
            tracing::debug!(edge = %log_name, "edge fetch timeout watchdog receiver dropped before send");
        }
        let mut stmt = conn
            .prepare(&wrapped)
            .context("failed to prepare the quack_query call")?;
        let batches: Vec<RecordBatch> = stmt
            .query_arrow([])
            .context(
                "quack_query call failed (edge unreachable, auth token rejected, or the edge's \
                 own authorization gate rejected this SQL)",
            )?
            .collect();
        Ok(batches)
    });

    let joined: anyhow::Result<Vec<RecordBatch>> = tokio::select! {
        r = &mut task => r.map_err(|e| anyhow::anyhow!("edge fetch task panicked: {e}")).and_then(|inner| inner),
        () = tokio::time::sleep(timeout) => {
            if let Ok(handle) = handle_rx.await {
                handle.interrupt();
            }
            (&mut task)
                .await
                .map_err(|e| anyhow::anyhow!("edge fetch task panicked: {e}"))
                .and_then(|inner| inner)
                .with_context(|| format!("exceeded the {timeout:?} per-edge timeout"))
        }
    };

    match joined {
        Ok(batches) => {
            let total_rows: i64 = batches.iter().map(|b| b.num_rows() as i64).sum();
            if total_rows > effective_cap {
                return Err((
                    edge_name,
                    format!(
                        "returned {total_rows} rows, exceeding the per-edge row cap of \
                         {effective_cap} — query is not sufficiently pruned/bounded; narrow the \
                         WHERE clause or pass --full-scan if this is intended"
                    ),
                ));
            }
            if batches.is_empty() {
                return Ok((edge_name, None));
            }
            let schema = batches[0].schema();
            match arrow::compute::concat_batches(&schema, &batches) {
                Ok(merged) => Ok((edge_name, Some(merged))),
                Err(e) => Err((
                    edge_name,
                    format!("failed to concatenate the edge's result batches: {e}"),
                )),
            }
        }
        Err(e) => Err((edge_name, format!("{e:#}"))),
    }
}

/// Run `sql` as a federated cross-edge query: resolve edges, plan the
/// query, scatter the per-edge partial to each edge concurrently, bridge
/// every edge's Arrow result into one coordinator DuckDB connection, and
/// fold. Fails loud (naming every offending edge) if any edge errors, times
/// out, or is unreachable — no partial/degraded success mode (that's the
/// documented `--partial` nice-to-have, not implemented here; see the Phase
/// C report).
pub async fn run_federated_query(
    sql: &str,
    cli_edges: &[String],
    allow_full_scan: bool,
) -> Result<Vec<Value>> {
    let edges = resolve_edges(cli_edges)?;
    tracing::info!(
        edges = ?edges.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        "resolved federated query edges"
    );

    let coord = duckdb::Connection::open_in_memory()
        .context("failed to open the embedded coordinator connection")?;
    coord
        .execute_batch("INSTALL json; LOAD json;")
        .context("failed to load the json extension needed to plan the federated query")?;

    let parsed = plan_query(&coord, sql)?;
    anyhow::ensure!(
        parsed.has_time_bound || allow_full_scan,
        "federated query has no bound on the timestamp column in its WHERE clause — this is the \
         un-prunable cross-edge blowup case. Add a WHERE timestamp filter, or pass --full-scan \
         to run anyway."
    );

    let row_cap = crate::config::query_row_cap()?;
    check_limit_within_cap(&parsed, row_cap)?;
    let effective_cap = effective_edge_cap(&parsed, row_cap);
    let timeout = Duration::from_secs(crate::config::query_timeout_secs()?);
    let raw_token = crate::config::QUACK_TOKEN
        .value()
        .context("failed to read SIGIRO_QUACK_TOKEN")?;
    let token = (!raw_token.is_empty()).then_some(raw_token);
    let partial_sql = build_partial_sql(&parsed, row_cap);
    tracing::debug!(partial_sql, "federated query per-edge partial");

    let mut set = tokio::task::JoinSet::new();
    for edge in &edges {
        set.spawn(fetch_edge(
            edge.clone(),
            partial_sql.clone(),
            token.clone(),
            timeout,
            effective_cap,
        ));
    }

    let mut edge_batches: Vec<(String, RecordBatch)> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok((name, Some(batch)))) => edge_batches.push((name, batch)),
            Ok(Ok((_name, None))) => {}
            Ok(Err((name, msg))) => failures.push((name, msg)),
            Err(join_err) => failures.push((
                "<unknown edge>".to_string(),
                format!("task panicked: {join_err}"),
            )),
        }
    }

    anyhow::ensure!(
        failures.is_empty(),
        "federated query failed — {} of {} edge(s) unreachable/errored:\n{}",
        failures.len(),
        edges.len(),
        failures
            .iter()
            .map(|(name, msg)| format!("  - {name}: {msg}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if edge_batches.is_empty() {
        tracing::info!("federated query: every edge returned zero rows");
        return Ok(Vec::new());
    }

    coord
        .register_table_function::<duckdb::vtab::arrow::ArrowVTab>("arrow")
        .context(
            "failed to register the Arrow bridge table function on the coordinator connection",
        )?;

    let mut part_tables = Vec::new();
    for (idx, (edge_name, batch)) in edge_batches.into_iter().enumerate() {
        let table = format!("part_{idx}_{}", sanitize_ident(&edge_name));
        let params = duckdb::vtab::arrow::arrow_recordbatch_to_query_params(batch);
        coord
            .execute(
                &format!(
                    "CREATE TEMP TABLE {} AS SELECT * FROM arrow(?, ?)",
                    quote_ident(&table)
                ),
                params,
            )
            .with_context(|| {
                format!("failed to materialize edge {edge_name}'s result into the coordinator")
            })?;
        part_tables.push(table);
    }

    let final_sql = build_final_sql(&parsed, &part_tables);
    tracing::debug!(final_sql, "federated query coordinator fold");
    let mut stmt = coord
        .prepare(&final_sql)
        .context("failed to prepare the federated coordinator fold query")?;
    let mut rows = stmt
        .query([])
        .context("failed to execute the federated coordinator fold query")?;
    let column_names: Vec<String> = rows
        .as_ref()
        .map(duckdb::Statement::column_names)
        .unwrap_or_default();
    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .context("failed to read a coordinator fold result row")?
    {
        let mut obj = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row
                .get_ref(i)
                .context("failed to read a coordinator fold result column")?;
            obj.insert(name.clone(), crate::server::duckdb_value_to_json(val));
        }
        results.push(Value::Object(obj));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn type_info(scale: i64) -> Value {
        json!({ "scale": scale })
    }

    /// `deparse_decimal` reconstructs the exact literal text from DuckDB's
    /// unscaled-mantissa + scale serialization. The E2E's `duration > 15.5`
    /// assertion only proves the boundary lands somewhere between 15 and 16
    /// — it can't tell "15.5" from a truncated "15" (a 10x-scale bug that
    /// coincidentally still excludes 15 for other reasons). This asserts the
    /// produced SQL literal text directly.
    #[test]
    fn deparse_decimal_reconstructs_exact_literal() {
        assert_eq!(deparse_decimal(&json!(15), &type_info(1)).unwrap(), "1.5");
        assert_eq!(deparse_decimal(&json!(5), &type_info(2)).unwrap(), "0.05");
        assert_eq!(deparse_decimal(&json!(-15), &type_info(1)).unwrap(), "-1.5");
        assert_eq!(deparse_decimal(&json!(150), &type_info(2)).unwrap(), "1.50");
        assert_eq!(deparse_decimal(&json!(42), &type_info(0)).unwrap(), "42");
    }

    /// A mantissa that doesn't fit in a 64-bit integer (e.g. a
    /// HUGEINT-backed DECIMAL) must fail loud, never silently
    /// truncate/wrap/misrender.
    #[test]
    fn deparse_decimal_bails_on_oversized_mantissa() {
        let oversized = json!(u64::MAX);
        let err = deparse_decimal(&oversized, &type_info(2)).unwrap_err();
        assert!(
            err.to_string().contains("does not fit"),
            "expected a 'does not fit' error, got: {err}"
        );
    }
}

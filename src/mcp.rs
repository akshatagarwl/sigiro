use anyhow::{Context, Result};
use duckdb::Connection;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::Implementation;
use rmcp::schemars;
use rmcp::{ServerHandler, ServiceExt as _, tool, tool_router};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::investigate;

pub struct SigiroMcp {
    conn: Arc<Mutex<Connection>>,
    table_prefix: &'static str,
    tool_router: ToolRouter<Self>,
}

/// Parameters for investigating a single service.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct InvestigateParams {
    /// Service name to investigate
    pub service: String,
    /// Start timestamp in epoch microseconds (defaults to now - 15m)
    pub from_ts: Option<i64>,
    /// End timestamp in epoch microseconds (defaults to now)
    pub to_ts: Option<i64>,
}

/// Parameters for investigating multiple services at once.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct InvestigateMultiParams {
    /// Service names to investigate (max 10)
    pub services: Vec<String>,
    /// Start timestamp in epoch microseconds (defaults to now - 15m)
    pub from_ts: Option<i64>,
    /// End timestamp in epoch microseconds (defaults to now)
    pub to_ts: Option<i64>,
}

/// Parameters for running a read-only SQL query against the DuckDB database.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// SQL query (read-only, SELECT only)
    pub sql: String,
}

#[derive(Serialize)]
struct StatusResponse {
    services: Vec<String>,
    tables: std::collections::BTreeMap<String, i64>,
}

#[tool_router]
impl SigiroMcp {
    /// Investigate a service — returns structured metrics, spans, logs, linked traces,
    /// operation summaries with baselines, coverage gaps, service context (topology),
    /// and derived events (deploys, K8s events).
    #[tool]
    async fn sigiro_investigate(&self, params: Parameters<InvestigateParams>) -> String {
        let p = params.0;
        let req = investigate::InvestigateRequest {
            service: p.service,
            from_ts: p.from_ts,
            to_ts: p.to_ts,
        };
        match investigate::investigate(&self.conn, &req, self.table_prefix) {
            Ok(block) => serde_json::to_string_pretty(&block)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e:#}"),
        }
    }

    /// Investigate multiple services in one call (max 10). Returns a map of service name
    /// to investigation block. Use this for incident topology — investigate all related
    /// services at once instead of making N separate calls.
    #[tool]
    async fn sigiro_investigate_multi(&self, params: Parameters<InvestigateMultiParams>) -> String {
        let p = params.0;
        let req = investigate::InvestigateMultiRequest {
            services: p.services,
            from_ts: p.from_ts,
            to_ts: p.to_ts,
        };
        match investigate::investigate_multi(&self.conn, &req, self.table_prefix) {
            Ok(blocks) => serde_json::to_string_pretty(&blocks)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e:#}"),
        }
    }

    /// Run a read-only SQL query against the DuckDB database. Only SELECT statements
    /// are allowed. Use this for ad-hoc exploration when the structured investigation
    /// doesn't cover your question.
    #[tool]
    async fn sigiro_query(&self, params: Parameters<QueryParams>) -> String {
        let sql = &params.0.sql;
        if let Err(e) = crate::server::validate_read_only(sql) {
            return format!("error: {e}");
        }
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(e) => return format!("error: connection mutex poisoned: {e}"),
        };
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => return format!("error: {e}"),
        };
        let mut rows = match stmt.query([]) {
            Ok(r) => r,
            Err(e) => return format!("error: {e}"),
        };
        let stmt_ref = match rows.as_ref() {
            Some(s) => s,
            None => return "error: no result set".to_string(),
        };
        let column_count = stmt_ref.column_count();
        let column_names: Vec<String> = (0..column_count)
            .map(|i| {
                stmt_ref
                    .column_name(i)
                    .map(ToString::to_string)
                    .unwrap_or_else(|_| "?".to_string())
            })
            .collect();

        let mut results = Vec::new();
        while let Ok(Some(row)) = rows.next() {
            let mut obj = serde_json::Map::new();
            for (i, name) in column_names.iter().enumerate() {
                if let Ok(val) = row.get_ref(i) {
                    obj.insert(name.clone(), crate::server::value_ref_to_json(val));
                }
            }
            results.push(serde_json::Value::Object(obj));
        }
        serde_json::to_string_pretty(&results)
            .unwrap_or_else(|e| format!("serialization error: {e}"))
    }

    /// Get sigiro status — services seen and row counts per table. Quick health check.
    #[tool]
    async fn sigiro_status(&self) -> String {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(e) => return format!("error: connection mutex poisoned: {e}"),
        };

        let prefix = self.table_prefix;
        let mut table_counts = std::collections::BTreeMap::new();
        for table in crate::storage::TABLE_NAMES {
            let qualified = format!("{prefix}{table}");
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {qualified}"), [], |row| {
                    row.get(0)
                })
                .unwrap_or(0);
            table_counts.insert(qualified, count);
        }

        let mut services = Vec::new();
        if let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT DISTINCT service_name FROM {prefix}sigiro_spans
             UNION SELECT DISTINCT service_name FROM {prefix}sigiro_logs
             UNION SELECT DISTINCT service_name FROM {prefix}sigiro_metrics_gauge",
        )) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(s) = row.get::<_, String>(0) {
                        services.push(s);
                    }
                }
            }
        }

        let resp = StatusResponse {
            services,
            tables: table_counts,
        };
        serde_json::to_string_pretty(&resp).unwrap_or_else(|e| format!("serialization error: {e}"))
    }
}

#[rmcp::tool_handler]
impl ServerHandler for SigiroMcp {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::default()
            .with_server_info(
                Implementation::new("sigiro", env!("CARGO_PKG_VERSION"))
                    .with_title("Sigiro — observability for AI agents"),
            )
            .with_instructions("Sigiro is an observability backend for AI agents. Use sigiro_investigate to analyze a service, sigiro_investigate_multi for multi-service incidents, sigiro_query for ad-hoc SQL, and sigiro_status for a health check.")
    }
}

impl SigiroMcp {
    pub fn new(conn: Arc<Mutex<Connection>>, table_prefix: &'static str) -> Self {
        Self {
            conn,
            table_prefix,
            tool_router: Self::tool_router(),
        }
    }
}

/// Run the MCP stdio server. Blocks until the client disconnects.
pub async fn run_mcp_server(
    conn: Arc<Mutex<Connection>>,
    table_prefix: &'static str,
) -> Result<()> {
    let server = SigiroMcp::new(conn, table_prefix);
    let transport = rmcp::transport::io::stdio();
    let service = server
        .serve(transport)
        .await
        .context("MCP server failed to start")?;
    service.waiting().await.context("MCP server error")?;
    Ok(())
}

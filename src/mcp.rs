use anyhow::{Context, Result};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::Implementation;
use rmcp::schemars;
use rmcp::{ServerHandler, ServiceExt as _, tool, tool_router};
use serde::Deserialize;

use crate::client::SigiroClient;

/// Serialize a value to a JSON string, or return a formatted error on failure.
/// Consistent error format across all MCP tool responses.
fn serialize_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|e| format!("internal error: failed to serialize response: {e}"))
}

/// Format an error as a string, consistent across all MCP tool responses.
fn format_error(e: &anyhow::Error) -> String {
    format!("error: {e:#}")
}

pub struct SigiroMcp {
    client: SigiroClient,
    tool_router: ToolRouter<Self>,
}

/// Parameters for investigating one or more services.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct InvestigateParams {
    /// Single service name to investigate
    pub service: Option<String>,
    /// Multiple service names to investigate (max 10). Use for incident topology.
    pub services: Option<Vec<String>>,
    /// Start timestamp in epoch microseconds (required)
    pub from_ts: i64,
    /// End timestamp in epoch microseconds (required)
    pub to_ts: i64,
}

impl From<InvestigateParams> for crate::investigate::InvestigateRequest {
    fn from(p: InvestigateParams) -> Self {
        crate::investigate::InvestigateRequest {
            service: p.service,
            services: p.services,
            from_ts: p.from_ts,
            to_ts: p.to_ts,
        }
    }
}

/// Parameters for running a read-only SQL query against the DuckDB database.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// SQL query (read-only, SELECT only)
    pub sql: String,
}

#[tool_router]
impl SigiroMcp {
    /// Investigate one or more services — returns a map of service name to investigation block.
    /// Pass `service` for a single service or `services` for multi-service incidents (max 10).
    /// Each block contains structured metrics, spans, logs, linked traces,
    /// operation summaries with baselines, coverage gaps, service context (topology),
    /// and derived events (deploys, K8s events).
    #[tool]
    async fn sigiro_investigate(&self, params: Parameters<InvestigateParams>) -> String {
        let req = crate::investigate::InvestigateRequest::from(params.0);
        match self
            .client
            .investigate_multi(req.service.as_deref(), req.services, req.from_ts, req.to_ts)
            .await
        {
            Ok(blocks) => serialize_json(&blocks),
            Err(e) => format_error(&e),
        }
    }

    /// Run a read-only SQL query against the DuckDB database. Only SELECT statements
    /// are allowed. Use this for ad-hoc exploration when the structured investigation
    /// doesn't cover your question.
    #[tool]
    async fn sigiro_query(&self, params: Parameters<QueryParams>) -> String {
        match self.client.query(&params.0.sql).await {
            Ok(rows) => serialize_json(&rows),
            Err(e) => format_error(&e),
        }
    }

    /// Get sigiro status — services seen and row counts per table. Quick health check.
    #[tool]
    async fn sigiro_status(&self) -> String {
        match self.client.services().await {
            Ok(val) => serialize_json(&val),
            Err(e) => format_error(&e),
        }
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
            .with_instructions("Sigiro is an observability backend for AI agents. Use sigiro_investigate to analyze one or more services (pass 'service' for one, 'services' for multiple), sigiro_query for ad-hoc SQL, and sigiro_status for a health check.")
    }
}

impl SigiroMcp {
    pub fn new(client: SigiroClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }
}

/// Run the MCP stdio server. Blocks until the client disconnects.
pub async fn run_mcp_server(client: SigiroClient) -> Result<()> {
    let server = SigiroMcp::new(client);
    let transport = rmcp::transport::io::stdio();
    let service = server
        .serve(transport)
        .await
        .context("MCP server failed to start")?;
    service.waiting().await.context("MCP server error")?;
    Ok(())
}

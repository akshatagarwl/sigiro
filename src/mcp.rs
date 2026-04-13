use anyhow::{Context, Result};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::Implementation;
use rmcp::schemars;
use rmcp::{ServerHandler, ServiceExt as _, tool, tool_router};
use serde::Deserialize;

use crate::client::SigiroClient;

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

#[tool_router]
impl SigiroMcp {
    /// Investigate one or more services — returns a map of service name to investigation block.
    /// Pass `service` for a single service or `services` for multi-service incidents (max 10).
    /// Each block contains structured metrics, spans, logs, linked traces,
    /// operation summaries with baselines, coverage gaps, service context (topology),
    /// and derived events (deploys, K8s events).
    #[tool]
    async fn sigiro_investigate(&self, params: Parameters<InvestigateParams>) -> String {
        let p = params.0;
        match self
            .client
            .investigate_multi(p.service.as_deref(), p.services, p.from_ts, p.to_ts)
            .await
        {
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
        match self.client.query(&params.0.sql).await {
            Ok(rows) => serde_json::to_string_pretty(&rows)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e:#}"),
        }
    }

    /// Get sigiro status — services seen and row counts per table. Quick health check.
    #[tool]
    async fn sigiro_status(&self) -> String {
        match self.client.services().await {
            Ok(val) => serde_json::to_string_pretty(&val)
                .unwrap_or_else(|e| format!("serialization error: {e}")),
            Err(e) => format!("error: {e:#}"),
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

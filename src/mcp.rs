//! MCP (Model Context Protocol) server — exposes sigiro's agent loop as tools.
//!
//! This is the same surface `/v1` already publishes, spoken over MCP so any
//! agent client (Claude Desktop, Cursor, Letta, a benchmark harness like
//! SREGym) can drive the detect→diagnose→drill loop without a bespoke
//! adapter.
//!
//! Every tool delegates to [`SigiroClient`], the same HTTP client the CLI
//! uses. There is no second copy of the query logic, the window defaulting,
//! or the correlation rules: a tool call and a `sigiro` subcommand hit the
//! identical server code path.
//!
//! Tool descriptions are the agent's only documentation, so they carry the
//! same wording as the `#[utoipa::path]` descriptions in `server.rs`.

use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use std::sync::Arc;

use crate::client::SigiroClient;

/// Serialize any successful response to pretty JSON, matching the CLI's
/// "one format, so a human and a pipeline read the same bytes" rule.
fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(value).map_err(|e| {
        ErrorData::internal_error(format!("failed to serialize response: {e}"), None)
    })?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// An upstream failure is reported as a tool error, never swallowed: the
/// agent must be able to tell "no anomalies" from "the server is down".
fn upstream_error(context: &str, e: &anyhow::Error) -> ErrorData {
    ErrorData::internal_error(format!("{context}: {e:#}"), None)
}

/// Tool argument structs.
///
/// `#[schemars(crate = "rmcp::schemars")]` is load-bearing. The `JsonSchema`
/// derive expands to a BARE `schemars::` path, so without it the macro looks
/// for a `schemars` crate in our dependency list and fails to compile —
/// re-exporting through `rmcp::schemars` alone does not satisfy the expansion.
/// The attribute points the derive at rmcp's re-export instead, which keeps
/// `schemars` out of our Cargo.toml and prevents a second, independently
/// versioned copy from drifting away from the one rmcp actually uses.
#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AnomaliesArgs {
    /// Optional service name to filter to. Omit for every service.
    pub(crate) service: Option<String>,
    /// Only shifts at or after this time, MICROSECONDS since the Unix epoch
    /// (a 16-digit value, e.g. 1780000000000000).
    pub(crate) from_ts: Option<i64>,
    /// Only shifts at or before this time, microseconds since the Unix epoch.
    pub(crate) to_ts: Option<i64>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DiagnoseArgs {
    /// Service to diagnose. Call `list_services` first to discover valid names.
    pub(crate) service: String,
    /// Window start, MICROSECONDS since the Unix epoch (a 16-digit value,
    /// e.g. 1780000000000000). Seconds, millis, and nanos are rejected.
    /// Omit both bounds for the last 15 minutes.
    pub(crate) from_ts: Option<i64>,
    /// Window end, microseconds since the Unix epoch. Must be > `from_ts`.
    /// Providing this without `from_ts` is rejected.
    pub(crate) to_ts: Option<i64>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct QueryArgs {
    /// The SQL to execute. Paste a `drill_down_sql` or `drill_all_sql` string
    /// VERBATIM from a `list_anomalies` or `diagnose_service` response.
    pub(crate) sql: String,
}

/// The MCP tool surface over a running sigiro server.
#[derive(Clone)]
pub struct SigiroMcp {
    client: SigiroClient,
    tool_router: ToolRouter<Self>,
}

type DocsService = RunningService<rmcp::RoleClient, ()>;

/// Connect to Blume's canonical docs MCP and copy only its advertised tool
/// metadata into the local router. The implementation and schemas remain
/// upstream-owned; this process is only a protocol adapter.
pub async fn connect_docs() -> anyhow::Result<(Arc<DocsService>, Vec<Tool>)> {
    let url = crate::config::DOCS_MCP_URL.value()?;
    connect_docs_at(url).await
}

/// Connect to a specific documentation MCP endpoint. Kept separate from the
/// environment-backed entry point so callers and tests can inject an endpoint.
pub async fn connect_docs_at(
    url: impl Into<String>,
) -> anyhow::Result<(Arc<DocsService>, Vec<Tool>)> {
    let transport = StreamableHttpClientTransport::from_uri(url.into());
    let service = ().serve(transport).await?;
    let tools = service.list_all_tools().await?;
    Ok((Arc::new(service), tools))
}

#[tool_router]
impl SigiroMcp {
    pub fn new(client: SigiroClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    pub fn with_docs(mut self, docs: Arc<DocsService>, tools: Vec<Tool>) -> anyhow::Result<Self> {
        for tool in tools {
            anyhow::ensure!(
                !self.tool_router.map.contains_key(&tool.name),
                "documentation MCP tool name collides with an operational tool: {}",
                tool.name
            );
            let name = tool.name.clone();
            let upstream = Arc::clone(&docs);
            let route = ToolRoute::new_dyn(tool, move |context| {
                let upstream = Arc::clone(&upstream);
                let name = name.clone();
                Box::pin(async move {
                    let params = CallToolRequestParams::new(name)
                        .with_arguments(context.arguments.unwrap_or_default());
                    match upstream.call_tool(params).await {
                        Ok(result) => Ok(result.into()),
                        Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                            "documentation MCP unavailable: {error}"
                        ))])
                        .into()),
                    }
                })
            });
            let route_name = route.name().to_owned();
            self.tool_router.map.insert(route_name.into(), route);
        }
        Ok(self)
    }

    #[tool(
        description = "Every service that has reported telemetry, with the first and last \
timestamp seen for each (microseconds since the Unix epoch). The entry point of an agent \
loop: pick a service here, then diagnose it."
    )]
    async fn list_services(&self) -> Result<CallToolResult, ErrorData> {
        let response = self
            .client
            .services()
            .await
            .map_err(|e| upstream_error("list_services failed", &e))?;
        json_result(&response)
    }

    #[tool(
        description = "Precomputed anomalies: worsening regime shifts (BOCPD changepoints, \
threshold-free) detected continuously over persisted 5-minute series of every signal — \
error_rate and latency_p95 per operation, log_volume, error_log_rate and profile_cost per \
service. Ordered by the unit-free shift magnitude (after - before) / (after + before) \
descending (1.0 = appeared from a zero baseline, 1/3 = doubled), so the most significant \
shifts lead regardless of the signal's units — nothing is filtered out; judge significance \
from `before`/`after` (raw signal units: %, µs, logs, profile cost). Each row carries a \
ready-to-run `drill_down_sql` for the `run_query` tool. Rows whose shifts fall in the same \
or adjacent 5-minute detection bucket on the same service share an `incident_id` — one \
incident across signals, not N independent anomalies; `incident_id` is null for \
uncorrelated rows."
    )]
    async fn list_anomalies(
        &self,
        Parameters(args): Parameters<AnomaliesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let resp = self
            .client
            .anomalies(args.service.as_deref(), args.from_ts, args.to_ts)
            .await
            .map_err(|e| upstream_error("list_anomalies failed", &e))?;
        json_result(&resp)
    }

    #[tool(
        description = "Root-cause an incident for one service over a time window. Returns a \
triage summary — NOT raw telemetry: a ranked `findings` array (read this first), \
per-operation anomaly flags, and bounded samples of spans/logs. Every lead carries a \
ready-to-run SQL string (`drill_down_sql` on findings, operations, spans, and log patterns; \
`drill_all_sql` on the spans/logs sections) that you paste VERBATIM into the `run_query` \
tool to fetch the underlying rows and go deeper. Typical agent loop: `list_services` → \
`diagnose_service` → for any finding or anomalous operation, run its `drill_down_sql` via \
`run_query`. Timestamps are microseconds since the Unix epoch."
    )]
    async fn diagnose_service(
        &self,
        Parameters(args): Parameters<DiagnoseArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let block = self
            .client
            .diagnose(&args.service, args.from_ts, args.to_ts)
            .await
            .map_err(|e| upstream_error("diagnose_service failed", &e))?;
        json_result(&block)
    }

    #[tool(
        description = "Execute SQL against the telemetry store and return the rows. This is \
the drill-down step: pass a `drill_down_sql` or `drill_all_sql` string VERBATIM from a \
`list_anomalies` or `diagnose_service` response to fetch the underlying rows behind a \
finding. Timestamps in the data are microseconds since the Unix epoch."
    )]
    async fn run_query(
        &self,
        Parameters(args): Parameters<QueryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let rows = self
            .client
            .query(&args.sql)
            .await
            .map_err(|e| upstream_error("run_query failed", &e))?;
        json_result(&rows)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SigiroMcp {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo is #[non_exhaustive]: build from Default and set fields,
        // so an added upstream field is a recompile rather than a break.
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "Observability tools over a sigiro server. Start with `list_anomalies` to \
find what changed, then `diagnose_service` on the affected service, then `run_query` with \
the `drill_down_sql` each response hands you. All timestamps are MICROSECONDS since the \
Unix epoch."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

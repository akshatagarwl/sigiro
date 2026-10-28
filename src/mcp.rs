//! MCP (Model Context Protocol) server — exposes service discovery and
//! tenant-scoped, read-only telemetry queries to agent clients.
//!
//! Every operational tool delegates to [`SigiroClient`], the same HTTP
//! client the CLI uses.
//!
//! Tool descriptions are the agent's only documentation, so they carry the
//! same wording as the `#[utoipa::path]` descriptions in `server.rs`.

use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ServerCapabilities, ServerConfig, Tool,
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

/// An upstream failure is reported as a tool error, never swallowed.
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
pub struct QueryArgs {
    /// Read-only SQL against allowlisted raw telemetry tables.
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

    pub fn with_docs(mut self, docs: &Arc<DocsService>, tools: Vec<Tool>) -> anyhow::Result<Self> {
        for tool in tools {
            anyhow::ensure!(
                !self.tool_router.map.contains_key(&tool.name),
                "documentation MCP tool name collides with an operational tool: {}",
                tool.name
            );
            let name = tool.name.clone();
            let upstream = Arc::clone(docs);
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
loop: pick a service here, then query its telemetry. Fails rather than returning a partial list \
if service count exceeds SIGIRO_MAX_RESULT_ROWS."
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
        description = "Execute read-only SQL against allowlisted telemetry tables. \
Timestamps in the data are microseconds since the Unix epoch."
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
    fn get_info(&self) -> ServerConfig {
        // ServerConfig is #[non_exhaustive]: build from Default and set fields,
        // so an added upstream field is a recompile rather than a break.
        let mut info = ServerConfig::default();
        info.instructions = Some(
            "Observability tools over a sigiro server. Use `list_services` to find a \
service, then `run_query` to inspect its raw telemetry. All timestamps are \
MICROSECONDS since the Unix epoch."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

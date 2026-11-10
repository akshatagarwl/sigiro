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
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::client::SigiroClient;
use crate::docs_mcp::OfflineDocs;

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

/// How long the TCP (and TLS) connect to the hosted docs MCP may take.
const DOCS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long connecting, the MCP handshake and `tools/list` may take together
/// before `sigiro mcp` starts on the built-in docs instead.
const DOCS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one proxied documentation call may take before it is answered
/// from the built-in docs instead.
const DOCS_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Connect to Blume's canonical docs MCP and copy only its advertised tool
/// metadata into the local router. The implementation and schemas remain
/// upstream-owned; this process is only a protocol adapter. An empty
/// `SIGIRO_DOCS_MCP_URL` means "no hosted docs": this fails at once, and
/// `sigiro mcp` answers from the built-in docs.
pub async fn connect_docs() -> anyhow::Result<(Arc<DocsService>, Vec<Tool>)> {
    let url = crate::config::DOCS_MCP_URL.value()?;
    anyhow::ensure!(
        !url.trim().is_empty(),
        "SIGIRO_DOCS_MCP_URL is empty, so no hosted documentation MCP is used"
    );
    connect_docs_at(url).await
}

/// Connect to a specific documentation MCP endpoint. Kept separate from the
/// environment-backed entry point so callers and tests can inject an endpoint.
/// Gives up after [`DOCS_DISCOVERY_TIMEOUT`], so an unreachable or silent
/// endpoint costs `sigiro mcp` seconds, not the OS's TCP timeout.
pub async fn connect_docs_at(
    url: impl Into<String>,
) -> anyhow::Result<(Arc<DocsService>, Vec<Tool>)> {
    let url = url.into();
    let transport = StreamableHttpClientTransport::with_client(
        docs_http_client()?,
        StreamableHttpClientTransportConfig::with_uri(url.clone()),
    );
    let discover = async {
        let service = ().serve(transport).await?;
        let tools = service.list_all_tools().await?;
        anyhow::Ok((Arc::new(service), tools))
    };
    tokio::time::timeout(DOCS_DISCOVERY_TIMEOUT, discover)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "no answer from {url} within {}s",
                DOCS_DISCOVERY_TIMEOUT.as_secs()
            )
        })?
}

/// rmcp's default client with a connect timeout added: no idle pooling, and
/// only same-origin 307/308 redirects followed, which keep the POST body.
/// No request timeout, since the server-to-client SSE stream is long-lived;
/// each call is bounded by [`DOCS_CALL_TIMEOUT`] instead.
fn docs_http_client() -> anyhow::Result<reqwest::Client> {
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        let same_origin = attempt
            .previous()
            .last()
            .is_some_and(|from| from.origin() == attempt.url().origin());
        let keeps_body = matches!(attempt.status().as_u16(), 307 | 308);
        if same_origin && keeps_body && attempt.previous().len() <= 10 {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    Ok(reqwest::Client::builder()
        .connect_timeout(DOCS_CONNECT_TIMEOUT)
        .pool_max_idle_per_host(0)
        .redirect(redirects)
        .build()?)
}

/// Why a documentation call was not answered by the hosted endpoint.
fn hosted_failure(
    answer: Result<Result<CallToolResult, rmcp::ServiceError>, tokio::time::error::Elapsed>,
) -> Result<CallToolResult, String> {
    match answer {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("no answer within {}s", DOCS_CALL_TIMEOUT.as_secs())),
    }
}

#[tool_router]
impl SigiroMcp {
    pub fn new(client: SigiroClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    /// Proxy the hosted documentation tools, with no fallback: a failed call
    /// is a tool error.
    pub fn with_docs(self, docs: &Arc<DocsService>, tools: Vec<Tool>) -> anyhow::Result<Self> {
        self.with_hosted_docs(docs, tools, None)
    }

    /// Proxy the hosted documentation tools. A call the hosted endpoint does
    /// not answer — transport error, protocol error, or no answer within
    /// [`DOCS_CALL_TIMEOUT`] — is answered from `offline` when that knows the
    /// tool, and from then on every call goes to `offline` directly: a
    /// session whose endpoint failed once does not wait on it again.
    pub fn with_hosted_docs(
        mut self,
        docs: &Arc<DocsService>,
        tools: Vec<Tool>,
        offline: Option<&Arc<OfflineDocs>>,
    ) -> anyhow::Result<Self> {
        let hosted_failed = Arc::new(AtomicBool::new(false));
        for tool in tools {
            let name = tool.name.clone();
            let upstream = Arc::clone(docs);
            let offline = offline.filter(|offline| offline.has_tool(&name)).cloned();
            let hosted_failed = Arc::clone(&hosted_failed);
            self.add_docs_tool(ToolRoute::new_dyn(tool, move |context| {
                let upstream = Arc::clone(&upstream);
                let offline = offline.clone();
                let hosted_failed = Arc::clone(&hosted_failed);
                let name = name.clone();
                Box::pin(async move {
                    let arguments = context.arguments.unwrap_or_default();
                    if let Some(offline) = &offline
                        && hosted_failed.load(Ordering::Relaxed)
                    {
                        return Ok(offline.call(&name, &arguments).into());
                    }
                    let params = CallToolRequestParams::new(name.clone()).with_arguments(arguments.clone());
                    let answer =
                        tokio::time::timeout(DOCS_CALL_TIMEOUT, upstream.call_tool(params)).await;
                    match (hosted_failure(answer), offline) {
                        (Ok(result), _) => Ok(result.into()),
                        (Err(error), Some(offline)) => {
                            if !hosted_failed.swap(true, Ordering::Relaxed) {
                                tracing::warn!(
                                    "documentation MCP failed ({error}); answering from the built-in docs"
                                );
                            }
                            Ok(offline.call(&name, &arguments).into())
                        }
                        (Err(error), None) => Ok(CallToolResult::error(vec![ContentBlock::text(
                            format!("documentation MCP unavailable: {error}"),
                        )])
                        .into()),
                    }
                })
            }))?;
        }
        Ok(self)
    }

    /// Answer the documentation tools from the docs built into the binary:
    /// the same tool names, schemas and answers as the hosted docs MCP, with
    /// no network.
    pub fn with_offline_docs(mut self, offline: &Arc<OfflineDocs>) -> anyhow::Result<Self> {
        for tool in offline.tools() {
            let name = tool.name.clone();
            let offline = Arc::clone(offline);
            self.add_docs_tool(ToolRoute::new_dyn(tool, move |context| {
                let result = offline.call(&name, &context.arguments.unwrap_or_default());
                Box::pin(std::future::ready(Ok(result.into())))
            }))?;
        }
        Ok(self)
    }

    fn add_docs_tool(&mut self, route: ToolRoute<Self>) -> anyhow::Result<()> {
        let name = route.name().to_owned();
        anyhow::ensure!(
            !self.tool_router.map.contains_key(name.as_str()),
            "documentation MCP tool name collides with an operational tool: {name}"
        );
        self.tool_router.map.insert(name.into(), route);
        Ok(())
    }

    #[tool(
        description = "Every service that has reported telemetry, with the first and last \
timestamp seen for each (UTC text such as `2026-01-01 12:00:00.123`). The entry point of an agent \
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
`timestamp` is a TIMESTAMP column (UTC): bound it with `timestamp > now() - INTERVAL '1 hour'` \
or `TIMESTAMP '2026-01-01 00:00:00'`, never an integer. Result rows render timestamps as \
microseconds since the Unix epoch, and `duration` is in microseconds."
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
service, then `run_query` to inspect its raw telemetry. Bound every query by \
`timestamp` with TIMESTAMP values (`now() - INTERVAL '1 hour'`); comparing it to an \
integer fails. Query results render timestamps, and `duration`, in MICROSECONDS."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

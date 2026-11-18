use axum::{Router, middleware, response::Response};
use parking_lot::Mutex;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig,
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
    },
};
use serde::Deserialize;
use sigiro::{
    client::{ClientAuth, SigiroClient},
    mcp::SigiroMcp,
};
use std::{collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    query: String,
}

#[derive(Clone)]
struct DocsFixture {
    tool_router: ToolRouter<Self>,
}

impl DocsFixture {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl DocsFixture {
    #[tool(description = "Search the documentation")]
    async fn search_docs(&self, Parameters(args): Parameters<SearchArgs>) -> String {
        format!("search result for {}", args.query)
    }

    #[tool(description = "Get a documentation page")]
    async fn get_page(&self, Parameters(args): Parameters<SearchArgs>) -> String {
        format!("page for {}", args.query)
    }

    #[tool(description = "List documentation pages")]
    async fn list_pages(&self) -> String {
        "pages".to_string()
    }

    #[tool(description = "Get documentation navigation")]
    async fn get_navigation(&self) -> Result<String, rmcp::ErrorData> {
        Err(rmcp::ErrorData::internal_error(
            "fixture navigation failure",
            None,
        ))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DocsFixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

async fn capture_headers(
    axum::extract::State(captured): axum::extract::State<Arc<Mutex<Vec<axum::http::HeaderMap>>>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    captured.lock().push(request.headers().clone());
    next.run(request).await
}

async fn spawn_docs_fixture() -> (
    String,
    CancellationToken,
    tokio::task::JoinHandle<()>,
    Arc<Mutex<Vec<axum::http::HeaderMap>>>,
) {
    let cancellation = CancellationToken::new();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let service = StreamableHttpService::new(
        || Ok(DocsFixture::new()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .with_cancellation_token(cancellation.child_token()),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let router =
        Router::new()
            .nest_service("/mcp/docs", service)
            .layer(middleware::from_fn_with_state(
                captured.clone(),
                capture_headers,
            ));
    let handle = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(cancellation.cancelled_owned())
                .await
                .expect("fixture server");
        }
    });
    (
        format!("http://{address}/mcp/docs"),
        cancellation,
        handle,
        captured,
    )
}

async fn stdio_exercise(
    service: SigiroMcp,
) -> (
    Vec<rmcp::model::Tool>,
    rmcp::model::CallToolResult,
    rmcp::model::CallToolResult,
) {
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        service.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });
    let client = ().serve(client_transport).await.expect("stdio client");
    let tools = client.list_all_tools().await.expect("tools/list");
    let success = client
        .call_tool(
            rmcp::model::CallToolRequestParams::new("search_docs").with_arguments(
                serde_json::json!({"query": "ducklake"})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .expect("proxy search_docs");
    let failure = client
        .call_tool(rmcp::model::CallToolRequestParams::new("get_navigation"))
        .await
        .expect("isolated proxy failure result");
    client.cancel().await.expect("cancel stdio client");
    server
        .await
        .expect("stdio server join")
        .expect("stdio server");
    (tools, success, failure)
}

#[tokio::test]
async fn stdio_composition_discovers_proxies_and_isolates_docs() -> anyhow::Result<()> {
    let (url, cancellation, fixture, captured) = spawn_docs_fixture().await;
    let (docs, docs_tools) = sigiro::mcp::connect_docs_at(url).await?;
    assert_eq!(docs_tools.len(), 4);
    let local = SigiroClient::new("http://127.0.0.1:1", ClientAuth::Open);
    let composed = SigiroMcp::new(local).with_docs(&docs, docs_tools)?;

    let (tools, result, failure) = stdio_exercise(composed).await;
    let names: BTreeSet<_> = tools
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "search_docs".to_string(),
            "get_page".to_string(),
            "list_pages".to_string(),
            "get_navigation".to_string(),
            "list_services".to_string(),
            "run_query".to_string(),
        ])
    );

    assert_eq!(
        result.content[0].as_text().expect("text").text,
        "search result for ducklake"
    );
    assert_eq!(failure.is_error, Some(true));

    let headers = captured.lock().clone();
    assert!(
        !headers
            .iter()
            .any(|headers| headers.contains_key("authorization") || headers.contains_key("cookie"))
    );

    Arc::try_unwrap(docs)
        .map_err(|_| anyhow::anyhow!("docs client still referenced"))?
        .cancel()
        .await?;
    cancellation.cancel();
    fixture.await?;
    Ok(())
}

#[tokio::test]
async fn docs_collision_and_discovery_failure_are_deterministic() -> anyhow::Result<()> {
    let (url, cancellation, fixture, _) = spawn_docs_fixture().await;
    let (docs, _) = sigiro::mcp::connect_docs_at(url).await?;
    let local = SigiroClient::new("http://127.0.0.1:1", ClientAuth::Open);
    let collision = SigiroMcp::new(local).with_docs(
        &docs,
        vec![rmcp::model::Tool::new(
            "list_services",
            "collision",
            serde_json::Map::new(),
        )],
    );
    let error = match collision {
        Ok(_) => anyhow::bail!("operational collision must fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("collides with an operational tool"));

    let failure = sigiro::mcp::connect_docs_at("http://127.0.0.1:1/mcp/docs").await;
    assert!(
        failure.is_err(),
        "unreachable docs endpoint must be explicit"
    );
    Arc::try_unwrap(docs)
        .map_err(|_| anyhow::anyhow!("docs client still referenced"))?
        .cancel()
        .await?;
    cancellation.cancel();
    fixture.await?;
    Ok(())
}

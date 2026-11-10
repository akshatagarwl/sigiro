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

fn offline_docs() -> Arc<sigiro::docs_mcp::OfflineDocs> {
    Arc::new(
        sigiro::docs_mcp::OfflineDocs::from_json(include_bytes!("fixtures/docs-mcp/mcp-data.json"))
            .expect("fixture snapshot"),
    )
}

fn search(query: &str) -> rmcp::model::CallToolRequestParams {
    rmcp::model::CallToolRequestParams::new("search_docs").with_arguments(
        serde_json::json!({ "query": query, "limit": 2 })
            .as_object()
            .cloned()
            .unwrap(),
    )
}

/// The routes of a `search_docs` answer.
fn routes(result: &rmcp::model::CallToolResult) -> Vec<String> {
    let text = &result.content[0].as_text().expect("text").text;
    let hits: Vec<serde_json::Value> = serde_json::from_str(text).expect("search json");
    hits.iter()
        .map(|hit| hit["route"].as_str().expect("route").to_string())
        .collect()
}

/// Serve `service` over an in-process stdio pipe and run `calls` against it.
async fn call_over_stdio(
    service: SigiroMcp,
    calls: Vec<rmcp::model::CallToolRequestParams>,
) -> (Vec<rmcp::model::Tool>, Vec<rmcp::model::CallToolResult>) {
    let (server_transport, client_transport) = tokio::io::duplex(256 * 1024);
    let server = tokio::spawn(async move {
        service.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });
    let client = ().serve(client_transport).await.expect("stdio client");
    let tools = client.list_all_tools().await.expect("tools/list");
    let mut results = Vec::new();
    for call in calls {
        results.push(client.call_tool(call).await.expect("tools/call"));
    }
    client.cancel().await.expect("cancel stdio client");
    server
        .await
        .expect("stdio server join")
        .expect("stdio server");
    (tools, results)
}

/// No network: the hosted endpoint is a TEST-NET-1 address (RFC 5737), which
/// is never routed, so the connect either fails at once (no route) or hangs
/// until the connect timeout. Discovery gives up within seconds, and the
/// built-in docs answer the same four tools.
#[tokio::test]
async fn unreachable_docs_fall_back_to_the_built_in_docs() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let failure = sigiro::mcp::connect_docs_at("http://192.0.2.1/mcp/docs").await;
    assert!(failure.is_err(), "an unroutable endpoint cannot connect");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "discovery must give up quickly, took {:?}",
        started.elapsed()
    );

    let offline = offline_docs();
    let local = SigiroClient::new("http://127.0.0.1:1", ClientAuth::Open);
    let service = SigiroMcp::new(local).with_offline_docs(&offline)?;
    let (tools, results) = call_over_stdio(service, vec![search("executor")]).await;

    // The hosted tool definitions, verbatim, next to the operational tools.
    let advertised: Vec<_> = tools
        .iter()
        .filter(|tool| offline.has_tool(&tool.name))
        .cloned()
        .collect();
    let mut expected = offline.tools();
    expected.sort_by(|a, b| a.name.cmp(&b.name));
    let mut advertised = advertised;
    advertised.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(advertised, expected);
    assert_eq!(tools.len(), 6);
    assert_eq!(results[0].is_error, None);
    assert_eq!(routes(&results[0])[0], "/docs/how-to/executor");
    Ok(())
}

/// An endpoint that accepts the connection but never answers is given up on
/// after the discovery timeout, not the OS's.
#[tokio::test]
async fn silent_docs_endpoint_times_out() -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    // Accept and hold connections without reading or answering.
    let holder = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let started = std::time::Instant::now();
    let failure = sigiro::mcp::connect_docs_at(format!("http://{address}/mcp/docs")).await;
    let Err(error) = failure else {
        anyhow::bail!("a silent endpoint must fail");
    };
    let error = error.to_string();
    assert!(error.contains("no answer"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
    holder.abort();
    Ok(())
}

/// The hosted endpoint answers while it is up; once it goes away mid-session,
/// the same tools answer from the built-in docs instead of failing.
#[tokio::test]
async fn hosted_docs_failing_mid_session_fall_back() -> anyhow::Result<()> {
    let (url, cancellation, fixture, _) = spawn_docs_fixture().await;
    let (docs, docs_tools) = sigiro::mcp::connect_docs_at(url).await?;
    let local = SigiroClient::new("http://127.0.0.1:1", ClientAuth::Open);
    let service =
        SigiroMcp::new(local).with_hosted_docs(&docs, docs_tools, Some(&offline_docs()))?;

    let (server_transport, client_transport) = tokio::io::duplex(256 * 1024);
    let server = tokio::spawn(async move {
        service.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });
    let client = ().serve(client_transport).await?;

    let hosted = client.call_tool(search("executor")).await?;
    assert_eq!(
        hosted.content[0].as_text().expect("text").text,
        "search result for executor"
    );

    cancellation.cancel();
    fixture.await?;
    let fallback = client.call_tool(search("executor")).await?;
    assert_eq!(fallback.is_error, None);
    assert_eq!(routes(&fallback)[0], "/docs/how-to/executor");

    client.cancel().await?;
    server.await??;
    Ok(())
}

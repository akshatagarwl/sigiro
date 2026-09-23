//! Real self-hosted server: exercise query HTTP, OTLP HTTP, and OTLP gRPC sockets.
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, trace_service_client::TraceServiceClient,
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::{ExportLogsServiceRequest, logs_service_client::LogsServiceClient},
    metrics::v1::{ExportMetricsServiceRequest, metrics_service_client::MetricsServiceClient},
    profiles::v1development::{
        ExportProfilesServiceRequest, profiles_service_client::ProfilesServiceClient,
    },
};
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tonic::{Code, Request};

const SECRET: &str = "self-hosted-e2e-secret-0123456789abcdef";
const WRONG: &str = "self-hosted-e2e-secret-0123456789abcdeg";
const HTTP: u16 = 45881;
const OTLP: u16 = 45882;
const GRPC: u16 = 45883;

struct Server {
    child: Child,
    _dir: tempfile::TempDir,
}
impl Server {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
            .arg("serve")
            .env("SIGIRO_DATA_DIR", dir.path())
            .env("SIGIRO_STATIC_BEARER_TOKEN", SECRET)
            .env("SIGIRO_HTTP_PORT", HTTP.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", OTLP.to_string())
            .env("SIGIRO_GRPC_PORT", GRPC.to_string())
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env_remove("SIGIRO_AUTH_ISSUER")
            .env_remove("SIGIRO_AUTH_AUDIENCE")
            .env_remove("SIGIRO_AUTH_JWKS_URL")
            .env_remove("SIGIRO_CATALOG_URL")
            .env_remove("SIGIRO_QUERY_SURFACE")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut server = Self { child, _dir: dir };
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if reqwest::get(format!("http://127.0.0.1:{HTTP}/health"))
                .await
                .is_ok()
            {
                return server;
            }
            if let Some(status) = server.child.try_wait().unwrap() {
                let mut stderr = String::new();
                server
                    .child
                    .stderr
                    .as_mut()
                    .unwrap()
                    .read_to_string(&mut stderr)
                    .unwrap();
                assert!(!stderr.contains(SECRET), "startup leaked secret");
                panic!("server exited before ready ({status}): {stderr}");
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        panic!("self-hosted server never became ready");
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn static_bearer_on_all_three_listeners() {
    let _server = Server::start().await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("http://127.0.0.1:{HTTP}/health"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for (port, path, method) in [
        (HTTP, "/v1/services", "GET"),
        (HTTP, "/v1/query", "POST"),
        (OTLP, "/v1/traces", "POST"),
        (OTLP, "/v1/logs", "POST"),
        (OTLP, "/v1/metrics", "POST"),
        (OTLP, "/v1development/profiles", "POST"),
        (OTLP, "/push.v1.PusherService/Push", "POST"),
    ] {
        let url = format!("http://127.0.0.1:{port}{path}");
        for (header, expected) in [
            (None, 401),
            (Some(format!("Bearer {WRONG}")), 401),
            (Some(format!("AgentAuth {SECRET}")), 401),
            (Some(format!("bearer {SECRET}")), 401),
            (Some(format!("Bearer {SECRET}x")), 401),
            (Some(format!("Bearer {SECRET}")), 200),
        ] {
            // Valid empty OTLP traces/logs/metrics payloads are no-ops; profiles and
            // Pyroscope validate bodies after auth, so accept any non-401 response.
            let request = if method == "GET" {
                client.get(&url)
            } else {
                client
                    .post(&url)
                    .body(if port == HTTP { "SELECT 1" } else { "" })
            };
            let request = if let Some(header) = header {
                request.header("Authorization", header)
            } else {
                request
            };
            let response = request.send().await.unwrap();
            if expected == 200
                && port == OTLP
                && (path.contains("profiles") || path.contains("Push"))
            {
                assert_ne!(response.status().as_u16(), 401, "{path} valid token");
            } else {
                assert_eq!(response.status().as_u16(), expected, "{path} header case");
            }
            let body = response.text().await.unwrap();
            assert!(!body.contains(SECRET), "response leaked secret");
        }
    }
    let mut grpc = TraceServiceClient::connect(format!("http://127.0.0.1:{GRPC}"))
        .await
        .unwrap();
    for (header, expected) in [
        (None, Code::Unauthenticated),
        (Some(format!("Bearer {WRONG}")), Code::Unauthenticated),
        (Some(format!("AgentAuth {SECRET}")), Code::Unauthenticated),
        (Some(format!("Bearer {SECRET}")), Code::Ok),
    ] {
        let mut request = Request::new(ExportTraceServiceRequest::default());
        if let Some(header) = header {
            request
                .metadata_mut()
                .insert("authorization", header.parse().unwrap());
        }
        let result = grpc.export(request).await;
        if expected == Code::Ok {
            assert!(result.is_ok(), "valid gRPC bearer: {result:?}");
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code(), expected);
            assert!(!error.message().contains(SECRET));
        }
    }
    let mut logs = LogsServiceClient::connect(format!("http://127.0.0.1:{GRPC}"))
        .await
        .unwrap();
    let mut metrics = MetricsServiceClient::connect(format!("http://127.0.0.1:{GRPC}"))
        .await
        .unwrap();
    let mut profiles = ProfilesServiceClient::connect(format!("http://127.0.0.1:{GRPC}"))
        .await
        .unwrap();
    for (authorization, authorized) in [
        (None, false),
        (Some(format!("Bearer {WRONG}")), false),
        (Some(format!("AgentAuth {SECRET}")), false),
        (Some(format!("Bearer {SECRET}")), true),
    ] {
        let mut log_request = Request::new(ExportLogsServiceRequest::default());
        let mut metric_request = Request::new(ExportMetricsServiceRequest::default());
        let mut profile_request = Request::new(ExportProfilesServiceRequest::default());
        if let Some(header) = authorization {
            for metadata in [
                log_request.metadata_mut(),
                metric_request.metadata_mut(),
                profile_request.metadata_mut(),
            ] {
                metadata.insert("authorization", header.parse().unwrap());
            }
        }
        let log_result = logs.export(log_request).await;
        let metric_result = metrics.export(metric_request).await;
        let profile_result = profiles.export(profile_request).await;
        if authorized {
            assert!(log_result.is_ok(), "valid gRPC logs bearer: {log_result:?}");
            assert!(metric_result.is_ok());
            assert!(
                profile_result.is_ok(),
                "valid gRPC profile bearer: {profile_result:?}"
            );
        } else {
            assert_eq!(log_result.unwrap_err().code(), Code::Unauthenticated);
            assert_eq!(metric_result.unwrap_err().code(), Code::Unauthenticated);
            assert_eq!(profile_result.unwrap_err().code(), Code::Unauthenticated);
        }
    }
}

#[test]
fn misconfiguration_fails_without_disclosing_secret() {
    for (token, name, value, expected) in [
        ("", "", "", "at least 32 ASCII bytes"),
        ("short", "", "", "at least 32 ASCII bytes"),
        (
            SECRET,
            "SIGIRO_AUTH_ISSUER",
            "",
            "cannot be combined with JWT",
        ),
        (
            SECRET,
            "SIGIRO_AUTH_AUDIENCE",
            "https://api.example.test",
            "cannot be combined with JWT",
        ),
        (
            SECRET,
            "SIGIRO_AUTH_JWKS_URL",
            "",
            "cannot be combined with JWT",
        ),
        (
            SECRET,
            "SIGIRO_QUERY_SURFACE",
            "quack",
            "requires SIGIRO_QUERY_SURFACE=none",
        ),
        (
            SECRET,
            "SIGIRO_CATALOG_URL",
            "postgres://localhost/sigiro",
            "requires local storage",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sigiro"));
        command
            .arg("serve")
            .env("SIGIRO_DATA_DIR", dir.path())
            .env("SIGIRO_STATIC_BEARER_TOKEN", token)
            .env_remove("SIGIRO_AUTH_ISSUER")
            .env_remove("SIGIRO_AUTH_AUDIENCE")
            .env_remove("SIGIRO_AUTH_JWKS_URL")
            .env_remove("SIGIRO_CATALOG_URL")
            .env_remove("SIGIRO_QUERY_SURFACE");
        if !name.is_empty() {
            command.env(name, value);
        }
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "{name} with token length {} started",
            token.len()
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "unexpected startup failure for {name}: {stderr}"
        );
        if !token.is_empty() {
            assert!(!stderr.contains(token), "secret leaked in startup error");
        }
    }
}

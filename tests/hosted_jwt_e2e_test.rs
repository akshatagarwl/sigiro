//! Deterministic hosted-auth E2E coverage.
//!
//! The test runs the real `sigiro serve` binary with JWT resource-server
//! settings, while a loopback JWKS server supplies a fixed Ed25519 key. It
//! deliberately uses local DuckLake files: the hosted boundary under test is
//! JWT authentication, scope enforcement, OTLP routing, and tenant selection;
//! PostgreSQL/S3 are covered by the storage-mode E2Es.

#![allow(clippy::disallowed_methods)]

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ISSUER: &str = "http://127.0.0.1:1/better-auth";
const AUDIENCE: &str = "http://127.0.0.1:2/api/";
const RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource/api";
const RESOURCE_METADATA_URL: &str = "http://127.0.0.1:2/.well-known/oauth-protected-resource/api";
const KID: &str = "hosted-e2e-key-1";
const PUBLIC_KEY_X: &str = "Tn3DoRl__bp9FXiHIBEXzVMxEOXRVx1l-cD2mzDZMN4";
const PRIVATE_KEY: &[u8] = include_bytes!("fixtures/auth_ed25519_private.der");
const BASE_PORT: u16 = 42100;

const INGEST: &str = "telemetry:ingest";
const QUERY: &str = "query:read";

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    aud: &'a str,
    exp: u64,
    nbf: u64,
    sub: &'a str,
    tid: &'a str,
    scope: &'a str,
}

fn token(tenant: &str, scope: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    let claims = Claims {
        iss: ISSUER,
        aud: AUDIENCE,
        exp: now + 300,
        nbf: now.saturating_sub(1),
        sub: "hosted-e2e-user",
        tid: tenant,
        scope,
    };
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some(KID.to_owned());
    encode(&header, &claims, &EncodingKey::from_ed_der(PRIVATE_KEY))
        .expect("encode deterministic E2E JWT")
}

struct JwksServer {
    address: String,
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl JwksServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind JWKS server");
        listener
            .set_nonblocking(true)
            .expect("set JWKS listener nonblocking");
        let address = listener.local_addr().expect("JWKS address").to_string();
        let (stop, stopped) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            let body = format!(
                r#"{{"keys":[{{"kty":"OKP","crv":"Ed25519","x":"{PUBLIC_KEY_X}","alg":"EdDSA","use":"sig","kid":"{KID}"}}]}}"#
            );
            loop {
                if stopped.try_recv().is_ok() {
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => respond_jwks(&mut stream, &body),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://{}/jwks", self.address)
    }
}

impl Drop for JwksServer {
    fn drop(&mut self) {
        self.stop.send(()).unwrap_or(());
        // Wake the nonblocking accept loop so cleanup is prompt.
        drop(TcpStream::connect(&self.address));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

fn respond_jwks(stream: &mut TcpStream, body: &str) {
    let mut request = [0_u8; 1024];
    drop(stream.read(&mut request));
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("write JWKS response");
}

struct Server {
    child: Child,
    _data_dir: tempfile::TempDir,
    http_port: u16,
    otlp_port: u16,
}

impl Server {
    fn start(jwks: &JwksServer) -> Self {
        let data_dir = tempfile::tempdir().expect("create E2E data dir");
        let child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
            .arg("serve")
            .env("SIGIRO_DATA_DIR", data_dir.path())
            .env("SIGIRO_HTTP_PORT", BASE_PORT.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", (BASE_PORT + 1).to_string())
            .env("SIGIRO_GRPC_PORT", (BASE_PORT + 2).to_string())
            .env("SIGIRO_FLUSH_INTERVAL_SECS", "1")
            .env("SIGIRO_EXTENSION_DIR", extension_dir())
            .env("SIGIRO_AUTH_ISSUER", ISSUER)
            .env("SIGIRO_AUTH_AUDIENCE", AUDIENCE)
            .env("SIGIRO_AUTH_JWKS_URL", jwks.url())
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sigiro hosted JWT E2E server");
        let server = Self {
            child,
            _data_dir: data_dir,
            http_port: BASE_PORT,
            otlp_port: BASE_PORT + 1,
        };
        server.wait_ready();
        server
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.http_port)
    }

    fn wait_ready(&self) {
        let client = reqwest::blocking::Client::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        while std::time::Instant::now() < deadline {
            match client.get(format!("{}/health", self.base())).send() {
                Ok(response) if response.status().is_success() => return,
                _ => {}
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("hosted JWT E2E server did not become ready");
    }

    fn request(&self, path: &str, jwt: Option<&str>) -> reqwest::blocking::RequestBuilder {
        let request = reqwest::blocking::Client::new().get(format!("{}{path}", self.base()));
        match jwt {
            Some(jwt) => request.bearer_auth(jwt),
            None => request,
        }
    }

    fn query(&self, jwt: &str, sql: &str) -> reqwest::blocking::Response {
        reqwest::blocking::Client::new()
            .post(format!("{}/v1/query", self.base()))
            .bearer_auth(jwt)
            .header("content-type", "text/plain")
            .body(sql.to_owned())
            .send()
            .expect("query request")
    }

    fn ingest(&self, jwt: &str, payload: &str, path: &str) -> reqwest::blocking::Response {
        reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{}{path}", self.otlp_port))
            .bearer_auth(jwt)
            .header("content-type", "application/json")
            .body(payload.to_owned())
            .send()
            .expect("OTLP request")
    }

    fn wait_for_count(&self, jwt: &str, table: &str, service: &str, expected: i64) -> i64 {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let sql =
            format!("SELECT count(*) AS n FROM lake.{table} WHERE service_name = '{service}'");
        loop {
            let response = self.query(jwt, &sql);
            if response.status().is_success()
                && let Ok(rows) = response.json::<Vec<Value>>()
                && let Some(count) = rows.first().and_then(|row| row["n"].as_i64())
                && (count >= expected || std::time::Instant::now() >= deadline)
            {
                return count;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "telemetry did not flush"
            );
            thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_owned())
    })
}

fn traces(service: &str) -> String {
    format!(
        r#"{{"resourceSpans":[{{"resource":{{"attributes":[{{"key":"service.name","value":{{"stringValue":"{service}"}}}}]}},"scopeSpans":[{{"spans":[{{"traceId":"0102030405060708090a0b0c0d0e0f10","spanId":"0102030405060708","name":"hosted-e2e-span","startTimeUnixNano":"1700000000000000000","endTimeUnixNano":"1700000001000000000"}}]}}]}}]}}"#
    )
}

fn logs(service: &str) -> String {
    format!(
        r#"{{"resourceLogs":[{{"resource":{{"attributes":[{{"key":"service.name","value":{{"stringValue":"{service}"}}}}]}},"scopeLogs":[{{"logRecords":[{{"timeUnixNano":"1700000000000000000","severityNumber":9,"severityText":"INFO","body":{{"stringValue":"hosted JWT E2E log"}}}}]}}]}}]}}"#
    )
}

fn metrics(service: &str) -> String {
    format!(
        r#"{{"resourceMetrics":[{{"resource":{{"attributes":[{{"key":"service.name","value":{{"stringValue":"{service}"}}}}]}},"scopeMetrics":[{{"metrics":[{{"name":"hosted.jwt.e2e","gauge":{{"dataPoints":[{{"timeUnixNano":"1700000000000000000","asDouble":1.0}}]}}}}]}}]}}]}}"#
    )
}

#[test]
#[ignore = "spawns a real server; run with `mise run check-hosted-jwt-e2e`"]
fn hosted_jwt_auth_scopes_otlp_and_tenant_isolation() {
    let jwks = JwksServer::start();
    let server = Server::start(&jwks);
    let acme_ingest = token("tenant_acme", INGEST);
    let acme_query = token("tenant_acme", QUERY);
    let acme_all = token("tenant_acme", "telemetry:ingest query:read services:read");
    let globex_all = token("tenant_globex", "telemetry:ingest query:read services:read");
    let service_a = "hosted-jwt-acme";
    let service_b = "hosted-jwt-globex";

    let metadata = server
        .request(RESOURCE_METADATA_PATH, None)
        .send()
        .expect("protected-resource metadata request");
    assert_eq!(metadata.status(), reqwest::StatusCode::OK);
    assert_eq!(
        metadata.headers()[reqwest::header::CONTENT_TYPE],
        "application/json"
    );
    let metadata = metadata.json::<Value>().expect("metadata JSON");
    assert_eq!(metadata["resource"], AUDIENCE);
    assert_eq!(metadata["authorization_servers"][0], ISSUER);
    assert_eq!(metadata["bearer_methods_supported"][0], "header");

    let missing = server
        .request("/v1/services", None)
        .send()
        .expect("missing-token request");
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing.headers()[reqwest::header::WWW_AUTHENTICATE],
        format!("Bearer resource_metadata=\"{RESOURCE_METADATA_URL}\"")
    );

    let invalid = server
        .request("/v1/services", Some("not-a-jwt"))
        .send()
        .expect("invalid-token request");
    assert_eq!(invalid.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        invalid.headers()[reqwest::header::WWW_AUTHENTICATE],
        format!("Bearer resource_metadata=\"{RESOURCE_METADATA_URL}\", error=\"invalid_token\"")
    );

    let insufficient = server
        .request("/v1/services", Some(&acme_query))
        .send()
        .expect("insufficient-scope request");
    assert_eq!(insufficient.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        insufficient.headers()[reqwest::header::WWW_AUTHENTICATE],
        format!(
            "Bearer resource_metadata=\"{RESOURCE_METADATA_URL}\", error=\"insufficient_scope\", scope=\"services:read\""
        )
    );
    assert_eq!(
        server
            .request("/v1/services", Some(&acme_all))
            .send()
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );

    assert_eq!(
        server
            .ingest(&acme_query, &traces(service_a), "/v1/traces")
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    for (path, payload) in [
        ("/v1/traces", traces(service_a)),
        ("/v1/logs", logs(service_a)),
        ("/v1/metrics", metrics(service_a)),
    ] {
        assert_eq!(
            server.ingest(&acme_ingest, &payload, path).status(),
            reqwest::StatusCode::OK,
            "{path}"
        );
    }
    assert_eq!(
        server.wait_for_count(&acme_query, "sigiro_spans", service_a, 1),
        1
    );
    assert_eq!(
        server.wait_for_count(&acme_query, "sigiro_logs", service_a, 1),
        1
    );
    assert_eq!(
        server.wait_for_count(&acme_query, "sigiro_metrics_gauge", service_a, 1),
        1
    );

    assert_eq!(
        server
            .ingest(&globex_all, &traces(service_b), "/v1/traces")
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        server.wait_for_count(&globex_all, "sigiro_spans", service_b, 1),
        1
    );
    let cross_tenant = server.query(
        &acme_query,
        &format!("SELECT count(*) AS n FROM lake.sigiro_spans WHERE service_name = '{service_b}'"),
    );
    assert_eq!(cross_tenant.status(), reqwest::StatusCode::OK);
    assert_eq!(
        cross_tenant.json::<Vec<Value>>().unwrap()[0]["n"].as_i64(),
        Some(0)
    );
}

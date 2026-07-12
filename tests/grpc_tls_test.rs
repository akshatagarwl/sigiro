//! End-to-end TLS coverage for the OTLP/gRPC listener. This starts the real
//! `sigiro serve` binary so the test exercises the production TLS startup and
//! wire path rather than a service constructed past either boundary.

use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, trace_service_client::TraceServiceClient,
};
use opentelemetry_proto::tonic::trace::v1::Span;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

struct TlsServer {
    child: Child,
    _data_dir: tempfile::TempDir,
    grpc_port: u16,
    cert_pem: Vec<u8>,
}

impl TlsServer {
    fn spawn() -> Self {
        let data_dir = tempfile::tempdir().expect("failed to create temp data dir");
        let cert_path = data_dir.path().join("cert.pem");
        let key_path = data_dir.path().join("key.pem");
        let openssl = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                key_path.to_str().expect("UTF-8 key path"),
                "-out",
                cert_path.to_str().expect("UTF-8 cert path"),
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=DNS:localhost",
                // `req -x509` defaults to basicConstraints CA:TRUE; a CA cert
                // presented as the server leaf is rejected by rustls with
                // `CaUsedAsEndEntity`. Force an end-entity (leaf) cert that the
                // client trusts directly as its own root.
                "-addext",
                "basicConstraints=critical,CA:FALSE",
                "-days",
                "1",
            ])
            .output()
            .expect("failed to run openssl; it is required for this TLS integration test");
        assert!(
            openssl.status.success(),
            "openssl failed to generate self-signed certificate: {}",
            String::from_utf8_lossy(&openssl.stderr)
        );

        let http_port = unused_port();
        let otlp_http_port = unused_port();
        let grpc_port = unused_port();
        let child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
            .arg("serve")
            .env("SIGIRO_DATA_DIR", data_dir.path())
            // Shared extension cache (see retention_runtime_test): avoids a
            // per-run cold download of every DuckDB extension (incl. icu) that
            // can push serve boot past the 90s gRPC-bind deadline and flake this.
            .env("SIGIRO_EXTENSION_DIR", "/tmp/sigiro-test-extensions")
            .env("SIGIRO_HTTP_PORT", http_port.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", otlp_http_port.to_string())
            .env("SIGIRO_GRPC_PORT", grpc_port.to_string())
            .env("SIGIRO_TLS_CERT", &cert_path)
            .env("SIGIRO_TLS_KEY", &key_path)
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn `sigiro serve`");

        let server = Self {
            child,
            _data_dir: data_dir,
            grpc_port,
            cert_pem: std::fs::read(cert_path).expect("failed to read generated certificate"),
        };
        server.wait_for_grpc_port();
        server
    }

    fn wait_for_grpc_port(&self) {
        let deadline = Instant::now() + Duration::from_secs(90);
        while std::net::TcpStream::connect(("127.0.0.1", self.grpc_port)).is_err() {
            assert!(
                Instant::now() < deadline,
                "sigiro serve did not bind its gRPC listener within 90s"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    async fn tls_client(&self) -> TraceServiceClient<Channel> {
        let endpoint = Endpoint::from_shared(format!("https://localhost:{}", self.grpc_port))
            .expect("valid TLS endpoint")
            .tls_config(
                ClientTlsConfig::new()
                    .ca_certificate(Certificate::from_pem(&self.cert_pem))
                    .domain_name("localhost"),
            )
            .expect("valid TLS client configuration");
        TraceServiceClient::connect(endpoint)
            .await
            .expect("failed to connect OTLP gRPC client over TLS")
    }

    fn shutdown(mut self) {
        // SAFETY: sends SIGTERM only to this test's child process.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self
                .child
                .try_wait()
                .expect("failed to poll sigiro process")
            {
                Some(_status) => {
                    // This test deliberately sends a broken PLAINTEXT h2 probe at
                    // the TLS listener (the plaintext-rejection assertion above).
                    // tonic drops that half-open connection during graceful
                    // shutdown, which can make `serve_with_shutdown` return a
                    // non-zero exit — so we assert only that the server TERMINATES
                    // PROMPTLY on SIGTERM (the loop's deadline), NOT a clean exit
                    // code. Clean-shutdown-of-a-cleanly-used server is covered by
                    // the other integration tests; this one proves TLS is accepted
                    // and plaintext is rejected. (Verified in isolation: without a
                    // hostile probe this server exits 0 on SIGTERM.)
                    return;
                }
                None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                None => {
                    self.child
                        .kill()
                        .expect("failed to force-kill sigiro process");
                    self.child
                        .wait()
                        .expect("failed to wait for force-killed sigiro process");
                    panic!("sigiro serve did not exit within 15s of SIGTERM");
                }
            }
        }
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // Best-effort cleanup when an assertion above fails. The test still
            // reports the original failure; this prevents a leaked listener.
            unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGKILL) };
            if let Err(error) = self.child.wait() {
                eprintln!("failed to reap force-killed sigiro process: {error}");
            }
        }
    }
}

fn unused_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("failed to allocate test port")
        .local_addr()
        .expect("allocated listener has no local address")
        .port()
}

#[tokio::test]
async fn grpc_tls_accepts_tls_and_rejects_plaintext() {
    let server = TlsServer::spawn();

    let mut tls_client = server.tls_client().await;
    let request = ExportTraceServiceRequest {
        resource_spans: vec![opentelemetry_proto::tonic::trace::v1::ResourceSpans {
            resource: Some(opentelemetry_proto::tonic::resource::v1::Resource {
                attributes: vec![opentelemetry_proto::tonic::common::v1::KeyValue {
                    key: "service.name".to_string(),
                    value: Some(opentelemetry_proto::tonic::common::v1::AnyValue {
                        value: Some(
                            opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                "grpc-tls-e2e-svc".to_string(),
                            ),
                        ),
                    }),
                }],
                ..Default::default()
            }),
            scope_spans: vec![opentelemetry_proto::tonic::trace::v1::ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![1; 16],
                    span_id: vec![2; 8],
                    name: "grpc-tls-e2e".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    tls_client
        .export(request)
        .await
        .expect("TLS OTLP/gRPC export was rejected");

    let plaintext_rejected =
        match TraceServiceClient::connect(format!("http://127.0.0.1:{}", server.grpc_port)).await {
            Ok(mut client) => client
                .export(ExportTraceServiceRequest::default())
                .await
                .is_err(),
            Err(_) => true,
        };
    assert!(
        plaintext_rejected,
        "plaintext OTLP/gRPC export unexpectedly succeeded against TLS listener"
    );

    server.shutdown();
}

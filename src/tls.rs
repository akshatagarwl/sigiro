//! TLS support for HTTP servers.
//!
//! When `SIGIRO_TLS_CERT` and `SIGIRO_TLS_KEY` are set, HTTP-based
//! servers (main API + OTLP HTTP) serve over TLS. gRPC TLS is not yet
//! implemented — use a sidecar or load balancer for gRPC TLS termination.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tower::util::ServiceExt;

/// Build a TLS acceptor from PEM certificate and key files.
pub fn tls_acceptor(cert_path: &str, key_path: &str) -> Result<TlsAcceptor> {
    let cert_file = std::fs::read(cert_path)
        .with_context(|| format!("failed to read TLS cert: {cert_path}"))?;
    let key_file =
        std::fs::read(key_path).with_context(|| format!("failed to read TLS key: {key_path}"))?;

    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_file.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .context("failed to parse TLS certificate PEM")?;

    let key = rustls_pemfile::private_key(&mut key_file.as_slice())
        .context("failed to parse TLS private key PEM")?
        .context("no private key found in key file")?;

    anyhow::ensure!(!certs.is_empty(), "no certificates found in {cert_path}");

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("invalid TLS certificate / key pair")?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Serve an axum app over TLS.
/// Accepts TLS connections and serves each with axum's HTTP handler.
pub async fn serve_tls(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    app: axum::Router,
    name: &str,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    use hyper_util::rt::TokioIo;
    use hyper_util::server::conn::auto::Builder;

    let app = Arc::new(app);
    let mut connections = JoinSet::new();
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => Some(accepted?),
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    return Err(error).context("TLS connection task panicked");
                }
                None
            }
            _ = shutdown.changed() => break,
        };
        let Some((stream, peer)) = accepted else {
            continue;
        };
        let acceptor = acceptor.clone();
        let app = Arc::clone(&app);
        let name = name.to_string();
        connections.spawn(async move {
            match acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    let io = TokioIo::new(tls_stream);
                    let service = hyper::service::service_fn(
                        move |req: axum::http::Request<hyper::body::Incoming>| {
                            let app = Arc::clone(&app);
                            async move { axum::Router::oneshot((*app).clone(), req).await }
                        },
                    );
                    if let Err(e) = Builder::new(hyper_util::rt::TokioExecutor::new())
                        .serve_connection_with_upgrades(io, service)
                        .await
                    {
                        tracing::debug!(%peer, server = %name, "TLS connection closed: {e}");
                    }
                }
                Err(e) => {
                    tracing::warn!(%peer, server = %name, "TLS handshake failed: {e}");
                }
            }
        });
    }

    while let Some(result) = connections.join_next().await {
        result.context("TLS connection task panicked")?;
    }
    Ok(())
}

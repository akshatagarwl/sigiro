//! Pyroscope continuous profiling integration.
//!
//! Enabled when `SIGIRO_PYROSCOPE_ENDPOINT` env var is set. Sigiro will:
//!   - Tag all profiles with `service=<OTEL_SERVICE_NAME or "sigiro">`
//!   - Send CPU and sampled jemalloc heap profiles to the Pyroscope server
//!
//! The SDK snapshots each backend every 10 seconds and uploads the pprof payload
//! via an HTTP POST to `{endpoint}/push.v1.PusherService/Push`. Version 2.0.4
//! clears collected CPU samples without dropping and recreating the profiler
//! guard, avoiding a large allocation and retained-memory spike each cycle.
//!
//! Sigiro's own flush settings (SIGIRO_FLUSH_INTERVAL_SECS, SIGIRO_FLUSH_ROW_THRESHOLD,
//! SIGIRO_BACKPRESSURE_POLICY) control how fast ingested profiles are persisted
//! to DuckDB — they are independent of the SDK's upload cycle.

use anyhow::{Context, Result};
use pyroscope::backend::{BackendImpl, BackendUninitialized};
use pyroscope::pyroscope::{PyroscopeAgent, PyroscopeAgentBuilder, PyroscopeAgentRunning};
use std::collections::HashMap;
use tracing_attributes::instrument;

type RunningAgent = PyroscopeAgent<PyroscopeAgentRunning>;

/// Handles to the Pyroscope agents, if configured.
pub struct ProfilingHandle {
    _cpu_agent: RunningAgent,
    _heap_agent: RunningAgent,
}

/// Start Pyroscope profiling if `SIGIRO_PYROSCOPE_ENDPOINT` is set.
/// Returns None if the endpoint is not configured.
#[instrument(skip_all)]
pub fn start_profiling() -> Result<Option<ProfilingHandle>> {
    let endpoint = match crate::config::PYROSCOPE_ENDPOINT.value() {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };

    let service_name = crate::config::OTEL_SERVICE_NAME
        .value()
        .map_err(anyhow::Error::from)?;

    let cpu_backend = pyroscope::backend::pprof_backend(
        pyroscope::backend::pprof::PprofConfig::default(),
        pyroscope::backend::BackendConfig::default(),
    );
    let heap_backend = pyroscope::backend::jemalloc_backend();

    // When sigiro is self-profiling into its own OTLP endpoint, the SDK
    // must authenticate so the server doesn't reject the push with 401.
    // `SIGIRO_OTEL_AUTH_KEY` is the same key the OTel self-instrumentation
    // uses (see src/otel.rs) — reusing it here keeps self-profiling working
    // out of the box without inventing a new env var.
    let auth_key = crate::otel::otel_auth_key_from_env()?;
    let http_headers = match auth_key.as_ref() {
        Some(key) => {
            let mut h = HashMap::new();
            h.insert(
                "Authorization".to_string(),
                format!("Bearer {}", key.as_ref()),
            );
            Some(h)
        }
        None => None,
    };

    let cpu_agent = build_agent(
        &endpoint,
        &service_name,
        "pyroscope-rs",
        cpu_backend,
        http_headers.clone(),
    )
    .context("failed to start Pyroscope CPU agent")?;
    let heap_agent = build_agent(
        &endpoint,
        &service_name,
        "pyroscope-jemalloc",
        heap_backend,
        http_headers,
    )
    .context("failed to start Pyroscope heap agent")?;

    tracing::info!(
        pyroscope_endpoint = %endpoint,
        service = %service_name,
        cpu_sample_rate = 100,
        heap_sample_interval_bytes = 524_288,
        auth_configured = auth_key.is_some(),
        "Pyroscope CPU and heap profiling enabled"
    );

    Ok(Some(ProfilingHandle {
        _cpu_agent: cpu_agent,
        _heap_agent: heap_agent,
    }))
}

fn build_agent(
    endpoint: &str,
    service_name: &str,
    spy_name: &str,
    backend: BackendImpl<BackendUninitialized>,
    http_headers: Option<HashMap<String, String>>,
) -> Result<RunningAgent> {
    let mut builder = PyroscopeAgentBuilder::new(
        endpoint,
        service_name,
        100,
        spy_name,
        env!("CARGO_PKG_VERSION"),
        backend,
    );
    if let Some(headers) = http_headers {
        builder = builder.http_headers(headers);
    }
    builder
        .build()
        .context("failed to build Pyroscope agent")?
        .start()
        .context("failed to start Pyroscope agent")
}

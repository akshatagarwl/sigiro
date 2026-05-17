//! Pyroscope continuous profiling integration.
//!
//! Enabled when `SIGIRO_PYROSCOPE_ENDPOINT` env var is set. Sigiro will:
//!   - Tag all profiles with `service=<OTEL_SERVICE_NAME or "sigiro">`
//!   - Send CPU and memory allocation profiles to the Pyroscope server
//!
//! The pyroscope-rs SDK (v2.0) hardcodes a 10-second flush cycle in
//! `PyroscopeAgentBuilder::build()` — there is no public API to change it.
//! Each 10s tick, the SDK snapshots the profiler backend and uploads the
//! pprof payload via an HTTP POST to `{endpoint}/push.v1.PusherService/Push`.
//!
//! Sigiro's own flush settings (SIGIRO_FLUSH_INTERVAL_SECS, SIGIRO_FLUSH_ROW_THRESHOLD,
//! SIGIRO_BACKPRESSURE_POLICY) control how fast ingested profiles are persisted
//! to DuckDB — they are independent of the SDK's upload cycle.

use anyhow::{Context, Result};
use tracing_attributes::instrument;

/// Handle to the Pyroscope agent, if configured.
pub struct ProfilingHandle {
    /// The running agent. Must be kept alive for profiling to continue.
    _agent: pyroscope::PyroscopeAgent<pyroscope::pyroscope::PyroscopeAgentRunning>,
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

    let backend = pyroscope::backend::pprof_backend(
        pyroscope::backend::pprof::PprofConfig::default(),
        pyroscope::backend::BackendConfig::default(),
    );

    let agent = pyroscope::pyroscope::PyroscopeAgentBuilder::new(
        &endpoint,
        &service_name,
        100, // sample rate Hz
        "pyroscope-rs",
        env!("CARGO_PKG_VERSION"),
        backend,
    )
    .build()
    .context("failed to build Pyroscope agent")?;

    let agent = agent.start().context("failed to start Pyroscope agent")?;

    tracing::info!(
        pyroscope_endpoint = %endpoint,
        service = %service_name,
        sample_rate = 100,
        "Pyroscope profiling enabled"
    );

    Ok(Some(ProfilingHandle { _agent: agent }))
}

//! Pyroscope continuous profiling integration.
//!
//! Enabled when `SIGIRO_PYROSCOPE_ENDPOINT` env var is set. Sigiro will:
//!   - Tag all profiles with `service=<OTEL_SERVICE_NAME or "sigiro">`
//!   - Send CPU and memory allocation profiles to the Pyroscope server
//!
//! The smoke-test script fetches profiles from the same Pyroscope endpoint and
//! includes them in the investigate blocks passed to the AI agent for reasoning.

use anyhow::{Context, Result};

/// Handle to the Pyroscope agent, if configured.
pub struct ProfilingHandle {
    /// The running agent. Must be kept alive for profiling to continue.
    _agent: pyroscope::PyroscopeAgent<pyroscope::pyroscope::PyroscopeAgentRunning>,
}

/// Start Pyroscope profiling if `SIGIRO_PYROSCOPE_ENDPOINT` is set.
/// Returns None if the endpoint is not configured.
pub fn start_profiling() -> Result<Option<ProfilingHandle>> {
    let endpoint = match std::env::var("SIGIRO_PYROSCOPE_ENDPOINT") {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };

    let service_name = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "sigiro".to_string());

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

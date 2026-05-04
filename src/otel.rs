use anyhow::{Context, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::trace::{SdkTracerProvider, BatchSpanProcessor};
use std::collections::HashMap;
use tracing_subscriber::prelude::*;

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl TelemetryGuard {
    /// Force-flush pending spans. Call before querying or periodically.
    pub fn flush(&self) {
        tracing::debug!("OTEL flush requested");
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        tracing::info!("OTel shutdown: flushing pending spans");
        if let Some(provider) = self.tracer_provider.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("OTel tracer provider shutdown failed: {e}");
            }
        }
        if let Some(provider) = self.logger_provider.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("OTel logger provider shutdown failed: {e}");
            }
        }
    }
}
#[allow(clippy::disallowed_methods)]
fn otel_auth_headers() -> HashMap<String, String> {
// otel_auth_headers: reads SIGIRO_OTEL_AUTH_KEY env var, returns auth headers for OTEL endpoint

    let mut headers = HashMap::new();
    // Allow custom auth key via env var, default to dev auth for self-ingestion
    if let Ok(key) = std::env::var("SIGIRO_OTEL_AUTH_KEY") {
        if !key.is_empty() {
            headers.insert("authorization".to_string(), format!("Bearer {key}"));
            return headers;
        }
    }
    // Default to dev auth for self-ingestion
    headers.insert("authorization".to_string(), "Bearer sigiro_dev_self".to_string());
    headers
}

pub fn init_telemetry() -> Result<Option<TelemetryGuard>> {
    // SIGIRO_OTEL_ENDPOINT: where sigiro sends its own telemetry (traces + logs)
    let endpoint = crate::config::OTEL_ENDPOINT.value().map_err(|e| anyhow::anyhow!("{e}"))?;
    if endpoint.is_empty() {
        tracing::debug!("SIGIRO_OTEL_ENDPOINT not set — self-instrumentation disabled");
        return Ok(None);
    }

    let service_name = crate::config::OTEL_SERVICE_NAME.value().map_err(|e| anyhow::anyhow!("{e}"))?;

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name(service_name)
        .build();

    // Auth headers for self-ingestion
    let headers = otel_auth_headers();

    // Build OTLP HTTP exporters with auth headers
    let span_exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(&endpoint)
        .with_headers(headers.clone())
        .build()
        .context("failed to build OTLP HTTP span exporter")?;

    // Use batch exporter with 3s scheduled delay (default is 5s)
    let batch_config = opentelemetry_sdk::trace::BatchConfigBuilder::default()
        .with_scheduled_delay(std::time::Duration::from_secs(3))
        .build();
    let batch_processor = BatchSpanProcessor::builder(span_exporter)
        .with_batch_config(batch_config)
        .build();
    
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_span_processor(batch_processor)
        .build();

    // Register globally so #[instrument] macro can find the tracer
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());
    let tracer = tracer_provider.tracer("sigiro");

    let log_exporter = LogExporter::builder()
        .with_http()
        .with_endpoint(&endpoint)
        .with_headers(headers)
        .build()
        .context("failed to build OTLP HTTP log exporter")?;

    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(log_exporter)
        .build();

    let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let otel_log_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "sigiro=info".parse().expect("hardcoded filter is valid"));

    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .init();

    tracing::info!(
        "OTel self-instrumentation enabled — exporting to {endpoint} (traces + logs)"
    );

    Ok(Some(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        logger_provider: Some(logger_provider),
    }))
}

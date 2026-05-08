use anyhow::{Context, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::trace::{BatchSpanProcessor, SdkTracerProvider};
use tracing_subscriber::prelude::*;

/// OTel configuration — read once at startup, passed to init_telemetry()
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub endpoint: String,
    pub service_name: String,
    pub auth_key: Option<String>,
}

impl TelemetryConfig {
    /// Load from env vars at startup
    pub fn from_env() -> Result<Self> {
        let endpoint = crate::config::OTEL_ENDPOINT
            .value()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let service_name = crate::config::OTEL_SERVICE_NAME
            .value()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let auth_key = crate::config::OTEL_AUTH_KEY
            .value()
            .ok()
            .filter(|k| !k.is_empty());
        Ok(Self { endpoint, service_name, auth_key })
    }
}

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl TelemetryGuard {
    /// Force-flush pending spans. Call before querying or periodically.
    pub fn flush(&self) {
        if let Some(ref provider) = self.tracer_provider {
            if let Err(e) = provider.force_flush() {
                tracing::warn!("OTEL tracer force_flush failed: {e}");
            }
        }
        if let Some(ref provider) = self.logger_provider {
            if let Err(e) = provider.force_flush() {
                tracing::warn!("OTEL logger force_flush failed: {e}");
            }
        }
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

pub fn init_telemetry(config: TelemetryConfig) -> Result<Option<TelemetryGuard>> {
    if config.endpoint.is_empty() {
        tracing::debug!("SIGIRO_OTEL_ENDPOINT not set — self-instrumentation disabled");
        return Ok(None);
    }

    let service_name = config.service_name;

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name(service_name)
        .with_attribute(opentelemetry::KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION"),
        ))
        .build();

    // Auth headers for self-ingestion — auth_key read at app entry, not at runtime.
    // Defaults to dev key for local development; production should set SIGIRO_OTEL_AUTH_KEY.
    let auth_key = config.auth_key.as_deref().unwrap_or_else(|| {
        tracing::warn!("SIGIRO_OTEL_AUTH_KEY not set — using default dev key. Set SIGIRO_OTEL_AUTH_KEY in production.");
        "sigiro_dev_self"
    });
    let mut headers = std::collections::HashMap::new();
    headers.insert("authorization".to_string(), format!("Bearer {auth_key}"));
    // Build OTLP HTTP exporters with auth headers
    // Note: with_endpoint() does NOT auto-append signal paths — must include full path
    let traces_url = format!("{}/v1/traces", config.endpoint.trim_end_matches('/'));
    let logs_url = format!("{}/v1/logs", config.endpoint.trim_end_matches('/'));

    let span_exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(&traces_url)
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
        .with_endpoint(&logs_url)
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
        "OTel self-instrumentation enabled — exporting to {} (traces + logs)",
        config.endpoint
    );

    Ok(Some(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        logger_provider: Some(logger_provider),
    }))
}

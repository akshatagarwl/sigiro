use anyhow::{Context, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{LogExporter, SpanExporter};
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::prelude::*;

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
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

pub fn init_telemetry() -> Result<Option<TelemetryGuard>> {
    if crate::config::OTEL_EXPORTER_OTLP_ENDPOINT.value().map(|s| s.is_empty()).unwrap_or(true) {
        tracing::debug!("OTEL_EXPORTER_OTLP_ENDPOINT not set — self-instrumentation disabled");
        return Ok(None);
    }

    let service_name = crate::config::OTEL_SERVICE_NAME.value().map_err(|e| anyhow::anyhow!("{e}"))?;

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name(service_name)
        .build();

    let span_exporter = SpanExporter::builder()
        .with_http()
        .build()
        .context("failed to build OTLP HTTP span exporter")?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();

    let tracer = tracer_provider.tracer("sigiro");

    let log_exporter = LogExporter::builder()
        .with_http()
        .build()
        .context("failed to build OTLP HTTP log exporter")?;

    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(log_exporter)
        .build();

    let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let otel_log_layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "sigiro=info".parse().expect("hardcoded filter is valid"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .init();

    tracing::info!("OTel self-instrumentation enabled (traces + logs)");

    Ok(Some(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        logger_provider: Some(logger_provider),
    }))
}

use anyhow::{Context, Result};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::prelude::*;

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("OTel tracer provider shutdown failed: {e}");
            }
        }
    }
}

pub fn init_telemetry() -> Result<Option<TelemetryGuard>> {
    if crate::config::OTEL_EXPORTER_OTLP_ENDPOINT.value().map(|s| s.is_empty()).unwrap_or(true) {
        tracing::debug!("OTEL_EXPORTER_OTLP_ENDPOINT not set — self-instrumentation disabled");
        return Ok(None);
    }

    let exporter = SpanExporter::builder()
        .with_http()
        .build()
        .context("failed to build OTLP HTTP span exporter")?;

    let service_name = crate::config::OTEL_SERVICE_NAME.value().map_err(|e| anyhow::anyhow!("{e}"))?;

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name(service_name)
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();

    let tracer = tracer_provider.tracer("sigiro");

    let otel_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "sigiro=info".parse().expect("hardcoded filter is valid"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .init();

    tracing::info!("OTel self-instrumentation enabled");

    Ok(Some(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
    }))
}

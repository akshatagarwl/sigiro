use opentelemetry_sdk::trace::{SdkTracerProvider, SpanExporter};
use opentelemetry_sdk::export::trace::SpanData;
use opentelemetry_sdk::runtime::{Runtime, ConfiguredRuntime};
use std::time::Duration;

/// Creates a SpanProcessor that flushes on a timer
pub fn create_periodic_processor<E>(exporter: E, period_secs: u64) -> impl SpanProcessor
where
    E: SpanExporter + Send + Sync + 'static,
{
    // Use with_batch with schedule_delay and max_queue_size
    let config = opentelemetry_sdk::trace::Config {
        sampler: Box::new(opentelemetry_sdk::trace::Sampler::AlwaysOn),
        ..Default::default()
    };
    
    SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_config(config)
        .build()
}
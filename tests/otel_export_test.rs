// Minimal test to verify OTEL export works
// Run with: cargo test --package sigiro test_otel_export

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{Tracer, Span, SpanBuilder, Status, TracerProvider};
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::resource::Resource;

    #[tokio::test]
    async fn test_otel_span_export() {
        // Create exporter
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint("http://localhost:4318/v1/traces")
            .build()
            .unwrap();
        
        // Create tracer provider with batch exporter
        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(Resource::builder().with_service_name("test").build())
            .with_batch_exporter(exporter)
            .build();
        
        // Create span
        let tracer = tracer_provider.tracer("test");
        let mut span = tracer.span_builder("test-span")
            .with_status(Status::Ok)
            .start(&tracer);
        
        span.end();
        
        // Force flush
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        tracer_provider.force_flush().unwrap();
        tracer_provider.shutdown().unwrap();
    }
}

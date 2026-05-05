// Minimal test to verify OTEL export works
// Run with: cargo test --package sigiro test_otel_export

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{Tracer, Span, Status, TracerProvider};
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::resource::Resource;

    #[tokio::test]
    async fn test_otel_span_export() {
        // Create exporter
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint("http://localhost:4318/v1/traces")
            .build()
            .unwrap();
        
        // Create tracer provider with batch exporter
        let provider = SdkTracerProvider::builder()
            .with_resource(Resource::builder().with_service_name("sigiro-test").build())
            .with_batch_exporter(exporter)
            .build();

        let tracer = provider.tracer("test");

        // Create a simple span
        let mut span = tracer.start("test-span");
        span.set_status(Status::Ok);
        span.end();

        // Flush
        provider.shutdown().unwrap();
    }
}
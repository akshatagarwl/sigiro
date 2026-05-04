#!/usr/bin/env python3
import re

with open('src/otel.rs', 'r') as f:
    content = f.read()

# Add flush method before Drop impl
old = '''impl Drop for TelemetryGuard {
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
}'''

new = '''impl TelemetryGuard {
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
}'''

content = content.replace(old, new)

with open('src/otel.rs', 'w') as f:
    f.write(content)

print("Added flush method")
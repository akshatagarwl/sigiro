use sigiro::otel::{TelemetryConfig, init_telemetry};

// A small instrumented function to exercise #[tracing::instrument].
// We keep the name and a field to verify instrumentation at runtime.
#[tracing::instrument(name = "instrumented_test_function", fields(input_value = %value))]
fn instrumented_test_function(value: i32) -> i32 {
    value + 1
}

#[test]
fn integration_otel_self_instrumentation_tests() {
    // The typed-env static for OTEL_EXPORTER_OTLP_ENDPOINT is evaluated at program
    // startup (static init time), before test code runs. So we can't test the
    // "unset" path by setting/unsetting env vars at test time.
    //
    // Instead, we test: init_telemetry() completes without panic (it may or may
    // not return a guard depending on whether the env var was set at startup).
    // The guard (if any) is dropped cleanly, and instrumented functions work.

    let mut config = TelemetryConfig::from_env().expect("TelemetryConfig should not fail");
    // Ensure auth key is set so init_telemetry succeeds when endpoint is configured.
    if config.auth_key.is_none() || config.auth_key.as_ref().unwrap().is_empty() {
        config.auth_key = Some("test_auth_key_12345".to_string());
    }
    let guard_opt = init_telemetry(config).expect("init_telemetry should not fail");

    // Use a function annotated with #[instrument] to ensure instrumentation path works
    let _ = instrumented_test_function(4);

    // Guard shutdown on drop (if we got one)
    if let Some(guard) = guard_opt {
        drop(guard);
    }

    // After any guard drop, instrumented function can still be called without panic
    let _ = instrumented_test_function(6);
}

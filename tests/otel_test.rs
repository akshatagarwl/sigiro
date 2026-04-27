use sigiro::otel::{init_telemetry, TelemetryGuard};
use std::env;

// A small instrumented function to exercise #[tracing::instrument].
// We keep the name and a field to verify instrumentation at runtime.
#[tracing::instrument(name = "instrumented_test_function", fields(input_value = %value))]
fn instrumented_test_function(value: i32) -> i32 {
    value + 1
}

#[test]
fn integration_otel_self_instrumentation_tests() {
    // Ensure a clean slate for env vars
    unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT"); }
    unsafe { std::env::remove_var("OTEL_SERVICE_NAME"); }

    // 1) No-op when env var is not set
    let res_no_op = init_telemetry().expect("init_telemetry should not fail");
    assert!(res_no_op.is_none(), "Expected no telemetry guard when OTEL_EXPORTER_OTLP_ENDPOINT is not set");

    // 2) Init with env var
    unsafe { std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4317"); }
    let guard_opt = init_telemetry().expect("init_telemetry should succeed");
    let guard = guard_opt.expect("expected Some(TelemetryGuard)");
    // 3) Use a function annotated with #[instrument] to ensure instrumentation path works
    let _ = instrumented_test_function(4);

    // 4) Guard shutdown on drop
    drop(guard);

    // 5) After shutdown, ensure instrumented function can still be called without panic
    let _ = instrumented_test_function(6);
}

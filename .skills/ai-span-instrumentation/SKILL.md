# AI-Assisted OpenTelemetry Span Instrumentation

## Purpose

This skill guides AI agents to automatically instrument services with OpenTelemetry spans using minimal code changes. It supports both zero-config approaches (for supported languages) and attribute-based instrumentation (for Rust).

## Language-Specific Approaches

### Zero-Code Auto-Instrumentation (Python, Java, Go, Node.js)

For languages with OTel auto-instrumentation agents:

```bash
# Python - just set env vars
OTEL_SERVICE_NAME=my-service \
OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4317 \
python -m opentelemetry.instrumentation.auto \
  python my_app.py

# Java - attach agent JAR
java -javaagent:opentelemetry-javaagent.jar \
     -Dotel.service.name=my-service \
     -Dotel.exporter.otlp.endpoint=http://collector:4317 \
     -jar my-app.jar

# Node.js - require auto-instrumentation
OTEL_SERVICE_NAME=my-service \
node -r @opentelemetry/auto-instrumentations-node/register \
  my_app.js
```

See [OpenTelemetry Zero-Code](https://opentelemetry.io/docs/concepts/instrumentation/automatic) for details.

### Minimal-Code Instrumentation (Rust)

Rust compiles to native binaries, so zero-config agents don't work. Use the `#[instrument]` macro:

```bash
# Add dependency
cargo add tracing-attributes
```

```rust
use tracing_attributes::instrument;

// Zero-effort span from function name
#[instrument]
pub fn my_function(data: &[u8]) -> Result<Vec<u8>> {
    // Automatic span: "my_function"
    // Automatic attributes: data.len()
}

// With custom name and attributes
#[instrument(name = "processor.transform", fields(data.len = data.len()))]
pub fn transform(data: &[u8]) -> Result<()> {
    // Span named "processor.transform" with data.len attribute
}

// Skip entering span (for trivial functions)
#[instrument(skip(data), fields(data.len = data.len()))]
pub fn process(data: &[u8]) {
    // Span created only if not skipped
}
```

## Three-Layer Span Instrumentation

### Layer 1: Entry Points (HTTP/gRPC Handlers)

Use `#[instrument]` on handlers with `skip()` for large params:

```rust
use tracing_attributes::instrument;

#[instrument(skip(state), fields(method = "POST", path = "/v1/query"))]
async fn query_handler(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    body: String,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    // Automatic span with method and path attributes
}

#[instrument(skip_all, fields(endpoint = "/v1/logs"))]
async fn otlp_http_logs(
    State(state): State<Arc<OtlpHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Span with endpoint attribute
}
```

### Layer 2: Data Pipeline Functions

```rust
#[instrument(skip_all, fields(rows = batch.num_rows()))]
pub fn transform_logs(bytes: Vec<u8>) -> Result<RecordBatch> {
    // Span with row count attribute
}

#[instrument(skip_all, fields(table = "sigiro_logs", operation = "INSERT"))]
pub fn buffer(&mut self, signal: SignalTable, batch: RecordBatch) {
    // Span with table and operation
}
```

### Layer 3: Storage Operations

```rust
#[instrument(skip_all, fields(rows = rows.len()))]
pub async fn flush_to_duckdb(&self, rows: &[Row]) -> Result<usize> {
    // Span with row count
}
```

## Instrumenting with `#[instrument]` Macro

The `#[instrument]` macro from `tracing-attributes` automatically:

1. Creates a span at function entry with function name (or custom `name`)
2. Records function arguments as span attributes (unless `skip(arg)`)
3. Records return value or error (unless `err` is in `skip`)
4. Adds source location (file, line) as resource

### Key Attributes

| Attribute | Purpose | Example |
|-----------|---------|---------|
| `name` | Custom span name | `name = "custom.span"` |
| `skip` | Don't record param | `skip(data)` |
| `fields` | Add custom attributes | `fields(size = data.len())` |
| `skip_all` | Skip all params | `skip_all` |
| `err` | Record errors | `err` |

### Async Functions

For async functions, `#[instrument]` automatically:
- Attaches span context to future
- Detaches on completion

```rust
#[instrument]
async fn async_handler(data: Data) -> Result<Response> {
    // Span lives for entire async operation
}
```

## Minimal Change Pattern for HTTP Servers

For axum/Rocket/Warp servers, wrap handlers with tower trace layer first:

```rust
use tower_http::trace::TraceLayer;
use tracing_opentelemetry::OpenTelemetrySpanExt;

// Add trace layer to router
let app = Router::new()
    .layer(TraceLayer::new_for_http())
    .layer(axum_mw::from_fn(auth))
    .with_state(state);

// The TraceLayer creates spans for ALL HTTP requests automatically
```

Then add `#[instrument]` to slow/important internal functions.

## Self-Instrumentation Pattern

For services that consume their own telemetry:

```rust
// In your OTEL setup, create tracer and pass to AppState
pub struct AppState {
    pub tracer: Arc<dyn Tracer>,
    pub router: MsgRouter,
    // ...
}

// Handlers automatically create spans
#[instrument(skip(state))]
async fn my_handler(State(state): State<Arc<AppState>>) {
    let tracer = state.tracer.clone();
    // Use tracer for explicit child spans if needed
}
```

## Testing Instrumentation

```bash
# After instrumentation, run self-loop test
curl -s http://localhost:9999/health

# Verify spans exist
curl -s -X POST http://localhost:9999/v1/query \
  -H "Authorization: Bearer sigiro_dev_test" \
  -d "SELECT count(*) FROM lake.sigiro_spans"

# Verify logs correlate
curl -s -X POST http://localhost:9999/v1/investigate \
  -d '{"service": "sigiro"}' | jq '.coverage'
```

Expected result:
- `span_count > 0`
- `has_logs: true`

## Common Pitfalls

1. **Skipping too much**: If `skip_all`, you lose valuable attributes
2. **Missing error recording**: Add `err` to record panics/errors
3. **Sync/async mismatch**: `#[instrument]` handles both, but don't mix
4. **Large params in skip**: Skip large data to avoid attribute bloat

## Output Checklist

When instrumenting a service, ensure:
- [ ] Entry handlers have spans (via TraceLayer or `#[instrument]`)
- [ ] Data pipeline functions have spans
- [ ] Storage operations have spans
- [ ] Attributes use consistent naming (snake_case)
- [ ] Error paths are captured (via `err` or manual)
- [ ] Self-loop test passes (spans AND logs visible)
- [ ] Minimal code changes (prefer `#[instrument]` over manual spans)

---
title: Send Pyroscope profiles to sigiro
description: Send pprof profiles from the verified Pyroscope Rust SDK to sigiro over Pyroscope-compatible HTTP, then confirm the profile rows with SQL.
sidebar:
  order: 6
---

This guide shows you how to send application CPU profiles to sigiro. It uses the
verified first-party Pyroscope Rust SDK and its pprof backend.

This is different from [sending your agent's own
telemetry](/docs/how-to/agent-telemetry), which records agent sessions with
OpenTelemetry. Use this guide when the process that you want to inspect produces
pprof profiles.

## Before you start

You need a sigiro server. Start one with `sigiro serve`, or use the Docker command
in the [quickstart](/docs/tutorials/quickstart#1-run-it).

Pyroscope-compatible HTTP is available on the main sigiro port, `9999`, and on
the OTLP HTTP port, `4318`. Use the main port unless you deliberately separate
ingest traffic. Sigiro does not listen for profiles on port `4040`.

A self-hosted server in Open auth mode needs no key. A server in TenantKeys mode,
including hosted sigiro, requires `Authorization: Bearer YOUR_TENANT_KEY`.

## Configure pyroscope-rs

Sigiro verifies the first-party `pyroscope` crate with its pprof backend. This
repository currently pins version `2.0.4`:

```toml
[dependencies]
pyroscope = { version = "=2.0.4", features = ["backend-pprof-rs"] }
```

For self-hosted sigiro, use this target and no headers:

```rust
let endpoint = "http://localhost:9999";
let http_headers = None;
```

For hosted sigiro, use the HTTPS origin with no port and add the tenant key:

```rust
use std::collections::HashMap;

let endpoint = "https://api.sigiro.com";
let mut headers = HashMap::new();
headers.insert(
    "Authorization".to_string(),
    format!("Bearer {}", std::env::var("SIGIRO_API_KEY")?),
);
let http_headers = Some(headers);
```

Pass that target and optional header map to the SDK at application startup:

```rust
use pyroscope::backend::pprof::PprofConfig;
use pyroscope::backend::{BackendConfig, pprof_backend};
use pyroscope::pyroscope::PyroscopeAgentBuilder;

let backend = pprof_backend(PprofConfig::default(), BackendConfig::default());
let mut builder = PyroscopeAgentBuilder::new(
    endpoint,
    "checkout",
    100,
    "pyroscope-rs",
    env!("CARGO_PKG_VERSION"),
    backend,
);
if let Some(headers) = http_headers {
    builder = builder.http_headers(headers);
}
let _profile_agent = builder.build()?.start()?;
```

Keep `_profile_agent` in scope for the life of the process. The SDK sends a batch
every 10 seconds. If the handle is dropped early, no later batch reaches sigiro.

Do not put the tenant key in source control. Set `SIGIRO_API_KEY` in the
environment that starts the process.

## HTTP contract

The SDK can post to any of these equivalent paths:

- `/`
- `/push.v1.PusherService/Push`
- `/ingest`
- `/ingest/pyroscope`

The Rust SDK appends `/push.v1.PusherService/Push` to the target above. A target
that already includes that path can therefore send to the wrong URL without an
obvious error in sigiro. Give this SDK the origin only.

Sigiro accepts these request forms:

- `application/proto`, `binary/octet-stream`, or no content type for a gzipped
  Pyroscope `PushRequest`
- `multipart/form-data` with a file field named exactly `profile` for either raw
  pprof bytes or a `PushRequest`

The `name` query parameter changes how sigiro decodes the body. If `name` is
present, sigiro treats the `profile` bytes as raw pprof and uses the text before
the first `{` as the service name. If `name` is absent, the body must decode as a
`PushRequest`, and each series must contain a non-empty `service_name` label.
Sending raw pprof without `name`, or a `PushRequest` without `service_name`,
returns `400` and stores nothing.

Sigiro accepts gzip and limits each request to 8 MiB after decompression by
default. A request over the HTTP body limit returns `413`. A malformed or
unsupported profile returns `400`, a missing or invalid TenantKeys bearer token
returns `401`, and a capacity gate returns `507`. A `200` response means sigiro
accepted the batch.

Send pprof data only. Sigiro does not decode Java Flight Recorder data, so this
guide does not claim Java or JFR compatibility. Other language SDK setups are
omitted until they have a runnable end-to-end verification.

## Confirm that it works

Start the application and let it do some work for at least 10 seconds. Then read
the profile rows back from the main HTTP API:

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

For hosted sigiro, change the origin and add
`-H "Authorization: Bearer YOUR_TENANT_KEY"`. A non-empty result confirms the SDK
push, profile decoding, and HTTP readback. An empty result after one full SDK
interval usually means the profile handle was dropped, the target includes an
extra path, or the service name in the query does not match the SDK configuration.

## Next

- [Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding) — configure
  hosted authentication and the other telemetry signals
- [About the tables](/docs/explanation/tables) — see which stored telemetry each
  table contains

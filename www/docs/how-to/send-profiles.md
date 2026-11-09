---
title: Send profiles to sigiro
description: Send continuous profiles to sigiro over OTLP, from otelprofile for Go, the OpenTelemetry eBPF profiler, or an OpenTelemetry Collector, then confirm the profile rows with SQL.
sidebar:
  order: 7
---

This guide shows you how to send continuous profiles (CPU, heap and the other
pprof profile types) to sigiro. sigiro accepts profiles in one format: OTLP
profiles over HTTP or gRPC, the OpenTelemetry profiles signal.

This is different from [sending your agent's own
telemetry](/docs/how-to/agent-telemetry), which records agent sessions with
OpenTelemetry. Use this guide when you want to see where a process spends its
CPU time and memory.

## Before you start

You need a sigiro server. Start one with `sigiro serve`, or use the Docker command
in the [quickstart](/docs/tutorials/quickstart#1-run-it).

A self-hosted server receives OTLP/HTTP on port `4318` and on the main port
`9999`, and OTLP/gRPC on port `4317`. Hosted sigiro receives OTLP/HTTP only, at
`https://sigiro.com` with no port.

A self-hosted server in Open auth mode needs no key. A server in Better Auth OAuth/JWT mode,
including hosted sigiro, requires `Authorization: Bearer YOUR_TENANT_KEY`.

The OpenTelemetry profiles signal is still in development upstream, and its wire
format has changed between releases. sigiro reads the layout of
`opentelemetry-proto` v1.10. A producer built on a different revision can send
profiles that sigiro rejects with `400`.

## Go: otelprofile

[otelprofile](https://github.com/akshatagarwl/otelprofile) profiles a Go program
from the inside, with Go's own runtime profiler, and exports OTLP profiles. Add it
to your module:

```bash
go get github.com/akshatagarwl/otelprofile@v0.2.0
```

Start it once, at program startup, and stop it on shutdown:

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "http://localhost:4318",
})
if err != nil {
    log.Fatal(err)
}
defer p.Stop()
```

By default it sends a heap profile every 15 seconds and a CPU profile every 30
seconds. Set `ProfileTypes`
(or `OTEL_GO_X_PROFILES_TYPES=all`) to add goroutine, mutex and block profiles.

For hosted sigiro, set the endpoint to `https://sigiro.com` and add the key:

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "https://sigiro.com",
    Headers: map[string]string{
        "Authorization": "Bearer " + os.Getenv("SIGIRO_ACCESS_TOKEN"),
    },
})
```

The standard `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS` and
`OTEL_SERVICE_NAME` environment variables work too, when the matching field is
left empty. Do not put the access token in source control.

If the program also sends traces through the OpenTelemetry Go SDK, otelprofile
labels CPU samples with the span they ran in, and the dashboard opens a span's
profile from the trace view. See the otelprofile README for the setup.

## Any Linux process: the OpenTelemetry eBPF profiler

The [OpenTelemetry eBPF profiler](https://github.com/open-telemetry/opentelemetry-ebpf-profiler)
samples the CPU of every process on a Linux host, with no change to the programs.
It runs inside a Collector distribution, `otelcol-ebpf-profiler`, as the
`profiling` receiver, and it needs root (or `CAP_SYS_ADMIN`, `CAP_PERFMON` and
`CAP_BPF`) and access to `/proc`.

The profiler names each process with `process.executable.name` but sets no
`service.name`, and sigiro rejects a profile without a service name. The
`resource` processor below fills it in from the executable name:

```yaml
receivers:
  profiling: {}

processors:
  resource:
    attributes:
      - key: service.name
        from_attribute: process.executable.name
        action: insert

exporters:
  otlp_http:
    endpoint: http://localhost:4318
    # For hosted sigiro:
    # endpoint: https://sigiro.com
    # headers:
    #   Authorization: "Bearer ${env:SIGIRO_ACCESS_TOKEN}"

service:
  pipelines:
    profiles:
      receivers: [profiling]
      processors: [resource]
      exporters: [otlp_http]
```

Start the Collector with the profiles feature gate:

```bash
sudo otelcol-ebpf-profiler --feature-gates=+service.profilesSupport --config config.yaml
```

Collector releases before the `otlp_http` name use `otlphttp` for the same
exporter.

## Through an OpenTelemetry Collector

A Collector that already receives your OTLP can forward profiles to sigiro. Add a
`profiles` pipeline from the `otlp` receiver to an `otlp_http` exporter, and start
the Collector with `--feature-gates=+service.profilesSupport`:

```yaml
receivers:
  otlp:
    protocols:
      http:
      grpc:

exporters:
  otlp_http/sigiro:
    endpoint: http://localhost:4318

service:
  pipelines:
    profiles:
      receivers: [otlp]
      exporters: [otlp_http/sigiro]
```

The exporter appends `/v1development/profiles` to the endpoint. Set
`profiles_endpoint` instead if you need to give the full URL.

## HTTP contract

- **Path:** `POST /v1development/profiles`.
- **Body:** a protobuf `ExportProfilesServiceRequest`, with content type
  `application/x-protobuf`. JSON is not accepted for profiles.
- **Service:** every resource must carry a `service.name` attribute. A request
  without one returns `400` and stores nothing.
- **Size:** gzip is accepted. A request is limited to 8 MiB after decompression
  by default, and a larger one returns `413`.
- **Errors:** a malformed profile, or one with no non-zero samples, returns
  `400`. A missing or invalid Better Auth OAuth/JWT bearer token returns `401`,
  and a capacity gate returns `507`. A `2xx` response means sigiro accepted the
  batch.

## Confirm that it works

Let the profiler run for at least one upload interval. Then read the profile rows
back from the main HTTP API:

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

For hosted sigiro, change the origin and add
`-H "Authorization: Bearer YOUR_TENANT_KEY"`. A non-empty result confirms the
export, profile decoding, and HTTP readback. Open the dashboard's **Profiles**
page to see the same data as a flame graph. An empty result after a full interval
usually means the endpoint is wrong, the service name in the query does not match
the producer, or the producer's log shows a `400` for a format sigiro does not
read.

## Next

- [Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding) — configure
  hosted authentication and the other telemetry signals
- [About the tables](/docs/explanation/tables) — see which stored telemetry each
  table contains

---
title: Try sigiro beside Jaeger
description: Mirror OTLP traces from an existing Jaeger v2 Collector to a loopback-only sigiro instance without repointing applications.
sidebar:
  order: 8
---

This is a **traces-only evaluation**, not a migration. Keep applications sending
to the existing Jaeger v2 OTLP receiver on `4317`/`4318`. Add sigiro as a second
exporter on that receiver's **traces** pipeline; retain Jaeger's storage
exporter and every other pipeline. Jaeger accepts trace data, not OTLP logs or
metrics. If a separate OpenTelemetry Collector fronts Jaeger, make the same
traces-pipeline change there instead, retaining its existing Jaeger destination.

## 1. Start sigiro on different host ports

On the **same host** as Jaeger, with `14317`, `14318`, and `9999` free:

```bash
docker run -d --name sigiro-trial --restart unless-stopped \
  -p 127.0.0.1:14317:4317 \
  -p 127.0.0.1:14318:4318 \
  -p 127.0.0.1:9999:9999 \
  -v sigiro-trial-data:/var/lib/sigiro \
  ghcr.io/sigiroai/sigiro
curl --fail http://127.0.0.1:9999/health
```

`14317` is the remapped gRPC port; this guide uses only OTLP HTTP on
`14318`. If `9999` is occupied, remap its **host** side too (for example,
`-p 127.0.0.1:19999:9999`) and use that port in the checks below. Wait for
health to succeed, or inspect `docker logs sigiro-trial`. Do not change any
application exporter endpoint.

**Network boundary:** the example assumes Jaeger/Collector can reach the host's
`127.0.0.1:14318` from its own network namespace. A Collector in a different
container or on another host cannot use that loopback address; arrange a
private route through a TLS/authenticating proxy (or equivalent network
controls) and use its reachable address instead. Sigiro itself supplies no
authentication. Do not change the Docker binding to `0.0.0.0` just to make this
work.
Self-hosted sigiro does not authenticate OTLP or query requests: never publish
these ports directly to an untrusted network. A loopback binding alone does not
make local processes trusted.

## 2. Add a bounded mirror to the existing traces pipeline

Back up your Jaeger v2/Collector YAML. **Merge** the following entries into
it; this is a fragment, not a replacement configuration. Leave existing
receivers, processors, exporters, extensions, Jaeger storage, and non-trace
pipelines unchanged. If the traces pipeline has more exporters or processors,
retain them in their existing order. This example uses Jaeger **2.0.0** component IDs: that release calls the OTLP
HTTP exporter `otlphttp`. Newer Collector distributions call it `otlp_http`
(the old spelling is a deprecated alias). Check your installed binary with
`validate` and use the name it accepts consistently in both places.

```yaml
exporters:
  # Add alongside the existing jaeger_storage_exporter; do not replace it.
  otlphttp/sigiro:
    endpoint: http://127.0.0.1:14318
    sending_queue:
      enabled: true
      queue_size: 1000
    retry_on_failure:
      enabled: true
      initial_interval: 5s
      max_interval: 30s
      max_elapsed_time: 300s

service:
  pipelines:
    traces:
      # Preserve the actual receiver and processor lists from your config.
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger_storage_exporter, otlphttp/sigiro]
```

`endpoint` is an OTLP/HTTP **base** URL: the exporter appends `/v1/traces`.
Do not add `/v1/traces` here or use the per-signal `traces_endpoint` without
the full path. Plain HTTP here is appropriate only for same-host loopback;
`tls: { insecure: true }` is not needed for an `http://` endpoint. For a remote
private destination configure TLS and authentication on both sides instead.

The queue is **in memory** and holds at most 1,000 requests, not 1,000 spans.
Retries end after five minutes per batch; a full queue, expired retries, or a
Collector restart can lose mirrored spans. It is **not** guaranteed-lossless
fan-out. Monitor the Collector's exporter queue capacity/size and failed-send
and enqueue-failure metrics; size the queue against load and memory. If those
failures or Jaeger ingest errors rise, stop the trial and restore the original
config. Do not add the sigiro exporter to a metrics or logs pipeline in Jaeger.

## 3. Validate before changing the incumbent

First run the **same Jaeger v2 or Collector binary/image version** against the
edited file with its `validate --config=<edited-file>` command. Validation
checks the configuration, **not** network access, delivery, or storage. Then
start a separate scratch instance of that version with a copy of the edited
config: remap **every** listener in the scratch copy to unused ports, including
OTLP `4317`/`4318`, Jaeger query/UI `16686`, internal Prometheus metrics
`8888` (and any `8889`), health `13133`, and any other active extensions,
receivers, and query listeners. Isolate its storage from production or use a
read-only test backend; do not let a scratch exporter write to production
storage. Confirm the scratch process starts and sends a test trace to both
scratch destinations before restarting the incumbent. A Docker `-p` remap
alone does **not** change listeners inside a shared network namespace or
prevent storage collisions. If scratch isolation cannot be arranged, do not
use a production restart as the first syntax test.

Once scratch checks pass, apply the edited config using your normal controlled
Jaeger/Collector restart or rollout. Check its startup logs and the existing
Jaeger query UI immediately. Keep your original YAML for rollback.

## 4. Verify both backends

Generate a **new**, uniquely named test span through the **existing** app or
its original OTLP endpoint (not directly to sigiro). Record the trace ID,
service name, and time. After the batch/export delay:

1. Find that trace ID in the existing Jaeger UI (usually on port `16686`) and
   confirm its spans are present. This proves the incumbent storage path still
   works; a successful Collector startup does not.
2. On sigiro, check `curl --fail http://127.0.0.1:9999/v1/services` for the
   service. Query for the exact trace ID within the test window using
   `POST /v1/query`, or find the corresponding span there. For example, after
   substituting your trace ID and a UTC time window around the test span. Use
   literal bounds: `now()` can prevent DuckLake file pruning, and the bounded
   query policy requires a half-open literal interval.

   ```bash
   curl --fail -X POST http://127.0.0.1:9999/v1/query \
     --data "SELECT trace_id, span_name, timestamp FROM sigiro_spans
             WHERE trace_id = '<your-trace-id>'
               AND timestamp >= TIMESTAMP '<start-utc:YYYY-MM-DD HH:MM:SS>'
               AND timestamp < TIMESTAMP '<end-utc:YYYY-MM-DD HH:MM:SS>'"
   ```

If either backend lacks the span, inspect Jaeger/Collector logs, exporter
failure/queue metrics, Sigiro logs, the network namespace, and the endpoint
before drawing conclusions. Compare actual trace IDs, not only total counts;
sampling or filtering upstream can affect both destinations.

## Roll back

Restore the backed-up YAML (remove `otlphttp/sigiro` **from the traces
exporters list and the exporters map**), validate it, and use the same
controlled restart/rollout. Jaeger's original receiver, storage exporter,
application settings, and other pipelines remain in place. Confirm a new
trace in Jaeger. Then stop and remove the trial:

```bash
docker rm -f sigiro-trial
# Only after deciding to discard the trial's stored telemetry:
docker volume rm sigiro-trial-data
```

Removing the volume is irreversible; omit the last command to keep trial data.
Do not stop Jaeger or repoint applications as part of rollback.

## Configuration references

- [Collector configuration and validation](https://opentelemetry.io/docs/collector/configuration/)
- [Jaeger 2.0.0 exporter factory](https://github.com/jaegertracing/jaeger/blob/v2.0.0/cmd/jaeger/internal/components.go) and [Collector 0.112.0 component ID](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/internal/metadata/generated_status.go)
- [OTLP HTTP exporter settings (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/README.md)
- [Exporter queue/retry behavior (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/exporterhelper/README.md)
- [Collector troubleshooting and exporter metrics](https://opentelemetry.io/docs/collector/troubleshooting/)
- [Jaeger v2 API/port reference](https://www.jaegertracing.io/docs/2.0/architecture/apis/)

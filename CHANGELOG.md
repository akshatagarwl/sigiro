# Changelog

<!-- Changesets inserts each release above the newest version heading. -->

## 0.2.0

### Breaking changes

- **Pyroscope ingest is removed.** OTLP is now the only way to send profiles:
  `POST /v1development/profiles` over HTTP, or the OTLP profiles service over
  gRPC. See [Send profiles to sigiro](https://sigiro.com/docs/how-to/send-profiles).
  - **Removed routes:** `/`, `/push.v1.PusherService/Push`, `/ingest` and
    `/ingest/pyroscope` no longer accept profiles, on a self-hosted server or
    on hosted sigiro.
  - **Removed decoders:** the `transform_pyroscope_profiles` family in
    `sigiro-segments` and the WASM `decode_pyroscope` export.
  - **Worker WAL:** a hosted ingest cell drops any queued Pyroscope request it
    still holds when it next flushes, and logs it.
- **Self-profiling now uses OTLP.** `SIGIRO_PYROSCOPE_ENDPOINT` is gone.
  - Set `SIGIRO_SELF_PROFILING=true` to profile sigiro's own CPU, and its heap
    when jemalloc profiling is on (`MALLOC_CONF=prof:true,prof_active:true`).
  - Profiles go to `OTEL_EXPORTER_OTLP_PROFILES_ENDPOINT`, else
    `OTEL_EXPORTER_OTLP_ENDPOINT` plus `/v1development/profiles`, else the
    server itself. They are sent with `SIGIRO_OTEL_AUTH_KEY` as the bearer key.
  - The instance status key `pyroscope_status` is now `self_profiling_status`.
    Rows written before the change keep the old key.

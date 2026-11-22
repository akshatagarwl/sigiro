# Sigiro — observability that's cheap, local, and SQL-native

Observability got expensive because it got complicated: clusters to run,
proprietary query languages to learn, and a bill that grows with every span.
It doesn't have to be that way.

Sigiro is one binary. It takes OpenTelemetry traces, logs, metrics, and profiles,
writes them as Parquet files on your own disk, and lets you query all of it with
plain SQL. When you want to keep data for months instead of days, point it at S3
and let cheap object storage hold the history.

SQL matters for another reason: it's the language coding agents query best. Hand
yours the CLI or MCP server and it can investigate your telemetry the same way it
already reasons about data. When you'd rather look for yourself, the dashboard
is right there.

[Website](https://sigiro.com) · [Quickstart](https://sigiro.com/docs/tutorials/quickstart) · [Documentation](https://sigiro.com/docs) · [Releases](https://github.com/akshatagarwl/sigiro/releases) · [Contributing](CONTRIBUTING.md)

[![Latest release](https://img.shields.io/github/v/release/akshatagarwl/sigiro)](https://github.com/akshatagarwl/sigiro/releases/latest)
[![ci](https://github.com/akshatagarwl/sigiro/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/akshatagarwl/sigiro/actions/workflows/ci.yml)
[![License: FSL-1.1-ALv2](https://img.shields.io/badge/license-FSL--1.1--ALv2-blue)](LICENSE.md)

![Dashboard overview: requests, errors, latency and log volume for two small demo services](docs/assets/dashboard-overview.png)

## Run it

```bash
curl -fsSL https://sigiro.com/install | sh
SIGIRO_BIND=127.0.0.1 sigiro serve
```

No account or configuration file is required. Open-mode servers listen on
`127.0.0.1` by default:

| Port | What |
|---|---|
| `4317` | OTLP gRPC |
| `4318` | OTLP HTTP |
| `9999` | dashboard (`/dashboard/`), HTTP API, `/health` |

![Your apps send OTLP to the sigiro binary, which stores Parquet files and answers through the dashboard, HTTP API, CLI and MCP](docs/assets/flow.svg)

Data goes to `~/.local/share/sigiro` on Linux and `~/Library/Application Support/sigiro` on macOS.
Change it with `--data-dir` or `SIGIRO_DATA_DIR`.
Local open mode has no login and makes no calls to Sigiro Cloud authentication.
The dashboard reads `/v1/auth/config` to learn the server's mode; an unreachable
server shows a connection error, never a sign-in prompt.

Network exposure is explicit: `SIGIRO_BIND` changes the listener address. An open
server does **not** authenticate requests, even on a private network. If you
expose it beyond loopback, put it behind an authenticated reverse proxy or an
access-controlled network perimeter. Do not expose it directly to the internet.
Hosted JWT-authenticated deployments retain their existing listener defaults.

In Docker, pass `-e SIGIRO_BIND=0.0.0.0` so published ports can reach the
listener **inside the container**. Keep host-side publishing on loopback
(for example, `-p 127.0.0.1:9999:9999`); the container listener setting does
not provide authentication.

## Send it telemetry

Point any OpenTelemetry SDK or Collector at it:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_SERVICE_NAME=checkout
```

Use a shell without hosted exporter authorization headers. Local ingestion needs
no `sigiro auth login` or `sigiro auth token`. Port `4318` is for OTLP/HTTP;
`4317` is for OTLP/gRPC. Neither the dashboard port nor a frontend preview port
is an ingestion endpoint.

![A span waits in memory, is flushed to a Parquet file within a second, can be queried, and is deleted after 7 days](docs/assets/life.svg)

Data is kept for 7 days. Set `SIGIRO_RETENTION_DAYS=0` to keep it all.

For long-term retention, store the Parquet files in S3 or an S3-compatible bucket
instead of on local disk. Set `SIGIRO_S3_BUCKET`, `SIGIRO_S3_REGION`,
`SIGIRO_S3_ENDPOINT`, and credentials (`SIGIRO_S3_KEY_ID`/`SIGIRO_S3_SECRET`, or
the standard `AWS_*` variables). S3 storage also needs a PostgreSQL catalog via
`SIGIRO_CATALOG_URL`.

## Look at it

Open <http://localhost:9999/dashboard/>. Navigation has five destinations:
Overview, Traces, Logs, Profiles, and SQL. There is no separate Services or Metrics
page; use Overview for summaries and SQL for the stored telemetry tables.

Overview starts with all services, the last 24 hours, and live refresh off. It
shows loading and query failures separately from a successfully loaded empty
window. If it is empty, check the exporter destination, selected service, and time
range; data outside that scope will not appear. Enable Live for refreshes every
10 seconds, or refresh manually. SQL defines its own query window.

**Local setup:** the current empty-state instructions describe hosted ingestion;
use the [local quickstart](https://sigiro.com/docs/tutorials/quickstart) instead.
[Known entry and first-run limitations](docs/dashboard-state-machines.md#current-entry-and-first-run-behavior)
include misleading sign-in fallback in frontend-only previews. The binary also
serves documentation at <http://localhost:9999/docs/>.

Click a trace to see its waterfall:

![A checkout trace: the call to the inventory service and its slow database lookup](docs/assets/dashboard-trace.png)

Profiles get a flame graph. This one is sigiro profiling itself:

![Profiles page: CPU time over 15 minutes, top functions and a flame graph of the sigiro process](docs/assets/dashboard-profiles.png)

## Ask it

SQL over HTTP. The body is one `SELECT`:

```bash
curl -s localhost:9999/v1/query --data-binary \
  "SELECT service_name, count(*) AS spans FROM sigiro_spans GROUP BY 1 ORDER BY 1"
```

```json
[{"service_name":"checkout","spans":6759},{"service_name":"inventory","spans":4502},{"service_name":"latency-probe","spans":1}]
```

Give your agent the MCP server (`list_services`, `run_query`, plus docs search):

```bash
claude mcp add sigiro -- sigiro mcp
```

Check that data is arriving:

```bash
sigiro status
```

The CLI prints JSON. The tables are listed in the
[telemetry tables reference](https://sigiro.com/docs/reference/telemetry-tables).

> If you have signed in to sigiro.com, the CLI and MCP talk to it instead.
> Set `SIGIRO_ENDPOINT=http://localhost:9999` to use your own server.

## Hosted

Don't want to run it? Use the hosted service at [sigiro.com](https://sigiro.com):

```bash
sigiro signup --name "Your Name" --email you@example.com
```

Docs: <https://sigiro.com/docs>.

## Build from source

sigiro builds with [Nix](https://nixos.org) (flakes enabled), on x86_64 and
aarch64 Linux and on Apple silicon. Everything is pinned, and the build runs from
locked inputs in an offline sandbox.

```bash
git clone https://github.com/akshatagarwl/sigiro.git
cd sigiro
nix build                          # ./result/bin/sigiro, dashboard and docs built in
SIGIRO_BIND=127.0.0.1 ./result/bin/sigiro serve
```

The result is one self-contained file. On Linux it is fully static: it needs no
libc, no shared libraries, no CA bundle and no network to start, and every
DuckDB extension it uses is compiled in. Copy it to any Linux machine and run it.

For working on the code, `nix develop` opens a shell with the pinned Rust
toolchain, Node, pnpm and DuckDB; `cargo build` and `cargo test` then work as
usual (binaries land in `target/<target triple>/`). `nix flake check` runs
everything CI runs on the Rust side.

For dashboard development, run a current `sigiro serve` locally, then run the
Next.js dev server in `dashboard/`. Development requests under `/v1/` proxy to
`http://127.0.0.1:9999` by default; set `SIGIRO_DEV_BACKEND` to another backend
origin if needed. This does not proxy production sign-in. Static production
builds still use same-origin API routes supplied by their serving infrastructure.
Checks and the repo layout are in [CONTRIBUTING.md](CONTRIBUTING.md).
Words used in the code are defined in [CONTEXT.md](CONTEXT.md).
Report security issues as described in [SECURITY.md](SECURITY.md).

## License

[FSL-1.1-ALv2](LICENSE.md), © 2026 Akshat Agarwal.
Use it for anything except a competing commercial product.
Each version becomes Apache 2.0 two years after its release.
FSL is source-available, not an OSI-approved open-source license.

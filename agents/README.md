# sigiro agents

The customer-hosted agent runs exactly two long-lived supervised processes:

```text
S3 or R2 ── celld v0.4.0 ── copied @ewhauser/world-celld@0.4.1 worker ── Eve 0.51 Workflow runtime
```

S3 stores celld state. The image copies
`node_modules/@ewhauser/world-celld/celld-worker` to `/app/world-celld-worker`,
deploys that worker during startup, and then supervises only celld and Eve.
Both service ports stay on container loopback. The host celld mapping is named
`CELLD_PORT`.

Each job's repository sandbox runs remotely on Ascii Box rather than as a
container on this host. `agent/lib/box-sandbox.ts` adapts `@asciidev/eve-box`
so Eve 0.51 can drive it; see [the threat model](docs/threat-model.md) for
what that sandbox does and does not isolate.

## Security boundaries

One deployment is one customer security domain, not a hostile multi-tenant
service. The trusted agent container stores provider authentication in
`agent-data` and holds the Box API key; a job sandbox never receives that key,
celld's world secret, the workflow callback secret, or AWS credentials.
Bind published ports to loopback or an authenticated private tunnel.
Customer GitHub and subscription-provider login remain native client flows.

The deployment creates a persistent `WORLD_SECRET` and a separate callback
secret under `/var/lib/sigiro-agent`. They are never placed in the sandbox
environment. The world secret authenticates world-celld requests; the callback
secret is forwarded by the deployed worker as `x-workflow-callback-secret`.
Eve 0.51 source does not document or consume a callback-secret environment
variable, so none is invented here.

## Workflow and publication

The public job surface is the world-celld HTTP API:

- `POST /v1/jobs` submits a job.
- `GET /v1/jobs/{jobId}` reads status and the terminal result.
- `POST /v1/jobs/{jobId}/cancel` requests cancellation.

The exact request and response field names must follow the source-side contract
when that contract is finalized. A submitted job is not a publication result.
A successful publication is proved by a durable receipt containing the tested
commit and draft pull request metadata. A completed model turn, HTTP 2xx, or
workflow event alone is not proof that a pull request exists.

World-celld is experimental. The integration must prove compatibility between
Eve 0.51, `@workflow/*`, and `@ewhauser/world-celld@0.4.1` in an end-to-end
run. A package or build-version difference alone is not a confirmed runtime
failure.

## Development

```sh
bun install
bun run build
bun run eve:build
```

See [customer-hosted setup](docs/customer-hosted.md) for Docker Compose
installation, login, health checks, and direct API examples. See the
[threat model](docs/threat-model.md) for deployment boundaries and known
limitations.

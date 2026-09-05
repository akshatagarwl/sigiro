# Real anomaly-to-PR E2E fixture

This disposable checkout service deliberately fails when a request omits its
coupon. `npm test` starts the service and sends real HTTP requests covering
missing, empty, padded, valid, and unknown coupons, plus unsupported routes and
methods. It must fail before the agent repairs `app.mjs`.

The fixture uses the OpenTelemetry SDK. Its `x-e2e-event-time` header compresses
five hours of baseline and regression buckets into a short test run. Requests
still execute against the real service, and Sigiro still ingests OTLP, runs its
normal five-minute scheduled detector, and serves its real diagnosis API.

Copy this directory into a new disposable private GitHub repository, excluding
this Dockerfile if desired. Commit and push it, and run `npm ci` in that checkout.
Use your own repository in the bridge policy and `SIGIRO_REPOSITORY`; this test
creates a real draft PR. Allow only `app.mjs`, set `maxAttempts: 1`, and require
`npm ci --ignore-scripts --offline && npm test`.

Build the sandbox with:

```sh
docker build -t sigiro-bridge-fixture:local agents/test/fixtures/bridge
```

Provision isolated current-source Sigiro, celld, Eve, and an outbound worker as
described in [the bridge guide](../../../docs/anomaly-bridge.md). Set
`EVE_SANDBOX_IMAGE=sigiro-bridge-fixture:local` for Eve. Use fresh bridge, worker,
coordinator, and receipt state; configure the same local evidence directory and
worker token for the bridge and Eve.

In addition to the bridge environment, set these absolute paths and loopback
endpoint, then run from `agents/`:

```sh
export SIGIRO_E2E_CONFIG=/absolute/path/to/policy.json
export SIGIRO_E2E_FIXTURE_DIR=/absolute/path/to/disposable-checkout
export SIGIRO_E2E_OUTPUT_DIR=/absolute/path/to/proof
export SIGIRO_E2E_OTLP_URL=http://127.0.0.1:14328
bun test/bridge-e2e.ts traffic
bun test/bridge-e2e.ts verify
bun test/bridge-e2e.ts negative
```

`verify` waits for the scheduled detector, races two bridge processes, loses the
first successful **real celld** submission response, verifies the identical
retry, waits for the actual worker/Eve draft PR, and runs HTTP tests from its
published commit. It also checks that an open PR blocks another repair.

`negative` uses copies of the submitted job to exercise corrupt, oversized, and
missing evidence; an unreachable coordinator; process death before POST; and
restart with the identical frozen job. Its replay uses the already-existing job
ID, so these checks do not create another repair. It verifies coordinator
authentication and observes bridge/worker/Eve spans through Sigiro. Enable the
existing local telemetry receiver at `http://127.0.0.1:14318` for these checks.

Proof JSON, process logs, and an independent checkout of the published commit
remain in `SIGIRO_E2E_OUTPUT_DIR`. Stop the isolated runtimes afterward; retain
the draft PR for review. The test never merges it.

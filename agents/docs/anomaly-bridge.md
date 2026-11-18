# Anomaly → diagnosis → job

The bridge runs on the trusted Eve host. Each pass selects at most one unhandled
incident from `/v1/anomalies`, requests `/v1/diagnose` for its frozen service and
time window, saves evidence, resolves the configured GitHub branch, and submits
an immutable job to celld. The existing worker and Eve then investigate, edit,
test, and publish a draft PR.

## Configure and run

Run from `agents/` after `aube install --frozen-lockfile` and `aube run build`.
The host needs an
authenticated `gh` CLI with access to the repository. Configure the existing
coordinator, pull worker, and Eve separately.

Use one bridge deployment for one source, service, and repository, with one
persistent state directory on one trusted host. The repository must match Eve's
`SIGIRO_REPOSITORY`. The evidence directory must be the same directory, visible
at the same absolute path, as Eve's `SIGIRO_EVIDENCE_DIR`.

Example `policy.json` (replace the service, source, repository, paths, and test
command with the actual trusted configuration):

```json
{
  "sourceId": "production-project",
  "service": "checkout",
  "repository": "example/checkout",
  "baseRef": "main",
  "maxAgeSeconds": 3600,
  "policy": {
    "maxAttempts": 1,
    "deadlineSeconds": 900,
    "allowedPaths": ["src/**"],
    "tests": [{ "name": "e2e", "command": "bun run test:e2e" }]
  }
}
```

`sourceId` is a stable, nonsecret identity for the Sigiro project whose telemetry
the API key reads. Keep it unchanged when rotating credentials or moving the
endpoint. `baseRef` is a short branch name such as `main` or `release/next`, not
`refs/heads/main`, a tag, or a SHA. Required tests and allowed paths must be
nonempty; commands must work inside the existing Eve sandbox image.

```sh
export SIGIRO_URL=http://127.0.0.1:8080
export SIGIRO_API_KEY_FILE=/absolute/path/secrets/sigiro-api-key
export COORDINATOR_URL=http://127.0.0.1:9876
export EVE_WORKER_TOKEN_FILE=/absolute/path/secrets/worker-token
export SIGIRO_REPOSITORY=example/checkout
export SIGIRO_EVIDENCE_DIR=/absolute/path/sigiro-evidence
export BRIDGE_STATE_DIR=/absolute/path/sigiro-bridge

bun run bridge --config policy.json
bun run bridge --config policy.json --watch
```

Use the actual Sigiro HTTP API address for `SIGIRO_URL`. Secret files contain
only their secret value. `SIGIRO_API_KEY` and `EVE_WORKER_TOKEN` are the direct
environment alternatives; a `_FILE` setting takes precedence. The worker token
must match the coordinator's configured `WORKER_TOKEN`.

The default command runs one pass and prints a JSON result: `idle`, `waiting`, or
`submitted`. `--watch` repeats after `BRIDGE_POLL_SECONDS` (default `300`).
Transient failures are logged and retried on the next pass; permanent bridge
failures stop the process. HTTP requests also retry transient failures up to
three times with bounded exponential backoff, using the same request payload.

`maxAgeSeconds` must be positive and at most `604800` (seven days). Enabling the
bridge can submit existing anomalies inside this age window; it does not start
from an activation checkpoint. Choose the window deliberately. An incomplete
detection bucket is saved and waits for completion before diagnosis.

## State and recovery

```text
GET anomalies
      |
      v
  DETECTED -------- diagnosis + SHA + evidence --------> READY
  saved rows,                                          saved JobSpec,
  identity, bounds,                                   evidence URI + hash
  frozen settings                                            |
      |                                                       | POST /v1/jobs
      | preparation error                  submission error   |
      +--------------------> BLOCKED <------------------------+
                             permanent                        |
                             failure                          v
                                                          SUBMITTED
                                                          acknowledged job
```

Transient errors preserve the current state with `lastError`. A ready job is
replayed exactly after a timeout, process restart, or lost coordinator response:
its ID, evidence, SHA, prompt, and policy do not change. Permanent preparation or
submission errors persist `blocked`; a blocked record retains any prepared job
for investigation. Records are atomically written and synced before submission.

Submitted jobs reserve this bridge's repository while queued, accepted, running,
or while their draft PR remains open. A closed or merged PR releases the
reservation. A `no_fix` result also releases it. Failed, cancelled, and
`needs_review` jobs keep the reservation and report `waiting`; investigate their
receipts and coordinator state before further work. The current worker normally
reports only success or failure, so `no_fix` is not yet a normal automatic exit.
A missing acknowledged job or an invalid publication URL fails loudly.

There is no automatic reset or retry command for blocked incidents. Preserve
the state and evidence while investigating; deleting state removes deduplication
and can launch the same work again. Changing policy affects new candidates;
saved candidates retain their original settings.

## Boundaries

- The source must expose the `truncated` boolean in `/v1/anomalies`. A truncated
  response is rejected because correlation over partial rows is incomplete.
  The bridge bounds anomaly responses to 1 MiB, diagnosis responses to 2 MiB,
  and saved evidence to Eve's 1 MiB limit. Missing signal coverage, mismatched
  diagnosis windows/services, and oversized evidence stop preparation. The full
  job is checked against Eve's 256 KiB dispatch limit before saving evidence.
- Deduplication remembers member identities from source, service, operation,
  signal, and shift timestamp. Overlapping regrouped incidents extend the
  remembered members. `detected_at` and evidence hashes are not incident keys.
  Fully shifted changepoints can still appear as new incidents. The source is
  a replaceable snapshot, not a durable incident feed: incidents disappearing
  between polls or during an outage can be missed.
- Reservation and deduplication are local to `BRIDGE_STATE_DIR`. Another state
  directory or host can submit concurrent work for the same repository. This
  version does not provide distributed repository admission or multiple-service
  scheduling.
- Publication identity still belongs to an execution attempt. The bridge does
  not guarantee one PR across expired leases, old executions, and new attempts.
  Durable publication reconciliation across attempts remains a prerequisite
  for that guarantee. The sample uses one attempt; this does not repair an
  ambiguous publication outcome.

DI is limited to the existing Effect HTTP client and child-process capabilities.
The bridge reuses secret loading and telemetry; set
`OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:14318` (or `http://sigiro:4318`
inside the Compose network) to emit its sanitized spans
and logs under `sigiro-agent-bridge`. SQLite is used solely for an ownership lock
released by the OS after process death. JSON records hold progress; there is no
additional workflow framework.

## Real E2E validation

The [tracked checkout fixture and harness](../test/fixtures/bridge/README.md)
exercise actual HTTP traffic, OTLP ingestion, scheduled detection, diagnosis,
coordinator submission, worker execution, and tested draft-PR publication.
They also exercise lost acknowledgements, concurrent runs, process death,
evidence verification, and repository reservation while the PR is open.

# Run Sigiro and the investigation agent with Docker Compose

This deployment has three long-running containers:

| Service | Responsibility | Persistent data |
| --- | --- | --- |
| `sigiro` | Application and agent telemetry | `sigiro-data` |
| `rustfs` | Celld object storage | `rustfs-data` |
| `agent` | Celld, Eve, and the pull worker, supervised together | `agent-data` |

Eve creates temporary Docker sandboxes for investigations. These are additional
containers while work runs. Use one deployment per customer and repository;
celld 0.4 is alpha, and this is not a shared hostile-tenant service.

Celld receives object-store credentials and a persistent `WORKER_TOKEN`, which
authenticates coordinator submission, status, claim, heartbeat, and completion
endpoints. The worker and Eve share this token. Celld does not receive the
customer's Claude or Codex login, and sandbox tools receive neither the worker
token nor object-store credentials.

## 1. Configure the installation

Install Docker Engine with the Compose plugin on your Linux host, then clone
Sigiro. From the repository root:

```sh
cd agents/deploy
cp env.example .env
chmod 600 .env
openssl rand -hex 32
```

Edit `.env` and set:

- `SIGIRO_REPOSITORY`: the GitHub `OWNER/REPO` the agent may repair.
- `EVE_SANDBOX_IMAGE`: an image already built on this Docker host containing
  that project's runtime and offline build/test dependencies.
- `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY`: local RustFS credentials; use the
  generated value as the secret key.

The sandbox has no network access, and images are not pulled during jobs. Build
and validate your project's sandbox image before submitting an investigation.
This deployment does not provide a universal dependency image or a measured
4 GB capacity guarantee.

```sh
docker compose config --quiet
docker compose build agent
```

## 2. Connect GitHub and Claude

Run the native login flows. They use the same persistent volume and user as the
running agent; neither login installs a client on the host.

```sh
docker compose run --rm --no-deps agent gh auth login --web --git-protocol https
docker compose run --rm --no-deps agent claude auth login --claudeai
```

Follow the displayed browser instructions using the customer's own accounts.
For a remote host, follow the native client's remote login instructions if a
callback cannot reach the container. Do not copy subscription tokens into `.env`.

Verify GitHub has access to the configured repository, replacing `OWNER/REPO`:

```sh
docker compose run --rm --no-deps agent gh auth status
docker compose run --rm --no-deps agent gh repo view OWNER/REPO --json nameWithOwner
docker compose run --rm --no-deps agent claude auth status
```

Claude inference uses `ai-sdk-provider-claude-code` and the official Claude Agent
SDK with the customer's native login. It is subject to the provider's account
permissions and subscription limits; this installation is not a token broker.
GitHub access must permit reading source and creating branches and pull requests.

Authentication lives under `/var/lib/sigiro-agent/auth` in `agent-data`. The
trusted agent container runs as root and controls the host Docker daemon through
its socket. Treat access to this container as access to the Docker host. Repository
sandboxes receive neither that socket nor GitHub/provider credentials. RustFS
credentials and worker authentication are not supplied to sandbox tools.

## 3. Start and check the services

```sh
docker compose up -d
docker compose ps
curl --fail http://127.0.0.1:19999/health
curl --fail http://127.0.0.1:18080/.well-known/celld/health
docker compose logs --tail=100 agent
```

These URLs use the default host ports in `.env`. Agent startup creates the RustFS
bucket, deploys the built coordinator, generates a persistent worker token, and
starts celld, Eve, and the worker. It does not use celld's development watcher.
The agent health check requires all three processes and the coordinator to be
available; it does not certify provider login or successful model inference.

Published ports bind to loopback. Submit jobs from the Docker host or an SSH
tunnel, supplying the worker bearer token for job creation and status. RustFS has
no published port. Application telemetry can use `http://sigiro:4318` from this
Compose network, or `http://127.0.0.1:14318` from the host. Remote application
collection requires a separately configured private connection or authenticated
ingestion endpoint. Agent telemetry exports to this stack's Sigiro receiver.

## 4. Submit an investigation

The [anomaly bridge](anomaly-bridge.md) can detect incidents, collect diagnosis
evidence, and submit jobs automatically. This Compose installation supervises
celld, Eve, and the worker only; it does not configure or start the bridge. Follow
the bridge guide to run it separately, including its required Sigiro API response
contract and shared evidence path. The manual submission flow below lets a trusted
operator collect evidence from Sigiro and submit it through the coordinator API.

Save the reviewed evidence as `evidence.json`. It must be JSON containing
`service`, `from_ts`, `to_ts`, and `diagnosis`, and no larger than 1 MiB. Copy it
into the agent and calculate the digest of the bytes the agent will read:

```sh
docker compose cp evidence.json agent:/var/lib/sigiro-agent/evidence/incident.json
docker compose exec agent sha256sum /var/lib/sigiro-agent/evidence/incident.json
```

Create `job.json` following [the job contract](../src/contracts.ts). For example,
replace every uppercase placeholder and use a real test command:

```json
{
  "jobId": "incident-001",
  "generation": 1,
  "repository": "OWNER/REPO",
  "baseRef": "main",
  "baseSha": "EXACT_COMMIT_SHA",
  "incidentId": "incident-001",
  "evidenceUri": "file:///var/lib/sigiro-agent/evidence/incident.json",
  "evidenceSha256": "SHA256_FROM_THE_COMMAND_ABOVE",
  "prompt": "Investigate the supplied evidence and fix the root cause. Run the required tests.",
  "policy": {
    "maxAttempts": 1,
    "deadlineSeconds": 600,
    "allowedPaths": ["src/"],
    "tests": [{ "name": "e2e", "command": "npm run test:e2e" }]
  }
}
```

Use the configured repository, its exact base commit, permitted paths, and at
least one required test. The evidence URI is an absolute path **inside the agent
container**, not a host path. Give each incident an immutable evidence file;
do not overwrite it while an investigation is running.

```sh
worker_token=$(docker compose exec -T agent cat /var/lib/sigiro-agent/worker.token)
curl --fail-with-body http://127.0.0.1:18080/v1/jobs \
  -H "Authorization: Bearer $worker_token" \
  -H 'Content-Type: application/json' --data-binary @job.json
curl --fail-with-body http://127.0.0.1:18080/v1/jobs/incident-001 \
  -H "Authorization: Bearer $worker_token"
unset worker_token
```

A successful job returns a draft PR URL and actual test results. Publication
requires the tests to pass and changed paths to satisfy policy. Nothing merges
automatically. The ignored local demo helpers are not needed for these API calls.

## Recovery and upgrades

For diagnosis, start with `docker compose ps` and `docker compose logs --tail=100
agent sigiro rustfs`. Check authentication with the commands above. If a provider
requires sign-in again, repeat its native login against the existing volume.

```sh
docker compose restart agent
# After updating the checkout:
docker compose build agent
docker compose up -d
# Stop while retaining stored data and authentication:
docker compose down
```

Back up all three volumes consistently. `agent-data` contains authentication,
worker jobs, publication receipts, evidence, and Eve state; RustFS contains celld's
object store, and Sigiro retains telemetry locally. Never use `docker compose down
-v` when you intend to preserve the installation. If credentials are lost, sign
in again; if durable execution data is lost, do not assume in-flight work can
resume safely.


## Known concurrency limitation

The single-worker manual repair flow and persistence were exercised successfully.
An additional concurrent queue probe timed out against production object storage;
see [issue #50](https://github.com/akshatagarwl/sigiro/issues/50).
Concurrent submissions and multi-worker operation are not verified by this setup.

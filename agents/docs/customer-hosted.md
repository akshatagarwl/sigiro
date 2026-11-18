# Run a customer-hosted pull worker

This deployment runs one celld fleet and one outbound pull worker for one
customer. Do not put mutually distrusting customers in the same fleet. Celld
v0.4.0 is alpha and is not safe for hostile multi-tenant use.

The worker makes outbound requests to celld. It has no published port and no
inbound worker listener. A local or customer-hosted worker therefore works
without inbound worker networking, proxying, or worker TLS. Celld's public
listener is local-only in this example so clients on the host can submit and
read jobs.

## Credential boundary

Celld receives an object-store credential and `WORKER_TOKEN`, which is used
only on worker-facing COORDINATOR claim, heartbeat, and completion endpoints. The worker receives
the same token from a customer-owned file, plus the customer's coding-provider
login. Celld never receives Claude or Codex OAuth state. The provider process
also does not receive object-store credentials or the worker token.

The coordinator and the Eve host currently share one worker token: the same
secret authenticates the worker to the coordinator and the worker's dispatch to
Eve's `/internal/celld/job` route, so it is one credential and not two.

GitHub App credentials are read from `GITHUB_APP_ID`,
`GITHUB_APP_PRIVATE_KEY`, and `GITHUB_WEBHOOK_SECRET`, or from the
corresponding `_FILE` variants naming a mode `0600` file. A `_FILE` value is
read lazily on first use, so the secret never enters the environment.

## Prepare the host

From `agents/deploy`:

```sh
cp env.example .env
```

Create state, workspace, and provider-login directories for uid/gid 65532:

```sh
sudo install -d -m 0700 -o 65532 -g 65532 /var/lib/sigiro-agent/celld
sudo install -d -m 0700 -o 65532 -g 65532 /var/lib/sigiro-agent/jobs
sudo install -d -m 0700 -o 65532 -g 65532 /var/lib/sigiro-agent/workspaces
sudo install -d -m 0700 -o 65532 -g 65532 /var/lib/sigiro-agent/auth/claude
sudo install -d -m 0700 -o 65532 -g 65532 /var/lib/sigiro-agent/auth/codex
```

Set the matching paths, a unique worker ID, and provider binaries in `.env`:

```text
CELLD_STATE_DIR=/var/lib/sigiro-agent/celld
EXECUTOR_STATE_DIR=/var/lib/sigiro-agent/jobs
EXECUTOR_WORKSPACE_DIR=/var/lib/sigiro-agent/workspaces
EXECUTOR_AUTH_DIR=/var/lib/sigiro-agent/auth
WORKER_ID=customer-a-worker-1
```

Create a mode `0600` vars file with one line, and a separate mode `0600` token
file. Both contain only the customer-owned worker credential:

```text
WORKER_TOKEN=<random token>
```

Set `COORDINATOR_VARS_FILE` and `WORKER_TOKEN_FILE` to their absolute paths.
Set the bucket, endpoint, region, and bucket-scoped object-store credentials.
Use one bucket or key prefix per customer fleet. The object store must support
conditional writes.

## Authenticate the subscription

Provider login stays in the customer-owned directories mounted into the worker.
It is never copied into an image, celld, or a Sigiro service. Authenticate the
unmodified client with the mounted customer home:

```sh
docker compose run --rm --entrypoint claude worker setup-token
docker compose run --rm --entrypoint codex worker login
```

Both paths use a subscription and no API key. Verify the login files exist
under the corresponding customer-owned directory before starting the worker.

## Deploy and start

Validate and build the deployment:

```sh
docker compose config --quiet
docker compose build celld-deploy worker
docker compose up -d --build
docker compose ps
curl --fail http://127.0.0.1:${CELLD_PUBLIC_PORT:-8080}/.well-known/celld/health
```

The worker has outbound access through the control and egress networks. It has
no `ports` or `expose` entry. Startup has no cycle: celld waits only for the
one-shot deployment, while the worker starts independently and polls celld.

Inspect logs without printing environment variables:

```sh
docker compose logs --tail=100 celld worker
```

## Recovery and limits

`CELLD_STATE_DIR` and the object store hold celld state. `EXECUTOR_STATE_DIR`
holds local worker records and `EXECUTOR_WORKSPACE_DIR` holds disposable
checkouts. The provider login remains under `EXECUTOR_AUTH_DIR`.

The loop ends at a tested patch in the job record. Real private-repository
authentication, GitHub draft-PR publishing, trigger and dedup integration,
hosted tenant isolation, and production celld fleet failover remain open. Eve's
GitHub integration was audited and is not used: it requires the Eve runtime and
lacks a reusable branch, commit, and draft-PR publisher. Do not claim
any-infrastructure or any-repository support yet.

There is no GitHub credential in this deployment and no evidence broker. A job
carries its prompt and an evidence reference; it does not read telemetry itself.

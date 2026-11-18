# Customer-hosted agent with Docker Compose

This installation runs Sigiro and one trusted agent container. The agent
container has exactly two long-lived supervised processes: celld v0.4.1 and
Eve 0.51. Startup deploys the copied worker shipped by
`@ewhauser/world-celld@0.4.1`. Each job's repository sandbox runs remotely on
Ascii Box, not in this container.

Use one deployment per customer and repository. World-celld is experimental
and this Compose file is not a hostile-tenant boundary.

## Provision what you own

Before configuring the deployment, provision two things yourself:

- An S3 bucket for celld state. Amazon S3 and Cloudflare R2 both work.
  celld requires a store with conditional-write support for its ownership
  fencing. Amazon S3 and Cloudflare R2 both provide it. MinIO Community does
  not, which is why this deployment no longer runs a local object-store
  container. Create the bucket and its credentials before you start the agent.
  For R2, create the bucket with `wrangler r2 bucket create NAME`, then set
  `S3_ENDPOINT` to `https://ACCOUNT_ID.r2.cloudflarestorage.com` and
  `AWS_REGION` to `auto`. Wrangler cannot create the S3 credentials: make an
  R2 API token in the dashboard under Account Details, API Tokens, Manage, and
  use its Access Key ID and Secret Access Key as the AWS variables.
- A Box API key from [box.ascii.dev](https://box.ascii.dev). Job sandboxes
  run as boxes under this account.

## Configure

From the repository root:

```sh
cd agents/deploy
cp env.example .env
chmod 600 .env
```

Set these values in `.env`:

- `SIGIRO_REPOSITORY`: the permitted GitHub `OWNER/REPO`.
- `BOX_API_KEY`: the Box API key for the account job sandboxes run under.
- `CELLD_BUCKET` and `AWS_REGION`: the S3 bucket and region you provisioned.
- `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`: the standard AWS
  credentials for the IAM identity celld uses to reach that bucket.

Build and inspect the resolved Compose configuration:

```sh
docker compose config --quiet
docker compose build agent
```

Job sandboxes run on Box, not on this Docker host, so this container no
longer needs the Docker socket or a locally built project image. Box does
not enforce Eve's fine-grained network policies, so sandbox code can reach
the network; see [the threat model](threat-model.md) for what that changes
and what still holds. Boxes run only in Box's EU regions (Germany, Finland,
France), so job data and box snapshots stay there. Box machines come in
`small`, `default`, and `large`, all `x86_64`; billing is per second of
machine time, with 2 TB of egress included per box per month. Box has no
compliance certification yet; SOC 2 is in progress.

A cold box carries language runtimes but not your repository's packages, so
each job declares `setup` commands next to its `tests`. The trusted host runs
them inside the sandbox after the exact-SHA checkout and before the model's
first turn, and a non-zero exit aborts the job rather than handing the model a
workspace that cannot build. Omit `setup` and the model is told to install
what it needs itself.

Every job pays that install cost, because the adapter cannot yet fork a
prepared template box; see
[ariana-dot-dev/eve-box#1](https://github.com/ariana-dot-dev/eve-box/issues/1).
Forking would let one prepared snapshot serve every job.

## Customer login

Use native clients against the persistent `agent-data` volume:

```sh
docker compose run --rm --no-deps agent gh auth login --web --git-protocol https
docker compose run --rm --no-deps agent claude auth login --claudeai
docker compose run --rm --no-deps agent gh auth status
docker compose run --rm --no-deps agent claude auth status
```

Use the customer's own accounts. Do not copy subscription tokens into `.env`.
GitHub login must permit reading the configured repository and creating branches
and pull requests.

## Start and health

```sh
docker compose up -d
docker compose ps
curl --fail http://127.0.0.1:19999/health
curl --fail http://127.0.0.1:${CELLD_PORT:-18080}/v1/health
docker compose logs --tail=100 agent
```

The agent health check asserts that supervisor reports exactly two RUNNING
programs, celld's authenticated `/v1/health` responds, and Eve's stable
`/eve/v1/health` responds. It does not certify provider login, Box account
access, or model inference. The host celld port is loopback-only and is
configured with `CELLD_PORT`; Eve uses `EVE_PORT` and is also loopback-only.

Startup persists separate world and callback secrets under
`/var/lib/sigiro-agent`, exports celld and Workflow URLs, sets 90-day workflow
retention, deploys `/app/world-celld-worker` against your S3 bucket, and
starts celld and Eve. The worker forwards the callback secret as
`x-workflow-callback-secret`.

## Submit and inspect a job

Use the direct world-celld API. The exact JSON field names are source-side
contract details and must be confirmed against the installed API before use:

```sh
curl --fail-with-body "http://127.0.0.1:${CELLD_PORT:-18080}/v1/jobs" \
  -H 'Content-Type: application/json' \
  --data-binary @job.json
curl --fail-with-body "http://127.0.0.1:${CELLD_PORT:-18080}/v1/jobs/JOB_ID"
curl --fail-with-body -X POST \
  "http://127.0.0.1:${CELLD_PORT:-18080}/v1/jobs/JOB_ID/cancel"
```

Keep evidence immutable while a job runs. A job submission, a completed Eve
turn, or a celld status response is not publication proof. Treat the result as
published only when the receipt-backed result names the tested commit and draft
pull request. Nothing merges automatically.

## Recovery and data

```sh
docker compose logs --tail=100 agent sigiro
docker compose restart agent
docker compose down
```

Back up `agent-data` and `sigiro-data` consistently, and rely on the
durability and backup properties of the S3 bucket you provisioned for celld
state. Never use `docker compose down -v` when retaining state. Lost secrets,
receipts, or workflow data can make in-flight work unsafe to resume.

## Integration acceptance

Before calling this deployment production-ready, prove in an integration run
that Eve 0.51 and the pinned `@workflow/*` packages interoperate with the
world-celld 0.4.1 worker, including authenticated health, submit, status,
cancel, callback delivery, 90-day retention configuration, and a receipt-backed
publication result. Do not infer a runtime incompatibility from build metadata
alone.

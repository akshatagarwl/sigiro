# Sigiro hosted multi-tenant deployment handoff (2026-07-15, corrected 2026-08-24)

Resume context for the hosted multi-tenant deployment on the shared Hetzner box.

**Read the correction below before trusting anything else here.** The 2026-07-15
body described a dogfood `sigiro` accessory and a loopback-registry image. Both
were gone within days and the document was never updated, so on 2026-08-24 it
caused a deploy to be planned against infrastructure that does not exist. The
sections after the correction are otherwise still accurate, but verify against
the live box (`docker ps`) and against worldpol's `config/deploy.yml` rather
than against this file.

## Correction (verified against the box on 2026-08-24)

- **There is no dogfood `sigiro` accessory.** It was removed on 2026-07-17 in
  worldpol `6faa832`, "chore(deploy): remove Hetzner Sigiro stack". `docker ps -a
  --filter name=sigiro` returns exactly `sigiro-hosted`, `sigiro-edge` and
  `sigiro-catalog-db`. Ignore every reference to it below.
- **The image comes from ghcr, not the loopback registry.** The running container
  is `ghcr.io/sigiroai/sigiro:<sha>`, built multi-arch (`linux/amd64` and
  `linux/arm64`). The `127.0.0.1:5555/linguabench/sigiro` tags below are historic.
  Pushing therefore needs no SSH tunnel; only `kamal accessory reboot` needs the box.
- **`sigiro-hosted` no longer exports its own telemetry anywhere.** It has no
  `OTEL_EXPORTER_OTLP_ENDPOINT` set. worldpol's own web and worker export to
  `https://api.sigiro.com` instead (worldpol `cc3d8c2`).

## TL;DR state

- **Hosted multi-tenant Sigiro is public and E2E-verified** at `api.sigiro.com`.
  HTTPS/API traffic uses the existing kamal-proxy on 443. Direct TLS OTLP is public on
  IPv4 4317/4318. TenantKeys auth is mandatory; no-auth requests return 401.
- The public root is a tiny resource-capped Nginx deny backend because kamal-proxy
  v0.9.2 requires the hostname's root-path service to own TLS. Only the explicit Sigiro
  path allowlist reaches `sigiro-hosted`; `/admin`, docs, Swagger, and unknown paths are
  404. There is no Caddy process.
- `sigiro-hosted` exports its own traces and logs **one-way** to the separate private
  dogfood `sigiro` accessory as service `sigiro-hosted`. Never point it at itself: ingest
  emits telemetry and can recurse. The public-rollout attack/resource window was clean.
- Deploy config lives in the **worldpol** repo (`/Users/akshat/workspace/worldpol`),
  not here — that's the Kamal project that deploys the box.
- **Build images on the Mac, never on the box** (the box is 2-core/3.8 GB/no-swap and
  shared with corpus prod — an on-box build pinned it to load ~50 and OOM-bounced the
  worker on 2026-07-12).
- **The corpus reconciler is restored and singleton-verified as of 2026-07-14 15:09
  UTC.** Worldpol release `fecaf05` runs the durable bootstrap plus a 3,600-second
  maintenance timeout. The first reconcile completed on attempt 1 and spawned exactly
  one successor; `refetch-stale` also has exactly one sleeping chain.

## Box + access

- Box: Hetzner CX23, `91.98.118.235`, `ssh -i ~/.ssh/linguabench root@91.98.118.235`.
- Shared with **corpus prod** (linguabench): `linguabench-postgres` (the protected
  corpus — `responses`/`response_judgments`/`judgment_poles`), `-web`, `-worker`,
  `-analysis`, `-registry`, and the dogfood `-sigiro` sidecar. NEVER disrupt these.
- On any prod-down: `df -h /` first. Never `docker restart` / full `kamal deploy`
  (re-runs corpus migrations). Accessory-only: `kamal accessory reboot|boot <name>`.

## What's deployed (Kamal accessories on the box)

| accessory | image / role | ports | volume |
|---|---|---|---|
| `sigiro` (dogfood) | `127.0.0.1:5555/linguabench/sigiro:2396421fba74`, Open mode | API `127.0.0.1:9999` | `linguabench_sigiro_data` |
| `sigiro-catalog-db` | `linguabench/postgres:17`, `cmd: postgres` — sigiro_control + per-tenant DuckLake catalogs | kamal-network only | `linguabench_sigiro_catalog_data` |
| `sigiro-edge` | pinned `nginxinc/nginx-unprivileged:1.30.3-alpine` deny root | kamal-network only; owns `api.sigiro.com` root/TLS in kamal-proxy | none |
| `sigiro-hosted` | `.../sigiro:2396421fba74`, **TenantKeys** mode | API `127.0.0.1:9998`; public IPv4 TLS OTLP 4317/4318; allowlisted API via 443 | `linguabench_sigiro_hosted_data` |

`sigiro-hosted` env: `SIGIRO_CATALOG_URL` (secret, points to catalog-db),
`SIGIRO_ADMIN_TOKEN` (secret), `SIGIRO_DATA_DIR=/var/lib/sigiro-hosted`,
`SIGIRO_MAX_DISK_BYTES=4294967296` (4 GiB), `SIGIRO_RETENTION_DAYS=7`, one DuckDB
thread, 128 MiB per DuckDB instance, 10-second query timeout, 10,000 result-row cap,
and 8 MiB ingest-body cap. `OTEL_EXPORTER_OTLP_ENDPOINT` points only to
`http://linguabench-sigiro:4318`, with `OTEL_SERVICE_NAME=sigiro-hosted`.

The container runs as `10001:10001` with a read-only root filesystem, a 64 MiB
`noexec,nosuid,nodev` tmpfs at `/tmp`, all capabilities dropped, and
`no-new-privileges`. Docker caps it at 0.75 CPU, 768 MiB RAM with no extra swap, and
256 PIDs. Docker checks `/health` and restarts the accessory if it fails.

`sigiro-catalog-db` has its own 64-character password. The corpus Postgres password
does not enter the catalog or hosted containers. Docker caps the catalog at 0.25 CPU,
256 MiB RAM, 128 PIDs, 30 connections, 64 MiB shared buffers, and 2 MiB `work_mem`.

## Commits

**sigiro (LOCAL only — never pushed):**
- `907f86a` fix(retention): Rust-computed cutoff + `make_timestamp` (no-icu lockdown)
- `649e5bb` feat(auth): client-side bearer via SIGIRO_API_KEY/--key + loud open warning
- `c9ba4f8` fix(query): pre-load `icu` so tz/`now()-INTERVAL` queries work under lockdown
- `f702db6` docs: auth/config model → Open | TenantKeys
- `b1c865d` build(duckdb): compile amalgamation ONCE into a cached static archive, link static
- `a27a872` test: shared extension cache for server-boot tests (fix 90s-boot flakes)
- `72878cb` fix(hosted): keep proxy upstream HTTP plaintext
- `05e09eb` fix(hosted): bound public request work
- `7c16ab7` fix(rate-limit): use governor replenishment intervals
- `704f90d` fix(container): run Sigiro as non-root
- `946a88f` docs(hosted): record hardened private deployment
- `5f6a1e8` docs(hosted): record corpus maintenance incident
- `82d84cc` docs(hosted): record verified worker rollout
- `de59db9` docs(hosted): record public rollout gate
- `336d69f` docs(hosted): record public production rollout
- `6fa7fd9` fix(ingest): accept Node Pyroscope profiles
- `2396421` fix(self-improve): query stable API directly — deployed HEAD

**worldpol (pushed to origin/main):**
- deploy bumps to `907f86abc053` then `b1c865d1f9ca`, then `3a6a951`
  feat(sigiro): provision hosted multi-tenant + control/catalog Postgres.
- `8c28333` fix(sigiro): constrain hosted resource usage
- `c755157` fix(sigiro): isolate catalog credentials
- `27e1f0b` fix(deploy): preserve catalog secret bytes
- `b7096a9` fix(sigiro): deploy hardened hosted image (superseded by targeting fix)
- `ea506e5` fix(sigiro): target hardened image at hosted
- `2eb2ed0` fix(pipeline): enforce durable maintenance singletons
- `9e0918e` fix(pipeline): allow long maintenance scans
- `fecaf05` fix(deploy): build app images on mac — deployed app release
- `7d417c7` feat(sigiro): prepare hardened public ingress
- `52b29ee` fix(sigiro): give public TLS a deny root
- `1a44973` feat(sigiro): export hosted telemetry to dogfood — deployed app release
- `6cde541` feat(observability): profile LinguaBench with Sigiro — HEAD, pushed;
  Sigiro accessory tags deployed, web/worker app image not yet rolled out

## Architecture (dual-mode)

- **StorageMode**: `LocalFile` (self-host/edge, zero-config) vs `Distributed` (hosted:
  shared PG catalog, per-tenant `tenant_<id>` metadata schema). Resolved once at boot
  from `SIGIRO_CATALOG_URL` (set ⇒ Distributed).
- **AuthMode**: `Open` (no per-request auth, perimeter-protected) vs `TenantKeys`
  (bearer keys via `ControlPlane` in the `sigiro_control` PG schema). Set by
  `SIGIRO_CATALOG_URL` presence.
- **Tenant/key minting** (admin-token auth, `Authorization: Bearer $SIGIRO_ADMIN_TOKEN`):
  `POST /admin/tenants {"id"}`, `POST /admin/keys {"tenant_id"}` → returns raw `key`
  (`sk_<tenant>_<rand>`), `DELETE /admin/keys/{short_token}`.
- **Isolation** proven: each tenant sees only its own data; no-auth → 401.

## Verified E2E (re-runnable)

All via `docker exec linguabench-sigiro-hosted` so keys never leave the container
(`$SIGIRO_ADMIN_TOKEN` is in the container env):
```sh
AUTH="Authorization: Bearer $SIGIRO_ADMIN_TOKEN"; CT="Content-Type: application/json"
curl -sf -X POST localhost:9999/admin/tenants -H "$AUTH" -H "$CT" -d '{"id":"acme"}'
KEY=$(curl -sf -X POST localhost:9999/admin/keys -H "$AUTH" -H "$CT" -d '{"tenant_id":"acme"}' | sed -E 's/.*"key":"([^"]+)".*/\1/')
curl -sf -X POST localhost:4318/v1/traces -H "Authorization: Bearer $KEY" -H "$CT" -d '<otlp json>'
curl -s localhost:9999/v1/query -H "Authorization: Bearer $KEY" --data "SELECT count(*) FROM sigiro_spans"
```
Result: acme=1, globex=1 (not 2), each sees only its own `service_name`, no-auth → 401.

The `704f90df22b7` production check on 2026-07-13 also proved:

- acme ingested one unique span and read it back; globex read zero rows for that service;
- a 30-request SQL burst returned 429, and a revoked key returned 401;
- the cleanup trap left zero recent unrevoked keys;
- `sigiro-hosted` and corpus Postgres stayed healthy with zero OOMs or restarts;
- the root filesystem stayed read-only and the process ran as `10001:10001`;
- before public rollout, Docker published only `127.0.0.1:9998 -> 9999`.

The public production E2E on 2026-07-14/15 additionally proved:

- DNS-only `A api.sigiro.com -> 91.98.118.235`; Let's Encrypt certificate CN
  `api.sigiro.com`, valid 2026-07-14 through 2026-10-12;
- kamal-proxy HTTPS `/health` 200, missing-auth query 401, root/admin/docs/Swagger and
  raw/encoded traversal attempts 404, and HTTP-to-HTTPS redirect 301;
- direct public TLS OTLP/HTTP 4318 returned 401 without auth and accepted a valid tenant
  span; native HTTP/2 gRPC 4317 returned `grpc-status: 16` without auth and
  `grpc-status: 0` with a valid tenant key;
- two independently initialized tenants each ingested a distinct span, tenant B could
  not read tenant A's service, blocked `getenv`/`read_csv_auto` SQL returned 400, a
  35-query burst produced 25x 200 + 10x 429, an 8 MiB+1 body returned 413, and a
  revoked key returned 401;
- the Hetzner firewall `linguabench-fw` (ID `11177423`) has TCP 4317 and 4318 rules
  from `0.0.0.0/0` only. There are no IPv6 or UDP ingest rules;
- a five-minute attack/resource window peaked at 74.09% hosted-container CPU during
  first-tenant initialization, 117.1 MiB RAM, 49 PIDs, one established public
  connection, and 69% disk. Hetzner's 35-minute metrics peaked at only 4.3 KiB/s
  ingress and 11.125 packets/s: no volumetric-attack signature;
- post-observability rollout counts were 43,998 responses / 159,676 judgments /
  240,604 poles (all non-decreasing), with 12 GiB free. Hosted remained healthy with
  zero restart/OOM, and the deliberate OTLP auth failure appeared in dogfood Sigiro as
  a `WARN` for service `sigiro-hosted`.

The `2396421fba74` rollout on 2026-07-15 additionally proved the actual
`@pyroscope/nodejs@0.6.1` protocol through the public endpoint. The SDK sent multipart
raw pprof to `/ingest`; two tenants stored three profiles each, each read its own service,
tenant B read zero rows for tenant A's service, and a revoked key returned 401. The
cleanup trap left zero unrevoked `e2e_node_%` keys. Public `/health` remained 200,
no-auth query remained 401, the denied root remained 404, both Sigiro accessories were
up on the new image, and corpus Postgres stayed up with zero OOMs/restarts.

## Gotchas that cost real time (don't rediscover)

1. **Build on the Mac, not the box.** `docker buildx build --builder multiarch
   --platform linux/amd64 -t 127.0.0.1:5555/linguabench/sigiro:<sha>
   --output type=docker,dest=out.tar .` then `cat out.tar | ssh box 'docker load'`.
   A `DOCKER_HOST=ssh docker build` is cancelled when the ssh client drops; a `nohup`
   on-box build survives but pins the box. First cold build ~30-40 min (duckdb compile);
   cached after (see `b1c865d`).
2. **DuckDB build**: every target downloads the pinned `static-libs` release, verifies
   its SHA-256, merges every archive fragment, and links the resulting static
   `libduckdb_static.a`. Local macOS, CI releases, Lima cross-builds, and Docker use
   `scripts/prepare-duckdb-static.sh`; `otool -L`/`ldd` show no dynamic libduckdb.
3. **sigiro rejects a password-less distributed DSN** ("postgres URI missing or empty
   password") — trust auth is out; the catalog PG needs a password.
4. **Kamal injects `env.secret` by name and does not support aliases.** The catalog
   accessory receives `SIGIRO_CATALOG_PASSWORD`; its shell command exports that value as
   `POSTGRES_PASSWORD` before invoking the official Postgres entrypoint. The hosted DSN
   uses the same catalog-only value. A fresh-volume Postgres 17 test verified this wrapper.
   The catalog role was rotated in place. The dedicated password authenticates against
   catalog Postgres and fails against corpus Postgres.
5. **Kamal dotenv command substitution strips backslashes.** Do not use
   `printf '%s\n'` to return SOPS values in `.kamal/secrets`; Kamal turned the format into
   `%sn` and appended a literal `n` to the password. Use:
   `sops decrypt ops/prod/secrets.env.sops | grep "^SIGIRO_CATALOG_PASSWORD=" | cut -d= -f2-`.
6. **`ops/prod/secrets.env.sops` is binary-type SOPS** (`{"data": "ENC[...]"}`).
   `sops set` can add an encrypted top-level JSON field that the dotenv loader never
   sees. Edit via `EDITOR=<append-script> sops <file>`, then validate through the same
   `.kamal/secrets` extraction path; do not infer success from changed ciphertext.
   `SOPS_AGE_KEY_FILE=~/Library/Application Support/sops/age/keys.txt`; recipient
   `age14vg795w9zy32m54aqcjegu98a5zgthue7zh0h2ywwu5rgtc9gqfqh3z8kr`.
7. **Runtime icu trap** (`c9ba4f8`): the locked-down (`enable_external_access=false`)
   serve DuckDB can't autoload `icu`, so ALL tz ops (`now()-INTERVAL`, `AT TIME ZONE`,
   `epoch(now())`) fail unless `icu` is installed/loaded before the lockdown
   (`pool.rs lock_filesystem`) during normal project initialization.
8. **`tower-governor` intervals are replenishment times.** `.per_second(5)` means one
   request every five seconds, not five requests per second. The hosted limiters use
   `.per_millisecond(200)` for 5 req/s and `.per_millisecond(20)` for 50 req/s.
9. **Kamal pulls an accessory image before rebooting it.** `docker load` puts the image
   in the daemon but not in the loopback registry. After loading, run
   `docker push 127.0.0.1:5555/linguabench/sigiro:<tag>` on the box, then run
   `kamal accessory reboot sigiro-hosted`. The failed pull does not stop the old container.
10. sigiro commit pre-commit hook runs the FULL test suite (~3-5 min) with
   `RUSTC_WRAPPER=sccache` — sccache is blocked in the sandbox, so commit with
   `RUSTC_WRAPPER=` and run commits in the background (foreground 2-min kill).
11. Tenant IDs are PostgreSQL schema tokens: only ASCII letters, digits, and
    underscores, maximum 56 bytes. Hyphenated E2E IDs fail loudly with 400. Also ingest
    at least one record for each fresh tenant before querying it; provisioning creates
    the control-plane row, while first ingest opens that tenant's DuckDB project.
12. **Root-created volumes need a one-time non-root migration.** The dogfood volume was
    created by `b1c865d`, whose image ran as root. Rebooting directly to the hardened
    `10001:10001` image failed loudly with `SQLITE_READONLY_DIRECTORY`. Verify the exact
    mount first, then change ownership of only that Sigiro volume to `10001:10001` and
    let Docker's restart policy retry. Never apply this to either Postgres volume. The
    hosted volume had already been migrated and needed no change.

## Shared corpus worker maintenance incident (2026-07-14)

Sigiro exposed two identical reconciliation completions 78 ms apart. Direct Absurd
queue inspection then proved this was not duplicated logging: `pipeline-spawner` had
26 active `reconcile` tasks (13 pending, 2 running, 11 sleeping), all with `{}` params
and no shard/provider scope. The startup idempotency key was not durable because the
queue's one-hour cleanup TTL removes the terminal row that owns the key; later worker
starts can therefore create another root while keyless descendants remain alive.

Production cleanup used only `absurd.cancel_task` on queue rows:

- cancelled all 26 duplicate reconcile tasks and let the two in-flight read queries
  finish naturally (no `pg_cancel_backend`, container restart, or corpus-table write);
- cancelled 16 duplicate sleeping `refetch-stale` tasks and seeded one replacement;
  task `019f6114-cfd7-7dce-96f2-0558872bc328` reached `sleeping` in one attempt;
- disk stayed at 14 GiB free, all containers stayed up, and hosted Sigiro stayed healthy.

One replacement reconcile task (`019f60f4-8872-70ab-9f3e-2ee6015e9e55`) was tested.
It failed all five attempts with `$ClaimTimeout`: the worker's hard deadline was 300
seconds, while a single whole-corpus set-diff query exceeded five minutes. Timed-out
SQL kept running briefly while the next retry began, recreating overlap inside one task.
The task is terminal `failed` and there is deliberately no active reconcile task now.

The pushed worldpol fixes are:

- `2eb2ed0`: transaction-scoped Postgres advisory lock + active-state check for both
  maintenance loops, with a disposable real-Postgres race/recovery E2E;
- `9e0918e`: keep provider queues at 300 seconds but give `pipeline-spawner` a
  3,600-second claim timeout, sufficient for the sequential whole-corpus scans.

The user explicitly authorized a full Kamal deploy. Release `fecaf05` was built for
linux/amd64 on the Mac (never on the box), pushed through the SSH tunnel, and deployed
with the existing throwaway migration smoke, corpus-protection gates, and health-gated
cutover. Pre-deploy protected counts were 43,936 responses / 159,339 judgments /
240,060 poles; after deploy and pipeline activity they were 43,938 / 159,352 / 240,082
(no shrink). Disk remained healthy with 12 GiB free.

The new worker logged `pipeline-spawner ... claim timeout 3600s`. Reconcile task
`019f6126-ad18-7ef9-9805-a22f348214e7` ran past the old five-minute failure boundary,
completed on attempt 1, durably slept for 30 seconds, and spawned exactly one successor
(`019f612c-6596-780f-9e0b-0add1728bb1b`). One active reconcile SQL statement and one
active task were observed; no retry overlap returned. Worldpol `/up`, dogfood Sigiro,
hosted Sigiro, web, worker, and corpus Postgres all remained healthy.

## Pending / next

- **Certificate renewal operation**: no Cloudflare token is needed. kamal-proxy renews
  the `api.sigiro.com` certificate. Worldpol
  `ops/prod/sync-sigiro-tls` finds that exact combined certificate/key bundle, verifies
  its hostname, key, and remaining lifetime, and copies only it (not the proxy's other
  private keys or ACME account key) to `/root/.kamal/sigiro-hosted-tls/tls.pem` as
  `0400 10001:10001`. It is installed as `/usr/local/sbin/sync-sigiro-tls` on the box and
  passed against the real bundle plus an idempotent rerun. After renewal, rerun the sync
  and `kamal accessory reboot sigiro-hosted`; tonic loads the identity at process start.
- **LinguaBench web/worker rollout blocked by the corpus constraint**: `6cde541` installs
  and starts the official Node Pyroscope SDK in both roles, and production config points
  it at private `http://linguabench-sigiro:4318`. The pushed code passed `pnpm check`,
  `pnpm build`, and a real local SDK lifecycle E2E. It is not running in the production
  web/worker yet because `bin/docker-entrypoint` unconditionally executes
  `server/db/migrate.ts`; both full deploy and app boot therefore re-run corpus
  migrations. Add and verify one obvious migration-free role rollout path before booting
  either app role. Do not bypass the entrypoint ad hoc on the box.
- **GHCR push blocked**: the current Docker credential lacks `write:packages` and returns
  `permission_denied: token provided does not match expected scopes`. After obtaining a
  suitable token, push immutable tag `2396421fba74`. Do not replace multi-arch `latest`
  with this amd64-only build.
- **Ops-tenant self-observability backlog**: hosted self-telemetry currently goes to the
  private Open-mode dogfood project. Move it to a dedicated authenticated ops tenant only
  after choosing a key lifecycle that does not expose the tenant key in image layers,
  logs, or Kamal output.
- **Monitoring gap**: OTLP auth failures are exported as warnings and kamal-proxy records
  HTTP status/path metadata, but `/v1/query` auth denials currently return 401 without an
  application warning. Add one at the shared auth boundary if Sigiro-native counting of
  all auth failures becomes necessary. Avoid logging tokens, headers, or bodies.
- Operator docs for tenant onboarding are in `www/content/docs/hosted-onboarding.md`.

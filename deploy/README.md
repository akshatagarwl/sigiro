# Deploy hosted Sigiro

Alchemy is the only Sigiro production deployment controller:

- `www/alchemy.run.ts` manages the Cloudflare website, DNS, R2, and email resources.
- `www/alchemy.hetzner.run.ts` adopts and retains the existing Hetzner server, IPs, firewall, and SSH key.
- `www/alchemy.workload.run.ts` manages the remote Docker context, dedicated `sigiro` network, and workload deployment.
- `ops/prod/compose.yaml` is the single workload specification for PostgreSQL, the deny edge, and hosted Sigiro. Better Auth is not here: it runs as a Cloudflare Worker on D1 (`www/alchemy.run.ts`) behind `sigiro.com/api/auth`.
- `ops/prod/secrets.sops.json` is the encrypted production secret source.

The workload stack invokes Docker Compose through Alchemy because the pinned first-party `Docker.Container` resource cannot preserve the existing read-only filesystems, tmpfs mounts, capability drops, resource limits, and PID limits. There is no direct Compose or ad-hoc SSH deployment path.

## Topology

```text
sigiro.com
  -> sigiro-www Cloudflare Worker
     -> AUTH service binding -> sigiro-auth Worker -> D1
  -> api-origin.sigiro.com
  -> shared host TLS ingress
  -> dedicated Docker network: sigiro
     -> linguabench-sigiro-edge
     -> auth-db.sigiro.com
     -> linguabench-sigiro-catalog-db
     -> linguabench-sigiro-hosted
```

The existing host ingress container is shared with an unrelated Linguabench workload. Alchemy updates only Sigiro's remaining routes through its stable in-container API; no Kamal CLI, Ruby dependency, or Kamal workload configuration remains. Removing or replacing that shared host process is a separate host-wide migration.

Never rename, recreate, or delete these existing volumes:

- `linguabench_sigiro_catalog_data`
- `linguabench_sigiro_hosted_data`
- `linguabench_postgres_data`

PostgreSQL keeps the `auth-db.sigiro.com` alias and materializes its TLS certificate and key inside the existing database volume before startup. The alias and the `SIGIRO_AUTH_DB_TLS_*` secret names are historical: they serve the live Sigiro catalog, not the retired auth service.

## Prerequisites

- repository-pinned Aube and Alchemy dependencies
- Docker with Compose and SSH context support
- `sops` with the age identity from `.sops.yaml`
- `~/.ssh/linguabench`

Validate the encrypted file without printing values:

```bash
sops decrypt ops/prod/secrets.sops.json >/dev/null
```

Edit it with explicit dotenv types:

```bash
sops ops/prod/secrets.sops.json
```

## Plans

```bash
mise run www-install
mise run alchemy-plan
mise run alchemy-hetzner-plan

SIGIRO_IMAGE='ghcr.io/sigiroai/sigiro@sha256:<amd64-manifest-digest>' \
SIGIRO_ROLLBACK_IMAGE='ghcr.io/sigiroai/sigiro@sha256:<current-amd64-digest>' \
  sops exec-env ops/prod/secrets.sops.json 'mise run alchemy-workload-plan'
```

The Hetzner plan must remain no-op for server `143929427` and its adopted cloud resources. The workload plan may create the local `sigiro-prod` Docker context and the dedicated `sigiro` network; it must never replace the server or persistent volumes.

## Deploy

Production deployment is only:

```bash
SIGIRO_IMAGE='ghcr.io/sigiroai/sigiro@sha256:<amd64-manifest-digest>' \
SIGIRO_ROLLBACK_IMAGE='ghcr.io/sigiroai/sigiro@sha256:<current-amd64-digest>' \
  sops exec-env ops/prod/secrets.sops.json 'mise run alchemy-workload-deploy'
```

The nightly driver performs that command after publishing and verifying the image. Compose waits for healthy services, then the Alchemy-owned command updates the three shared-ingress targets and verifies the live immutable image reference.

The first ownership cutover requires a deliberate downtime window: stop and remove only the five existing Sigiro containers, then run the Alchemy workload deployment. Do not remove the database volume, hosted-data volume, shared ingress, or any unrelated container.

## Verify

```bash
curl -fsS https://sigiro.com/health
curl -fsS https://sigiro.com/.well-known/oauth-protected-resource | jq
curl -fsS https://sigiro.com/api/auth/jwks | jq
```

A deployment is incomplete until image identity, container health, persistent mounts, public health, OAuth metadata/challenges, MCP behavior, and a clean follow-up Alchemy plan are verified.

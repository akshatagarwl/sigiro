# Deploy hosted Sigiro

Alchemy is the only Sigiro production deployment controller:

- `www/alchemy.run.ts` manages the Cloudflare zone, DNS, R2, D1, and the `sigiro-www` and `sigiro-auth` Workers.
- `www/alchemy.hetzner.run.ts` adopts and retains the existing Hetzner server, IPs, firewall, and SSH key.
- `www/alchemy.workload.run.ts` runs `ops/prod/deploy-native-host.sh`, which ships the native systemd contract and verified release artifacts to the host.
- `ops/prod/secrets.sops.json` is the encrypted production secret source.

The host runs no container engine. Every service is a native systemd unit (`ops/prod/systemd/`).

## Topology

```text
sigiro.com
  -> sigiro-www Cloudflare Worker
     -> AUTH service binding -> sigiro-auth Worker -> D1
api-origin.sigiro.com
  -> kamal-proxy (native, :80/:443, shared with LinguaBench)
     -> sigiro-edge (nginx, :8080) -> sigiro (:9999)
     -> PostgreSQL 18 (auth-db.sigiro.com, loopback TLS)
  sigiro-ingest-relay (:14318) <- LinguaBench profiler uploads
```

`auth-db.sigiro.com` and the `SIGIRO_AUTH_DB_TLS_*` secret names are historical: they serve the live Sigiro catalog, not the retired auth service.

## Prerequisites

- repository-pinned Aube and Alchemy dependencies
- `sops` with the age identity from `.sops.yaml`
- `~/.ssh/linguabench`

```bash
sops decrypt ops/prod/secrets.sops.json >/dev/null
```

## Plan and deploy

```bash
mise run alchemy-plan
mise run alchemy-hetzner-plan
```

The Hetzner plan must stay a no-op for server `143929427`.

The workload stack needs one artifact and SHA-256 per service: `SIGIRO_ARTIFACT`, `NODE_ARTIFACT`, `KAMAL_PROXY_ARTIFACT` and `LINGUABENCH_ARTIFACT`, each with a matching `*_SHA256`. Every service except Sigiro may pass `installed` to keep the release already on the host. `deploy/nightly_release.py` builds the Sigiro artifact and runs:

```bash
sops exec-env ops/prod/secrets.sops.json 'mise run alchemy-workload-deploy'
```

`native-host.sh deploy` installs the contract and artifacts, restarts the units, and fails unless every unit is active and answers its health check.

## Verify

```bash
mise run native-verify
curl -fsS https://api-origin.sigiro.com/health
curl -fsS https://sigiro.com/health
curl -fsS https://sigiro.com/api/auth/jwks | jq
```

`native-verify` fails if a container engine is installed, a unit is inactive, or the catalog has no tenants.

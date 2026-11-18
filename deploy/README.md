# Deploy hosted Sigiro

Sigiro owns its production configuration in this repository:

- `www/alchemy.run.ts` manages Cloudflare website, DNS, R2, and email-domain resources.
- `www/alchemy.hetzner.run.ts` adopts and retains the existing Hetzner server, primary IPs, firewall, and SSH key.
- `config/deploy.yml` manages the hosted API, Better Auth, private catalog PostgreSQL, and deny-by-default edge as Kamal accessories.
- `ops/prod/secrets.env.sops` stores only Sigiro deployment secrets, encrypted with SOPS and age.

Alchemy owns cloud resources. Kamal remains the workload controller because the current Alchemy Hetzner provider does not model Compose-style named volumes, private networks, health-gated proxy cutover, or rollback.

## Topology

```text
sigiro.com auth routes
  -> sigiro-www Cloudflare Worker
  -> auth-origin.sigiro.com
  -> kamal-proxy (:443 TLS)
  -> linguabench-sigiro-auth (Node 24 / Better Auth)
  -> auth-db.sigiro.com (private Docker alias, verified TLS)
  -> linguabench-sigiro-catalog-db (PostgreSQL: sigiro_auth database)

Cloudflare private database path
  -> linguabench-sigiro-auth-tunnel
  -> auth-db.sigiro.com

sigiro.com API, OTLP/HTTP, and discovery routes
  -> sigiro-www Cloudflare Worker
  -> api-origin.sigiro.com
  -> kamal-proxy (:443 TLS)
  -> linguabench-sigiro-edge (deny unmatched paths)
  -> linguabench-sigiro-hosted (Rust resource server)
```

The explicit `service:` values in `config/deploy.yml` preserve the existing container names, `kamal` Docker network, proxy routes, and named volumes during the repository-ownership move. In particular, do not rename or replace:

- `linguabench_sigiro_catalog_data`
- `linguabench_sigiro_hosted_data`

The PostgreSQL accessory retains its existing `auth-db.sigiro.com` network alias and encrypted certificate/key inputs. Production Better Auth verifies that hostname against the configured CA certificate; do not replace the alias with an unverified container-name connection.

The existing `sigiro-auth-tunnel` remains declared during the Node auth cutover so moving repository ownership cannot orphan a running production path. Remove it only in a later reviewed change after live Node auth acceptance proves nothing still depends on the tunnel.

## Prerequisites

- Ruby and Bundler
- Kamal `2.12.0` from the committed `Gemfile.lock`
- `sops` with the age identity that matches `.sops.yaml`
- `~/.ssh/linguabench`
- The production Email Sending token present as `CLOUDFLARE_EMAIL_API_TOKEN` in the encrypted secrets file

```bash
bundle install
sops decrypt --input-type dotenv --output-type dotenv ops/prod/secrets.env.sops >/dev/null
bundle exec kamal config >/dev/null
```

Edit encrypted production values with explicit dotenv types because the `.sops` suffix otherwise makes SOPS infer JSON:

```bash
sops --input-type dotenv --output-type dotenv ops/prod/secrets.env.sops
```

`kamal config` contains resolved secrets. Always discard its output as shown.

## Infrastructure plans

```bash
mise run www-install
mise run alchemy-plan
mise run alchemy-hetzner-plan
```

`alchemy-hetzner-plan` uses `HCLOUD_TOKEN` when it is set and otherwise reads the
token from the active `hcloud` context without printing it.

Both production plans must be reviewed before deployment. The Hetzner plan must remain fully no-op for server `143929427` and its adopted resources.

## Build images

```bash
TAG=$(git rev-parse --short=9 HEAD)

mise run image -- \
  --set "*.tags=ghcr.io/sigiroai/sigiro:${TAG}" \
  --set "*.tags=ghcr.io/sigiroai/sigiro:latest" --push

docker buildx bake --builder multiplatform sigiro-auth \
  --set "sigiro-auth.tags=ghcr.io/akshatagarwl/sigiro-auth:${TAG}" --push
```

The Hetzner host is amd64. Verify the amd64 manifest digest before changing an image reference.

## Deploy workloads

This is an accessories-only configuration. Deploy one named workload at a time; do not use `kamal setup` or `kamal remove`.

Before any PostgreSQL or Better Auth migration, stream encrypted backups of both databases off the host and verify each archive with `pg_restore --list`. Never write an unencrypted dump to disk. The ownership-transfer backups are stored outside the repository under `~/Backups/sigiro/`.

```bash
bundle exec kamal accessory reboot sigiro-hosted
bundle exec kamal accessory reboot sigiro-edge

# Apply Better Auth schema changes through the existing private network first.
bundle exec kamal accessory exec sigiro-auth node dist/migrate.js
bundle exec kamal accessory reboot sigiro-auth
```

Do not routinely reboot `sigiro-catalog-db`. PostgreSQL and both named volumes must remain in place.

## Verify

```bash
ssh root@91.98.118.235 \
  'docker ps --format "{{.Names}}\t{{.Image}}\t{{.Status}}" | grep sigiro'

curl -fsS https://sigiro.com/health
curl -fsS https://sigiro.com/.well-known/oauth-protected-resource | jq
curl -fsS https://sigiro.com/api/auth/jwks | jq
curl -sS -D - -o /dev/null https://sigiro.com/v1/services \
  | grep -i '^www-authenticate:'
```

A deployment is incomplete until image identity, container health, public health, OAuth metadata/challenges, CIMD, PKCE/resource binding, tenant claims, refresh replay protection, and real OTP delivery have been verified at the live origins.

# Deploy Sigiro (Hetzner, via kamal)

Sigiro runs on the Hetzner box (`91.98.118.235`, `api.sigiro.com`) as a **kamal
accessory** managed from the **`worldpol` repo** (`worldpol/config/deploy.yml`,
the `sigiro-hosted` accessory). This repo only produces the image; it carries no
deploy config of its own.

## Topology (on the box)

```
kamal-proxy (:443 TLS)         -> nginx edge -> sigiro :9999 (loopback)
linguabench-sigiro-hosted         image ghcr.io/sigiroai/sigiro:<sha>
linguabench-sigiro-edge           nginx path allowlist for api.sigiro.com
linguabench-sigiro-catalog-db     postgres:17  DuckLake catalog + sigiro_control
127.0.0.1:5555                    on-box registry — catalog-db and the other
                                  linguabench accessories only, NOT sigiro
```

Hosted mode is on (`SIGIRO_CATALOG_URL` → the on-box catalog-db, plus R2), so
auth is per-tenant bearer keys. Secrets live in `worldpol/.kamal/secrets`
(SOPS + age), never here.

## Image

Built by this repo's `Dockerfile` — native cross-compile (no QEMU), distroless/cc
runtime, ~170 MB. Published multi-arch to **`ghcr.io/sigiroai/sigiro`**, which is
what the box pulls. The box is amd64, so verify that variant before deploying —
`docker pull --platform` reuses a cached image of the wrong arch, so pull the
amd64 manifest by digest instead.

Production moved to ghcr in worldpol `1095c5b` (2026-07-20): the on-box copy had
gone stale and an accessory reboot would have silently downgraded the running
release. Do not push deploy images to `127.0.0.1:5555`.

## Deploy (run from `../worldpol`)

```bash
# 1. Build + push the multi-arch image to ghcr.
TAG=$(git -C ../sigiro rev-parse --short HEAD)
mise run image -- \
  --set "*.tags=ghcr.io/sigiroai/sigiro:${TAG}" \
  --set "*.tags=ghcr.io/sigiroai/sigiro:latest" --push

# 2. Bump `image:` on the `sigiro-hosted` accessory in
#    worldpol/config/deploy.yml to the new tag, commit, then:
cd ../worldpol && kamal accessory reboot sigiro-hosted
```

`/health` returns 503 until DuckLake attaches + init completes, 200 after — use
it to confirm the new release is live before considering the deploy done.
Confirm the container image too, since the config and the box can disagree:

```bash
ssh root@91.98.118.235 'docker ps --format "{{.Names}}\t{{.Image}}\t{{.Status}}" | grep sigiro'
curl -s -o /dev/null -w '%{http_code}\n' https://api.sigiro.com/health
```

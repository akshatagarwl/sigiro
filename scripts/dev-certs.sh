#!/usr/bin/env bash
# Issue the local dev stack's PostgreSQL server certificate.
#
# Production refuses a hosted catalog URL that is not `sslmode=verify-full`
# with a non-empty `sslrootcert` (`storage.rs::validate_hosted_catalog_tls`),
# so a plaintext local Postgres cannot exercise the hosted storage path at all.
# mkcert issues a certificate for localhost/127.0.0.1/::1 from a local root, and
# we hand that same root to the client as `sslrootcert` — so `verify-full`
# GENUINELY verifies here instead of being quietly downgraded.
#
# Idempotent: re-running reuses existing certificates.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cert_dir="${SIGIRO_DEV_CERT_DIR:-$repo_root/.dev/certs}"

if [[ -s "$cert_dir/ca.crt" && -s "$cert_dir/server.crt" && -s "$cert_dir/server.key" ]]; then
  echo "dev-certs: reusing $cert_dir"
  exit 0
fi

if ! command -v mkcert >/dev/null 2>&1; then
  echo "dev-certs: mkcert is required (brew install mkcert)" >&2
  exit 1
fi

mkdir -p "$cert_dir"
mkcert -install >/dev/null
caroot="$(mkcert -CAROOT)"

(
  cd "$cert_dir"
  mkcert -cert-file server.crt -key-file server.key localhost 127.0.0.1 ::1 >/dev/null
)
cp "$caroot/rootCA.pem" "$cert_dir/ca.crt"
# The container copies these to a postgres-owned path and re-chmods; the key
# still must not be world-readable on the host.
chmod 600 "$cert_dir/server.key"
chmod 644 "$cert_dir/server.crt" "$cert_dir/ca.crt"

echo "dev-certs: wrote $cert_dir/{ca.crt,server.crt,server.key}"

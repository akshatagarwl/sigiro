#!/usr/bin/env bash
# Create the local dev stack's S3 bucket in RustFS.
#
# Alchemy's `Docker.Container` reports a container as reconciled once it is
# STARTED, not once its healthcheck passes — there is no `depends_on:
# service_healthy` equivalent in the pinned provider. So readiness is this
# script's job: `rc alias set` against a not-yet-listening RustFS fails, and
# retrying it IS the readiness probe (the same handshake the bucket create
# needs, rather than a proxy for it).
#
# Idempotent: `--ignore-existing` makes a second run a no-op, so the Alchemy
# resource can be unmemoized and re-run on every deploy.
set -euo pipefail

network="${SIGIRO_DEV_NETWORK:?SIGIRO_DEV_NETWORK is required}"
endpoint="${SIGIRO_DEV_RUSTFS_ENDPOINT:-http://rustfs:9000}"
bucket="${SIGIRO_DEV_BUCKET:-sigiro}"
access_key="${SIGIRO_DEV_RUSTFS_ACCESS_KEY:-rustfsadmin}"
secret_key="${SIGIRO_DEV_RUSTFS_SECRET_KEY:-rustfsadmin}"
# Pinned by digest, exactly as the retired `rustfs-setup` compose service was.
image="${SIGIRO_DEV_RC_IMAGE:-rustfs/rc:v0.1.30@sha256:1296510ec27800668d26a8bdca6251e587086187a9368c7b80134a169e8a3bb8}"

for attempt in $(seq 1 60); do
  # `--entrypoint /bin/sh`, not a bare `/bin/sh` argument: the rc image sets its
  # own entrypoint, so passing /bin/sh positionally makes rc treat it as a
  # subcommand and fail with `unrecognized subcommand '/bin/sh'`. The retired
  # compose service used `entrypoint:` for the same reason.
  if docker run --rm --network "$network" --entrypoint /bin/sh "$image" -c "
      rc alias set local '$endpoint' '$access_key' '$secret_key' &&
      rc bucket create --ignore-existing 'local/$bucket'
    "; then
    echo "dev-rustfs-bucket: local/$bucket ready"
    exit 0
  fi
  echo "dev-rustfs-bucket: RustFS not ready yet (attempt $attempt), retrying" >&2
  sleep 2
done

echo "dev-rustfs-bucket: RustFS never became reachable at $endpoint" >&2
exit 1

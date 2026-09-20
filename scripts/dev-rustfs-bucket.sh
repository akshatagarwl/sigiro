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

# Retrying is only meaningful for "RustFS has not finished starting". A missing
# network is permanent: the loop would burn 60 attempts over two minutes and
# then report an unreachable endpoint, which sends you debugging RustFS instead
# of the stack that never created the network. Fail on it immediately.
if ! docker network inspect "$network" >/dev/null 2>&1; then
  echo "dev-rustfs-bucket: docker network '$network' does not exist." >&2
  echo "  The dev stack creates it; start it first with: mise run dev" >&2
  exit 1
fi

for attempt in $(seq 1 60); do
  # `--entrypoint /bin/sh`, not a bare `/bin/sh` argument: the rc image sets its
  # own entrypoint, so passing /bin/sh positionally makes rc treat it as a
  # subcommand and fail with `unrecognized subcommand '/bin/sh'`. The retired
  # compose service used `entrypoint:` for the same reason.
  if output=$(docker run --rm --network "$network" --entrypoint /bin/sh "$image" -c "
      rc alias set local '$endpoint' '$access_key' '$secret_key' &&
      rc bucket create --ignore-existing 'local/$bucket'
    " 2>&1); then
    echo "dev-rustfs-bucket: local/$bucket ready"
    exit 0
  fi

  # Distinguish a permanent docker-level failure from RustFS still booting.
  # Connection refused means the daemon answered and the container could not
  # reach RustFS yet — that is worth retrying. Anything else (missing image,
  # bad credentials, unknown flag) will fail identically 60 times.
  if ! grep -qiE 'connection refused|connect: |no route to host|timeout|dial tcp' <<<"$output"; then
    echo "dev-rustfs-bucket: permanent failure, not retrying:" >&2
    echo "$output" >&2
    exit 1
  fi

  echo "dev-rustfs-bucket: RustFS not ready yet (attempt $attempt), retrying" >&2
  sleep 2
done

echo "dev-rustfs-bucket: RustFS never became reachable at $endpoint after 60 attempts" >&2
echo "$output" >&2
exit 1

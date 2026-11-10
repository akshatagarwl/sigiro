#!/usr/bin/env bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
name="sigiro-admin-pg-$$"
port=$((20000 + RANDOM % 10000))
password="$(openssl rand -hex 16)"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true; rm -rf "$work"' EXIT

openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=sigiro test CA" \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" \
  -keyout "$work/ca.key" -out "$work/ca.crt" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout "$work/server.key" -out "$work/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature,keyEncipherment\n' > "$work/server.ext"
openssl x509 -req -in "$work/server.csr" -CA "$work/ca.crt" -CAkey "$work/ca.key" -CAcreateserial -days 1 \
  -extfile "$work/server.ext" -out "$work/server.crt" 2>/dev/null
docker run -d --name "$name" -p "127.0.0.1:$port:5432" -e POSTGRES_PASSWORD="$password" \
  -v "$work:/certs:ro" postgres:17 \
  bash -c 'cp /certs/server.* /tmp/ && chown postgres /tmp/server.* && chmod 600 /tmp/server.key && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/server.crt -c ssl_key_file=/tmp/server.key' >/dev/null
for _ in $(seq 1 60); do
  docker exec "$name" pg_isready -U postgres >/dev/null 2>&1 && break
  sleep 1
done
sleep 2

export SIGIRO_TEST_POSTGRES_URL="postgresql://postgres:$password@localhost:$port/postgres?sslmode=verify-full&sslrootcert=$work/ca.crt"
cd "$repo"
scripts/run-with-duckdb-static.sh mbx test --test admin_schema_test -- --nocapture
scripts/run-with-duckdb-static.sh mbx test --lib -- alerts::tests::postgres --nocapture --test-threads=1

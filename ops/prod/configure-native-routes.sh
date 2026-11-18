#!/usr/bin/env bash
set -euo pipefail
proxy=${KAMAL_PROXY_BIN:-/opt/kamal-proxy/current/kamal-proxy}

"$proxy" deploy linguabench-web \
  --target 127.0.0.1:3001 \
  --host linguabench.akshat.dev \
  --host linguabench-origin.akshat.dev \
  --tls \
  --health-check-path /up \
  --health-check-interval 3s \
  --health-check-timeout 5s \
  --target-timeout 30s \
  --buffer-requests \
  --buffer-responses \
  --buffer-memory 1048576 \
  --log-request-header Cache-Control \
  --log-request-header Last-Modified \
  --log-request-header User-Agent

"$proxy" deploy linguabench-sigiro-edge \
  --target 127.0.0.1:8080 \
  --host api-origin.sigiro.com \
  --tls \
  --health-check-path /health \
  --health-check-interval 3s \
  --health-check-timeout 3s \
  --target-timeout 5s \
  --buffer-requests \
  --buffer-responses \
  --buffer-memory 65536 \
  --max-request-body 65536 \
  --max-response-body 1048576

"$proxy" deploy linguabench-sigiro-hosted \
  --target 127.0.0.1:9999 \
  --host api-origin.sigiro.com \
  --path-prefix /v1 \
  --path-prefix /v1development/profiles \
  --path-prefix /health \
  --path-prefix /healthz \
  --path-prefix /ingest \
  --path-prefix /push.v1.PusherService/Push \
  --path-prefix /openapi.json \
  --path-prefix /.well-known/oauth-protected-resource \
  --strip-path-prefix=false \
  --health-check-path /health \
  --health-check-interval 3s \
  --health-check-timeout 5s \
  --target-timeout 90s \
  --buffer-requests \
  --buffer-responses \
  --buffer-memory 1048576 \
  --max-request-body 8388608 \
  --max-response-body 16777216

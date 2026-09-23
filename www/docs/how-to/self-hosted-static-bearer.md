---
title: Protect self-hosted Sigiro with a static bearer token
description: Require one shared bearer secret across the query API, OTLP HTTP, and OTLP gRPC listeners.
sidebar:
  order: 8
---

# Protect a self-hosted Sigiro server with one bearer token

Set `SIGIRO_STATIC_BEARER_TOKEN` **on the server** to a randomly generated secret of at least 32 ASCII bytes (bearer-token characters). For example, generate one with `openssl rand -hex 32` and inject it through your secret manager; do not commit it or put it in a shell command that will be saved in history. The server rejects an empty, short, malformed, or non-UTF-8 value at startup. With the variable unset, local self-hosting remains in Open mode; do not expose Open-mode listeners to an untrusted network.

Provide the same value to the CLI and stdio MCP client as `SIGIRO_ACCESS_TOKEN` or `--token`. For OTLP exporters and HTTP clients, send `Authorization: Bearer <token>` to the query HTTP listener, OTLP HTTP listener, and OTLP gRPC listener. In this mode only the literal `Bearer` scheme is accepted; `AgentAuth` is not. All authenticated traffic uses the local `default` tenant. `/health` remains public. The `/admin/*` routes have a separate `SIGIRO_ADMIN_TOKEN`; the static bearer is not an admin credential.

This mode is **only for local self-hosted storage**. It cannot be combined with `SIGIRO_AUTH_ISSUER`, `SIGIRO_AUTH_AUDIENCE`, `SIGIRO_AUTH_JWKS_URL`, a distributed/PostgreSQL catalog, or `SIGIRO_QUERY_SURFACE=quack`: startup fails rather than silently opening another query surface. It does not enable TLS on the main HTTP query listener; terminate TLS at a reverse proxy there. Configure `SIGIRO_TLS_CERT` and `SIGIRO_TLS_KEY` for native OTLP HTTP/gRPC TLS, or terminate TLS at a trusted proxy. A bearer secret sent over plaintext across hosts can be intercepted. Rotate the secret by updating clients and restarting the server; there is no dual-token rotation window or separate ingest/query authorization in this initial self-hosted mode.

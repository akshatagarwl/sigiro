---
title: Connect sigiro to Executor
description: Add sigiro's OpenAPI API or documentation MCP server to Executor, then expose the resulting tools to your agent.
sidebar:
  order: 4
---

[Executor](https://executor.sh/) is an integration layer for agents. It imports OpenAPI
specifications, MCP servers and GraphQL endpoints into one tool catalog. You can add
sigiro without writing an Executor plugin.

Executor also reads the [integrations.sh](https://integrations.sh/) registry to
populate its catalog, and sigiro publishes its surfaces there, so a search inside
Executor may find sigiro before you paste anything. The steps below are the explicit
import, which works either way.

## Add the sigiro API

The public OpenAPI document is:

```
https://api.sigiro.com/openapi.json
```

In the Executor web UI:

1. Open **Add Integration**.
2. Paste the OpenAPI URL.
3. Let Executor detect and index the operations.
4. Create a connection for the integration.
5. Add the hosted sigiro tenant key to that connection if the API asks for it.
6. Set a policy for each tool. Use approval for tools that run SQL or inspect telemetry.

The specification is public and does not require a key to read. The hosted API itself
still requires the tenant key. Self-hosted sigiro can use the same import pattern with
the OpenAPI URL served by your own instance — and needs no key at all, because a
self-hosted server runs in open mode and authenticates no request.

Executor also exposes a CLI. The equivalent import has this shape:

```bash
executor call executor openapi addIntegration '{
  "spec": "https://api.sigiro.com/openapi.json",
  "namespace": "sigiro",
  "baseUrl": "https://api.sigiro.com"
}'
```

Use the `baseUrl` field when you import a specification whose server URL is not the
URL you want Executor to call. Configure authentication on the resulting connection,
not in the public specification URL.

Confirm it is live:

```bash
executor tools integrations
```

## Add the sigiro docs MCP server

The docs site also exposes a public, read-only MCP server:

```
https://sigiro.com/mcp
```

Add it as an MCP integration in Executor and select the streamable HTTP transport. This
gives an agent documentation tools such as search and page retrieval. It does
not provide access to a tenant's telemetry.

Use the OpenAPI integration for live telemetry operations. Use the MCP one when an agent
needs to find or read sigiro documentation. You can add both to the same
Executor workspace.

## Connect your agent

After Executor indexes the integrations, connect your agent to Executor over MCP. Executor's
MCP endpoint is the one your agent uses; the agent does not need a separate sigiro
plugin. Follow [Executor's MCP connection guide](https://executor.sh/docs) for the
endpoint and transport used by your deployment.

## Verify the import

Check the integration in Executor before asking an agent to use it. The sigiro API import
should expose operations for services, traces, anomalies, investigations and queries.
The docs import should expose documentation search and page retrieval.

Then test one read-only call first. For a hosted API connection, confirm that the
request includes the tenant key and that the returned data belongs to the intended
sigiro tenant. Do not put a tenant key in this page, an OpenAPI URL, or an agent
prompt checked into a repository.

## Next

- [API reference](/docs/reference) — inspect the generated operation details
- [Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding) — create the
  hosted endpoint and tenant connection
- [Quickstart](/docs/tutorials/quickstart) — run sigiro locally and send test data

---
title: Onboard an agent with Agent Auth
description: Register an agent host, grant the minimum capability, and rotate or revoke its credentials through Sigiro's Agent Auth API.
sidebar:
  order: 2
---

Use Agent Auth when software, rather than a person, must authenticate to hosted
Sigiro. Sigiro supports delegated identities: a person approves the agent once
and binds it to their workspace; the agent can then run unattended with its own
key and short-lived tokens.

You do not need another Sigiro executable. Integrate the official
`@auth/agent` SDK into the agent host. Sigiro does not support autonomous agent
identities.

## Before you start

Create or sign in to your hosted Sigiro account first:

```bash
sigiro signup --name "Your Name" --email you@example.com
# Existing account:
sigiro auth login
```

The account must belong to exactly one Sigiro workspace. Agent Auth binds the
approved agent to that workspace.

Use durable secret storage for the SDK. It stores the host and agent Ed25519
private keys and the connection record. The default in-memory storage is only
suitable for a short-lived example; losing it requires enrolling a new identity.

## 1. Discover Sigiro

Check the live provider document instead of hard-coding lifecycle routes:

```bash
curl -fsS https://sigiro.com/.well-known/agent-configuration | jq
```

Confirm that the issuer is `https://sigiro.com/api/auth`, the protected
resource is `https://sigiro.com`, and `modes` contains only `delegated`.

List the available capabilities:

```bash
curl -fsS https://sigiro.com/api/auth/capability/list | jq
```

Request only what the process needs. An OTLP exporter needs
`telemetry:ingest`; it does not need read or alert-management capabilities.

## 2. Add the official SDK

Add the SDK to the agent host:

```bash
aube add @auth/agent@0.6.2
```

Create the client with the Sigiro service origin. In production, pass a
persistent `storage` implementation, such as `KVStorage` backed by your secret
store. Do not persist private keys in source control, logs, screenshots, or
unencrypted application state.

```ts
import { AgentAuthClient, MemoryStorage } from '@auth/agent';

const client = new AgentAuthClient({
  urls: ['https://sigiro.com'],
  allowDirectDiscovery: true,
  hostName: 'production-telemetry-host',
  storage: new MemoryStorage(), // Replace with durable secret storage in production.
  onApprovalRequired: (approval) => {
    const url = approval.verification_uri_complete ?? approval.verification_uri;
    approvalUi.show({ url, code: approval.user_code });
  },
});

const providers = await client.init();
if (providers.length !== 1) throw new Error('Sigiro discovery failed');
```

`approvalUi.show` represents your authenticated, non-persistent operator UI. It
must not write the approval URL or code to application logs.

`connectAgent` generates the host and agent Ed25519 keys. Sigiro's discovery
document and production configuration allow dynamic host registration, so a
separate host-enrollment command or token is not required for this path. An
administrator-provisioned host instead uses the SDK's `enrollHost` flow with its
one-time enrollment token; do not mix the two paths.

## 3. Register and approve the agent

Connect in delegated mode and request only telemetry ingestion:

```ts
const connection = await client.connectAgent({
  provider: 'https://sigiro.com',
  mode: 'delegated',
  name: 'production telemetry exporter',
  capabilities: ['telemetry:ingest'],
  reason: "Send this service's OpenTelemetry data to Sigiro",
  preferredMethod: 'device_authorization',
});

if (connection.status !== 'active') {
  throw new Error(`Agent is not active: ${connection.status}`);
}
```

Open the displayed verification URL, sign in to Sigiro, confirm the code, and
approve the requested capability. The SDK polls until approval succeeds or the
request expires. Approval links and codes are temporary, but you should still
keep them out of logs and support tickets.

After approval, keep `connection.agentId` with the SDK's durable state. Do not
export or copy the private key.

## 4. Authenticate telemetry

Sign a fresh, capability-scoped token immediately before a telemetry request:

```ts
const authorization = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

const headers = {
  Authorization: `Bearer ${authorization.token}`,
};
```

Attach that header to OTLP/HTTP requests to `https://sigiro.com/v1/traces`,
`/v1/metrics`, `/v1/logs`, or `/v1development/profiles`. Agent tokens are
short-lived. Your exporter transport must obtain a new token before expiry; do
not place one token in a long-lived environment variable.

Sigiro validates the agent status, workspace binding, audience, replay state,
and `telemetry:ingest` grant before accepting telemetry. Do not reuse a signed
token for another request.

## 5. Rotate the agent key

Rotate through the SDK. It replaces the registered public key and updates the
private key in the configured storage:

```ts
await client.rotateAgentKey(connection.agentId);
```

After rotation, discard any token signed with the previous key. Keep the SDK
storage available across restarts so the host can continue managing the agent.

## 6. Revoke and verify

Create a token only for the negative check, then revoke the agent:

```ts
const tokenBeforeRevocation = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

await client.disconnectAgent(connection.agentId);
```

`disconnectAgent` revokes the remote identity and its capability grants before
removing the local connection. A request made with `tokenBeforeRevocation` must
now fail. Sigiro's OTLP/HTTP endpoints return HTTP `401` for the revoked agent
token. Treat any accepted request as a failed revocation and stop the exporter.

To retire the whole host rather than one agent, revoke the host from the Sigiro
account that approved it. Host revocation also revokes every agent attached to
that host.

## Troubleshooting

- **Discovery fails:** use `https://sigiro.com` as the provider URL, not the
  issuer path. Verify the well-known document with `curl`.
- **Approval expires:** call `connectAgent` again and complete the new approval
  before its timer expires.
- **Capability is missing:** request `telemetry:ingest` explicitly. Do not add
  unrelated capabilities as a workaround.
- **The identity disappears after restart:** replace `MemoryStorage` with a
  durable `KVStorage` backend and protect that backend as secret material.
- **A signed token is rejected as a replay:** sign a new token for every
  request. Replay protection is shared across Sigiro auth instances.
- **A revoked agent still sends data:** stop the process, preserve the failing
  response, and contact Sigiro. Do not create a replacement identity until the
  revocation failure is understood.

For human signup, OAuth login, and ordinary access tokens, see
[Send telemetry to hosted Sigiro](/docs/how-to/hosted-onboarding). For Sigiro
CLI behavior, see the [CLI reference](/docs/reference/cli).

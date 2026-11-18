# Agent deployment threat model

## Boundary

One deployment is one customer security domain. It contains one celld fleet,
one fleet bucket credential, one outbound pull worker, and one coding-provider
login. Celld does not provide isolation between hostile applications, so
separate customers need separate fleets and operating-system boundaries.

The worker has outbound access to celld and the internet, but no inbound
listener or published port. Worker-facing COORDINATOR endpoints are
authenticated with `WORKER_TOKEN`; customer clients use celld's public listener.

## Protected values

- Customer source code and patches.
- Claude or Codex subscription state.
- Fleet bucket credentials and durable agent state.
- Worker token.
- Telemetry evidence that may contain production identifiers.

## Principal threats and controls

| Threat | Control |
|---|---|
| Repository text instructs the model to steal credentials | The provider process receives an allowlist of environment variables. Fleet, worker, object-store, and API credentials are absent by construction. Provider authentication is consumed by the unmodified coding client and is never exposed as a tool. |
| Worker claims the same alarm twice | Stable job and attempt identifiers. The queue removes a claimed job, and repeated completion is idempotent. |
| Stale execution overwrites a newer result | Celld stores a generation and fencing value, then rejects completions or reports for an older generation or attempt. |
| Model edits files outside the job's scope | Every changed path is checked against `allowedPaths` after the model exits and before tests run. Workflow, action, and git machinery paths are refused. |
| Worker becomes a remote shell | Its HTTP API accepts a typed job contract, never an arbitrary command. It has no inbound listener and only makes outbound calls. |
| Celld internal operator routes become public | The internal listener stays on container loopback and has no published port. |
| Worker reaches unrelated private services | Run one trusted application per fleet. Restrict host egress or use a proxy before placing the fleet beside unrelated control-plane services. |
| Container escapes or consumes the host | Non-root UID, all capabilities dropped, `no-new-privileges`, read-only root, bounded tmpfs, PID, memory, and CPU limits. A hosted service still needs a stronger VM or microVM boundary. |
| Bucket credential controls more than one customer | One credential is restricted to one bucket or prefix. No account-wide object-store key. |
| Logs disclose OAuth or repository content | Do not log environment variables, authorization headers, full model transcripts, or raw patches. Store bounded result metadata and artifact hashes. |
| A deployment tag changes underneath the operator | Celld and Bun base images are pinned by verified image digest. Celld release binaries in the deploy helper are checked against release-published SHA-256 values. |

## Known residue

- Celld v0.4.0 is alpha. Pinning the release prevents surprise upgrades but does
  not remove defects in that release.
- The single-node Compose deployment waits for the object store before
  acknowledging writes. A second node needs a private encrypted peer network
  and an advertised internal address.
- Docker containers are not a sufficient hostile-tenant boundary for a
  Sigiro-hosted shared product. Use a dedicated VM or microVM per customer.
- Real private-repository auth, GitHub draft-PR publishing, trigger and dedup,
  hosted tenant isolation, and production celld fleet failover are open. No
  GitHub credential exists in this deployment.
- Eve's GitHub integration was audited and is not used. It requires the Eve
  runtime and lacks a reusable branch, commit, and draft-PR publisher.
- Subscription-provider policy can change. Customers must authenticate through
  the provider's unmodified client, and the deployment must not intermediate
  consumer OAuth.

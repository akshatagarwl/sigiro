# Agents are generic pi-durable sessions in cells

Hosted Sigiro offers a generic agent instead of a telemetry optimizer. Each session is one cell hosting a `@earendil-works/pi-durable` Harness on that cell's SQLite, so model turns and tool calls recover inside the cell. Telemetry query, run sandbox, and GitHub draft pull requests are extensions an agent may enable, alongside tools from tenant-registered remote MCP servers. Credentials (GitHub App key, MCP credentials, Codex OAuth state) stay in trusted cells and never reach the model, a codemode script, or the sandbox. This replaces Eve, Rivet, the PostgreSQL lease worker, the Jev gate, service mappings, Microsandbox, host checkpoint archives, and the ingest relay. The existing optimizer API is removed without backwards compatibility. pi-durable is experimental, so its version is pinned.

## Consequences

- Jev assessment and abstention are no longer part of the request path; a telemetry-assessment gate could return later as an extension.
- The `optimizer:write` OAuth scope is replaced, which requires a guarded Auth D1 resource-scope migration like the one used to retire anomaly scopes.

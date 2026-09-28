# Sigiro assistant

A Vercel Eve coding agent handles each submitted session turn using `gpt-6-luna` through the controller's Codex login. Rivet World drives Eve's workflow, and agentOS provides its in-process workspace. This does not produce a Sigiro verification result.

See [the prototype's configuration and boundaries](agent/README.md). Keep the Codex CLI login on the controller; never include credentials in Discord or the repository. Eve's local HTTP server has no project-specific authentication and binds loopback only. No human approval step, external sandbox, or production containment boundary is established.

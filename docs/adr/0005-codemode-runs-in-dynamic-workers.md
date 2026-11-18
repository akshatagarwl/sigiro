# Codemode scripts run in Dynamic Workers, not pi-codemode's executor

`@earendil-works/pi-codemode` runs QuickJS in a `node:worker_threads` worker, which neither Cloudflare Workers nor celld provides. Sigiro keeps pi-codemode's source format and tool declarations but executes each script in a Dynamic Worker (`worker_loaders`, implemented on both platforms) with outbound network disabled. Every tool is an RPC binding back to the session cell, which checks authorization and holds credentials. `@cloudflare/codemode` is prior art for this executor.

## Considered Options

- Patching pi-codemode to run QuickJS-wasm in-process in the session cell: shares the cell's 128 MB heap and event loop with an untrusted script.
- Running pi-codemode inside the run sandbox: needs a sandbox for every session and a tool-call channel out of a credential-free, network-less container.

# Nightly release driver

`deploy/nightly_release.py` replaces the agentic release prompt with one deterministic, resumable path:

```text
source → preflight → gate → image → binaries → R2 → website → deployment → Kamal → health
```

Hermes only schedules `~/.hermes/scripts/sigiro-nightly-release.sh` and delivers its bounded stdout. It does not invoke an LLM.

## Guarantees

- The fetched 40-character `origin/main` SHA is the release identity.
- The persistent detached worktree is the only build checkout; primary checkouts are never reset, cleaned, or built.
- `mise run release` remains the authoritative gate. Cargo is never invoked directly.
- The OCI image carries the full source SHA label. Its nine-character tag is immutable; `0.1.0` and `latest` are mutable aliases.
- Four binary archives are built and verified locally, staged under `staging/<full-sha>/`, then promoted to `0.1.0/`. `LATEST` is written last.
- Registry, R2, Worker, Sigiro deployment config, live container, and public health writes are read back.
- Website verification failure rolls back the Worker. API rollback remains waived.
- Every phase writes an atomic receipt; completed phases are skipped only after phase-specific external revalidation.
- Verbose output is redacted into receipt logs. Hermes receives only the bounded final report.

Receipts live under:

```text
~/.hermes/workflows/sigiro/nightly-release/receipts/<full-sha>/
  receipt.json
  events.jsonl
  artifacts/
  downloads/
  logs/commands.log
```

A dry run validates the local checkout, production deployment shape, warm mbx target, exact Rust toolchain, and required executables while recording every mutation as `planned` rather than `complete`:

```sh
python3 deploy/nightly_release.py --dry-run --source "$(git rev-parse HEAD)"
```

A real invocation is intentionally guarded:

```sh
SIGIRO_RELEASE_ALLOW_WRITES=1 python3 deploy/nightly_release.py
```

The website phase installs, checks, and builds through the repository-pinned Aube
2.2.16 tool, then deploys the wrapper Worker only through `mise run alchemy-deploy`.
Wrangler is retained only for read-only deployment inspection and R2 object get/put,
through `aube -C www exec -- wrangler`; the driver never invokes Bun, `bunx`, or a
direct Wrangler deploy/rollback.

## Install

From the reviewed Sigiro checkout:

```sh
deploy/install-nightly-release.sh
```

The installer copies the driver to `~/.hermes/scripts/`, installs the no-argument wrapper required by Hermes, and pins the copied driver by SHA-256. The wrapper refuses modified bytes.

Configure the supported mbx managed-target retention policy in `/Users/akshat/Library/Application Support/mbx/config.toml`:

```toml
[target]
max_age = "none"
max_size = "none"
```

For the one-time migration away from a fabricated managed target, first move the DuckDB static archive cache outside Cargo's target tree, then recreate the target through mbx:

```sh
export DUCKDB_STATIC_CACHE="$HOME/Library/Caches/sigiro/duckdb-static"
mbx clean
scripts/run-with-duckdb-static.sh mbx build --release --locked --bin sigiro
```

`DUCKDB_STATIC_CACHE` must be set before the preparation wrapper runs; otherwise that wrapper creates a regular `target/` directory before mbx can install its managed symlink. `mbx clean` removes only that workspace's managed target. The following command recreates it through mbx and restores matching outputs from the shared action CAS. Never clone managed targets, synthesize metadata, or rewrite embedded paths.

## Tests

```sh
python3 -m unittest -v tests/nightly_release_driver_test.py
python3 -m py_compile deploy/nightly_release.py
bash -n deploy/install-nightly-release.sh
git diff --check
```

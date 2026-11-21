---
"sigiro": minor
---

sigiro is now one self-contained binary. On Linux it is fully static (musl) and runs on any distribution, in an empty container, or offline: every DuckDB extension it uses (ducklake, parquet, json, icu, httpfs, postgres, quack) is compiled in and nothing is downloaded on first start, and HTTPS works without a system CA bundle. Linux release archives are now named `*-unknown-linux-musl`; the installer handles both names. Intel macOS (`x86_64-apple-darwin`) is no longer a release target. `SIGIRO_EXTENSION_DIR` and `SIGIRO_DUCKDB_CACHE_DIR` are gone, as there are no extensions to cache. The API reference built into the binary now describes that binary's API. The container image is now built with Nix for linux/amd64 and linux/arm64 and published as `ghcr.io/akshatagarwl/sigiro`.

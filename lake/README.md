# lake

**Superseded:** the tenant lake was replaced by Parquet segments and a manifest
([ADR-0002](../docs/adr/0002-segments-and-manifest-replace-ducklake.md)). The
DuckDB runtime build here (json, icu, fileSystem) is still the reference for
what the query Worker's DuckDB lacks.

Workers runtime for the hosted tenant lake ([ADR-002](../docs/notes/decisions/002-hosted-sigiro-runs-only-on-workers.md),
[design](../docs/notes/designs/workers-tenant-lake.md)).

## Runtime

`runtime/build.sh` builds `@ducklings/workers-ducklake` from Sigiro's fork
([`akshatagarwl/ducklings@sigiro/lake-runtime`](https://github.com/akshatagarwl/ducklings/tree/sigiro/lake-runtime))
at a pinned commit. The fork adds the `json` and `icu` extensions and exports
`fileSystem()`. Whether to upstream it is undecided. The first build takes about 30 minutes; later builds
are incremental.

```sh
aube run runtime:build                 # -> runtime/dist/ducklings-workers-ducklake-1.5.5.tgz
aube install
aube run test:runtime-gate-e2e         # deploys a gate Worker, runs the gate, deletes it
```

The gate (`test/runtime-gate.feature`) builds a lake from `src/schema.rs`, runs
every SQL example in the English docs against seeded rows, and round-trips the
catalog through R2. A docs example that cannot run on Workers fails the gate.

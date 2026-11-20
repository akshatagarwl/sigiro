# Experimental cargo-dyndrv benchmark

**Result: retain Crane.** This trial did not improve the edit/build loop and did
not establish automatic cross-machine reuse of content-addressed realisations.
Production packages and the migration checkout were not changed.

## Scope

- Isolated branch `perf/nix-dyndrv-benchmark`, based on `f53abaf`.
- x86_64-linux PC; Nix CLI/daemon 2.36.0; Rust 1.99.1.
- cargo-dyndrv pinned to `d5245b85f82627fcaf041e2c57c8aeb7d57d20fe`.
- Unbundled development builds, same Rust sources, musl target, static relocation,
  and Cargo development profile. No persisted application incremental state.
- All timed commands used `--max-jobs 8 --cores 8`, sequentially, on a shared PC.
- Crane's dependency derivation builds only; it skips the default preliminary
  `cargo check`, since this benchmark is not measuring checks.
- This is **not a release/LTO comparison**. The experimental builder does not
  reproduce all Cargo release-profile options and rustflags.

## Measurements

| Operation | Crane | cargo-dyndrv |
| --- | ---: | ---: |
| Unchanged build, median of three | 0.053 s | 0.271 s |
| One-line CLI description edit, one sample | 32.629 s | 51.058 s |

The edit changed only the Clap `about` text in `src/main.rs`. Both resulting
binaries printed the changed description. The source was subsequently restored;
the original dynamic build was reused without compiling anything.

Initial successful runs took 103.431 s for Crane and 161 s for cargo-dyndrv.
**These are not a controlled cold-build comparison:** compatibility attempts had
already warmed some dynamic dependencies, and the dynamic configuration was
subsequently corrected to keep Cargo's mutable state outside imported sources.
The final dynamic configuration built with existing dependency outputs in
51.532 s. Tool bootstrap alone took another 121 s and is excluded from the table.
An accidentally release-profile Crane attempt was stopped and discarded; the
measured reference explicitly uses `CARGO_PROFILE=dev`.

Dependency crates were reused on the dynamic edit, but the imported source for
the `sigiro` package includes both its library and binary. Editing `main.rs`
therefore invalidated its library too. Per-crate Nix caching is not within-crate
rustc incremental compilation.

## Compatibility work

The pinned upstream builder was not drop-in for this workspace. The Nix-only
prototype in `dyndrv-benchmark.nix`:

- materializes Crane's symlinked vendor tree before importing crate sources;
- patches missing Cargo package/crate/bin and build-script environment variables;
- supplies the development split-debuginfo and static-relocation flags, plus
  Cargo-equivalent dependency lint capping;
- supplies native build tools and the project's static DuckDB configuration;
- restores musl CC/CXX after stdenv setup, before constructing the unit graph;
- places Cargo home/target state outside source directories imported as NARs.

This is an experimental compatibility layer, not a claim of complete Cargo
semantics or a proposed replacement for the production build.

## Validation

- Both outputs passed the existing static-binary/empty-store-reference checks.
- Their original-source OpenAPI JSON was byte-identical.
- The dynamic binary passed health, OTLP log ingestion, DuckLake persistence and
  SQL-query smoke tests.
- Changed CLI descriptions were verified in both edited outputs.
- The CLI source was restored byte-for-byte. No full workspace test-suite run is
  claimed for this prototype.

## Shared-cache limitation observed

A separate tiny content-addressed derivation was built and automatically
uploaded by the existing PC hook. Its signed output downloaded successfully from
Cachix into a fresh local store. However, building the same derivation there with
`--max-jobs 0` failed, **even after the known output path had been downloaded**.
The current integration did not supply the derivation-to-output realisation
mapping. Downloading a known store path alone does not prove reusable CA builds.
This is an observed limitation of the tested integration, not a blanket claim
that every Cachix configuration is incapable of supporting such builds.

## Reproduce on the configured PC

Requires Nix 2.36.0 with `nix-command flakes ca-derivations dynamic-derivations`
and sandboxed `builder-rpc-v0` support (already configured declaratively on this
PC). These outputs are exposed only for x86_64-linux.

```sh
nix build .#dyndrv-dev --max-jobs 8 --cores 8
nix build .#crane-dev --max-jobs 8 --cores 8
nix build .#checks.x86_64-linux.dyndrv-dev-portable \
  .#checks.x86_64-linux.crane-dev-portable --no-link
```

For edit measurements, build both baselines, change the CLI description once,
time each build sequentially, verify both `--help` outputs, then restore the
source. Do not delete shared store paths or other agents' Cargo caches.

Raw evidence on the PC is in `/tmp/sigiro-benchmark-{noop,edit}-*`,
`/tmp/sigiro-crane-dev-corrected.{log,status}`,
`/tmp/sigiro-dyndrv-{musl,final-baseline,restored}.*`,
`/tmp/sigiro-dyndrv-final-checks.log`, and
`/tmp/sigiro-ca-realisations-{proof,after-copy}.log`.

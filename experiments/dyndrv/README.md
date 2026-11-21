# cargo-dyndrv versus Crane (rejected experiment)

**Result: keep Crane.** cargo-dyndrv builds each crate as its own dynamic Nix
derivation. In this trial it made the edit/build loop slower, and the shared
cache could not reuse its content-addressed outputs on another machine. The
production build (the root `flake.nix`) does not use it.

This directory is a separate flake, so the root flake, its lock and
`nix flake check` stay unchanged. It reuses the root flake's inputs and
`nix/rust.nix`, and adds a pinned cargo-dyndrv
(`d5245b85f82627fcaf041e2c57c8aeb7d57d20fe`, upstream HEAD when tested).

The raw evidence (build logs, exit statuses, derivation samples and timings)
is kept at commit
[`ac5315c`](https://github.com/akshatagarwl/sigiro/tree/ac5315cc399569496d6b6c6754775e9bc485000b/repro).
The upstream reports link to that commit, and the tag
`experiments/nix-dyndrv-benchmark` keeps it reachable.

## Requirements

x86_64-linux only, with Nix 2.36 and these experimental features:
`nix-command flakes ca-derivations dynamic-derivations`, plus sandboxed
`builder-rpc-v0` support. CI does not have them, and most machines don't
either.

## Measurements

These were taken at `f53abaf` on the x86_64-linux PC with Rust 1.99.1. Both
sides built the same Rust sources into unbundled development binaries for the
musl target with static relocation. Neither kept incremental state for the
application. Every command ran sequentially with `--max-jobs 8 --cores 8`.

| Operation | Crane | cargo-dyndrv |
| --- | ---: | ---: |
| Unchanged build, median of three | 0.053 s | 0.271 s |
| One-line CLI description edit, one sample | 32.629 s | 51.058 s |

The edit changed only the Clap `about` text in `src/main.rs`. These numbers come
from development builds, not release builds with LTO. Treat the first full
builds (103 s with Crane, 161 s with cargo-dyndrv) as uncontrolled, because
earlier attempts had already warmed some dynamic dependencies. Bootstrapping the
tool took another 121 s, which the table leaves out.

Two things explain the result, at least in part:

- **Source granularity.** cargo-dyndrv imports a package's whole manifest
  directory as the source of every crate in it. Editing `main.rs` therefore
  rebuilt the `sigiro` library as well. Crane rebuilt it too, so this alone
  doesn't explain the gap.
- **Extra LLVM bitcode.** cargo-dyndrv's development libraries keep `.llvmbc`
  sections, because it never passes `-C embed-bitcode=no` the way Cargo does.
  So the two builds didn't use identical compiler flags. The bitcode's share of
  the slowdown was never measured in isolation.

Both binaries passed the static-binary check and produced byte-identical
OpenAPI output. The dynamic binary also passed smoke tests for health, OTLP log
ingest, DuckLake persistence and SQL queries.

## Full-application builds

```sh
cd experiments/dyndrv
nix build .#crane-dev .#dyndrv-dev --max-jobs 8 --cores 8
nix build .#checks.x86_64-linux.crane-dev-portable \
  .#checks.x86_64-linux.dyndrv-dev-portable --no-link
```

To measure an edit, build both baselines first. Then change the CLI
description once, time each build in turn, check both `--help` outputs and
restore the source.

The upstream builder doesn't work on this workspace without changes.
`benchmark.nix` adds a compatibility layer:

- it copies Crane's vendored crates out of symlinks before importing them;
- it patches in missing Cargo package, crate and binary variables, plus
  build-script variables;
- it adds Cargo's development flags (split debug info, static relocation and
  lint capping for dependencies);
- it provides native build tools and the static DuckDB settings;
- it switches CC and CXX back to the musl compilers after stdenv setup;
- it keeps Cargo's home and target directories out of the imported sources.

## Minimal reproductions

`repro-crate/` contains one library, one binary and one build script.
`repros.nix` builds it with the **unpatched** upstream `buildDynamicCrate`:

```sh
cd experiments/dyndrv
nix build .#repro-cargo-control -L --no-link  # passes under plain Cargo via Crane
nix build .#repro-package-env -L --no-link    # fails: CARGO_PKG_NAME missing
nix build .#repro-build-env -L --no-link      # fails: DEBUG missing in build script
nix build .#repro-rustflags -L --no-link      # fails: RUSTFLAGS not forwarded
nix build .#repro-lib-bin -L --no-link        # passes
nix build .#repro-lib-bin-edited -L --no-link # passes; only main.rs differs
```

`repro-lib-bin` and `repro-lib-bin-edited` build different library derivations
even though `src/lib.rs` is byte-identical in both.

## Content-addressed outputs and Cachix

`cachix-ca.nix` is a tiny content-addressed derivation with no dependencies.
The PC's post-build hook uploaded its output to `akshat-sigiro-builds`. A fresh
store can download that output by path, but still can't build the derivation
with `--max-jobs 0`. Cachix lacked the realisation, the record that maps a
content-addressed derivation to its output:

```sh
proof=$(mktemp -d /tmp/sigiro-ca-proof.XXXXXX)
store="local?root=$proof"
nix build --file experiments/dyndrv/cachix-ca.nix --store "$store" --max-jobs 0 --no-link  # fails
nix copy --from https://akshat-sigiro-builds.cachix.org --to "$store" \
  /nix/store/n7ljimksma8r35gzahrw4z8njy59vn5r-sigiro-ca-realisations-probe
nix build --file experiments/dyndrv/cachix-ca.nix --store "$store" --max-jobs 0 --no-link  # still fails
```

The PC's hook also uploads only outputs whose names contain `-sigiro-`. Dynamic
crate outputs have names like `source`, so the hook skips them.

## Upstream tracking

- [cargo-dyndrv #7: missing Cargo environment](https://github.com/obsidiansystems/cargo-dyndrv/issues/7)
  ([PR #4](https://github.com/obsidiansystems/cargo-dyndrv/pull/4) adds the
  package and crate variables, but not `DEBUG` for build scripts).
- [cargo-dyndrv #8: RUSTFLAGS forwarding](https://github.com/obsidiansystems/cargo-dyndrv/issues/8).
- [cargo-dyndrv #9: development LLVM-bitcode mismatch](https://github.com/obsidiansystems/cargo-dyndrv/issues/9).
- [Cachix PR #732: CA realisation pushing](https://github.com/cachix/cachix/pull/732#issuecomment-7039547909).

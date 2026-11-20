# Public cargo-dyndrv and Cachix reproductions

Tested on x86_64-linux / NixOS with Nix CLI and daemon 2.36.0, Rust 1.99.1,
and Cachix 1.12.1. cargo-dyndrv is pinned to
`d5245b85f82627fcaf041e2c57c8aeb7d57d20fe` (upstream HEAD when tested).

The daemon needs `ca-derivations dynamic-derivations` enabled and sandboxed
`builder-rpc-v0` support. Standard `nix-command flakes` features are also needed.
No upload credentials are required to run the cargo-dyndrv reproductions.

## Tiny dependency-free crate

`cargo-dyndrv/` contains one library, one binary and one build script.
`../nix/dyndrv-repros.nix` calls upstream `buildDynamicCrate` with the **unpatched**
upstream tool, unlike the compatibility-patched full Sigiro benchmark.
It explicitly selects `--bin dyndrv-repro` so the declared output matches the
Cargo unit-graph root. The flags and environment are declared in Nix.

From the repository root:

```sh
# Passes all three checks under ordinary Cargo via Crane; verbose rustc args.
nix build .#dyndrv-repro-cargo-control -L --no-link

# Expected failure: CARGO_PKG_NAME missing at compile time.
nix build .#dyndrv-repro-package-env -L --no-link

# Expected failure: DEBUG missing from the build-script environment.
nix build .#dyndrv-repro-build-env -L --no-link

# Expected failure despite RUSTFLAGS="--cfg required_by_repro" in Nix.
nix build .#dyndrv-repro-rustflags -L --no-link

# Both pass; the edited source differs ONLY in the binary's printed string.
nix build .#dyndrv-repro-lib-bin -L --no-link
nix build .#dyndrv-repro-lib-bin-edited -L --no-link
```

Expected failure does not mean the Nix expression is malformed: both the ordinary
Cargo control and the featureless dynamic variants succeed.

## Why the initial experiment did not help

### Package-directory source granularity

The two successful dynamic variants produce different library derivations even
though `src/lib.rs` is byte-identical. A recursive diff of the source NARs shows
only `src/main.rs` changed; both library source hashes are
`4ae6bddbc6068d796a7c82a52bc684a8c44fdb1a588c17dcb5381a80291138f0`.

The recorded library derivations are:

- baseline: `7z7yk9aq9i8vgrsnf64ik1jrprcm46hk-source.drv`
- binary-only edit: `ai19pqmb31zmv3y7ld30m8chsdwxaj6y-source.drv`

See `evidence/dyndrv/sigiro-library-lib-bin*.json` for compiler arguments and
manifest directories. Upstream imports each package's entire manifest directory
as its source input. This is conservative package-source caching, not within-crate
incremental compilation. It explains why the library is not spared; **it does not
by itself explain the timing difference versus Crane**, which also rebuilt it.

### Extra LLVM bitcode in development artifacts

The tiny dynamic library contains `.llvmbc` and `.llvmcmd` sections. Its compiler
arguments omit `-C embed-bitcode=no`. The Cargo control's verbose log explicitly
passes `-C embed-bitcode=no`. Upstream's code also marks embed-bitcode handling
as a TODO.

This is a real compiler-flag mismatch despite requesting the same development
profile. It is a plausible source of extra compilation/store-I/O work, but we
have **not** measured an isolated bitcode-on/off A/B. Do not attribute the entire
full-application slowdown to it or claim a framework-only comparison.

## Cachix CA realisation mapping, separately

`cachix-ca.nix` is independent of Cargo and has no package dependencies. It uses
`/bin/sh`, which is exposed by the tested NixOS sandbox configuration. It has no
fixed expected hash: it is a floating content-addressed derivation.

The existing automatic PC hook uploaded the output. Public output:

```text
/nix/store/n7ljimksma8r35gzahrw4z8njy59vn5r-sigiro-ca-realisations-probe
```

To reproduce against the already-published object, with this cache and its public
key configured as substituter/trusted key:

```sh
proof=$(mktemp -d /tmp/sigiro-ca-proof.XXXXXX)
store="local?root=$proof"

# Fails: Nix cannot resolve the floating CA output; builds are disabled.
nix build --file repro/cachix-ca.nix --store "$store" --max-jobs 0 --no-link

# Succeeds: the signed NAR exists and downloads by its known path.
nix copy --from https://akshat-sigiro-builds.cachix.org --to "$store" \
  /nix/store/n7ljimksma8r35gzahrw4z8njy59vn5r-sigiro-ca-realisations-probe

# Still fails: possessing the NAR did not supply the realisation mapping.
nix build --file repro/cachix-ca.nix --store "$store" --max-jobs 0 --no-link
```

Public cache key:
`akshat-sigiro-builds.cachix.org-1:YjaKP3P+EoMjSjDAN1yI5U5dbZr5l9yNI5qeFphJvA8=`.
See the narinfo and both failure logs in `evidence/dyndrv/`.
This is distinct from a missing signature or a failed NAR upload.
Existing upstream work: https://github.com/cachix/cachix/pull/732.

Our PC's scoped hook also selects `*-sigiro-*` output names. Dynamic crate
artifacts commonly use names such as `source`, so that hook would need a
project-scoped compiler-artifact integration as well. Scrubbed runtime binaries
have no references to those build artifacts. This naming/closure limitation is
ours, not an upstream Cachix defect; the tiny `sigiro-ca-realisations-probe`
above deliberately matches the existing hook and isolates the metadata problem.

## Evidence and limits

`evidence/dyndrv/` publishes build logs, exit statuses, sampled derivations and
full-application timing logs. Checkout-local paths were replaced with
`<checkout>`; derivation records retain only names, compiler arguments and
manifest directories. Credentials, user configuration, application databases
and unrelated agents' files are not included.

See `../nix/dyndrv-benchmark.md` for the 51.058 s dynamic versus 32.629 s Crane
edit sample, cold-build caveats and runtime validation. These are development
build observations, not release/LTO results or a statistically controlled claim
that cargo-dyndrv cannot be faster for other workspaces.

# Contributing to Sigiro

Thanks for helping. Bug reports, fixes, and small focused pull requests are welcome.
For a larger change, open an issue first so we can agree on the approach.

## Before you start

* Read [CONTEXT.md](CONTEXT.md) for the project's vocabulary.
* Never commit secrets, tokens, customer data, or personal paths. CI runs
  [gitleaks](https://github.com/gitleaks/gitleaks) on every pull request.

## Set up

Follow [Build from source](README.md#build-from-source): the binary is built
with Nix. `nix develop` supplies Rust, wasm-bindgen, Python, Node and pnpm;
`nix develop .#javascript` is the smaller shell for website, auth and release
work. All tool versions come
from the locked Nix inputs and `rust-toolchain.toml`. No cloud accounts or
secrets are needed to build or test. There is no task runner or Git-hook installer.

Install JavaScript dependencies from the repository root with
`pnpm install --frozen-lockfile`, and in each package directory (`auth/`,
`cloud/`, `dashboard/`, `www/`) with its own frozen lockfile, for example
`pnpm -C dashboard install --frozen-lockfile`. Run installs explicitly; package
scripts do not install dependencies. `cloud/` imports a generated WASM module,
so run `pnpm -C cloud run build:wasm` once before `pnpm -C cloud run check`.

For a complete binary with embedded dashboard and docs, use `nix run . -- serve`.
For frontend editing, use `pnpm -C dashboard run dev` or `pnpm -C www run dev`.
If local PostgreSQL and RustFS are needed, run `scripts/dev-certs.sh` followed
by `pnpm -C www run alchemy:dev:deploy` (Docker required). Preview that stack
with `pnpm -C www run alchemy:dev:plan`. Its explicit
`pnpm -C www run alchemy:dev:destroy` command also deletes its local volumes;
there is no automatic teardown. Production planning/deployment remain explicit
`pnpm -C www run alchemy:plan` / `pnpm -C www run alchemy:deploy` commands.

### Fast local Rust development

Use `nix develop` and `cargo build --locked` for the edit/build loop, not
`nix build`: Cargo keeps incremental workspace artifacts in `target/`.
The dev and test profiles explicitly enable incremental compilation; release
optimization settings are unchanged. With direnv and nix-direnv installed,
run `direnv allow` once to activate the pinned shell automatically.

Linux additionally provides `nix develop .#mold` and `nix develop .#wild`
for linker comparisons. They keep separate caches in `target/dev-mold` and
`target/dev-wild`; the first build of each is cold. Neither linker is selected
by default. Apple Silicon macOS uses its normal linker and the default shell; mold and Wild
shells are intentionally absent there. These are development-only choices:
release and CI builds continue to use the existing toolchain.

### How the build works

`flake.nix` and `nix/` build the binary from locked inputs in an offline build
sandbox. `nix build --rebuild` can check whether a rebuild is byte-for-byte
identical; that comparison has not been recorded for this build. Nothing is pinned twice: Rust comes from
`rust-toolchain.toml`, Node and pnpm from the locked nixpkgs, and the version
from `Cargo.toml`.

* DuckDB comes from the [duckdb-static](https://github.com/akshatagarwl/duckdb-static)
  flake input: one static archive with every extension sigiro uses compiled
  in (core_functions, parquet, json, icu, ducklake, httpfs, postgres_scanner,
  quack) and their C dependencies. It updates itself and has its own binary
  cache; sigiro follows its nixpkgs so the archive is a cache hit. sigiro
  registers the extensions at startup (`src/pool.rs`) and never downloads or
  loads an extension at run time. To pick up a new DuckDB, run
  `nix flake update duckdb-static` and update the version pins in
  `src/storage.rs` and `src/quack.rs` if they changed.
* `rust.nix`: crane builds for nixpkgs' static platform: musl on Linux, so the
  binary is fully static; static libraries on macOS, linking only the system's
  libSystem and libc++. Dependencies are compiled once and shared. The
  `portable` check fails if the binary links anything outside the OS, and the
  build fails if any store path is left in it.
* `dashboard.nix`, `docs.nix`: the two static exports the binary embeds.
  Their npm dependencies come directly from committed `pnpm-lock.yaml` files.
  `pnpm-project.nix` fetches the locked dependencies for offline, frozen installs;
  there is no lockfile translation or second package manager.
* The docs' API reference is rendered from `sigiro openapi`, the spec of the
  binary being built, rather than from the live sigiro.com spec.
* `image.nix`: nix2container image metadata referencing just the static binary;
  no image archive is stored in the Nix store. `nix build .#image` builds this
  Linux system's metadata; `nix run .#image.copyToDockerDaemon` loads Docker.
  The pinned `akshatagarwl/nix2container` fork preserves Docker's Healthcheck
  field (upstream PR #198); return upstream once that fix is merged.
* `image-check.nix`: sandboxed config, ownership, layer and binary-identity
  checks, run by `nix flake check` without Docker or network access.
* `publish-image.nix`: `nix run .#publish-image -- ghcr.io/akshatagarwl/sigiro:<tag>`
  pushes the linux/amd64 and linux/arm64 images as `<tag>-amd64` and
  `<tag>-arm64`, then makes `<tag>` an index over both, skipping blobs the
  registry already has (log in with `docker login` first). Each binary is
  compiled natively on its own system and fetched from the cache; publishing
  needs both cached binaries but no QEMU. Platform verification builds and
  health-checks images on native runners on every push to main, then publishes
  that commit's image as `sha-<12-character commit>`. The Release workflow
  publishes version and `latest` image tags on version-tag releases; manual
  runs remain artifact-only. There is no separate image workflow.

After changing `dashboard/pnpm-lock.yaml` or `www/pnpm-lock.yaml`, update the
matching `hash` in `nix/dashboard.nix` or `nix/docs.nix`: set it to `""`,
run `nix build .#dashboard` (or `.#docs`), and copy the `got:` hash from the
error.

Without the duckdb-static cache, the first build compiles DuckDB (most of an
hour). Nix uses the cache the flake declares only for trusted users; otherwise
add it to `/etc/nix/nix.conf` (or `nix.settings` on NixOS):

```
extra-substituters = https://akshat-sigiro-builds.cachix.org https://github.com/akshatagarwl/duckdb-static/releases/download/nix-cache
extra-trusted-public-keys = akshat-sigiro-builds.cachix.org-1:YjaKP3P+EoMjSjDAN1yI5U5dbZr5l9yNI5qeFphJvA8= duckdb-static.github.akshatagarwl-1:1yQ0flJ3vpCoIicGqSTO8gHezxiCOj2M0VLyJXXqQVQ=
```

### Shared build cache

CI and developer machines share the public `akshat-sigiro-builds` Cachix cache.
Downloads require no credentials. On NixOS, declare the cache URL and public
key above through `nix.settings.extra-substituters` and
`nix.settings.extra-trusted-public-keys` instead of editing the generated
`nix.conf`.

CI uses the official `cachix/cachix-action` with its post-build-hook daemon.
The Cachix client is pinned through `.#cache-client`. New build outputs upload
automatically; no repository scripts or changes to build commands are needed.
Forked CI runs without a write token remain read-only.

On hcpc, Nix's system-wide `post-build-hook` queues newly built Sigiro outputs
to a systemd-managed Cachix daemon. Normal `nix build` and `nix flake check`
commands trigger background uploads. The hook selects store names containing
`-sigiro-`; required runtime closures upload too. Unrelated output roots are
not queued. Do not use a whole-store watcher with this public cache: it could
publish machine configuration and unrelated projects.

The PC daemon receives its cache-scoped token using systemd `LoadCredential`
from `~hc/.config/cachix/akshat-sigiro-builds.token` (mode `0600`). CI uses its
separate `CACHIX_AUTH_TOKEN` secret. Keep credentials outside the repository and
Nix store; never put token contents in Nix settings.

Hooks apply to newly built outputs, not substituted or already-valid paths.
The initial existing PC artifacts have been seeded separately. Uploading is
asynchronous, so a build can finish before its outputs become downloadable.
For another machine, use Cachix's documented post-build-hook/daemon setup or
`cachix watch-exec akshat-sigiro-builds -- nix build` for a one-off session.

The exposed `rust-deps`, `rust-workspace-deps`, and `rust-wasm-deps` packages
make Crane artifacts reusable alongside application, dashboard, docs, and check
outputs. These are Nix outputs, not the incremental Cargo `target/` directory.
Reusing an output requires the same derivation, including platform and locked
inputs; Linux builds do not replace native macOS or ARM64 builds. Monitor cache
storage: three platforms and multiple revisions may exceed the free 5 GB tier.

## Repository layout

| Path | What it is |
|---|---|
| `src/`, `crates/` | Rust: the `sigiro` binary (ingest, query, CLI, MCP), `sigiro-core`, segment format (+ WASM build) |
| `cloud/` | Hosted service: Cloudflare Worker and Durable Object cells |
| `auth/` | Hosted auth: Better Auth on Workers + D1 (OAuth, device flow, organizations) |
| `dashboard/` | Web dashboard (Next.js static export), embedded in the binary |
| `www/` | Website and documentation (sigiro.com/docs) |
| `tests/` | Rust integration tests, end-to-end and deployment contract tests |
| `experiments/` | Rejected build experiments kept for reproduction, each a separate flake (not built by CI) |

## Make a change

1. Branch from `main`.
2. Keep the change to one concern and include tests for behaviour changes.
3. Run the checks for what you touched:
   * Rust: `nix flake check` (rustfmt, clippy, tests, the binary and its
     portability), plus `nix develop --command python3 -m unittest tests.deployment.test_sync_postgres_boundary`
     for the deployment contract. Run single checks with
     `nix build .#checks.<system>.<name>` rather than plain `cargo`, so their
     outputs are cached and reused by CI.
   * TypeScript: `pnpm run fmt:check && pnpm run lint`, plus the package's own
     `check` script (for example `pnpm -C auth run check`)
4. Use [Conventional Commits](https://www.conventionalcommits.org/) for commit
   messages and the PR title, for example `fix(ingest): reject empty batches`.
5. If the change is visible to people who run `sigiro`, add a changeset (below).
6. Open a pull request that says what changed, why, and how you tested it.

## Changesets

The changelog and the version of the `sigiro` binary come from
[Changesets](https://github.com/changesets/changesets). Run
`pnpm run changeset`, choose the bump (`patch` for fixes, `minor` for features,
`major` for breaking changes), and write one line for the changelog. Commit the
generated `.changeset/*.md` file with your change.

The repository root (`package.json`, named `sigiro`) is the only versioned
package; its `version` is the binary's version. `auth/`, `cloud/`,
`dashboard/` and `www/` are not versioned. A PR that touches `src/`, `crates/`,
`dashboard/`, `build.rs` or `Cargo.toml` without a changeset gets a CI warning,
never a failure. For changes that need no changelog line, ignore it or run
`pnpm run changeset --empty`.

## Releases

1. When `main` has pending changesets, a maintainer (or an agent) runs
   `pnpm run release:pr`. From `origin/main` it runs `pnpm run release:version`,
   which bumps `package.json`, copies the version into `Cargo.toml` and
   `Cargo.lock`, adds the release to `CHANGELOG.md` and deletes the consumed
   changesets. It then commits on `release/v<version>`, pushes and opens the PR
   with `gh`. `pnpm run release:pr --dry-run` shows the commit without pushing.
2. CI runs on that PR like any other. Review the bump and the changelog, then
   merge it.
3. On the push to `main`, `tag-on-main.yml` sees a version with no tag, pushes
   `v<version>` and, in the same run, calls `release.yml`: three archives and
   `SHA256SUMS` on a GitHub Release. sigiro-infra then copies the release to
   sigiro.com/dl.

Release notes should lead with user-visible changes, then list supported
OS/architecture combinations from the actual uploaded archives, install/update
instructions, checksum verification, and known limitations or breaking changes.
Keep the generated contributor/PR changelog below that summary. Do not imply that
unreleased `main` features are included in an existing version, and do not replace
published archives or checksums just to improve the presentation.

Every other push to `main` finds the tag already there and does nothing.
`release.yml` refuses a tag that does not match `Cargo.toml` and a version that
already has a GitHub Release. If a release fails after the tag is pushed,
re-run the failed jobs of that run, or run `release.yml` manually on the tag
(`gh workflow run release.yml --ref v<version>`). Pushing a `v*` tag by hand
still starts `release.yml` too. No GitHub App, PAT or repository setting is
involved.

## What CI runs on forks

Pull requests from forks run only jobs that need no repository secrets
(JavaScript format/lint/typecheck and secret scanning). Release and deployment
jobs run only for maintainers.

## Repository presentation

Keep the GitHub About description, website (`https://sigiro.com`), and topics
aligned with the README. Topics describe the product; version tags identify
releases. Name the FSL license explicitly rather than calling the application
OSI-approved open source.

The repository social card is [`.github/social-preview.png`](.github/social-preview.png)
(1280×640). Upload it under the repository's **Settings → General → Social preview**;
committing the image alone does not configure GitHub's preview. The dashboard
crop shows example telemetry, not a production benchmark. The website's Open
Graph card is a separate asset with its own generation process.

## Reporting security issues

Do not open a public issue. See [SECURITY.md](SECURITY.md).

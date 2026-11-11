# Contributing to Sigiro

Thanks for helping. Bug reports, fixes, and small focused pull requests are welcome.
For a larger change, open an issue first so we can agree on the approach.

## Before you start

* Read [CONTEXT.md](CONTEXT.md) for the project's vocabulary.
* Never commit secrets, tokens, customer data, or personal paths. CI runs
  [gitleaks](https://github.com/gitleaks/gitleaks) on every pull request.

## Set up

Follow [Build from source](README.md#build-from-source). Toolchains are pinned in
`mise.toml`; no cloud accounts or secrets are needed to build or test.

DuckDB is linked statically from a pinned, checksum-verified prebuilt archive
(`scripts/prepare-duckdb-static.sh`), so DuckDB is never compiled here.

Install JavaScript dependencies from the repository root with
`aube install --frozen-lockfile`, and in each package directory (`auth/`,
`cloud/`, `dashboard/`, `www/`) with its own frozen lockfile, for example
`aube -C dashboard install --frozen-lockfile`. `cloud/` imports a generated
WASM module, so run `aube -C cloud run build:wasm` once before
`aube -C cloud run check`.

## Repository layout

| Path | What it is |
|---|---|
| `src/`, `crates/` | Rust: the `sigiro` binary (ingest, query, CLI, MCP), `sigiro-core`, segment format (+ WASM build) |
| `cloud/` | Hosted service: Cloudflare Worker and Durable Object cells |
| `auth/` | Hosted auth: Better Auth on Workers + D1 (OAuth, device flow, organizations) |
| `dashboard/` | Web dashboard (Next.js static export), embedded in the binary |
| `www/` | Website and documentation (sigiro.com/docs) |
| `tests/` | Rust integration tests, end-to-end and deployment contract tests |

## Make a change

1. Branch from `main`.
2. Keep the change to one concern and include tests for behaviour changes.
3. Run the checks for what you touched:
   * Rust: `mise run full` (rustfmt, cargo check, clippy, deployment contract
     tests; format with `mbx fmt`) and
     `scripts/run-with-duckdb-static.sh cargo test --workspace --locked`
   * TypeScript: `aube run fmt:check && aube run lint`, plus the package's own
     `check` script (for example `aube -C auth run check`)
4. Use [Conventional Commits](https://www.conventionalcommits.org/) for commit
   messages and the PR title, for example `fix(ingest): reject empty batches`.
5. If the change is visible to people who run `sigiro`, add a changeset (below).
6. Open a pull request that says what changed, why, and how you tested it.

## Changesets

The changelog and the version of the `sigiro` binary come from
[Changesets](https://github.com/changesets/changesets). Run
`aube run changeset`, choose the bump (`patch` for fixes, `minor` for features,
`major` for breaking changes), and write one line for the changelog. Commit the
generated `.changeset/*.md` file with your change.

The repository root (`package.json`, named `sigiro`) is the only versioned
package; its `version` is the binary's version. `auth/`, `cloud/`,
`dashboard/` and `www/` are not versioned. A PR that touches `src/`, `crates/`,
`dashboard/`, `build.rs` or `Cargo.toml` without a changeset gets a CI warning,
never a failure. For changes that need no changelog line, ignore it or run
`aube run changeset --empty`.

## Releases

1. When `main` has pending changesets, a maintainer (or an agent) runs
   `aube run release:pr`. From `origin/main` it runs `aube run release:version`,
   which bumps `package.json`, copies the version into `Cargo.toml` and
   `Cargo.lock`, adds the release to `CHANGELOG.md` and deletes the consumed
   changesets. It then commits on `release/v<version>`, pushes and opens the PR
   with `gh`. `aube run release:pr -- --dry-run` shows the commit without pushing.
2. CI runs on that PR like any other. Review the bump and the changelog, then
   merge it.
3. On the push to `main`, `tag-on-main.yml` sees a version with no tag, pushes
   `v<version>` and, in the same run, calls `release.yml`: four archives and
   `SHA256SUMS` on a GitHub Release. sigiro-infra then copies the release to
   sigiro.com/dl.

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

## Reporting security issues

Do not open a public issue. See [SECURITY.md](SECURITY.md).

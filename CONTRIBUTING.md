# Contributing to Sigiro

Thanks for helping. Bug reports, fixes, and small focused pull requests are welcome.
For a larger change, open an issue first so we can agree on the approach.

## Before you start

* Read [CONTEXT.md](CONTEXT.md) for the project's vocabulary and
  [docs/adr/](docs/adr/) for decisions that are already made.
* Never commit secrets, tokens, customer data, or personal paths. CI runs
  [gitleaks](https://github.com/gitleaks/gitleaks) on every pull request.

## Set up

Follow [Build from source](README.md#build-from-source). Toolchains are pinned in
`mise.toml`; no cloud accounts or secrets are needed to build or test.

## Make a change

1. Branch from `main`.
2. Keep the change to one concern and include tests for behaviour changes.
3. Run the checks for what you touched:
   * Rust: `mise run full` and
     `scripts/run-with-duckdb-static.sh cargo test --workspace --locked`
   * TypeScript: `aube run fmt:check && aube run lint`, plus the package's own
     `check` script (for example `aube -C auth run check`)
4. Use [Conventional Commits](https://www.conventionalcommits.org/) for commit
   messages and the PR title, for example `fix(ingest): reject empty batches`.
5. Open a pull request that says what changed, why, and how you tested it.

## What CI runs on forks

Pull requests from forks run only jobs that need no repository secrets
(JavaScript format/lint/typecheck and secret scanning). Release and deployment
jobs run only for maintainers.

## Reporting security issues

Do not open a public issue. See [SECURITY.md](SECURITY.md).

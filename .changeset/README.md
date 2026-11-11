# Changesets

Each file here describes one user-visible change to the `sigiro` binary. Add
one with `aube run changeset`, pick `patch`, `minor` or `major`, and write the
line the changelog should show. See [CONTRIBUTING.md](../CONTRIBUTING.md#changesets).

The only versioned package is the repository root, named `sigiro`. Its
`version` is the binary's version: `aube run release:version` bumps it and
copies it into `Cargo.toml` and `Cargo.lock`; `aube run release:pr` opens the
release PR. See [CONTRIBUTING.md](../CONTRIBUTING.md#releases).

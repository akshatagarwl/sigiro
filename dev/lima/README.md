# Sigiro Development VMs (Lima)

Lima VM configs for local E2E testing on macOS (Apple Silicon).

## Prerequisites

- [Lima](https://lima-vm.io/) installed (`brew install lima`)
- [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) for cross-compilation (`brew install zig && cargo install cargo-zigbuild`)
- LLVM's `llvm-ar` for cross-compile archiving (`brew install llvm`)

## Cross-Compilation Setup

Lima VMs run Linux aarch64. Cross-compile from macOS arm64:

```bash
# Required: zig 0.16.0's built-in ar is broken, use homebrew llvm-ar
export AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar
ulimit -n 65536

cargo zigbuild --release --target aarch64-unknown-linux-gnu
```

See `.cargo/config.toml` for details on the cross-compile workaround.

## VMs

| VM | Config | Purpose |
|---|---|---|
| `sigiro-local` | `sigiro-local.yaml` | Local storage mode (DuckLake files on disk) |
| `sigiro-s3` | `sigiro-s3.yaml` | S3 storage mode (MinIO + PostgreSQL catalog) |

## Quick Start

```bash
# 1. Start a VM
limactl start --name=sigiro-local dev/lima/sigiro-local.yaml

# 2. Cross-compile and copy binary
ulimit -n 65536 && \
  AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar \
  cargo zigbuild --release --target aarch64-unknown-linux-gnu
limactl copy target/aarch64-unknown-linux-gnu/release/sigiro sigiro-local:/home/akshat.guest/sigiro-latest

# 3. Run E2E test
limactl copy tests/lima-e2e-test.sh sigiro-local:/tmp/lima-e2e-test.sh
limactl shell sigiro-local bash -c \
  'SIGIRO_BIN=/home/akshat.guest/sigiro-latest MODE=local \
   SIGIRO_DATA=/tmp/sigiro-e2e-test bash /tmp/lima-e2e-test.sh'
```

## Known Issues

- **9p mount stale caching**: Files written on the host may not appear immediately in the VM.
  Use `limactl copy <file> <vm>:/tmp/<file>` instead of relying on the shared mount.
- **Federated mode**: Not yet supported by these VMs. Will need a `sigiro-central` config
  plus multiple `sigiro-collector` instances.

## Teardown

```bash
limactl stop sigiro-local sigiro-s3
limactl delete sigiro-local sigiro-s3
```

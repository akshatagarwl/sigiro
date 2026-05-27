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
| `fed-central` | `sigiro-fed-central.yaml` | Federation central node (k3s server + sigiro --central) |
| `fed-collector-1` | `sigiro-fed-collector.yaml` | Federation collector (k3s worker + sigiro --ebpf) |
| `fed-collector-2` | `sigiro-fed-collector.yaml` | Federation collector (k3s worker + sigiro --ebpf) |

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

## Federation E2E Test

Multi-node k3s cluster with quack-based federation:

```bash
# Run the full federation E2E test (3 VMs, k8s, OnlineBoutique)
./dev/lima/sigiro-fed-test.sh
```

The test script:
1. Starts 3 Lima VMs — `fed-central` (k3s server), `fed-collector-1/2` (k3s workers)
2. Cross-compiles sigiro and copies to all VMs
3. Builds Docker image and loads into k3s
4. Deploys k8s manifests: PostgreSQL, MinIO, sigiro-central, sigiro-collector DaemonSet
5. Deploys OnlineBoutique demo services
6. Verifies federation via `/v1/federation/workers` and `/v1/investigate`

Manual steps (if not using the script):

```bash
# 1. Start central VM (k3s server)
limactl start --name=fed-central dev/lima/sigiro-fed-central.yaml

# 2. Get k3s token
TOKEN=$(limactl shell fed-central sudo cat /var/lib/rancher/k3s/server/node-token)
IP=$(limactl list fed-central --json | jq -r '.[0].ssh_address' | sed 's/127.0.0.1://')

# 3. Start collector VMs
limactl start --name=fed-collector-1 --env K3S_SERVER_IP=$IP --env K3S_AGENT_TOKEN=$TOKEN dev/lima/sigiro-fed-collector.yaml
limactl start --name=fed-collector-2 --env K3S_SERVER_IP=$IP --env K3S_AGENT_TOKEN=$TOKEN dev/lima/sigiro-fed-collector.yaml
```

## Known Issues

- **9p mount stale caching**: Files written on the host may not appear immediately in the VM.
  Use `limactl copy <file> <vm>:/tmp/<file>` instead of relying on the shared mount.

## Teardown

```bash
# Single-node VMs
limactl stop sigiro-local sigiro-s3
limactl delete sigiro-local sigiro-s3

# Federation VMs
limactl stop fed-central fed-collector-1 fed-collector-2
limactl delete fed-central fed-collector-1 fed-collector-2
```

# Sigiro Development VMs (Lima)

Lima VM configs for local E2E testing on macOS (Apple Silicon).

## Prerequisites

- [Lima](https://lima-vm.io/) installed (`brew install lima`)
- GNU aarch64-linux cross-compiler (`brew install aarch64-elf-gcc` or `brew install messense/homebrew-macos-cross-toolchains/aarch64-unknown-linux-gnu`)

## Setup

The VM configs use `{{.Home}}/sigiro` as the mount point. Before starting VMs, create a symlink from your repo to `~/sigiro`:

```bash
# One-time: symlink your repo clone to ~/sigiro
ln -s "$(pwd)" ~/sigiro
```

This ensures the mount works regardless of where the repository is cloned.

## Cross-Compilation Setup

Lima VMs run Linux aarch64. Cross-compile from macOS arm64:

```bash
source scripts/lima-cross-env.sh
ulimit -n 65536

# Optional: ccache for faster DuckDB C++ rebuilds
export CC="ccache ${CC_aarch64_unknown_linux_gnu}"
export CXX="ccache ${CXX_aarch64_unknown_linux_gnu}"
export CC_aarch64_unknown_linux_gnu="$CC"
export CXX_aarch64_unknown_linux_gnu="$CXX"

cargo build --release --target aarch64-unknown-linux-gnu --bin sigiro
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
# 1. Ensure ~/sigiro symlink exists (see Setup above)
# 2. Start a VM
limactl start --name=sigiro-local dev/lima/sigiro-local.yaml

# 3. Cross-compile and copy binary
source scripts/lima-cross-env.sh
ulimit -n 65536
cargo build --release --target aarch64-unknown-linux-gnu --bin sigiro
limactl copy target/aarch64-unknown-linux-gnu/release/sigiro sigiro-local:~/sigiro/bin/sigiro-latest

# 4. Run E2E test
limactl copy tests/vm/lima-e2e-test.sh sigiro-local:/tmp/lima-e2e-test.sh
limactl shell sigiro-local bash -c \
  'SIGIRO_BIN=~/sigiro/bin/sigiro-latest MODE=local \
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
6. Verifies federation via `/v1/federation/workers` and `/v1/diagnose`

Manual steps (if not using the script):

```bash
# 1. Start central VM (k3s server)
limactl start --name=fed-central dev/lima/sigiro-fed-central.yaml

# 2. Get k3s token
TOKEN=$(limactl shell fed-central sudo cat /var/lib/rancher/k3s/server/node-token)
IP=$(limactl shell fed-central -- ip -4 addr show eth0 | grep 'inet ' | awk '{print $2}' | cut -d/ -f1)

# 3. Generate per-instance collector YAMLs (Lima does not support --env)
sed -e "s|\\\${K3S_SERVER_IP}|$IP|g" -e "s|\\\${K3S_AGENT_TOKEN}|$TOKEN|g" dev/lima/sigiro-fed-collector.yaml > /tmp/collector-1.yaml
sed -e "s|\\\${K3S_SERVER_IP}|$IP|g" -e "s|\\\${K3S_AGENT_TOKEN}|$TOKEN|g" dev/lima/sigiro-fed-collector.yaml > /tmp/collector-2.yaml

# 4. Start collector VMs
limactl start --name=fed-collector-1 /tmp/collector-1.yaml
limactl start --name=fed-collector-2 /tmp/collector-2.yaml
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

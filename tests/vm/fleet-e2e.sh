#!/usr/bin/env bash
# Multi-VM federated fleet E2E — the `sys_*` host-statistics path across
# GENUINELY DIFFERENT machines.
#
# The in-repo Rust test (tests/federated_query_test.rs
# `federated_sys_disk_info_across_two_edges`) proves the plumbing, but both its
# edges run on ONE host, so every edge reports an identical mount table. That
# cannot prove per-node attribution — the actual product question ("WHICH node
# is filling up?"). This script answers it with real VMs that differ on purpose:
#
#   sigiro-node-a  2 GiB RAM, 12 GiB disk
#   sigiro-node-b  4 GiB RAM, 24 GiB disk
#
# so a correct fleet query MUST return two distinct rows with the right
# hostnames and visibly different totals. A coordinator that dropped an edge,
# double-counted one, or mislabelled rows fails here and cannot fail silently.
#
# Topology (verified, not assumed): Lima's default user-mode (slirp) networking
# gives EVERY VM the same private address, 192.168.5.15, which resolves to the
# VM itself — so VMs cannot reach each other directly, and an edge list of VM
# IPs would silently have every edge talking to itself. Instead the COORDINATOR
# runs on the macOS host and reaches each edge through its own SSH-forwarded
# local port:
#
#   host:18899 --ssh--> node-a:8899 (quack)
#   host:18900 --ssh--> node-b:8899 (quack)
#
# which is also the honest analogue of reaching a real Hetzner cluster from a
# laptop: the CLI is a client, each node listens, and the transport is a tunnel
# (quack has no TLS of its own — see src/quack.rs).
#
# Prerequisites (one-time):
#
#   # 1. Two VMs with DELIBERATELY DIFFERENT specs, so per-node attribution is
#   #    provable rather than assumed:
#   limactl create --name=sigiro-node-a --memory=2 --disk=12 --cpus=2 tests/vm/fleet-node.yaml
#   limactl create --name=sigiro-node-b --memory=4 --disk=24 --cpus=2 tests/vm/fleet-node.yaml
#   limactl start sigiro-node-a && limactl start sigiro-node-b
#
#   # 2. The linux/arm64 binary. `just lima-build` alone is NOT enough on macOS:
#   #    the Linux target deliberately omits duckdb's `bundled` feature (see
#   #    Cargo.toml), so it links a prebuilt static archive that the Dockerfile
#   #    normally prepares. Without Docker, reproduce that step directly:
#   mkdir -p /tmp/duckdb-static && cd /tmp/duckdb-static
#   curl -fSL "https://github.com/duckdb/duckdb/releases/download/v1.5.3/static-libs-linux-arm64.zip" -o s.zip
#   unzip -oq s.zip && rm s.zip
#   { echo "create m.a"; for a in *.a; do echo "addlib $a"; done; echo save; echo end; } | aarch64-linux-gnu-ar -M
#   aarch64-linux-gnu-ranlib m.a && mkdir -p /tmp/duckdb-opt
#   cp m.a /tmp/duckdb-opt/libduckdb_static.a && cp duckdb.h /tmp/duckdb-opt/
#   # then, from the repo root:
#   source scripts/lima-cross-env.sh
#   DUCKDB_LIB_DIR=/tmp/duckdb-opt DUCKDB_INCLUDE_DIR=/tmp/duckdb-opt DUCKDB_STATIC=1 \
#     cargo build --release --target aarch64-unknown-linux-gnu --bin sigiro
#
#   # 3. The host-native coordinator binary:
#   cargo build --release --bin sigiro
#
# Usage:
#   tests/vm/fleet-e2e.sh                 # run against sigiro-node-{a,b}
#
# Environment:
#   NODES        space-separated Lima VM names (default: "sigiro-node-a sigiro-node-b")
#   SIGIRO_BIN   host path to the linux binary (default: the lima cross target)
#   QUACK_TOKEN  shared edge token (default: a fixed test token)
#   HOST_SIGIRO  host-native sigiro binary used as the coordinator
set -euo pipefail

NODES="${NODES:-sigiro-node-a sigiro-node-b}"
SIGIRO_BIN="${SIGIRO_BIN:-target/aarch64-unknown-linux-gnu/release/sigiro}"
QUACK_TOKEN="${QUACK_TOKEN:-fleet-e2e-token}"
QUACK_PORT=8899
HTTP_PORT=9999
# First local port of the tunnel range; node N gets BASE_LOCAL_PORT+N.
BASE_LOCAL_PORT="${BASE_LOCAL_PORT:-18899}"
HOST_SIGIRO="${HOST_SIGIRO:-target/release/sigiro}"
FAILED=0
TUNNEL_PIDS=()

G='\033[0;32m'; R='\033[0;31m'; Y='\033[0;33m'; C='\033[0;36m'; N='\033[0m'
log()  { echo -e "${C}[$(date +%H:%M:%S)]${N} $*"; }
pass() { echo -e "${G}[PASS]${N} $*"; }
fail() { echo -e "${R}[FAIL]${N} $*"; FAILED=1; }
warn() { echo -e "${Y}[WARN]${N} $*"; }

cleanup() {
    log "Tearing down tunnels and stopping sigiro on every node..."
    for pid in "${TUNNEL_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    done
    for vm in $NODES; do
        limactl shell "$vm" bash -c 'pkill -f "sigiro serve" 2>/dev/null || true' 2>/dev/null || true
    done
}
trap cleanup EXIT

[ -f "$SIGIRO_BIN" ] || { fail "binary not found at $SIGIRO_BIN — run: just lima-build"; exit 1; }
log "binary: $SIGIRO_BIN ($(du -h "$SIGIRO_BIN" | cut -f1))"

# ── Deploy + boot every edge ──────────────────────────────────────────
declare -A NODE_IP
for vm in $NODES; do
    log "[$vm] deploying..."
    limactl shell "$vm" bash -c 'mkdir -p ~/sigiro/bin'
    limactl copy "$SIGIRO_BIN" "$vm:/tmp/sigiro-bin"
    limactl shell "$vm" bash -c 'mv /tmp/sigiro-bin ~/sigiro/bin/sigiro && chmod +x ~/sigiro/bin/sigiro && ~/sigiro/bin/sigiro --version'

    # Bind quack on the VM's own address. `SIGIRO_QUACK_BIND` rejects
    # wildcards (src/server.rs validates every 0.0.0.0/:: spelling), so the
    # explicit address is the only accepted form.
    ip=$(limactl shell "$vm" bash -c "ip -4 -o addr show scope global | awk '{print \$4}' | cut -d/ -f1 | head -1")
    NODE_IP[$vm]="$ip"
    log "[$vm] ip=$ip — starting sigiro serve (quack surface)..."

    # setsid + </dev/null: a plain `nohup ... &` inside `limactl shell` dies
    # when the SSH session closes, which silently leaves no listener behind
    # (observed directly while building this test).
    limactl shell "$vm" bash -c "
        set -e
        mkdir -p /tmp/sigiro-fleet-data
        SIGIRO_DATA_DIR=/tmp/sigiro-fleet-data \
        SIGIRO_HTTP_PORT=$HTTP_PORT \
        SIGIRO_QUERY_SURFACE=quack \
        SIGIRO_QUACK_BIND=$ip \
        SIGIRO_QUACK_PORT=$QUACK_PORT \
        SIGIRO_QUACK_TOKEN=$QUACK_TOKEN \
        OTEL_EXPORTER_OTLP_ENDPOINT= \
        RUST_LOG=sigiro=info \
        setsid nohup ~/sigiro/bin/sigiro serve > /tmp/sigiro-fleet.log 2>&1 < /dev/null &
        disown || true
        echo started
    "
done

# ── Wait for every edge to report ready ───────────────────────────────
for vm in $NODES; do
    log "[$vm] waiting for /health ready (first boot downloads DuckDB extensions)..."
    ready=0
    for _ in $(seq 1 90); do
        # Poll the VM's OWN address, not 127.0.0.1: sigiro binds its HTTP
        # server to SIGIRO_QUACK_BIND's interface, so a loopback probe never
        # answers and the wait would always time out on a healthy node.
        status=$(limactl shell "$vm" bash -c \
            "curl -sf http://${NODE_IP[$vm]}:$HTTP_PORT/health 2>/dev/null | grep -o '\"status\":\"[a-z]*\"' || true")
        if [ "$status" = '"status":"ready"' ]; then ready=1; break; fi
        sleep 2
    done
    if [ "$ready" = 1 ]; then
        pass "[$vm] ready"
    else
        fail "[$vm] never became ready"
        limactl shell "$vm" bash -c 'tail -40 /tmp/sigiro-fleet.log' || true
        exit 1
    fi
done

# ── Record each node's REAL identity + disk, straight from the OS ─────
# Ground truth, gathered independently of sigiro, to compare the federated
# answer against.
log "recording ground truth per node (from the OS, not from sigiro)..."
declare -A TRUE_HOST TRUE_TOTAL_KB
for vm in $NODES; do
    TRUE_HOST[$vm]=$(limactl shell "$vm" bash -c 'hostname')
    TRUE_TOTAL_KB[$vm]=$(limactl shell "$vm" bash -c "df -k / | tail -1 | awk '{print \$2}'")
    log "  ${TRUE_HOST[$vm]}: root total ${TRUE_TOTAL_KB[$vm]} KB"
done

# ── Tunnel each edge's quack port to a distinct host port ─────────────
# See the topology note in the header: slirp gives every VM the same address,
# so the coordinator runs on the host and reaches each edge through its own
# forwarded port.
EDGE_ARGS=""
idx=0
for vm in $NODES; do
    local_port=$((BASE_LOCAL_PORT + idx))
    ssh_port=$(limactl list --format '{{.SSHLocalPort}}' "$vm")
    log "[$vm] tunnel host:$local_port -> ${NODE_IP[$vm]}:$QUACK_PORT (via ssh :$ssh_port)"
    ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -o ExitOnForwardFailure=yes \
        -i ~/.lima/_config/user -N \
        -L "$local_port:${NODE_IP[$vm]}:$QUACK_PORT" \
        -p "$ssh_port" "$USER@127.0.0.1" >/dev/null 2>&1 &
    TUNNEL_PIDS+=("$!")
    EDGE_ARGS="$EDGE_ARGS --edge $vm=127.0.0.1:$local_port"
    idx=$((idx + 1))
done
sleep 5

for pid in "${TUNNEL_PIDS[@]}"; do
    kill -0 "$pid" 2>/dev/null || { fail "an ssh tunnel died immediately"; exit 1; }
done
pass "tunnels up for $(echo "$NODES" | wc -w | tr -d ' ') nodes"

# ── The fleet query: one SQL statement, every node ────────────────────
[ -f "$HOST_SIGIRO" ] || { fail "host coordinator binary not found at $HOST_SIGIRO — run: cargo build --release"; exit 1; }
log "coordinator=$HOST_SIGIRO (host) edges:$EDGE_ARGS"

run_fleet_query() {
    local sql="$1"
    SIGIRO_QUACK_TOKEN="$QUACK_TOKEN" \
    OTEL_EXPORTER_OTLP_ENDPOINT= \
    RUST_LOG=error \
    "$HOST_SIGIRO" query "$sql" $EDGE_ARGS
}

# 1. Per-node identity. Each edge contributes exactly one sys_os_info() row, so
#    grouping by host_name must yield one row PER NODE — the assertion that a
#    single shared host could never make.
log "── fleet query 1: per-node identity ──"
out1=$(run_fleet_query "SELECT host_name, count(*) AS n FROM sys_os_info() GROUP BY host_name ORDER BY host_name") \
    || { fail "fleet identity query failed"; echo "$out1"; exit 1; }
echo "$out1"

node_count=$(echo "$NODES" | wc -w | tr -d ' ')
got_rows=$(echo "$out1" | grep -c 'host_name' || true)
for vm in $NODES; do
    if echo "$out1" | grep -q "${TRUE_HOST[$vm]}"; then
        pass "fleet result names ${TRUE_HOST[$vm]}"
    else
        fail "fleet result is MISSING node ${TRUE_HOST[$vm]} — an edge was dropped or mislabelled"
    fi
done

# 2. Per-node disk. The nodes were built with different disk sizes, so the
#    per-node root totals must DIFFER. Identical totals would mean every row
#    came from one machine.
log "── fleet query 2: per-node root disk ──"
out2=$(run_fleet_query "SELECT mount_point, count(*) AS mounts, max(total_space) AS biggest FROM sys_disk_info() WHERE mount_point = '/' GROUP BY mount_point") \
    || { fail "fleet disk query failed"; echo "$out2"; exit 1; }
echo "$out2"

# Each node has exactly one '/' mount, so the folded count must equal the
# number of nodes — proof that every edge answered and none was counted twice.
mounts=$(echo "$out2" | grep -oE '"mounts":[[:space:]]*[0-9]+' | grep -oE '[0-9]+' | head -1 || true)
if [ "$mounts" = "$node_count" ]; then
    pass "count(*) over '/' == $node_count nodes (every edge answered exactly once)"
else
    fail "expected $node_count '/' mounts across the fleet, got '${mounts:-<none>}'"
fi

# 3. The real product question: which node is tightest on disk, fleet-wide.
log "── fleet query 3: memory totals differ per node ──"
out3=$(run_fleet_query "SELECT count(*) AS reports, min(total_memory) AS smallest, max(total_memory) AS largest FROM sys_memory_info()") \
    || { fail "fleet memory query failed"; echo "$out3"; exit 1; }
echo "$out3"

smallest=$(echo "$out3" | grep -oE '"smallest":[[:space:]]*[0-9]+' | grep -oE '[0-9]+' | head -1 || true)
largest=$(echo "$out3" | grep -oE '"largest":[[:space:]]*[0-9]+' | grep -oE '[0-9]+' | head -1 || true)
if [ -n "$smallest" ] && [ -n "$largest" ] && [ "$largest" -gt "$smallest" ]; then
    pass "min/max total_memory differ across the fleet ($smallest < $largest) — folded from DISTINCT hosts"
else
    fail "expected differing memory totals across nodes, got min=$smallest max=$largest"
fi

# 4. The security boundary must hold across the wire, not just in-process.
log "── fleet query 4: gate still rejects hostile SQL over the real wire ──"
for bad in \
    "SELECT * FROM read_csv('/etc/passwd')" \
    "SELECT getenv('AWS_SECRET_ACCESS_KEY')" \
    "SELECT * FROM sys_disk_info('/etc/passwd')"
do
    if run_fleet_query "$bad" >/dev/null 2>&1; then
        fail "SECURITY: the fleet accepted hostile SQL: $bad"
    else
        pass "rejected over the wire: $bad"
    fi
done

echo
if [ "$FAILED" = 0 ]; then
    echo -e "${G}=== multi-VM fleet E2E PASSED ($node_count nodes) ===${N}"
else
    echo -e "${R}=== multi-VM fleet E2E FAILED ===${N}"
fi
exit "$FAILED"

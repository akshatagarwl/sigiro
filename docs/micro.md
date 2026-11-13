# Microcontroller ingress (experimental)

Sigiro's **existing host binary** can now receive MCU batches through MQTT 5.
There is no separate gateway daemon. The broker remains external. MCU firmware
is a different, target-specific executable; it does not link DuckDB, Arrow or the
host runtime.

This is a tested integration milestone, **not a production-certified MCU SDK**.
The physical-board release gates below still apply.

## Run

```sh
sigiro serve --micro-config /etc/sigiro/micro.json
sigiro micro status --data-dir /var/lib/sigiro
```

Without `--micro-config`, ingress is disabled and existing OTLP behavior is
unchanged. This first implementation is **self-hosted only**: JWT/hosted mode is
refused rather than bypassing tenant authentication. Existing self-hosted HTTP
listeners are open; protect them with the normal Sigiro network perimeter.

Example `micro.json`:

```json
{
  "broker": {
    "host": "mqtt.example.com",
    "port": 8883,
    "client_id": "sigiro-edge-1",
    "tls": true,
    "username": "sigiro-server",
    "password_file": "mqtt-password",
    "ca_file": "broker-ca.pem"
  },
  "namespace": "sigiro/micro/v1",
  "devices": [{ "id": "sensor-001", "project_id": "default" }],
  "max_batch_bytes": 4096,
  "max_inbox_bytes": 104857600,
  "max_batches": 100000
}
```

Paths are relative to the config file. Omit `ca_file` to use platform trust
anchors. TLS validates both chain and broker hostname, with an explicit aws-lc
provider; verification is never disabled. Password files must be private
(`chmod 600` on Unix); trailing CR/LF characters are stripped. Credentials are
required outside explicitly enabled loopback tests. Choose a unique server
client ID, so another server does not disconnect this subscription.

Provision broker authentication and **deny-by-default ACLs**. For example:

```text
user sigiro-server
topic read sigiro/micro/v1/+/logs
topic read sigiro/micro/v1/+/traces
topic read sigiro/micro/v1/+/metrics
topic write sigiro/micro/v1/+/ack

user sensor-001
topic write sigiro/micro/v1/sensor-001/logs
topic write sigiro/micro/v1/sensor-001/traces
topic write sigiro/micro/v1/sensor-001/metrics
topic read sigiro/micro/v1/sensor-001/ack
```

Use distinct, high-entropy credentials per device and for the server; never
share the server credential with devices. Never allow devices to publish receipts
or another device's ingress. The broker
is a trusted security boundary, not an untrusted relay: the current receipt has
no independent signature. An authenticated broker plus verified TLS and correct
ACLs are required on **both device and host**. Compromising the broker can forge
acceptance. A client-supplied topic is not proof of device identity by itself.

For tests only, `allow_insecure_loopback: true` permits plaintext to a **literal
loopback IP** with `broker.tls: false`. It does not permit remote plaintext.
Config is bounded to 64 KiB, at most 32 provisioned devices, and at most 1 MiB per
batch. Project IDs use the existing Sigiro tenant identifier rules.

## Receipt and storage contract

```text
original OTLP bytes
    -> MCU persistent outbox
    -> MQTT broker (PUBACK is NOT recording acceptance)
    -> sigiro: protobuf admission + provisioned device/project binding
    -> SQLite transaction: original bytes + identity + SHA-256 + pending state
    -> WAL synchronous=FULL commit
    -> matching application receipt (durably recorded)
    -> MCU may pop

pending raw row -> normal Sigiro transform -> committed writer flush
                -> indexed state -> existing SQL/dashboard/MCP projections
```

The application receipt promises **durable raw recording**, not query visibility
or complete semantic support for every future OTLP field. Empty requests are
valid. Malformed protobuf is rejected before insertion. Well-formed input rejected
by the normal semantic transforms is durably quarantined with a bounded diagnostic. Unknown
protobuf fields remain intact in raw storage even if a projection ignores them.
A partial log transformation quarantines the complete original batch rather than
silently presenting its valid subset as complete.

The raw inbox is `<data-dir>/.micro/inbox.sqlite`. Its hidden directory is not a
project. `(device, batch_id)` identifies a batch across all three signals:

- Same ID, project, signal and bytes: repeat the committed receipt; do not enqueue
  another projection.
- Same ID but different project, signal or bytes: permanent conflict, no success.
- New ID over logical byte/count quota: retry later, never overwrite old raw data.
- Disk-capacity gate tripped: retry later, including duplicate requests until the
  global disk condition recovers.
- SQLite commit/read error or detected raw checksum mismatch: fail the service
  closed; do not fabricate a success receipt.

SHA-256 detects changed payloads when pending records or duplicates are read; it
is not a receipt signature or a continuous archive scrubber. SQLite durability
assumes the OS/filesystem/device honor synchronization. Host process-kill tests
are not a physical power-loss certification.

**Projection is at-least-once.** DuckLake commit and SQLite `indexed` status are
not one distributed transaction. A crash/failure between them, or a partially
successful multi-table metric projection, can repeat query rows on recovery.
This does not duplicate the authoritative raw record or permit false MCU
acceptance. Do not claim universal exactly-once delivery/indexing. In particular,
ordinary HTTP/gRPC 200/OK responses have not been changed into this raw receipt
contract; do not treat those existing in-memory enqueue acknowledgements as the
new durable boundary.

## Experimental MQTT profile

This is a Sigiro mapping, not a standardized OTLP transport:

- Topic: `<namespace>/<provisioned-device>/{logs,traces,metrics}`.
- Payload: **original** OTLP `Export*ServiceRequest` protobuf, not a re-encoded
  projection or local outbox envelope.
- MQTT 5 QoS 1/2, non-retained, `ContentType=application/x-protobuf`.
- `CorrelationData`: stable 16-byte batch ID, never regenerated on retry.
- `ResponseTopic`: exactly `<namespace>/<same-device>/ack`; arbitrary reflection
  targets and retained ingress are rejected without application success.
- Receipt: non-retained QoS 1, content type
  `application/vnd.sigiro.micro-receipt.v1`, matching correlation data.
- Receipt bytes: `SAK1`, 16-byte ID, signal byte (logs=1/traces=2/metrics=3),
  outcome byte, two reserved zero bytes. Outcome 1 means full raw recording
  acceptance; 3 means retry later; 5 means permanent rejection/conflict.
- Device must verify sender/topic/correlation/signal and authenticate the receipt
  before calling `Outbox::apply_ack`. Only matching outcome 1 removes data.

Host queues, packet size, receive window and payload size are bounded. Receipt
queue saturation drops the application receipt, not the record: retry deduplicates
it. Transient broker disconnects use capped exponential backoff with jitter;
TLS/authentication/subscription refusal fails closed. Projection retries are
persistently scheduled with a 60-second cap so one temporarily failing project
does not block every other device. Invalid projections are quarantined.

## Operations and retention

`sigiro micro status --data-dir DIR` is read-only and reports recording/projection
counts by device. It does not report device connectivity. Quarantine diagnostics
are available in the SQLite `batches.diagnostic` column; raw payloads are BLOBs.
Protect this database as sensitive telemetry. Use SQLite's online backup API or
`.backup`, or stop Sigiro before a file-level backup; copying only a live main
SQLite file can omit its WAL.

**Raw retention is independent of `SIGIRO_RETENTION_DAYS`.** That setting still
controls query projections. Raw data and dedup identities are currently retained
indefinitely up to the explicitly configured byte/count quotas, after which new
batches receive retry. Indexed/quarantined rows still count. There is deliberately
no automatic deletion/receipt expiry yet: deleting a receipt can make an old
redelivery appear new. Increase quotas only with disk headroom and a backup plan.
Logical quotas exclude SQLite metadata/WAL; set `SIGIRO_MAX_DISK_BYTES` with
headroom and monitor disk usage. Inbox I/O failure terminates `serve`; supervise
it and alert on failures. Broker outage does not stop raw projection or existing
HTTP queries; `/health` by itself is not proof of MQTT connectivity.

## Reproducible checks

```sh
scripts/run-with-duckdb-static.sh cargo build --locked --bin sigiro
scripts/run-with-duckdb-static.sh cargo test --locked --lib
rustup target add --toolchain stable thumbv7m-none-eabi
(cd firmware/qemu-recorder && rustup run stable cargo build --locked --release --target thumbv7m-none-eabi)
uv run --locked tests/micro/simulate.py --no-network
uv run --locked tests/micro/native.py --binary target/debug/sigiro
```

Install QEMU, Docker and OpenSSL first. `SIGIRO_EXTENSION_DIR` may explicitly point
the native harness at a preinstalled cache for this binary's DuckDB version.
Otherwise the ordinary server extension installation runs on first start.
Results live in ignored `artifacts/` directories. The native suite uses a private
throwaway CA, authenticated/ACL-restricted real Mosquitto, actual `sigiro`, SQL
queries, SIGKILL/restart, a real broker outage, quotas, quarantine and exact raw
comparisons. It also models the projection commit/status gap and demonstrates
its duplicate-row limitation instead of hiding it.

`simulate.py --no-network` is the NOR fault-budget suite. Without that flag it
also runs the original **reference Collector gateway lab**, not the native Sigiro
integration. In both suites, actual Cortex-M instructions execute in QEMU on
`mps2-an385`; a semihosting file models NOR and a host adapter supplies networking.
No physical flash controller, radio, MCU TCP or MCU TLS is being emulated.

## Licensing and remaining release gates

`crates/otlp-outbox`, `firmware/qemu-recorder` and the simulator harness are
MIT OR Apache-2.0, imported from `akshatagarwl/sigiro-micro` commit `d631614`.
The existing host integration remains under Sigiro's FSL license. The permissive
crate explicitly overrides workspace license inheritance and has no desktop
runtime dependencies. Its `publish=false` remains until API/release review.

Before calling an MCU release production-ready: select one supported board and
NOR driver; implement actual verified-TLS networking, persistent unique IDs,
clock/trust provisioning, bounded device retries and a durable terminal-rejection
policy; measure stack/RAM; run real interruption/endurance tests. Host release
work still includes hosted tenant admission, signed receipts if the broker must
be untrusted, an explicit dedup-aware retention/replay policy, operational metrics
and either transactional/idempotent projection or a documented acceptance of its
at-least-once query behavior.

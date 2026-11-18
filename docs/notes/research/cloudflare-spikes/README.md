# Cloudflare-native spikes

Throwaway probes for the open questions in [the spec](../../designs/cloudflare-native-sigiro.md). Each one was run on 2026-10-10 under `wrangler dev` (local workerd, wrangler 4) and `celld dev` (celld 0.5.1), then deployed to a Workers Paid account and torn down the same day. The client was in Taiwan and its requests ran in the San Jose colo. See [Measured on Cloudflare](#measured-on-cloudflare).

## Ingest durability and cost

`ingest-model/IngestCell.tla` models one ingest cell. It covers exporter retries, acknowledgement, segment writes, manifest commits, compaction, orphan sweeping, and eviction at any step. TLC 1.8.0 checks two invariants. `NoAckedLoss` requires every acknowledged request to be in a live segment, or in the write-ahead rows in `wal` mode. `ManifestObjectsExist` requires every manifest entry to point at a stored object. Two requests and four object IDs were checked exhaustively.

| Acknowledge after | Orphan guard | Result |
| --- | --- | --- |
| segment committed (`hold`) | yes | no error, 50,549 states |
| one SQLite write-ahead row (`wal`) | yes | no error, 32,753 states |
| in-memory buffer (`fast`) | yes | `NoAckedLoss` violated: Receive, Ack, then eviction |
| `hold` or `wal` | no | `ManifestObjectsExist` violated: Receive, Receive, WriteSegment, SweepOrphan, CommitSegment |

Rerun with `java -cp tla2tools.jar tlc2.TLC -deadlock -config hold-TRUE.cfg IngestCell.tla`.

Two rules follow. Never acknowledge from memory alone. Record a segment key as pending in cell storage before writing the object, and let the sweep delete only objects that are neither live nor pending. OTLP requests carry no idempotency key, so a retry after a lost acknowledgement can duplicate rows in every design. Exactly-once delivery is not achievable without a client-supplied key.

`ingest-model/cost.py` prices each safe design per ingest cell per month at Cloudflare list prices (fetched 2026-10-10). Included allowances are ignored.

| req/s | hold, 5 s window | wal, 60 s alarm flush | Queue | object per request |
| --- | --- | --- | --- | --- |
| 0.1 | 3.62 | 0.88 | 0.63 | 1.56 |
| 1 | 8.16 | 6.62 | 4.13 | 15.63 |
| 10 | 18.66 | 63.96 | 39.12 | 156.30 |
| 100 | 123.64 | 637.36 | 389.04 | 1546.39 |

Below about 2 req/s, per-cell duration and R2 writes dominate, so `wal` or a Queue is cheapest. Above about 2 req/s, per-request charges dominate. Each write-ahead row costs $1 per million, and deleting it after a flush costs another $1 per million. `hold` then wins because it pays only per flush. These figures are assumptions until measured: R2 write latency from a cell, wall-clock billing while a cell holds requests, and rows billed for write-ahead deletes.

## DuckDB-wasm query Worker

`duckdb-wasm/` runs `@ducklings/workers` 1.5.5. A `fetch` shim serves `https://segments.invalid/<key>` from the R2 binding, honoring HEAD and byte ranges, so DuckDB's httpfs reads R2 objects with no S3 credentials. The test object is a 46 MB, 2-million-row Parquet file.

- The bundle is 10,219 KiB gzip, against Cloudflare's 10 MiB paid limit, which leaves about 21 KiB. Nothing else fits in this Worker.
- On both runtimes, a filtered aggregate over 2 of 20 services returns the same result as the DuckDB CLI. It reads 5.8 MB in 41 ranged GETs, and a repeat runs from DuckDB's cache with no GETs.
- `count(DISTINCT trace_id)` over all 2 million rows succeeds and reads 36.7 MB.
- DuckDB defaults to a 102.3 MiB memory limit and one thread. A `string_agg` over the whole file fails with `Out of Memory Error: Allocation failure`, and the next query on the same isolate succeeds.

## Codemode in Dynamic Workers

`codemode/` loads each script into a Dynamic Worker with `globalOutbound: null` and `limits.cpuMs: 200`. The only binding is a loopback `Tools` entrypoint whose `ctx.props` carry the session and tenant. Declarations come from `@earendil-works/pi-codemode`'s `renderDeclarations`. `run.sh` drives the probes.

| Probe | wrangler dev | celld dev |
| --- | --- | --- |
| Two tool calls composed; props reach the host | pass | pass |
| `fetch()` from the script | refused | refused |
| Unknown tool | typed error | typed error |
| `while (true) {}` | hangs: local workerd does not enforce `cpuMs` | `Worker exceeded CPU limit of 200 ms` |
| Next script after the loop | not reached | pass |

The CPU limit must be checked again on Cloudflare.

## Measured on Cloudflare

The Workers ran in the San Jose colo (`colo=SJC`). The first R2 bucket defaulted to the APAC location hint. A second bucket used the WNAM hint, beside the compute.

**Deployment.** Cloudflare accepted the DuckDB-wasm Worker at 10,219 KiB gzip. The codemode Worker was 4.5 KiB gzip.

**DuckDB-wasm queries.** The results matched local runs.

| Query (2M rows, 46 MB file) | APAC bucket | WNAM bucket |
| --- | --- | --- |
| Filtered aggregate, uncached (41 ranged GETs, 5.8 MB) | 8.5 to 20.2 s | 7.1 s |
| Same query, DuckDB cache warm | 0.13 s | 0.13 s |
| `count(DISTINCT trace_id)` (21 GETs, 36.8 MB) | 13.0 s | not run |
| `string_agg` over the file | out of memory at 71.1 of 102.3 MiB; the next query succeeded | not run |

One 128 KiB R2 `get` with a range takes 129 to 200 ms sequentially and stays near 150 ms in the same region. Ten in parallel take 340 to 358 ms in total. DuckDB-wasm issues its reads one at a time, so uncached latency is roughly 150 ms times the GET count. **A query Worker must fetch each segment's needed byte ranges in parallel before DuckDB runs, or read whole small segments at once.** Per-request byte throughput is not the limit.

**Codemode in Dynamic Workers.** Every probe matched celld. Composed calls and props pass, `fetch()` is refused, and an unknown tool returns its error. `while (true) {}` fails with `Worker exceeded CPU time limit.` after 2.8 s of wall time, and the next script succeeds. The first requests after a deploy briefly returned `error code: 1042`, and later runs did not.

**Ingest cell storage and R2 writes** (`ingest-cloudflare/`). Row counts come from the SQLite cursor's `rowsWritten`.

| Operation | Rows written |
| --- | --- |
| Insert a 32 or 128 KiB write-ahead blob | 1 |
| Delete that blob after a flush | 1 |
| Register a pending segment key in a table with a primary key | 2 (table and index) |
| Mark the segment live | 1 |

`storage.sync()` after the writes took 31 to 34 ms. An R2 `put` from the cell took the following:

| Object size | APAC bucket | WNAM bucket |
| --- | --- | --- |
| 1 MiB | 754 to 1,701 ms | 310 to 453 ms |
| 4 MiB | 785 to 1,005 ms | 281 to 444 ms |
| 16 MiB | 1,467 to 2,764 ms | 551 to 977 ms |

Duration billing while a cell holds requests open was not measured. It needs Cloudflare's analytics after sustained load.

## Ed25519 JWT verification on celld

celld 0.5.1's WebCrypto rejects Ed25519 verification in every form tried. JWK import with `Ed25519` succeeds, but `verify` throws `NotSupportedError: unsupported verify algorithm: ED25519`. JWK import with `NODE-ED25519` and `raw` import with `Ed25519` both throw `unsupported key import`. Its compatibility page says Ed25519 verifies, so this looks like a celld defect. The ingest Worker therefore tries WebCrypto first and falls back to `node:crypto`'s `verify`, which needs the `nodejs_compat` flag. Recheck on each celld release and delete the fallback once WebCrypto works.

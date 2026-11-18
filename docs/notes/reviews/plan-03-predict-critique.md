---
title: Plan 03 Architecture Critique — Multi-Persona Prediction
created: 2026-03-21
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'competitor', 'data-model']
---

# Plan 03 Architecture Critique — Multi-Persona Prediction

**Date:** 2026-03-21
**Scope:** docs/plans/03-s3-cold-tier-and-[[architecture/distributed-query.md|distributed-query]].md, docs/architecture/distributed-query.md, src/[[architecture/investigation.md|investigate]].rs, src/server.rs, src/inserter.rs
**Method:** 6-persona swarm, 2 rounds of cross-examination
**Total findings:** 34 (8 critical, 12 high, 10 medium, 4 low)

---

## Persona 1: Distributed Systems Engineer

### Finding D1 [CRITICAL]: Sketch merge requires DuckDB SQL, not Rust — architectural mismatch

The [[company/plan.md|plan]] says the coordinator merges sketches via `datasketch_tdigest_merge()` and `datasketch_hll_merge()`. These are **[[architecture/data-model.md|DuckDB]] SQL aggregate functions**, not Rust library calls. The coordinator must:

1. Receive BLOB sketches over HTTP from peers
2. INSERT BLOBs into a [[architecture/data-model.md|DuckDB]] temp table
3. Run `SELECT datasketch_tdigest_merge(sketch_blob) FROM temp_sketches WHERE op = ?`
4. Read the merged result back
5. Repeat per operation, per sketch type

This is ~5 [[architecture/data-model.md|DuckDB]] roundtrips per operation per merge, all going through the same `Arc<Mutex<Connection>>` that serves ingest and queries. With 20 operations across 3 peers, that's ~100 mutex acquisitions during a single /[[architecture/investigation.md|investigate]] call.

**The [[company/plan.md|plan]]'s pseudocode hides this.** The Rust merge loop at AR-4 Phase 2 looks clean but glosses over how `datasketch_tdigest_merge()` actually gets called.

**Alternative:** Use the `tdigest` Rust crate directly (crate exists, ~500 stars). Serialize/deserialize in Rust without touching [[architecture/data-model.md|DuckDB]]. But then you have two t-digest implementations (DuckDB's for single-node, Rust's for distributed) that may produce slightly different results.

### Finding D2 [HIGH]: No distributed coverage/truncation metadata

Current `InvestigationBlock` has `Truncation` (spans_total, spans_returned) and `Coverage` (has_spans, has_logs, has_metrics). In distributed mode:

- `spans_total` = sum across peers? But each peer may have truncated its own top-50.
- `coverage.has_spans = true` if ANY peer has spans, even if 2/3 peers are down.
- The AI agent has NO way to know "this [[architecture/investigation.md|investigation]] is based on 1/3 of cluster data."

**Fix needed:** Add `nodes_queried`, `nodes_responded`, `nodes_total` to the response.

### Finding D3 [HIGH]: Clock skew between nodes

`from_ts` and `to_ts` are microsecond-precision UTC timestamps. If Node 1's clock is 2 seconds ahead of Node 2, the same `investigate(service, last_15m)` query:
- Node 1: window [T-15m, T] (Node 1's T)
- Node 2: window [T-15m, T] but Node 2's T is 2s behind — misses 2s of data from Node 1's perspective

For 15-minute windows this is ~0.2% data loss. For 1-minute windows (metric buckets), it's 3.3%. Not catastrophic but undocumented.

### Finding D4 [MEDIUM]: Static peer discovery via env var

`SIGIRO_PEERS=node2:9999,node3:9999` requires restart to add/remove peers. In Kubernetes, pods come and go. Either:
- Use a headless Service + DNS SRV lookup (standard K8s pattern)
- Or accept that peers are stable (DaemonSet, one per node) and document this constraint

### Finding D5 [MEDIUM]: No quorum or consistency model documented

"Peer unreachable: warn, return partial results" — but how partial is acceptable?
- 1/10 peers down: probably fine
- 5/10 peers down: [[architecture/investigation.md|investigation]] is based on half the data, conclusions are unreliable
- Should there be a minimum peer response threshold before returning results?

### Finding D6 [MEDIUM]: Split-brain on cold tier S3 writes

Multiple nodes write cold [[architecture/data-model.md|Parquet]] to shared S3 prefix. Two nodes exporting the same service's data for the same day create separate files. The cross-tier view `read_parquet('s3://.../**/*.parquet')` reads both — duplicating data if compaction hasn't run. No dedup mechanism documented.

---

## Persona 2: DuckDB Internals Expert

### Finding DB1 [CRITICAL]: INSTALL downloads from internet at runtime

`INSTALL datasketches` fetches from `extensions.duckdb.org` at process startup. This:
- Adds 2-10 seconds of startup latency (DNS + download + verify)
- Fails in air-gapped environments ([[roadmap.md|Docker]] without internet, corporate firewalls)
- Fails if the extension repository is down (has happened: github.com/duckdb/duckdb/issues/10234)
- No version pinning — you get whatever version is current. A breaking change in datasketches silently breaks production.

**Fix:** Bundle extensions in the [[roadmap.md|Docker]] image. `INSTALL datasketches FROM '/path/to/extensions/'` with local files. Or use `INSTALL datasketches FROM 'https://specific-version-url'` if supported.

### Finding DB2 [CRITICAL]: READ_PARQUET is in the blocklist — cross-tier views break /query

`server.rs:47` blocks `READ_PARQUET`. But `AR-3` creates views like:
```sql
CREATE VIEW sigiro_spans_all AS
    SELECT * FROM sigiro_spans
    UNION ALL
    SELECT * FROM read_parquet('s3://...', hive_partitioning=true);
```

Querying through the VIEW is fine (the user doesn't write `read_parquet`). But:
- `POST /query "SELECT * FROM sigiro_spans_all"` — works, because the blocklist checks the user's SQL, not the expanded view definition
- `POST /query "SELECT * FROM read_parquet('s3://attacker/evil.parquet')"` — correctly blocked

This is actually OK. But worth documenting explicitly that the blocklist applies to user SQL only, not view internals.

### Finding DB3 [HIGH]: datasketches BLOB serialization over HTTP

[[architecture/data-model.md|DuckDB]]'s `datasketch_tdigest()` returns a BLOB. To send this from peer to coordinator:
1. Peer must read the BLOB from [[architecture/data-model.md|DuckDB]] result
2. Serialize to JSON (base64 encode? hex encode?)
3. Coordinator must decode back to bytes
4. INSERT into temp table as BLOB
5. Run merge SQL

duckdb-rs returns BLOBs as `Vec<u8>` via `row.get::<_, Vec<u8>>()`. This works. But the HTTP serialization format isn't specified in the [[company/plan.md|plan]]. serde_json doesn't handle `Vec<u8>` as base64 by default — need `serde_bytes` or manual encoding.

### Finding DB4 [HIGH]: httpfs credential refresh for IAM roles

[[architecture/data-model.md|DuckDB]]'s `credential_chain` provider reads AWS credentials at SECRET creation time. If using IRSA (Kubernetes IAM roles), tokens expire every 12 hours. DuckDB does NOT automatically refresh secrets — the secret stores the credentials at creation time.

**Impact:** After 12 hours, all cold tier reads fail with 403 Forbidden. Requires process restart or `DROP SECRET; CREATE SECRET` cycle.

**Fix:** Periodic secret refresh on a timer, or recreate the secret before each cold-tier query.

### Finding DB5 [MEDIUM]: cache_httpfs cache invalidation

`cache_httpfs` caches [[architecture/data-model.md|Parquet]] file reads locally. When the compactor writes new Parquet files to S3, the cache doesn't know about them. [[architecture/data-model.md|DuckDB]] must LIST the S3 prefix to discover new files, but cached file contents are stale only if files are overwritten (they aren't — new files get new names). This is actually fine for append-only cold tier. But after compaction (AR-5), old files are replaced by merged files. The cache still serves the old files until evicted.

### Finding DB6 [LOW]: Extension load order matters

`INSTALL httpfs; LOAD httpfs; INSTALL cache_httpfs; LOAD cache_httpfs;` — cache_httpfs must be loaded AFTER httpfs (it wraps httpfs). This is correct in the [[company/plan.md|plan]] but fragile. If someone reorders the init code, silent failure.

---

## Persona 3: Cost Optimizer

### Finding C1 [HIGH]: Parquet file proliferation — O(tables x hours x days) files

6 tables x 24 hours/day x 365 days = 52,560 files/year per table, 315,360 total.

S3 LIST costs: $0.005 per 1,000 LIST requests. Each `read_parquet('s3://.../**/*.parquet')` query triggers at least 1 LIST per 1,000 objects. With 315K files: 315 LIST requests per query = $0.0016/query.

At 100 queries/day: $0.16/day = $4.80/month just for LIST operations. This exceeds the $2.06/month total [[company/cost.md|cost]] estimate.

**Mitigation:** AR-5 (compaction) is critical. But compaction is listed as AR-5 (lowest priority). Without it, costs grow linearly with time.

### Finding C2 [HIGH]: Small Parquet files are expensive AND slow

Hourly export of a low-traffic service (10 spans/hour) produces ~1KB [[architecture/data-model.md|Parquet]] files. S3 PUT [[company/cost.md|cost]] is the same for 1KB as 1GB ($0.005 per 1,000 PUTs). But the real cost is query latency:

[[architecture/data-model.md|DuckDB]] must open each [[architecture/data-model.md|Parquet]] file, read footer metadata, evaluate predicate pushdown. For 52K files, that's 52K HTTP GETs for metadata alone — even with Hive partition pruning (reduces to ~files-per-day but still O(hours-per-day) = 24 files per day per table per partition).

**Fix:** Buffer exports — don't write hourly, write daily or when buffer reaches 10MB. One daily file per table is 6 files/day, 2,190/year. Manageable.

### Finding C3 [MEDIUM]: OpenDAL layer overhead is measurable but acceptable

OpenDAL adds RetryLayer (3 retries with backoff) + LoggingLayer (per-request log) + potential OtelTraceLayer. Each layer wraps the S3 call. For the compactor (1 PUT/hour/table = 144 PUTs/day), overhead is negligible. For query reads that fan out to many files, the per-request logging may produce excessive log volume.

### Finding C4 [MEDIUM]: The $14.40/mo SaaS price doesn't account for compute

The [[company/cost.md|cost analysis]] in MEMORY.md says "Our cost: $2.06" but this only covers storage (S3 + [[architecture/data-model.md|DuckDB]] disk). It doesn't include:
- Compute: even a small VM (t3.small) is ~$15/month
- Network: S3 egress (free within region, $0.09/GB cross-region)
- The [[architecture/data-model.md|DuckDB]] process needs enough RAM to hold hot data + query working memory

At the stated 44 GB/mo ingest rate with 7-day hot retention: hot data = 44 * 7/30 = ~10 GB. [[architecture/data-model.md|DuckDB]] needs ~2x working memory = 20 GB. That's a t3.xlarge ($120/month) or equivalent.

**Impact:** The $14.40/mo price point is unsustainable. Either the price must increase or the hosted architecture needs multi-tenant [[architecture/data-model.md|DuckDB]] (which doesn't exist).

### Finding C5 [LOW]: R2 free egress doesn't help for compute-side reads

The [[company/plan.md|plan]] notes R2 has free egress. But egress only matters for client downloads. The sigiro process reading from R2/S3 is within the same cloud — this is already free (same-region). R2's advantage is irrelevant for the cold tier read path.

---

## Persona 4: SRE/Operator

### Finding O1 [CRITICAL]: Compactor data loss window — no idempotency

The compactor's 3-step process:
1. Read old data from [[architecture/data-model.md|DuckDB]] (brief lock)
2. Write [[architecture/data-model.md|Parquet]] to S3 via OpenDAL (no lock)
3. DELETE old data from [[architecture/data-model.md|DuckDB]] (brief lock)

If the process crashes between step 2 and step 3:
- Data exists in both hot and cold
- On restart, the compactor runs again: re-exports the same data to S3 (new file, different name)
- Now you have duplicates in cold tier
- Cross-tier views UNION ALL both — double-counting in all queries

If the process crashes between step 1 and step 2:
- No data loss, but the data read in step 1 is in memory and lost
- On restart, re-reads and re-exports — fine, idempotent at this step

If S3 write (step 2) succeeds but returns a network error:
- Compactor thinks it failed, doesn't proceed to step 3
- On retry: duplicate file on S3

**Fix needed:** Write a marker file (e.g., `_exported_until=<timestamp>`) to S3 before DELETE. On restart, check the marker and skip re-export if marker exists. Or use a local WAL/journal for compactor state.

### Finding O2 [CRITICAL]: DELETE FROM in DuckDB doesn't reclaim space

`DELETE FROM sigiro_spans WHERE timestamp < now() - INTERVAL '7 DAY'` marks rows as deleted but doesn't free disk space. [[architecture/data-model.md|DuckDB]] reclaims space only on CHECKPOINT (auto or manual). With continuous ingest, the WAL grows and checkpoints may be delayed.

**Impact:** The "hot" [[architecture/data-model.md|DuckDB]] file grows monotonically even as old data is exported and deleted. After weeks, the file is 10x the hot data size. Disk fills up.

**Fix:** Run `CHECKPOINT` after DELETE. Or better: use `ALTER TABLE DROP PARTITION` if [[architecture/data-model.md|DuckDB]] supports it for the table's partitioning scheme (it doesn't — DuckDB doesn't have user-defined partitions like ClickHouse). So CHECKPOINT is the only option.

### Finding O3 [HIGH]: No compactor observability

The compactor runs as a tokio::time::interval background task. No metrics exposed:
- Rows exported per cycle
- Bytes written to S3
- DELETE duration
- Files created
- Errors/retries
- Last successful run timestamp

If compaction silently fails for a week, hot data grows unbounded. No alert fires because there's no metric to alert on.

### Finding O4 [HIGH]: Extension version drift across nodes

In distributed mode, Node 1 might have datasketches v0.3 and Node 2 has v0.4. If the sketch serialization format changes between versions, `datasketch_tdigest_merge()` on the coordinator may fail or produce wrong results when merging sketches from different versions.

No version negotiation protocol exists. No way to detect version mismatch.

### Finding O5 [MEDIUM]: S3 credential rotation requires process restart

(Cross-reference with DB4.) In production Kubernetes with IRSA, credentials rotate every 12 hours. No automated SECRET refresh means cold-tier reads break every 12 hours unless the operator adds a sidecar or init container to handle rotation.

### Finding O6 [MEDIUM]: No cold tier data integrity verification

After writing [[architecture/data-model.md|Parquet]] to S3, no read-back verification. If the file is silently corrupted (S3 eventual consistency edge case, network corruption past TLS), queries on cold data return wrong results with no error.

**Fix:** After write, read back file header + row count and compare to expected. Or compute and store a checksum.

---

## Persona 5: Product/UX

### Finding P1 [CRITICAL]: Vibe coders will NEVER configure this

The entire [[company/plan.md|Plan]] 03 requires setting 4-8 environment variables (S3 bucket, region, prefix, endpoint, key_id, secret, peers, retention days). The target user ("vibe coders" on Replit/Lovable) can barely add one env var.

The only path where [[company/plan.md|Plan]] 03 features get used:
1. **Hosted mode** — sigiro team configures everything, user sets zero env vars
2. **PaaS templates** — Railway/Render template pre-configures S3

For OSS self-hosted users: [[company/plan.md|Plan]] 03 adds 940 LOC of complexity that ~0% of them will use. Every line is maintenance burden.

**Recommendation:** Build [[company/plan.md|Plan]] 03 ONLY for hosted mode. Don't expose S3/peers config in OSS at all. The single-node local-only mode IS the OSS product.

### Finding P2 [HIGH]: 940 LOC estimate is 2-3x too low

The [[company/plan.md|plan]] estimates ~940 LOC across 6 ARs. Real-world accounting:
- AR-1 (80 LOC): realistic for just init
- AR-2 (200 LOC): missing error handling, retry on S3 failure, idempotency, CHECKPOINT after DELETE, metrics. Real: 400-500 LOC
- AR-3 (100 LOC): realistic for SQL view creation
- AR-4 (400 LOC): HTTP client + serde for PeerSketchResponse + base64 BLOB encoding + temp table management + merge SQL + timeout handling + partial failure handling + new endpoint registration. Real: 800-1200 LOC
- AR-5 (80 LOC): realistic
- AR-6 (80 LOC): realistic

Total realistic: 1,600-2,000 LOC. Plus tests: ~1,000 LOC. Total delta: ~3,000 LOC.

### Finding P3 [RETRACTED]: Distributed query is a real customer requirement

~~Original finding assumed no customer needed distributed yet.~~ **Corrected:** There is a real customer requiring distributed query. The 20-service/44GB scale is the starting point, not the ceiling. AR-4 is a top priority, not premature.

**Recommendation:** Ship AR-4 (distributed) as planned. Focus critique energy on making the sketch merge implementation correct (D1) and operationally solid (O3, O4), not on whether to build it.

### Finding P4 [MEDIUM]: Hosted mode invalidates the architecture

In hosted mode:
- You run [[architecture/data-model.md|DuckDB]], not the user
- You need multi-tenant isolation (per-project [[architecture/data-model.md|DuckDB]] is stated in MEMORY.md)
- You control S3, not the user
- "Peers" are your infrastructure, not user-configured

This means the env-var-driven architecture (SIGIRO_S3_BUCKET, SIGIRO_PEERS) is wrong for hosted. Hosted needs a control plane that provisions per-tenant [[architecture/data-model.md|DuckDB]] instances, manages S3 buckets, and handles peer topology.

[[company/plan.md|Plan]] 03 builds the OSS distributed architecture. Hosted mode needs a different architecture entirely. Building both is 2x the work.

### Finding P5 [LOW]: The pricing gap doesn't survive hosted mode costs

$14.40/mo SaaS price assumes $2.06 [[company/cost.md|cost]]. But hosted mode with per-tenant [[architecture/data-model.md|DuckDB]] instances:
- 1 [[architecture/data-model.md|DuckDB]] per tenant = 1 process per tenant
- Even lightweight, 100 tenants = 100 [[architecture/data-model.md|DuckDB]] processes
- Memory: 100 x 200MB (minimum) = 20GB RAM just for idle instances
- Need process management, health checks, auto-restart

Serverless [[architecture/data-model.md|DuckDB]] (MotherDuck) would solve this but contradicts the "no external deps" philosophy.

---

## Persona 6: Security Researcher (added — blocklist conflict finding)

### Finding S1 [HIGH]: READ_PARQUET blocklist creates false sense of security

`server.rs:47` blocks `READ_PARQUET` to prevent arbitrary S3/file reads via `/query`. But:
- `read_parquet` (lowercase) is also valid [[architecture/data-model.md|DuckDB]] syntax — the blocklist uses `to_uppercase()` so this IS caught
- `READ_JSON` and `READ_CSV` are blocked but `read_ndjson`, `read_blob`, `st_read` (spatial) are not
- `COPY` is blocked but `EXPORT_DATABASE` is not
- The blocklist is a deny-list approach — every new [[architecture/data-model.md|DuckDB]] function that reads external resources must be manually added

**Alternative:** Use [[architecture/data-model.md|DuckDB]]'s `SET disabled_filesystems = 's3,gcs,http'` at connection level — but this would break the cross-tier views too.

The fundamental tension: the query endpoint needs to read S3 (for views) but users shouldn't be able to read arbitrary S3 paths. Views solve this (the user queries the view, [[architecture/data-model.md|DuckDB]] reads S3 internally), but a sophisticated attacker could potentially exploit DuckDB's macro system or UDFs to bypass the keyword blocklist.

### Finding S2 [MEDIUM]: Peer-to-peer HTTP has no authentication

`POST /investigate/sketches` is a new endpoint that returns raw sketch data for any service. If exposed on a network (not just localhost), any client can query any service's data from any peer. No auth token, no mTLS, no API key.

In Kubernetes this is mitigated by network policies, but the [[company/plan.md|plan]] doesn't mention this.

---

## Round 2: Cross-Examination

### Distributed + Product (D1 vs P3)

The Distributed Engineer's finding D1 (sketch merge requires [[architecture/data-model.md|DuckDB]] SQL roundtrips) reveals that the sketch merge is harder than the [[company/plan.md|plan]] suggests. Since distributed is a confirmed customer requirement, the focus should be on making D1's implementation explicit — either Rust-native t-digest or document the DuckDB temp-table dance with accurate LOC. **Consensus: build AR-4 with eyes open on D1 complexity.**

### Cost + SRE (C1 + O1)

The [[company/cost.md|Cost]] Optimizer's C1 (file proliferation) and the SRE's O1 (compactor idempotency) are connected. If compaction fails (O1), file count grows faster (C1), LIST costs increase (C1), and query latency degrades. The compactor is more critical than the [[company/plan.md|plan]]'s AR-5 priority suggests. **Consensus: move compaction to AR-2.5, right after cold export.**

### DuckDB + SRE (DB1 + O4)

Extension INSTALL from internet (DB1) and version drift across nodes (O4) compound: if nodes install extensions at different times, they get different versions. **Consensus: bundle extensions in [[roadmap.md|Docker]] image with pinned versions.**

### Cost + Product (C4 + P5)

Both independently found that the $14.40/mo price point doesn't survive scrutiny. Compute costs alone ($15-120/mo for a VM) exceed the revenue. Multi-tenant hosted mode is even more expensive. **Consensus: the [[designs/pricing.md|pricing]] must be rethought — likely tiered (free OSS, $49+ hosted).**

### Product + Security (P1 + S2)

Vibe coders won't configure auth between peers (P1) and peers have no auth (S2). In hosted mode, the operator (sigiro team) configures auth. In OSS mode, peers are on localhost or trusted network. **Consensus: add a shared secret / bearer token for peer communication, pre-configured in hosted mode.**

---

## Synthesis: Priority-Ordered Recommendations

### Must Fix Before Building (blocks correctness)

| # | Finding | Action |
|---|---------|--------|
| 1 | D1 | Decide: Rust-native t-digest crate OR document the DuckDB temp-table merge dance explicitly with LOC estimate |
| 2 | O1 | Add compactor idempotency: marker file OR timestamp watermark in a local metadata table |
| 3 | DB1 | Bundle DuckDB extensions in Docker image, don't INSTALL from internet at runtime |
| 4 | DB4/O5 | Implement periodic SECRET refresh for IAM credential rotation |
| 5 | O2 | Add CHECKPOINT after DELETE in compactor |

### Should Fix (blocks production readiness)

| # | Finding | Action |
|---|---------|--------|
| 6 | C1/C2 | Change export cadence from hourly to daily, or buffer until 10MB threshold |
| 7 | D2 | Add `nodes_queried/responded/total` to distributed InvestigationBlock |
| 8 | O3 | Add compactor metrics (rows_exported, bytes_written, last_success_timestamp) |
| 9 | S2 | Add bearer token auth for peer-to-peer /investigate/sketches |
| 10 | O4 | Pin extension versions in Docker, add version check at startup |

### Should Reconsider (architectural)

| # | Finding | Action |
|---|---------|--------|
| 11 | P1 | Build Plan 03 for hosted mode only, keep OSS single-node-only |
| 12 | C4/P5 | Rethink pricing: compute costs dominate, $14.40/mo is below cost |
| 13 | P4 | Design hosted control plane before building OSS distributed infra |

---

## Verdict

**[[company/plan.md|Plan]] 03's cold tier (AR-1-3) is sound and needed** for hosted mode data retention. Ship it with the compactor fixes (O1, O2, O3) and extension bundling (DB1).

**[[company/plan.md|Plan]] 03's distributed query (AR-4) is a top priority with a real customer need.** The sketch merge mechanism (D1) is harder than documented — resolve the Rust-native vs [[architecture/data-model.md|DuckDB]]-SQL merge question before coding. Add distributed coverage metadata (D2), peer auth (S2), and extension version pinning (O4) to the implementation.

**The [[designs/pricing.md|pricing]] model (C4, P5) is the biggest strategic risk** — more dangerous than any technical finding. A $14.40/mo price with $15-120/mo compute costs is a business model bug, not a feature.

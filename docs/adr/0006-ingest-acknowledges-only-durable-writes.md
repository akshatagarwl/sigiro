---
status: accepted
---

# Ingest acknowledges only durable writes, and chooses the durable point by request rate

A TLA+ model of the ingest cell (`docs/notes/research/cloudflare-spikes/ingest-model/`) shows that acknowledging from the in-memory buffer loses acknowledged data on eviction. It also shows that an orphan sweep can delete a segment between its write and its manifest commit unless the cell first records the key as pending. The cell therefore registers each segment key as pending before writing it, and acknowledges only after one of two durable points. TLC verified both exhaustively.

- **Write-ahead row.** The request's compressed payload is stored as one SQLite row and acknowledged after the write commits. Measured on Cloudflare, each request writes 1 row and its later delete writes 1 more, and the commit takes about 30 ms. That costs about $2 per million requests in rows.
- **Segment commit.** The cell holds the request until its segment is in R2 and the manifest records it. Measured from San Jose, an R2 `put` takes 0.3 to 1.0 s with a bucket in the same region, and up to 2.8 s across regions. The cell pays per flush, not per request.

A cell uses the write-ahead row below about 2 requests per second and holds for the segment commit above it, where the list-price model crosses over. Holds are capped at a 5 s window, so the hold plus a same-region `put` stays below the default 10 s OTLP exporter timeout. Each R2 bucket must be placed in the same region as the ingest cells that write to it, because a cross-region `put` uses most of the remaining timeout.

The 2 requests per second crossover depends on duration billing while a cell holds requests, which is not yet measured. Revisit it with Cloudflare analytics after sustained load. Registering a segment in a table with a text primary key wrote 2 rows, one for the table and one for its index. Try a `WITHOUT ROWID` table, which should need only one, and measure it before choosing.

# Hosted Sigiro runs only on the portable binding subset

Hosted Sigiro moves off Hetzner to one Cloudflare Workers application. It may use only bindings that celld also implements: Workers, Durable Objects with SQLite, R2, Queues, Workflows, Cron Triggers, D1, Dynamic Workers, and the Sandbox SDK. The same `wrangler.json` must then deploy to Cloudflare or run on a self-hosted celld fleet. Pipelines, R2 Data Catalog, Hyperdrive, Workers AI, and Analytics Engine are excluded even where they would be easier, because celld does not implement them.

## Consequences

- Telemetry tables are Sigiro-owned Parquet segments plus a manifest, not Iceberg in R2 Data Catalog.
- Every Worker and Durable Object isolate has a 128 MB heap on both platforms, so queries must be bounded.
- Hetzner (native server, PostgreSQL, Rivet, Eve, ingest relay, optimizer worker) is decommissioned after cutover; there is no data migration, and old telemetry ages out under the 7-day retention.

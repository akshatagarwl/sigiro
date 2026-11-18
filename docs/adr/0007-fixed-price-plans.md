---
status: proposed
---

# Hosted Sigiro sells fixed-price plans with hard caps, not per-GB billing

Customers pay a flat monthly price per plan. Each plan caps ingest volume and request rate, and reaching a cap never adds charges. Ingest cost follows request count more than bytes, so the request-rate cap is what keeps a flat price safe.

| Plan | Price/month | Ingest | Request rate | Retention |
| --- | --- | --- | --- | --- |
| Hobby | $0 | 10 GB | 1 req/s | 7 days |
| Pro | $19 | 100 GB | 5 req/s | 14 days |
| Team | $79 | 500 GB | 25 req/s | 30 days |
| Enterprise | $249 | 2 TB | 100 req/s | 30 days |

At list prices, the worst case (a full cap sent as 16 KB requests) costs about $1.19, $9.79, $22.94, and $70.75 per tenant per month, from `docs/notes/research/cloudflare-spikes/ingest-model/cost.py`. Those figures are modeled, not measured on Cloudflare.

Queries are rate-limited per plan, not metered. Agents use the customer's own model key and a per-plan monthly allowance of sandbox minutes.

## Open before acceptance

- How an over-cap request fails. OTLP retries a 429, so the recommended answer is a non-retryable error plus notices at 80% and 100% of the cap.
- Sandbox-minute allowances, which wait for measured Cloudflare Containers cost.
- Whether Enterprise launches before queries can fan out across Workers, because a single query Worker scans only about 1 GB per query.

---
"sigiro": patch
---

Ingest rejected because storage maintenance has stalled now returns HTTP 503 with `Retry-After: 60` (gRPC `UNAVAILABLE`) instead of 507 Insufficient Storage, so senders no longer report a full disk. Only the disk cap returns 507.

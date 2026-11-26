---
"sigiro": minor
---

API responses give every point in time as a pair, `<name>_ts` (microseconds since the Unix epoch) and `<name>_iso` (RFC 3339, six fractional digits, `Z`), in snake_case (#67). `/v1/services` returns `first_seen_ts`/`first_seen_iso`, `last_seen_ts`/`last_seen_iso` and `total_count`, replacing `firstSeen`, `lastSeen` (zone-less text) and `totalCount`. `/v1/traces/{trace_id}` spans return `start_ts`/`start_iso` and `end_ts`/`end_iso` instead of `timestamp` text, and `span_attributes`, `resource_attributes`, `events_json` and `links_json` as JSON rather than strings holding JSON (#59). Alerts return `starts_*` and `ends_*`, ingest keys `created_*`, `last_used_*` and `revoked_*`, and the delivery and storage health `oldest_queued_*` and `last_checkpoint_*`, each replacing an RFC 3339 `*_at` field.

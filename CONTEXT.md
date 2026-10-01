# Sigiro

Sigiro ingests a tenant's OpenTelemetry signals and answers SQL questions about them for agents and operators.

## Language

**Tenant**:
The organization that owns a set of telemetry; every signal and query belongs to exactly one tenant.
_Avoid_: account, customer, project (in hosted contexts)

**Signal**:
One kind of OpenTelemetry data: spans, logs, metrics, or profiles.
_Avoid_: event type, telemetry kind

**Tenant Lake**:
The single durable home of one tenant's signals, with exactly one writer at any moment.
_Avoid_: database, warehouse, store

**Lake Catalog**:
The record of which data files make up a tenant lake at each snapshot.
_Avoid_: metadata DB, manifest

**Snapshot**:
One committed, immutable version of a tenant lake; queries always read exactly one snapshot.
_Avoid_: commit, version

**Ingest Buffer**:
Telemetry a tenant lake has acknowledged but not yet flushed; acknowledged telemetry is never lost.
_Avoid_: queue, WAL

**Flush**:
Moving the ingest buffer into the tenant lake as exactly one new snapshot.
_Avoid_: commit, sync

**Query Budget**:
The memory one query may use; exceeding it is a reported error, never a partial answer.
_Avoid_: limit, quota

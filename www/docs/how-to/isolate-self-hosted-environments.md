---
title: Isolate self-hosted environments
description: Keep dev, staging, and production telemetry apart with separate Sigiro instances and storage.
sidebar:
  order: 8
---

Use **one self-hosted Sigiro instance and one data directory per environment**
when dev, staging, and production must be separate. Give each instance its own
OTLP and query listeners, credentials at the network boundary, storage and
backup access, and retention settings. Send each environment's exporters only to
its corresponding OTLP endpoint. A different port alone is not isolation if the
instances still share a data directory or storage backend.

For a local binary, assign a different directory and all three listener ports to
each `sigiro serve` process. For example, on a host reserved for development:

```bash
SIGIRO_RETENTION_DAYS=2 sigiro serve \
  --data-dir /srv/sigiro/dev \
  --http-port 19999 --otlp-http-port 14318 --otlp-grpc-port 14317
```

Run staging and production as **separate processes** with their own paths and
non-overlapping ports, preferably on separate hosts or network segments. This
example does not configure TLS or authentication; do not expose its listeners to
untrusted networks. Keep production telemetry, backups, and storage credentials
in the production security boundary. Configure each environment's retention
independently (`SIGIRO_RETENTION_DAYS`); deleting a directory is not a retention
policy. See the [self-hosted quickstart](/docs/tutorials/quickstart) for the
container volume and localhost-only port mapping, and the
[CLI reference](/docs/reference/cli)
for settings.

## What a shared instance does not isolate

In Open mode, Sigiro accepts requests without authenticating them and resolves
them to the synthetic `default` tenant. `default` is **not** an environment
boundary: dev and production requests sent to the same Open-mode instance share
its tenant and query surface. Do not put production and less-trusted traffic on
a shared Open endpoint. Do not infer cross-tenant protections from this guide;
it makes no claim about untested authenticated-tenant behavior.

`deployment.environment` is an OpenTelemetry **resource attribute**, not a
Sigiro project ID or an authorization rule. You can label telemetry and filter
a query on a deliberately shared instance, but a missing, misspelled, or
user-supplied label can mix or omit data. Built-in diagnoses and other queries
do not automatically acquire your SQL filter. For example, after your exporter
sets `deployment.environment=staging` on the resource of each span, post this
literal SQL to that instance's `/v1/query` endpoint:

```sql
SELECT service_name, count(*) AS span_count
FROM sigiro_spans
WHERE timestamp >= TIMESTAMP '2026-09-23 00:00:00'
  AND timestamp < TIMESTAMP '2026-09-24 00:00:00'
  AND (resource_attributes::JSON->>'deployment.environment') = 'staging'
GROUP BY service_name
ORDER BY span_count DESC;
```

Change the two timestamps to your intended finite half-open window. The field
is read from the JSON string stored in `sigiro_spans.resource_attributes`; it
is not a separate schema column. This filter is a **query convention**, never
access control, storage isolation, or a substitute for separate instances.

## Tradeoff

Separate instances prevent one environment's Open-mode query endpoint from
reading another's data **when their storage and network access are also kept
separate**. They do not offer a single local query across dev, staging, and
production. To compare environments, query each endpoint separately and
combine results in an authorized external workflow; grant that workflow access
to each environment explicitly. A shared instance makes cross-environment SQL
easier, but gives up this separation and makes filtering the operator's
responsibility. Protect any remote OTLP or query access with TLS and an
access-controlled ingress or trusted private network; do not send credentials
or telemetry over an exposed plaintext connection. Rotate and scope ingress,
exporter, and storage credentials separately for each environment.

---
title: About deploy detection, and what your spans must carry
description: sigiro reads one attribute for deploys — service.version on a span's resource. How a version becomes a deploy event, and what happens without one.
sidebar:
  order: 4
---

A diagnosis can name a suspect deploy: _incident ~14:30 on service 'checkout':
… ; suspect deploy: version 1.4.2 first seen at 14:29_. That is a strong claim,
and a reader is right to ask where it comes from.

It comes from one attribute, in one place. This page names the attribute, states
how sigiro separates a deploy from steady state, explains how one deploy becomes
the suspect, and says what you get when the attribute is absent. The last part
matters most, because the absence is silent.

## One attribute, and one place

sigiro reads `service.version` from the **resource attributes of a span**.

That is the whole source. sigiro reads no other field for a deploy:

| Not read for deploys                              | Why not                               |
| ------------------------------------------------- | ------------------------------------- |
| `deployment.environment`, `deployment.*`          | No query reads them                   |
| `git.commit.sha`, `vcs.*`                         | No query reads them                   |
| `service.version` on a log, a metric or a profile | sigiro scans `sigiro_spans` alone     |
| A CI webhook, a deploy API, a release feed        | sigiro has no ingest path except OTLP |
| The Kubernetes API server                         | sigiro never connects to it           |

Resource attributes arrive over OTLP and land verbatim as a JSON string in the
`resource_attributes` column of `sigiro_spans`. Three resource attributes get a
column of their own at ingest — `service.name`, `service.namespace` and
`service.instance.id`. `service.version` is not one of them, so sigiro reads it
back out of the JSON on each query.

Two consequences follow from "spans only". A service that exports logs but no
spans produces no deploy events, however good its logs are. A service that
exports spans from one process and not another reports a deploy only when the
instrumented process restarts with the new version.

The value itself is opaque to sigiro. A semver string, a git SHA, a build number
and a date all work the same way, because sigiro compares two strings for
equality and never parses either one. sigiro derives no order from a version, so
it cannot tell an upgrade from a rollback.

## A deploy is a version that the baseline did not have

sigiro has no deploy timestamp to read, so it derives one. A deploy event is a
`service.version` value that appears on a span inside the diagnosis window and
appears on **no** span inside the baseline window.

The baseline window is four times the length of the diagnosis window, and it
ends where the diagnosis window starts. The default diagnosis window is the last
15 minutes, so the default baseline is the hour before it.

Each deploy event carries:

- `event_type` — the literal string `deploy`
- `detected_at` — the earliest span timestamp in the window that carries that
  version, which is when sigiro first _saw_ the version, not when your deploy
  started
- `description` — `version 1.4.2 first seen`

The comparison against the baseline is the important half, and it fixes a real
failure. Without it, "first seen" means the first row of the window, so every
diagnosis of every healthy service reports a deploy at the start of the window.
That event is always available, it always sits next to the incident in time, and
an agent that reads it treats it as the cause. A permanent false lead is worse
than no lead, so a version that already ran before the window starts counts as
steady state.

Five more consequences follow directly, and each one surprises somebody:

- **A constant version produces nothing.** If every build ships
  `service.version=1.0.0`, sigiro reports a deploy event once and then never
  again. To make deploys visible, give each release a distinct value.
- **Several new versions produce several events.** The query groups by version,
  so a window with three new versions returns three deploy events.
- **A rollback reads as a deploy.** The old version is absent from the baseline
  window, so it is new by this definition.
- **A gap in telemetry reads as a deploy.** If a service sent no spans in the
  baseline window, the baseline set is empty and every version in the window is
  new. Read a deploy at the start of a window with care when the service is new
  or was quiet.
- **A wider window looks further back.** sigiro derives the baseline from the
  window you ask for, so a 6-hour diagnosis compares against the previous 24
  hours.

## Kubernetes events come from log text

The same `events` array carries a second event type, `k8s_event`, and it has a
different source.

sigiro scans the `body` column of the service's logs in the window for five
fixed substrings: `OOMKilled`, `Unhealthy`, `BackOff`, `FailedScheduling` and
`ScalingReplicaSet`. The match is case-sensitive and it is a plain substring
match. The description is the whole log body, and sigiro returns at most 20 of
these per diagnosis.

So this is not a Kubernetes integration. sigiro never talks to the API server.
You see these events only when something forwards cluster events into your OTLP
logs under the same `service.name` as the service you diagnose. A collector that
watches events and exports them as logs is the usual arrangement. When nothing
forwards them, the array holds deploy events alone.

A `k8s_event` never becomes a suspect. Only events of type `deploy` are eligible.

## How one deploy becomes the suspect

Two rules decide this, and both come from the detector's own 5-minute bucket.

First, only an **incident** gets a suspect. A single shift that correlates with
no other shift stays a single-signal item and carries no deploy field at all. Two
or more shifts in the same or adjacent bucket merge into an incident, and only
that item looks for a deploy. [About evidence instead of a
dashboard](/docs/explanation/evidence) explains why shifts correlate that way.

Second, a deploy qualifies when its bucket falls inside the incident's own span
of buckets, or in the one bucket immediately before the first shift. That extra
bucket is arithmetic rather than tolerance: a deploy at 14:29 that causes a shift
detected in the 14:30 bucket is one bucket earlier by construction. When several
deploys qualify, the one closest in time to the start of the incident wins.

The winner appears twice: as the `deploy` object on the incident, and as a clause
at the end of the item summary. Every other deploy event stays in the
service's `events` array, which sigiro sorts by time and never filters. So a
deploy that lost the comparison is still in the response for you to read.

sigiro claims correlation in time and nothing more. The word is _suspect_ on
purpose.

## What happens when a service sets no version

Plainly: you get no deploy events, and sigiro tells you nothing about it.

The query drops rows where `service.version` is null, so a service that never
sets the attribute contributes no deploy event. The `events` array then holds
Kubernetes events alone, or it is empty. Every incident carries `deploy: null`,
and the summary ends without a suspect clause.

No item warns you. sigiro raises an item of kind `missing_resource_attributes`
for absent `k8s.namespace.name` and `k8s.deployment.name`, and that item says
nothing about `service.version`. There is no equivalent for the version. The
degradation is silent.

This is the one part of the response you cannot read at face value. A window with
no deploy events looks the same in two very different situations: no deploy
happened, and no process sets `service.version`. The response does not
distinguish them.

One query does distinguish them:

```sql
SELECT DISTINCT resource_attributes::JSON->>'service.version' AS version
FROM sigiro_spans
WHERE service_name = 'checkout'
  AND timestamp > now() - INTERVAL '24 hours';
```

Read the result this way:

- One row, and it is `NULL` — nothing sets the attribute. sigiro can never report
  a deploy for this service.
- One row with a value, unchanged for days — something sets the attribute, but
  the value never changes. sigiro reported a deploy the first time it saw the
  value, and it reports none after that.
- Several rows — deploy detection works for this service.

Set the attribute on the OpenTelemetry resource of your service, and give each
release a distinct value. [Instrument your code with an
agent](/docs/how-to/install-with-ai) covers the resource setup itself.

## Read next

- [About evidence instead of a dashboard](/docs/explanation/evidence) — how a
  shift becomes an incident, and why every row carries a query
- [About the tables, and which machine they describe](/docs/explanation/tables) —
  where `resource_attributes` lives, and which table answers which question
- [API reference](/docs/reference) — the `DerivedEvent` and `Incident` fields, in
  full

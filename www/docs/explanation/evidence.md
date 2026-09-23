---
title: About evidence instead of a dashboard
description: Why sigiro answers with a ranked list and a runnable query on every row, rather than a chart. What a baseline is, and what this design refuses to do.
sidebar:
  order: 3
---

Ask sigiro why a service is unwell, and it returns one block of structured
evidence. The block opens with a ranked list of what deserves attention. Each item
carries a one-line summary with the numbers in it, and a SQL query you can run to
check the claim.

It does not return a chart, and it never will. This page explains that choice: how
sigiro decides what is abnormal, how it ranks what it found, why the query travels
with the answer, and what the design refuses to do.

## The reader cannot see

A chart is a good answer for a person. A person takes in a shape, notices the step
at 14:10, and moves on. Decades of observability tools are built around that
moment, and they are built well.

The thing that now repairs production is an agent, and an agent cannot see a
chart. It can read text. So the loop breaks upstream of the fix: the agent can
write the patch, but it cannot find out what broke, and it cannot tell whether its
patch worked.

sigiro is the part that closes that gap. It is the layer under a dashboard, and it
does not replace one. If you want panels, bring your own and point them at the
same tables.

## A baseline, not a threshold

There is nothing to configure, because there is no threshold to configure.

Each series is compared against its own recent history. sigiro buckets every
signal into 5-minute buckets over a 7-day window, and it runs Bayesian online
changepoint detection over the bucketed series. A changepoint is a point where the
statistical regime of the series changes. The only structural choice in the whole
detector is a direction: sigiro keeps a shift when the mean after the changepoint
is worse than the mean before it. More errors, slower, louder, costlier. That is a
comparison rather than a number somebody picked.

### Fixed model assumptions

Threshold-free does not mean assumption-free. sigiro standardizes each series and
uses the fixed defaults below. They are not configuration options.

| Assumption         | Fixed value               | Meaning                                                                  |
| ------------------ | ------------------------- | ------------------------------------------------------------------------ |
| Hazard timescale   | `250` buckets             | Expected run length of about 20 hours 50 minutes at 5 minutes per bucket |
| Normal-Gamma prior | `NormalGamma(0, 1, 1, 1)` | Prior for the standardized Gaussian series                               |
| Run-length cutoff  | `1e-6`                    | Discards negligible run-length probability mass                          |
| CDF threshold      | `1e-3`                    | Truncates the low-probability tail                                       |

The detector then selects changepoints from the most likely run length and drops
the initial point, which only marks the start of the series. Users cannot tune
these values. Changing them changes the shared detection model and therefore
requires a tested sigiro release, not a per-service setting.

Five signals are detected:

| Signal           | Keyed by              | Measures                                   |
| ---------------- | --------------------- | ------------------------------------------ |
| `error_rate`     | service and operation | percent of error spans in each bucket      |
| `latency_p95`    | service and operation | p95 span duration in each bucket           |
| `log_volume`     | service               | log count in each bucket                   |
| `error_log_rate` | service               | percent of ERROR-class logs in each bucket |
| `profile_cost`   | service               | total profiled cost in each bucket         |

Two consequences follow, and both surprise people.

A service that is always slow is not an anomaly. If p95 has been two seconds all
week, two seconds is normal for that service, and sigiro says nothing. This is
correct and it is also a real limitation: sigiro tells you what _changed_, and a
chronic fault does not change. Use a query for chronic faults.

A young server finds nothing. The detector needs several buckets before a series
has a regime to shift from. A constant series and a very short series both produce
no changepoints by construction, which is why there is no minimum-points option
either.

The 5-minute bucket is also a deliberate blunt edge. It makes the detector flag
sustained shifts and ignore one-minute blips. A one-minute outage is invisible to
this detector, and a paging system is the right tool for that.

## From shift to incident to a ranked list

A real fault moves several signals at once. Errors rise, latency rises, the log
volume rises, and all three are one event rather than three.

So shifts are correlated before they are ranked. Two shifts in the same 5-minute
detection bucket, or in adjacent buckets, merge into one _incident_ item that
carries its member shifts and the suspect deploy, if a deploy is near. A shift
with no partner stays a single-signal item.

Then the list is ordered. The first sort key is severity. Within that, anomaly
items rank by the number of correlated signals, then by the size of the shift, then
by the raw difference, and each key sorts from large to small. So a three-signal
incident outranks a lone latency shift, which is the right order for a reader with
one question and limited attention.

The size term is worth one sentence, because a naive version of it is wrong. sigiro
uses the symmetric relative change, `(after − before) / (after + before)`, rather
than the raw difference. A raw difference cannot compare a latency shift measured
in microseconds against an error-rate shift measured in percent: the microseconds
win every time, whatever they mean. The relative form is unit-free, so shifts of
different signals sort against each other honestly.

Items that are not anomalies come last: inconsistencies such as errors with no
logs, coverage gaps that you can act on, and faults in the telemetry pipeline
itself, such as duplicate spans or orphan spans. That last group answers a question
a reader should ask before any other: can I trust this data at all?

## The query travels with the answer

Every row that makes a claim carries `drill_down_sql`: the query that produced the
claim, ready to post to `/v1/query`.

This is the part of the design we would defend hardest, and the reason is not
convenience. It is that a claim you can check is a different kind of claim. You do
not have to trust an automated diagnosis, and neither does your agent. Run the
query. Widen the window. Change the filter and see whether the claim survives.

It also fixes a failure that the alternative cannot fix. A summary that cannot be
checked has to be believed or discarded, and an agent given an unfalsifiable
summary will believe it. A query is falsifiable. When sigiro is wrong — and it is
sometimes wrong — the query is how you find out cheaply.

There is a second, quieter benefit. The query is also where your next question
starts. Copy it, change one predicate, and you have asked something sigiro never
thought of. That is why the drill query is raw SQL against real tables rather than
a link to a saved view.

## Why one call rather than five

The old shape of an investigation is five round trips. Query the metrics store,
reason, query the trace store, reason, query the log store, reason, query the
events store, reason. Each round trip is cheap and each model call is not, so the
model dominates the wall clock and the total runs to tens of seconds. The agent
also loses earlier context as the window fills with fragments.

`/v1/diagnose` runs the queries in parallel inside sigiro and returns one
correlated block. The ranked list is assembled in Rust from data the same queries
already returned, so it costs no extra query and no extra memory. One round trip,
one model call, one coherent picture. A full block is tens of kilobytes of
structured evidence rather than a raw span dump, which is a small fraction of a
modern context window.

The design pays for this with a bigger response and a slower single call. That is
the right trade when the reader reasons for seconds between calls, and the wrong
trade for a dashboard that refreshes fifty panels every ten seconds. sigiro is
built for the first reader.

## What this design refuses to do

An honest list, because each of these is a reasonable thing to want.

**Anomaly incidents do not send notifications yet.** Poll `GET /v1/anomalies`
with a scheduled agent or cron job. The detector persists incidents but does not
enqueue them for webhook delivery; waking an agent on a new anomaly remains work
in [#218](https://github.com/akshatagarwl/sigiro/issues/218). This is different
from the existing, OOM-only alert path below. There are no user-defined alert
rules, escalation policies, or human paging integrations.

### Existing container OOM alerts

Ingested logs with `event_name` set to `container.oom_killed` create a **firing**
alert for a project, service, and container name. The log must include
`container.name` and `container.id` in its attributes. Further distinct OOM
events for that alert increase `occurrence_count` without another firing
notification. A `container.oom_recovered` event resolves an existing firing
alert for the same project, service, and container name (if its timestamp is not
older than the firing event). A later OOM after resolution reopens it. Replayed
occurrences are ignored; recovery without a matching firing alert creates no
alert or notification. Alert state and notifications are persisted in the
administrative store, independently of webhook delivery.

`GET /v1/alerts` returns `{"alerts": [...]}` for the authenticated project (or
the active project in open-auth mode). Use `?status=firing` or
`?status=resolved` to filter; any other status is rejected. Each entry has
`fingerprint`, `rule_id` (`container-oom`), `status`, `service_name`,
`container_name`, `starts_at` (RFC 3339), `ends_at` (RFC 3339 or `null`), and
`occurrence_count`. This endpoint lists **OOM alert state**, not anomaly
incidents or webhook delivery attempts. Use `GET /v1/anomalies` for detector
results.

To deliver OOM transitions to a receiver, set `SIGIRO_ALERT_WEBHOOK_URL` on the
server. Unset or empty means delivery is disabled; alerts still accumulate in
the persistent outbox. The URL must be HTTPS; plain HTTP is accepted only when
the URL host is `localhost`, `127.0.0.1`, or `::1`. Credentials in the URL are
rejected. Optionally set `SIGIRO_ALERT_WEBHOOK_TOKEN` to send
`Authorization: Bearer <token>` with each POST. This token authenticates the
**outgoing receiver request**, not callers of Sigiro's API. The JSON POST uses
the Alertmanager webhook shape (`version: "4"`) and contains one alert:

```json
{
  "receiver": "sigiro",
  "status": "firing",
  "alerts": [
    {
      "status": "firing",
      "labels": {
        "alertname": "ContainerOOMKilled",
        "project": "default",
        "service": "api",
        "container": "api-1",
        "severity": "critical"
      },
      "annotations": {
        "summary": "api-1 was killed because it ran out of memory",
        "occurrence_count": "1"
      },
      "startsAt": "2026-09-23T10:00:00Z",
      "endsAt": "0001-01-01T00:00:00Z",
      "generatorURL": "",
      "fingerprint": "<stable alert fingerprint>"
    }
  ],
  "groupLabels": { "alertname": "ContainerOOMKilled" },
  "commonLabels": {
    "alertname": "ContainerOOMKilled",
    "project": "default",
    "service": "api",
    "container": "api-1",
    "severity": "critical"
  },
  "commonAnnotations": {},
  "externalURL": "",
  "version": "4",
  "groupKey": "{}:{alertname=\"ContainerOOMKilled\",fingerprint=\"<stable alert fingerprint>\"}",
  "truncatedAlerts": 0
}
```

For resolution, both `status` fields become `resolved` and `endsAt` is the
recovery timestamp. `fingerprint` is stable for the project, OOM rule, service,
and container name; it is not a unique delivery ID. No incident ID or anomaly
evidence is included. A receiver that wakes an agent for OOM can use the project
and service labels to call Sigiro's authenticated APIs for further evidence.

Delivery checks the durable outbox about once per second. Each request has a
10-second timeout; only 2xx marks it sent, and redirects are not followed.
Failures (including non-2xx and timeouts) retry after 5, 10, 20, ... seconds,
with delays capped at one hour; there is no attempt limit. The outbox survives
restart. Delivery is **at least once**: a crash after receiver success but before
sent-state persistence can repeat the POST. Deduplicate by `fingerprint`,
`status`, and `startsAt` (the same fingerprint can fire again after recovery);
do not treat a repeated POST as a new incident.

**It does not rank by business impact.** The rank order is statistical. sigiro does
not know which of your services takes payments, so a large shift in a background
worker can outrank a small shift in checkout. You know which service matters;
sigiro knows which series moved.

**It does not explain a cause.** The ranked list says what changed, how much, and
which signals moved together, and it names a suspect deploy when one is near in
time. Correlation in time is not cause. The evidence and the drill query are for
your judgement, or your agent's, and the diagnosis is a hypothesis with the query
that produced it.

**It does not draw anything.** There are no panels to build and none to rot. That
is the point, and it is also the thing some readers will miss most.

## Read next

- [About the tables, and which machine they describe](/docs/explanation/tables)
- [About deploy detection, and what your spans must
  carry](/docs/explanation/deploys) — where the suspect deploy comes from
- [API reference](/docs/reference) — the `Finding` and `DiagnosisBlock` fields, in
  full

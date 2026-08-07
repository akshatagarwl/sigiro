---
title: About the one-process design
description: Why sigiro is one binary with an embedded query engine, what that choice rules out, and what you pay for it at read time. The costs are deliberate.
sidebar:
  order: 1
---

sigiro is one process. It receives OpenTelemetry, it writes the data to disk, it
detects regime shifts, and it answers queries. There is no collector, no message
queue, no separate query tier and no external database. The query engine is
DuckDB, embedded in the same binary rather than run as a service, and the files
it writes are Parquet. You start one binary and the product runs.

This page explains why, what the choice rules out, and what it costs you. If you
only want the steps, the [quickstart](/docs/tutorials/quickstart) is enough. Come
back here when a limit surprises you and you want to know whether it is a defect
or a decision.

## The reason is the reader, not the machine

The usual observability backend runs four stores: one for metrics, one for logs,
one for traces, one for profiles. Each store has its own storage, its own
retention rules and its own query language. A person can hold that arrangement
in mind, because a person reads one panel at a time and switches tabs between
them.

An agent cannot. An agent needs one question and one answer, and it needs the
answer to arrive already correlated. Four stores mean four round trips and code
that stitches the results together by `trace_id`. So the shape of the reader
decided the shape of the backend: one store, one query language, one process.

The second reason is the operator. sigiro's audience is a team of one to ten
engineers, and the person who feels the fault is the person who installs the
tool, on the same afternoon. A design with four components fails that test
before it starts, whatever its throughput.

## What the choice rules out

A constraint is only real if it stops you from doing things you would like to do.
This one has stopped several proposals in this project:

| Proposal | What replaced it | Reason |
| --- | --- | --- |
| Ship sigiro as a DuckDB extension | A standalone binary that embeds DuckDB | Simpler build, no unsigned-extension problem |
| Write a custom Parquet writer | The storage layer already in DuckDB | It already handles compaction, catalog and object storage |
| A trigram index for log text | DuckDB's own `LIKE` and text-search extension | A custom index for under one percent of readers |
| A sidecar index on the write path | Nothing | A second storage format to keep correct |
| Kubernetes attribute enrichment | Documentation for the standard collector | Do not rebuild a processor that exists |
| eBPF auto-instrumentation inside sigiro | Keep it separate | Do not add a privileged requirement to sigiro |
| A central node with self-registered workers | An operator-run coordinator command | The fan-out pulled whole tables across the network, and no network perimeter was required |

Each row is a real feature that a reader has asked for. The pattern is
consistent: sigiro adds a component only when nothing already present can do the
job, and the bar rises with each component.

The rejected central-node design is the sharpest example, because it shipped and
then was deleted. It let each collector register itself with a central sigiro
node, which fanned queries out. The fan-out did not push predicates to the
remote side, so it pulled whole tables across the network before it filtered
them. The replacement has no central node and no registry: each node keeps its
own data, and one operator command pushes a partial query to each node and folds
the partials. Fewer moving parts, and the correctness problem solved rather than
hidden.

## You pay for this at read time

One process with no pre-aggregation means every query reads raw data. That is a
genuine cost, and these are the four places you meet it.

**No secondary indexes.** sigiro keeps a sort order and per-file statistics, and
nothing else. A query that bounds `timestamp` reads a small number of files,
because the statistics let the engine skip the rest. A query with no bound reads
every file. This is why every example in these docs bounds `timestamp`, and why
the federated coordinator refuses an unbounded query unless you pass
`--full-scan`.

**No pre-aggregation.** There are no rollup tables and no materialized views.
Buckets, percentiles and top-N are computed for each request. A dashboard that
refreshes fifty panels every ten seconds would make this the wrong product. One
or two queries for each incident makes it the right one.

**Free-text log search is a full scan.** A search backend tokenizes every log
message at write time and builds an inverted index, so a keyword search is one
lookup. sigiro stores the message as a string in a column. `message LIKE '%connection
refused%'` therefore reads every message in every file in the time range.
Per-file minimum and maximum statistics do not help: a range of `"aaa"` to
`"zzz"` says nothing about a substring. Structured field queries
are fast; keyword hunts are not. If you need sub-second keyword search over
terabytes of logs, sigiro is the wrong tool and we would rather you knew now.

**The first query in a window is cold.** The query engine is embedded and holds
no persistent index, so there is no state to rebuild and no recovery time. You
can kill the process and restart it, and it works. The other side of that trade
is that the first query against a new time window reads from disk or from object
storage with no warm cache.

## The network is the security boundary

A self-hosted server runs in **open mode**. It authenticates no request. This is
not an oversight and it is not a default you should change, because there is no
option to change: tenant keys exist only on the hosted service.

The reason is the same constraint. Authentication for a self-hosted install
means a user store, a key store, a rotation path and a recovery path, and all
four are components. The alternative is to let the network do the job it already
does. Put the server on a tailnet, on a private network, or behind a reverse
proxy that terminates TLS and checks identity. Most teams of this size already
have exactly one of those.

The same constraint governs the cross-node query surface. A node that serves its
data to a coordinator has no usable TLS of its own, so the perimeter is mandatory
rather than advisory, and the node binds to a private address by default.

Two facts follow, and both are worth saying plainly. A self-hosted server
ignores every authentication header, so a wrong key fails silently rather than
with a 401. And a self-hosted server reachable from the public internet is
readable by the public internet.

## What you get for the cost

**Your data does not leave your machine.** sigiro writes to the disk or the
bucket you point it at, and it sends nothing to us. If this project stops, the
files on disk are still Parquet, and every analytical tool reads Parquet. A move
to another tool costs you nothing, which is a different promise from a cheap
export tool.

**There is nothing to maintain.** No thresholds, no alert rules, no panels. What
counts as normal is a property of the data rather than a number you chose, so
there is no configuration to rot. [About evidence instead of a
dashboard](/docs/explanation/evidence) explains how that works.

**One query language.** Your telemetry is a set of SQL tables, and the dialect is
DuckDB SQL. You learn no second query language, and an agent already writes SQL.
That is the reason plain SQL is the interface rather than a bespoke grammar. It
also means the DuckDB function manual is your reference for anything these docs
do not name.

## Where the honest seam is

Two of the claims above weaken as you grow.

The single-node design was sized for roughly twenty services, ten hosts and tens
of gigabytes a month. It handles far more than that on one machine, because
DuckDB answers analytical queries over millions of rows in milliseconds on one
core. But
"one process" is a statement about each node, not about a fleet. Across many
nodes you run many processes and one coordinator command, and that coordinator
accepts a restricted query grammar rather than everything SQL can express.

The hosted service is also not the same product. It shares a catalog across
tenants, which is why it blocks `information_schema` and why it rejects a `WITH`
clause. Hosted exists for people who would rather not run anything. It is the
convenience option, and it does not have the privacy property that the
self-hosted install has.

## Read next

- [About the tables, and which machine they describe](/docs/explanation/tables)
- [About evidence instead of a dashboard](/docs/explanation/evidence)
- [Quickstart](/docs/tutorials/quickstart) — if you want to see it run

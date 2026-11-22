# Contrast ranking over traces (experiment)

**Question:** can one SQL query over Sigiro's spans find the cause of a
slowdown, and tell a local fault from system-wide slowness, before any probe
or model is involved?

This is the first step of an investigation loop in which an agent splits
telemetry into bad and good, localizes the difference and only then attaches
deeper probes. The loop is only worth building if this step works.

## How it works

[`contrast.sql`](contrast.sql) marks a trace as bad when its root span is
slower than the 90th percentile in scope. It describes each trace by every
(dimension, value) pair on its spans: service, span name, and each resource
and span attribute. Then it ranks pairs by

```
separation = share of slow traces with the pair - share of other traces with it
```

The query runs through `/v1/query` as it is, with no CTEs and only allowlisted
functions.

[`faults.py`](faults.py) sends 1,500 synthetic checkout traces per scenario
(root span plus lock, database and payment calls on 12 hosts and 25
accounts), runs the query and counts **localized factors**. A localized factor
is a pair with separation ≥ 0.3 that is at least 3× more common among slow
traces. Pairs with identical counts are aliases and count once (for example
`host.name=node-03` and `service.instance.id=node-03-checkout`).

| Scenario | Injected fault | Expected |
|---|---|---|
| control | none | 0 factors |
| slow-dependency | 8% of payment calls go to `payments-v2`, +300 ms | 1: `peer.service=payments-v2` |
| lock-contention | 8% of requests are `acct-7`, +250 ms waiting on a lock | 1: `app.account_id=acct-7` |
| noisy-neighbour | every span on `node-03` is 4× slower | 1: `host.name=node-03` |
| systemic-overload | every trace slowed by a random factor | 0 factors |
| compound | slow dependency (5%) and noisy neighbour together | 2 |

## Result

Run against `sigiro 0.2.0 (78ae900)`, every scenario passed for seeds 1 to 5
and the default seed (36 of 36 runs). The injected cause ranked first in
every single-fault run. Separation for the cause ranged from 0.69 to 0.91.
The best spurious pair under control or systemic slowness reached at most
0.062. A full run of six scenarios takes about 14 s, almost all of it waiting
for ingest to flush.

```
pass control            factors=0 top=resource.host.name=node-02 sep=0.0452
pass slow-dependency    factors=1 top=span.peer.service=payments-v2 sep=0.9
pass lock-contention    factors=1 top=span.app.account_id=acct-7 sep=0.78
pass noisy-neighbour    factors=1 top=resource.host.name=node-03 sep=0.7733
pass systemic-overload  factors=0 top=resource.host.name=node-08 sep=0.0519
pass compound           factors=2 top=span.peer.service=payments-v2 sep=0.5726
```

## What this does and does not show

- The data is synthetic and clean, and every local fault is keyed by an
  attribute the spans record. Real telemetry has correlated attributes,
  missing attributes and several overlapping tails. This shows the mechanism
  works. It does not show the method is accurate on production data.
- **Zero factors means "not explained by what is recorded".** It does not
  mean "systemic". A local cause carried by no attribute (GC pauses, one hot
  cache line, a kernel lock) looks the same as real system-wide load. In the
  intended loop, zero factors is the signal to escalate: add a profile, an
  eBPF probe or a new span attribute, then contrast again.
- Factor counting here is a threshold on single pairs, the cheap stand-in
  for the factor-analysis idea. It counts a fault keyed on a combination of
  two values (such as one account on one host) as 0 or 2, not 1.
- Only latency is labelled. Errors (`status_code = 2`) would need the same
  query with a different label.

## Run it

```sh
nix shell nixpkgs#python3 -c python3 experiments/contrast/faults.py \
  --sigiro "$(nix build .#sigiro --print-out-paths)/bin/sigiro"
```

`--sigiro` starts a throwaway server on free loopback ports with its own data
directory. To use a running server instead, pass `--endpoint` (HTTP) and
`--otlp` (OTLP/HTTP); data goes in under service namespaces named after the
scenarios. Other options: `--seed`, `--only <scenario>`, `--json <file>` for
the ranked rows.

## Next steps

1. Faults with no recording attribute, and faults keyed on a combination of
   two values, to measure the escalation signal and the pairwise blind spot.
2. Contrast over profiles (`sigiro_profiles.stack_trace` frames) as the
   second level: once a host or service is localized, rank functions the same
   way.
3. Replay real incidents stored in a Sigiro instance and score the rank of
   the known cause.

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

## Second round: what the numbers say

These came from sweeping thousands of synthetic incidents offline
([`sweeps.py`](sweeps.py), plain Python over the same trace model). The main
result was then checked on a real server ([`zcheck.py`](zcheck.py)).

### 1. Separation mixes how common a fault is with how bad it is

For a value present in a fraction π of traces, with a fraction b labelled
slow, `separation` is the 2x2 phi coefficient rescaled:

```
separation = phi * sqrt(pi(1-pi) / (b(1-b)))      (exact; max error 3e-16 over every value)
phi        = (P(value, slow) - pi*b) / sqrt(pi(1-pi) b(1-b))
```

With the threshold fixed at p90 (b = 0.1), a fault in a fraction p of traces
can never score above about p / 0.1. A dependency that adds 300 ms to 1% of
requests scores 0.10 and the 0.3 rule misses it, while at p99 the same fault
scores 0.95. The fixed threshold silently decides which prevalence you can
see.

### 2. Use z = phi·√n, maximized over thresholds

[`contrast_z.sql`](contrast_z.sql) computes phi for each value at a grid of
thresholds (p50 to p99.8) and returns the largest `z = phi * sqrt(n)`. It
uses only allowlisted functions. On the server, 3,000 traces per scenario:

| Scenario | Cause z | Highest other z | Old separation |
|---|---|---|---|
| control / systemic | n/a | 3.5 / 3.5 | n/a |
| slow-dependency (8%) | 46.8 | 3.1 | 0.75 |
| lock-contention | 46.6 | 2.9 | 0.74 |
| noisy-neighbour | 52.0 | 3.8 | 0.91 |
| compound (both causes) | 42.8 | 4.0 | 0.49 |
| rare dependency (1%) | **54.8** | 4.1 | 0.10 |
| very rare (0.3%, ~9 traces) | **44.7** | 3.7 | 0.013 |

Two regularities:

- **√n ceiling.** A threshold that isolates exactly the affected traces gives
  z = √n regardless of prevalence (54.77 = √3000 above). z/√n is just phi:
  how well the value explains the slow set, between 0 and 1.
- **Noise floor ≈ 4.** Maximizing over ~50 values × 10 thresholds of
  unrelated data gives z of 3 to 4.5. A cut at z ≥ 6 had no false positives
  in any run.

### 3. How much evidence a fault needs

From `sweeps.py rarity` and `sweeps.py power` (n = 3,000, 8 seeds):

- A severe fault is found from about **3 affected traces** (z ≈ 35;
  1.5 traces: found 88% of the time).
- For mild faults z ≈ 0.9 · √k · d, where k is the number of affected traces
  and d is the added delay over the root latency spread (about 16 ms here).
  At k = 150, +10 ms (d ≈ 0.6) is found 62% of the time and +20 ms always; at
  k = 30 it takes +20 ms for 62% and +40 ms for 100%.

As a rule of thumb, a fault is visible once k·d² ≳ 50. That gives a
probe policy a stopping rule: to confirm a suspected effect of size d in a
fraction π of requests, collect about 50 / (π·d²) traces.

### 4. phi ≈ √(precision · recall) decides AND versus OR

For a clean slow set, phi is close to the geometric mean of precision
(P(slow | value)) and recall (P(value | slow)). That says which way to move:

- **high recall, low precision**: the cause is narrower than this value, so
  refine within it (AND);
- **high precision, low recall**: this value is one of several causes, so set
  its traces aside and look at the rest (OR).

This held on the interaction fault: with slowness only on
`acct-7` AND `node-03`, the single values still score z ≈ 12–18 and 8–12 (precision
0.08 and 0.04, recall 1). That is close to the predicted √(10/120)·√3000 ≈ 16 and
√(10/250)·√3000 ≈ 11, and the pair scores ≈ √n.

A greedy search built on this rule (`sweeps.py rules`) pulls the best value,
AND-refines while precision < 0.8, removes the covered traces, picks a new
threshold over what remains, and stops when nothing reaches z ≥ 6. It
recovers the exact injected rule:

| Injected rule | n = 5,000 (10 seeds) | n = 50,000 (3 seeds) |
|---|---|---|
| none, systemic, unrecorded 3% | 10/10 return no rule | 3/3 |
| dep; host | 10/10 | 3/3 |
| dep OR host | 10/10 | 3/3 |
| acct AND host | 10/10 | 3/3 |
| (acct AND host) OR dep | 10/10 | 3/3 |
| acct AND host AND dep | **0/10** | 3/3 |

The three-way AND fails at 5,000 traces for the reason in §2. Inside a subset
of m traces z cannot exceed √m, and about 17 traces have both acct-7 and
node-03 (√17 ≈ 4.1 < 6). A k-way conjunction needs roughly 36 traces in its
deepest subset, so the trace count must scale with 1 / ∏πᵢ. Depth of
explanation costs data multiplicatively.

The output is a small boolean program over recorded attributes, which can be
checked by replaying it, rather than a ranking a person has to read. That is
the "compile an investigation into a verifiable rule" step of the loop.

### Limits of the second round

The same caveats as above apply. Data is synthetic and every non-systemic
cause except "unrecorded" is keyed by an attribute. The rule search exists
only in Python. It needs one contrast per step restricted to a subset of
traces, which `/v1/query` can do (`trace_id IN (subquery)`) but this
experiment has not wired up. Thresholds (z ≥ 6, precision 0.8) were set once
and never tuned, so they are not overfit, but they are also not calibrated on
real data.

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

```sh
sigiro="$(nix build .#sigiro --print-out-paths)/bin/sigiro"
cd experiments/contrast
nix shell nixpkgs#python3 -c python3 zcheck.py "$sigiro"   # server: contrast vs contrast_z
nix shell nixpkgs#python3 -c python3 sweeps.py rules       # offline: identity|rarity|power|rules
```

## Next steps

1. Run the rule search through `/v1/query` (subset contrasts with
   `trace_id IN (...)`) and make it the default output.
2. Contrast over profiles (`sigiro_profiles.stack_trace` frames) as the
   second level: once a host or service is localized, rank functions the same
   way.
3. Replay real incidents stored in a Sigiro instance and score the rank of
   the known cause.

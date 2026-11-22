#!/usr/bin/env python3
"""Offline sweeps behind the README's findings. No Sigiro server needed.

  sweeps.py identity   separation vs phi/z on every value; checks the identity
  sweeps.py rarity     how rare a fault can be and still be found
  sweeps.py power      how small a slowdown can be and still be found
  sweeps.py rules      recover boolean fault rules (AND / OR) by greedy search
"""

import math
import random
import sys

import faults
from explore import quantile, traces
from faults import Scenario

QS = [0.5, 0.7, 0.8, 0.9, 0.95, 0.97, 0.98, 0.99, 0.995, 0.998]
# Dimensions that are constant or alias host.name in the synthetic model.
SKIP = {
    "resource.service.namespace", "resource.service.instance.id", "resource.service.name",
    "span.lock.name", "span.db.system", "span.http.route", "span",
}
DEP = ("span.peer.service", "payments-v2")
ZMIN = 6.0

_lognormal = faults.lognormal_ms


def set_dependency_delay(ms):
    """Change the payments-v2 delay that make_trace adds (300ms by default)."""
    def patched(rng, median, sigma=0.35):
        return _lognormal(rng, ms if median == 300 else median, sigma)
    faults.lognormal_ms = patched


def table(rows, slow, mask=None):
    """(n, n_slow, {value: (n_value, n_value_slow)}) over rows selected by mask."""
    idx = [i for i in range(len(rows)) if mask is None or mask[i]]
    counts = {}
    for i in idx:
        for k in rows[i][0]:
            if k[0] in SKIP:
                continue
            c = counts.setdefault(k, [0, 0])
            c[0] += 1
            c[1] += slow[i]
    return len(idx), sum(slow[i] for i in idx), counts


def phi(n, n_slow, n_val, n_val_slow):
    pa, pb = n_val / n, n_slow / n
    denom = pa * (1 - pa) * pb * (1 - pb)
    return (n_val_slow / n - pa * pb) / math.sqrt(denom) if denom > 0 else 0.0


def max_z(rows):
    """Per value, max over thresholds of z = phi * sqrt(n)."""
    ds = sorted(d for _, d in rows)
    best = {}
    for q in QS:
        t = quantile(ds, q)
        n, n_slow, counts = table(rows, [d > t for _, d in rows])
        for k, (a, ab) in counts.items():
            best[k] = max(best.get(k, -9.0), phi(n, n_slow, a, ab) * math.sqrt(n))
    return best


def cmd_identity():
    """separation == phi * sqrt(pi(1-pi) / (b(1-b))) for every value and threshold."""
    rows = traces(Scenario("x", {"slow_dependency": 0.05}), 3000, random.Random(1))
    ds = sorted(d for _, d in rows)
    worst = 0.0
    for q in QS:
        t = quantile(ds, q)
        n, nb, counts = table(rows, [d > t for _, d in rows])
        for a, ab in counts.values():
            if a in (0, n):
                continue
            sep = ab / nb - (a - ab) / (n - nb)
            pi, b = a / n, nb / n
            worst = max(worst, abs(sep - phi(n, nb, a, ab) * math.sqrt(pi * (1 - pi) / (b * (1 - b)))))
    print(f"max |separation - phi*sqrt(pi(1-pi)/(b(1-b)))| = {worst:.2e}")


def detect(p, delay_ms, n=3000, seeds=8):
    set_dependency_delay(delay_ms)
    zs, noise, hits = [], [], 0
    for seed in range(seeds):
        best = max_z(traces(Scenario("x", {"slow_dependency": p}), n, random.Random(seed)))
        z = best.get(DEP, 0.0)
        nz = max(v for k, v in best.items() if k != DEP)
        zs.append(z)
        noise.append(nz)
        hits += z > max(nz, ZMIN)
    set_dependency_delay(300)
    return sum(zs) / seeds, sum(noise) / seeds, hits / seeds


def cmd_rarity():
    print(f"n=3000, +300ms, sqrt(n)={math.sqrt(3000):.1f}")
    for p in [0.0005, 0.001, 0.002, 0.005, 0.01, 0.05]:
        z, nz, h = detect(p, 300)
        print(f"  p={p:<7} ~{p * 3000:5.1f} traces  z={z:5.1f}  noise={nz:4.1f}  found={h:.2f}")


def cmd_power():
    for p, delays in [(0.05, [2, 5, 10, 20, 40, 80]), (0.01, [10, 20, 40, 80])]:
        print(f"n=3000, p={p} ({p * 3000:.0f} affected traces)")
        for ms in delays:
            z, nz, h = detect(p, ms)
            print(f"  +{ms:<3}ms  z={z:5.1f}  noise={nz:4.1f}  found={h:.2f}")


def best_value(rows, slow, mask):
    """Value with the highest z inside mask, with its z, precision and recall."""
    n, nb, counts = table(rows, slow, mask)
    if nb in (0, n):
        return None, 0.0, 0.0, 0.0
    k, (a, ab) = max(
        ((k, c) for k, c in counts.items() if c[0] < n),
        key=lambda kc: phi(n, nb, *kc[1]),
        default=(None, (0, 0)),
    )
    if k is None:
        return None, 0.0, 0.0, 0.0
    return k, phi(n, nb, a, ab) * math.sqrt(n), ab / a, ab / nb


def label(rows, mask):
    """Slow flags at the threshold, over unexplained traces, where the best value's z peaks."""
    ds = sorted(rows[i][1] for i in range(len(rows)) if mask[i])
    best = (-1.0, [False] * len(rows))
    for q in QS:
        t = quantile(ds, q)
        slow = [d > t for _, d in rows]
        z = best_value(rows, slow, mask)[1]
        if z > best[0]:
            best = (z, slow)
    return best[1]


def refine(rows, slow, mask, first, precision):
    """AND-refine a clause while precision is low and a value still separates inside it."""
    clause = [first]
    mask = [mask[i] and first in rows[i][0] for i in range(len(rows))]
    while precision < 0.8:
        k, z, p, _ = best_value(rows, slow, mask)
        if k is None or z < ZMIN or p <= precision:
            break
        clause.append(k)
        precision = p
        mask = [mask[i] and k in rows[i][0] for i in range(len(rows))]
    return frozenset(v for _, v in clause), mask


def explain(rows):
    """Greedy OR of AND clauses that accounts for the slow traces."""
    remaining = [True] * len(rows)
    rule = set()
    for _ in range(4):
        slow = label(rows, remaining)
        k, z, precision, _ = best_value(rows, slow, remaining)
        if k is None or z < ZMIN:
            break
        clause, covered = refine(rows, slow, remaining, k, precision)
        rule.add(clause)
        remaining = [r and not c for r, c in zip(remaining, covered)]
        if sum(s for s, r in zip(slow, remaining) if r) < 3:
            break
    return rule


def inject(rows, rng, clauses, unrecorded=0.0, systemic=False):
    out = []
    for feats, d in rows:
        values = {v for _, v in feats}
        if any(c <= values for c in clauses):
            d += 300 * rng.lognormvariate(0, 0.2)
        if unrecorded and rng.random() < unrecorded:
            d += 300
        if systemic:
            d *= rng.lognormvariate(0, 0.6)
        out.append((feats, d))
    return out


F = frozenset
RULE_CASES = [
    ("none", [], {}),
    ("dep", [F({"payments-v2"})], {"p": 0.05}),
    ("host", [F({"node-03"})], {}),
    ("dep OR host", [F({"payments-v2"}), F({"node-07"})], {"p": 0.05}),
    ("acct AND host", [F({"acct-7", "node-03"})], {}),
    ("acct AND host AND dep", [F({"acct-7", "node-03", "payments-v2"})], {"p": 0.3}),
    ("(acct AND host) OR dep", [F({"acct-7", "node-03"}), F({"payments-v2"})], {"p": 0.03}),
    ("unrecorded 3%", [], {"unrecorded": 0.03}),
    ("systemic", [], {"systemic": True}),
]


def cmd_rules(n=5000, seeds=10):
    set_dependency_delay(0)  # only inject() adds latency
    print(f"n={n}, {seeds} seeds: exact rule recovered")
    for name, clauses, kw in RULE_CASES:
        ok, wrong = 0, []
        for seed in range(seeds):
            rng = random.Random(seed)
            sc = Scenario("x", {"slow_dependency": kw["p"]} if "p" in kw else {})
            rows = inject(traces(sc, n, rng), rng, clauses,
                          kw.get("unrecorded", 0.0), kw.get("systemic", False))
            got = explain(rows)
            ok += got == set(clauses)
            if got != set(clauses) and len(wrong) < 2:
                wrong.append(sorted(sorted(c) for c in got))
        print(f"  {name:24} {ok}/{seeds}  {wrong if wrong else ''}")
    set_dependency_delay(300)


if __name__ == "__main__":
    cmds = {"identity": cmd_identity, "rarity": cmd_rarity, "power": cmd_power}
    if len(sys.argv) >= 2 and sys.argv[1] == "rules":
        cmd_rules(*(int(a) for a in sys.argv[2:4]))
    elif len(sys.argv) == 2 and sys.argv[1] in cmds:
        cmds[sys.argv[1]]()
    else:
        sys.exit(__doc__)

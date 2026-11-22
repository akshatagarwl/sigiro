#!/usr/bin/env python3
"""Offline sweeps of the contrast ranking, without a Sigiro server.

Uses make_trace from faults.py and computes the same per-value counts that
contrast.sql computes, so a sweep over thousands of synthetic incidents runs
in seconds. Findings here are checked against a real server separately.
"""

import math
import random
import sys
from collections import Counter

from faults import Scenario, make_trace


def features(resource, spans):
    out = {("resource." + k, v) for k, v in resource.items()}
    for s in spans:
        out.add(("span", s["name"]))
        for a in s["attributes"]:
            out.add(("span." + a["key"], a["value"]["stringValue"]))
    return frozenset(out)


def traces(scenario, n, rng):
    rows = []
    for i in range(n):
        resource, spans = make_trace(rng, scenario, i)
        root = spans[0]
        ms = (int(root["endTimeUnixNano"]) - int(root["startTimeUnixNano"])) / 1e6
        rows.append((features(resource, spans), ms))
    return rows


def quantile(xs, q):
    xs = sorted(xs)
    pos = q * (len(xs) - 1)
    lo = math.floor(pos)
    return xs[lo] + (xs[min(lo + 1, len(xs) - 1)] - xs[lo]) * (pos - lo)


def contrast(rows, q):
    """separation per value at root-duration quantile q, as contrast.sql does."""
    t = quantile([d for _, d in rows], q)
    bad, good = Counter(), Counter()
    nb = ng = 0
    for feats, d in rows:
        if d > t:
            nb += 1
            bad.update(feats)
        else:
            ng += 1
            good.update(feats)
    return {
        f: (bad[f] / nb - good[f] / max(ng, 1), bad[f], good[f])
        for f in set(bad) | set(good)
    }, nb, ng


if __name__ == "__main__":
    sys.exit("import this module; see the sweep scripts")

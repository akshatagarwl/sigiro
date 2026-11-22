#!/usr/bin/env python3
"""Compare contrast.sql and contrast_z.sql on a throwaway Sigiro server.

Runs faults.py's scenarios plus two rare-fault scenarios and prints, per
scenario, the top value from contrast_z.sql, the z of the injected cause, the
highest z of any other value, and the cause's separation from contrast.sql.

  zcheck.py /path/to/bin/sigiro
"""

import random
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

import faults
from faults import SCENARIOS, SLOW_DEP, Scenario, query, scope, send, start_server, wait_ingested

HERE = Path(__file__).resolve().parent
QUANTILES = [0.5, 0.7, 0.8, 0.9, 0.95, 0.97, 0.98, 0.99, 0.995, 0.998]
CASES = SCENARIOS + [
    Scenario("rare-dep", {"slow_dependency": 0.01}, expected={SLOW_DEP}),
    Scenario("very-rare-dep", {"slow_dependency": 0.003}, expected={SLOW_DEP}),
]
# Constant in every scenario, or an alias of host.name.
IGNORE = {"resource.service.namespace", "resource.service.instance.id"}


def sql(name):
    text = (HERE / name).read_text()
    return "\n".join(line for line in text.splitlines() if not line.startswith("--"))


def lookup(rows, key, cause):
    return next((r[key] for r in rows if (r["dim"], r["val"]) == cause), 0)


def run_case(endpoint, otlp, sc, base, seed):
    where = scope(sc, base)
    send(otlp, sc, base, random.Random(seed))
    wait_ingested(endpoint, where, faults.TRACES * 4)

    rows = query(endpoint, sql("contrast_z.sql").format(scope=where, quantiles=QUANTILES, limit=200))
    rows = [r for r in rows if r["dim"] not in IGNORE]
    seps = query(endpoint, sql("contrast.sql").format(scope=where, quantile=0.9, limit=200))

    cause = next(iter(sc.expected), None)
    noise = max(r["z"] for r in rows if (r["dim"], r["val"]) not in sc.expected)
    print(f"{sc.name:18} top={rows[0]['val']:<14} z_cause={lookup(rows, 'z', cause):6.2f} "
          f"z_other={noise:5.2f} separation={lookup(seps, 'separation', cause)}")


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    faults.TRACES = 3000
    workdir = Path(tempfile.mkdtemp(prefix="sigiro-zcheck."))
    proc, endpoint, otlp = start_server(sys.argv[1], workdir)
    base0 = datetime.now(timezone.utc).replace(microsecond=0) - timedelta(hours=6)
    try:
        for i, sc in enumerate(CASES):
            run_case(endpoint, otlp, sc, base0 + timedelta(minutes=10 * i), 100 + i)
    finally:
        proc.terminate()
        proc.wait(timeout=30)
    print(f"evidence: {workdir}")


if __name__ == "__main__":
    main()

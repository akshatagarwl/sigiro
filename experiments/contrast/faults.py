#!/usr/bin/env python3
"""Inject known faults into synthetic traces and check that contrast.sql finds them.

Each scenario sends traces for a small checkout service to Sigiro over
OTLP/HTTP JSON, runs contrast.sql over that scenario's traces, and checks two
things against the injected fault:

1. the top-ranked factor is the injected cause, and
2. the number of localized factors matches the scenario: 1 for a local fault,
   2 for two faults at once, 0 for system-wide slowness or no fault.

Usage:
  faults.py --sigiro /path/to/bin/sigiro    start a throwaway server
  faults.py --endpoint http://127.0.0.1:9999 --otlp http://127.0.0.1:4318

Standard library only. Exits non-zero if any scenario fails.
"""

import argparse
import json
import os
import random
import secrets
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
HOSTS = [f"node-{n:02d}" for n in range(1, 13)]
ACCOUNTS = [f"acct-{n}" for n in range(1, 26)]
TRACES = 1500
QUANTILE = 0.9
# A factor counts as localized when it covers this much more of the slow
# traces than of the rest, and is this many times more common among them.
MIN_SEPARATION = 0.3
MIN_LIFT = 3.0


@dataclass
class Scenario:
    name: str
    faults: dict[str, float] = field(default_factory=dict)
    systemic: bool = False
    expected: set[tuple[str, str]] = field(default_factory=set)


SLOW_DEP = ("span.peer.service", "payments-v2")
HOT_LOCK = ("span.app.account_id", "acct-7")
NEIGHBOUR = ("resource.host.name", "node-03")

SCENARIOS = [
    Scenario("control"),
    Scenario("slow-dependency", {"slow_dependency": 0.08}, expected={SLOW_DEP}),
    Scenario("lock-contention", {"lock_contention": 0.08}, expected={HOT_LOCK}),
    Scenario("noisy-neighbour", {"noisy_neighbour": 1.0}, expected={NEIGHBOUR}),
    Scenario("systemic-overload", systemic=True),
    Scenario(
        "compound",
        {"slow_dependency": 0.05, "noisy_neighbour": 1.0},
        expected={SLOW_DEP, NEIGHBOUR},
    ),
]


def attrs(values):
    return [{"key": k, "value": {"stringValue": v}} for k, v in values.items()]


def lognormal_ms(rng, median, sigma=0.35):
    return median * rng.lognormvariate(0, sigma)


def make_trace(rng, scenario, start_ns):
    """One checkout request: root span plus lock, database and payment calls."""
    f = scenario.faults
    account = rng.choice(ACCOUNTS)
    if "lock_contention" in f and rng.random() < f["lock_contention"]:
        account = HOT_LOCK[1]
    elif account == HOT_LOCK[1] and "lock_contention" in f:
        account = ACCOUNTS[0]
    host = rng.choice(HOSTS)
    payments = "payments-v1"
    if "slow_dependency" in f and rng.random() < f["slow_dependency"]:
        payments = SLOW_DEP[1]

    lock_ms = lognormal_ms(rng, 2)
    db_ms = lognormal_ms(rng, 20)
    pay_ms = lognormal_ms(rng, 40)
    if account == HOT_LOCK[1] and "lock_contention" in f:
        lock_ms += lognormal_ms(rng, 250, 0.2)
    if payments == SLOW_DEP[1]:
        pay_ms += lognormal_ms(rng, 300, 0.2)
    slowdown = 1.0
    if "noisy_neighbour" in f and host == NEIGHBOUR[1]:
        slowdown *= 4.0
    if scenario.systemic:
        slowdown *= rng.lognormvariate(0, 0.6)
    lock_ms, db_ms, pay_ms = (x * slowdown for x in (lock_ms, db_ms, pay_ms))
    root_ms = (lock_ms + db_ms + pay_ms + lognormal_ms(rng, 5))

    trace_id = secrets.token_hex(16)
    root_id = secrets.token_hex(8)
    resource = {
        "service.name": "checkout",
        "service.namespace": scenario.name,
        "host.name": host,
        "service.instance.id": f"{host}-checkout",
    }

    def span(name, span_id, parent, offset_ms, ms, kind, extra):
        begin = start_ns + int(offset_ms * 1e6)
        return {
            "traceId": trace_id,
            "spanId": span_id,
            "parentSpanId": parent,
            "name": name,
            "kind": kind,
            "startTimeUnixNano": str(begin),
            "endTimeUnixNano": str(begin + int(ms * 1e6)),
            "attributes": attrs(extra),
        }

    spans = [
        span("GET /checkout", root_id, "", 0, root_ms, 2,
             {"http.route": "/checkout", "app.account_id": account}),
        span("lock.acquire", secrets.token_hex(8), root_id, 1, lock_ms, 1,
             {"lock.name": "cart"}),
        span("SELECT orders", secrets.token_hex(8), root_id, 1 + lock_ms, db_ms, 3,
             {"db.system": "postgresql"}),
        span("POST /charge", secrets.token_hex(8), root_id, 1 + lock_ms + db_ms, pay_ms, 3,
             {"peer.service": payments}),
    ]
    return resource, spans


def payload(resource, spans):
    return {
        "resourceSpans": [{
            "resource": {"attributes": attrs(resource)},
            "scopeSpans": [{"scope": {"name": "contrast-faults"}, "spans": spans}],
        }]
    }


def post(url, body, content_type):
    data = body if isinstance(body, bytes) else body.encode()
    req = urllib.request.Request(url, data=data, headers={"Content-Type": content_type})
    with urllib.request.urlopen(req, timeout=60) as resp:
        raw = resp.read()
    return json.loads(raw) if raw else None


def send(otlp, scenario, base, rng):
    by_resource = {}
    for i in range(TRACES):
        start = base + timedelta(milliseconds=150 * i)
        resource, spans = make_trace(rng, scenario, int(start.timestamp() * 1e9))
        by_resource.setdefault(json.dumps(resource, sort_keys=True), []).extend(spans)
    for key, spans in by_resource.items():
        for i in range(0, len(spans), 2000):
            post(f"{otlp}/v1/traces",
                 json.dumps(payload(json.loads(key), spans[i:i + 2000])),
                 "application/json")


def scope(scenario, base):
    fmt = "%Y-%m-%d %H:%M:%S"
    lo = (base - timedelta(hours=1)).strftime(fmt)
    hi = (base + timedelta(hours=1)).strftime(fmt)
    return (f"timestamp >= TIMESTAMP '{lo}' AND timestamp < TIMESTAMP '{hi}' "
            f"AND service_namespace = '{scenario.name}'")


def query(endpoint, sql):
    return post(f"{endpoint}/v1/query", sql, "text/plain")


def wait_ingested(endpoint, where, expected, deadline_s=60):
    sql = f"SELECT count(*) AS n FROM sigiro_spans WHERE {where}"
    end = time.monotonic() + deadline_s
    while time.monotonic() < end:
        if query(endpoint, sql)[0]["n"] >= expected:
            return
        time.sleep(1)
    raise TimeoutError(f"spans not visible after {deadline_s}s: {where}")


def factors(rows):
    """Localized factors, with aliases (identical bad/good counts) merged."""
    groups = {}
    for row in rows:
        lift = row["bad_share"] / max(row["good_share"], 1e-9)
        if row["separation"] >= MIN_SEPARATION and lift >= MIN_LIFT:
            groups.setdefault((row["bad"], row["good"]), []).append((row["dim"], row["val"]))
    return list(groups.values())


def evaluate(scenario, rows):
    found = factors(rows)
    problems = []
    if len(found) != len(scenario.expected):
        problems.append(f"expected {len(scenario.expected)} factor(s), found {len(found)}")
    for cause in scenario.expected:
        if not any(cause in group for group in found):
            problems.append(f"cause {cause[0]}={cause[1]} not among localized factors")
    if scenario.expected and rows and (rows[0]["dim"], rows[0]["val"]) not in scenario.expected:
        top = rows[0]
        if not any((top["dim"], top["val"]) in g and set(g) & scenario.expected for g in found):
            problems.append(f"top-ranked {top['dim']}={top['val']} is not an injected cause")
    return found, problems


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def start_server(binary, workdir):
    http, otlp, grpc = free_port(), free_port(), free_port()
    home = workdir / "home"
    home.mkdir()
    env = {"PATH": os.environ.get("PATH", ""), "HOME": str(home),
           "SIGIRO_BIND": "127.0.0.1", "SIGIRO_FLUSH_INTERVAL_SECS": "1"}
    log = open(workdir / "server.log", "w")
    proc = subprocess.Popen(
        [binary, "serve", "--data-dir", str(workdir / "data"), "--http-port", str(http),
         "--otlp-http-port", str(otlp), "--otlp-grpc-port", str(grpc)],
        env=env, stdout=log, stderr=subprocess.STDOUT)
    endpoint = f"http://127.0.0.1:{http}"
    for _ in range(60):
        if proc.poll() is not None:
            sys.exit(f"sigiro exited; see {workdir / 'server.log'}")
        try:
            urllib.request.urlopen(f"{endpoint}/health", timeout=2)
            return proc, endpoint, f"http://127.0.0.1:{otlp}"
        except (urllib.error.URLError, ConnectionError):
            time.sleep(1)
    proc.kill()
    sys.exit("sigiro did not become healthy in 60s")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--sigiro", help="sigiro binary to start on free loopback ports")
    parser.add_argument("--endpoint", help="existing Sigiro HTTP endpoint")
    parser.add_argument("--otlp", help="existing Sigiro OTLP/HTTP endpoint")
    parser.add_argument("--seed", type=int, default=1850)
    parser.add_argument("--only", action="append", help="run only this scenario")
    parser.add_argument("--json", type=Path, help="write ranked rows per scenario here")
    args = parser.parse_args()
    if bool(args.sigiro) == bool(args.endpoint):
        parser.error("pass exactly one of --sigiro or --endpoint")

    proc = None
    workdir = Path(tempfile.mkdtemp(prefix="sigiro-contrast."))
    if args.sigiro:
        proc, endpoint, otlp = start_server(args.sigiro, workdir)
    else:
        endpoint, otlp = args.endpoint, args.otlp or args.endpoint

    template = (HERE / "contrast.sql").read_text()
    template = "\n".join(l for l in template.splitlines() if not l.startswith("--"))
    rng = random.Random(args.seed)
    base0 = datetime.now(timezone.utc).replace(microsecond=0) - timedelta(hours=6)
    results, failed = {}, 0
    try:
        for i, scenario in enumerate(SCENARIOS):
            if args.only and scenario.name not in args.only:
                continue
            base = base0 + timedelta(minutes=10 * i)
            where = scope(scenario, base)
            send(otlp, scenario, base, rng)
            wait_ingested(endpoint, where, TRACES * 4)
            sql = template.format(scope=where, quantile=QUANTILE, limit=40)
            rows = query(endpoint, sql)
            found, problems = evaluate(scenario, rows)
            results[scenario.name] = {"rows": rows, "factors": found, "problems": problems}
            status = "pass" if not problems else "FAIL"
            failed += bool(problems)
            print(f"{status:4} {scenario.name:18} factors={len(found)} "
                  f"top={rows[0]['dim']}={rows[0]['val']} sep={rows[0]['separation']}")
            for group in found:
                print("       factor:", ", ".join(f"{d}={v}" for d, v in group))
            for problem in problems:
                print("       problem:", problem)
    finally:
        if proc:
            proc.terminate()
            proc.wait(timeout=30)
    if args.json:
        args.json.write_text(json.dumps(results, indent=2))
    print(f"evidence: {workdir}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

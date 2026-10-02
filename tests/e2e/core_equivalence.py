#!/usr/bin/env python3
"""Record what a real `sigiro serve` stores and answers for a fixed OTLP corpus.

usage: core_equivalence.py <sigiro-binary> <snapshot.json>

Two binaries are behaviour-equivalent for the OTLP decoder, DRAIN classifier and
SQL firewall when their snapshots are identical. Needs `opentelemetry-proto`.
"""

import base64
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

from google.protobuf.json_format import ParseDict
from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import ExportLogsServiceRequest
from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import ExportMetricsServiceRequest
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest

T0 = 1_790_000_000_000_000_000


def resource(service):
    return {"attributes": [
        {"key": "service.name", "value": {"stringValue": service}},
        {"key": "service.version", "value": {"stringValue": "1.4.2"}},
        {"key": "k8s.pod.name", "value": {"stringValue": "pod-7"}},
    ]}


def attrs(**kv):
    out = []
    for k, v in kv.items():
        if isinstance(v, bool):
            out.append({"key": k, "value": {"boolValue": v}})
        elif isinstance(v, int):
            out.append({"key": k, "value": {"intValue": str(v)}})
        elif isinstance(v, float):
            out.append({"key": k, "value": {"doubleValue": v}})
        else:
            out.append({"key": k, "value": {"stringValue": v}})
    return out


def traces(service, offset):
    spans = []
    for i in range(6):
        start = T0 + offset + i * 1_000_000_000
        spans.append({
            "traceId": f"{offset + i + 1:032x}", "spanId": f"{offset + i + 1:016x}",
            "parentSpanId": f"{offset + i:016x}" if i else "",
            "name": f"GET /items/{i % 3}", "kind": 2,
            "startTimeUnixNano": str(start), "endTimeUnixNano": str(start + (i + 1) * 1_234_567),
            "attributes": attrs(**{"http.route": f"/items/{i % 3}", "http.status_code": 500 if i == 4 else 200, "retry": i == 5, "ratio": 0.25 * i}),
            "events": [{"timeUnixNano": str(start + 10), "name": "cache.miss", "attributes": attrs(key=f"k{i}")}] if i % 2 else [],
            "links": [{"traceId": f"{offset + 1:032x}", "spanId": f"{offset + 1:016x}"}] if i == 3 else [],
            "status": {"code": 2, "message": "boom"} if i == 4 else {"code": 1},
        })
    return {"resourceSpans": [{"resource": resource(service), "scopeSpans": [{"scope": {"name": "fixture", "version": "1"}, "spans": spans}]}]}


LOG_LINES = [
    "user 4123 logged in from 10.0.0.1",
    "user 77 logged in from 192.168.1.20",
    "cache miss for key 0af7651916cd43dd8448eb211c80319c",
    "request 3f2c8a1e-9b7d-4c5e-8f00-1a2b3c4d5e6f took 125 ms",
    "request 11111111-2222-3333-4444-555555555555 took 9 ms",
    "disk /dev/sda1 at 93% capacity",
    "",
]


def logs(service, offset):
    records = []
    for i, line in enumerate(LOG_LINES):
        records.append({
            "timeUnixNano": str(T0 + offset + i * 1_000_000), "observedTimeUnixNano": str(T0 + offset + i * 1_000_000 + 5),
            "severityNumber": 17 if "capacity" in line else 9, "severityText": "ERROR" if "capacity" in line else "INFO",
            "body": {"stringValue": line}, "attributes": attrs(**{"session.id": f"s{i % 2}", "cost_usd": 0.5 * i}),
            "traceId": f"{offset + i + 1:032x}", "spanId": f"{offset + i + 1:016x}", "eventName": "fixture.log" if i == 0 else "",
        })
    return {"resourceLogs": [{"resource": resource(service), "scopeLogs": [{"scope": {"name": "fixture"}, "logRecords": records}]}]}


def metrics(service, offset):
    t = str(T0 + offset)
    start = str(T0 + offset - 60_000_000_000)
    point = lambda **extra: {"timeUnixNano": t, "startTimeUnixNano": start, "attributes": attrs(cpu="0"), **extra}
    return {"resourceMetrics": [{"resource": resource(service), "scopeMetrics": [{"scope": {"name": "fixture"}, "metrics": [
        {"name": "system.cpu.utilization", "unit": "1", "description": "cpu", "gauge": {"dataPoints": [point(asDouble=0.42), point(asInt="3")]}},
        {"name": "http.requests", "unit": "{request}", "sum": {"aggregationTemporality": 2, "isMonotonic": True, "dataPoints": [point(asInt="17")]}},
        {"name": "http.duration", "unit": "ms", "histogram": {"aggregationTemporality": 2, "dataPoints": [point(count="4", sum=10.5, min=0.5, max=6.0, bucketCounts=["1", "2", "1"], explicitBounds=[1.0, 5.0])]}},
        {"name": "http.duration.exp", "unit": "ms", "exponentialHistogram": {"aggregationTemporality": 2, "dataPoints": [point(count="5", sum=12.0, scale=2, zeroCount="1", zeroThreshold=0.0, positive={"offset": 1, "bucketCounts": ["2", "2"]}, negative={"offset": 0, "bucketCounts": []})]}},
    ]}]}]}


TABLES = ["sigiro_spans", "sigiro_logs", "sigiro_metrics_gauge", "sigiro_metrics_sum", "sigiro_metrics_histogram", "sigiro_metrics_exp_histogram"]

FIREWALL = [
    "SELECT 1",
    "SELECT count(*) FROM sigiro_spans",
    "SELECT * FROM read_csv('/etc/passwd')",
    "COPY sigiro_spans TO '/tmp/x.csv'",
    "DROP TABLE sigiro_spans",
    "INSERT INTO sigiro_logs SELECT * FROM sigiro_logs",
    "SELECT * FROM sigiro_spans AT (VERSION => 1)",
    "SELECT getenv('HOME')",
    "SELECT * FROM information_schema.tables",
    "ATTACH 'x.db' AS x",
    "SELECT 1; SELECT 2",
    "WITH t AS (SELECT 1 AS a) SELECT a FROM t",
    "SELECT json_extract_string(log_attributes, 'session.id') FROM sigiro_logs LIMIT 1",
    "PRAGMA version",
    "SELECT * FROM duckdb_settings()",
    "",
]


ID_FIELDS = {"traceId", "spanId", "parentSpanId"}


def protobuf_ids(value):
    """OTLP/JSON carries ids as hex; the protobuf JSON mapping expects base64 bytes."""
    if isinstance(value, dict):
        return {k: base64.b64encode(bytes.fromhex(v)).decode() if k in ID_FIELDS and v else protobuf_ids(v) for k, v in value.items()}
    if isinstance(value, list):
        return [protobuf_ids(v) for v in value]
    return value


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def post(url, body, content_type):
    req = urllib.request.Request(url, data=body, headers={"content-type": content_type}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def stable(body):
    try:
        value = json.loads(body)
    except ValueError:
        return body

    def strip(v):
        if isinstance(v, dict):
            return {k: strip(x) for k, x in sorted(v.items()) if not any(t in k for t in ("elapsed", "duration_ms", "took"))}
        if isinstance(v, list):
            return [strip(x) for x in v]
        return v
    return strip(value)


def main():
    binary, out_path = sys.argv[1], sys.argv[2]
    data = tempfile.mkdtemp(prefix="sigiro-core-eq-")
    http, otlp, grpc = free_port(), free_port(), free_port()
    env = {**os.environ, "SIGIRO_DATA_DIR": data, "SIGIRO_HTTP_PORT": str(http), "SIGIRO_OTLP_HTTP_PORT": str(otlp), "SIGIRO_GRPC_PORT": str(grpc), "SIGIRO_FLUSH_INTERVAL_SECS": "1", "RUST_LOG": "warn"}
    log = open(os.path.join(data, "server.log"), "w")
    server = subprocess.Popen([binary, "serve"], env=env, stdout=log, stderr=subprocess.STDOUT)
    try:
        for _ in range(240):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{http}/health", timeout=2) as r:
                    if r.status == 200:
                        break
            except (urllib.error.URLError, ConnectionError):
                time.sleep(0.5)
        else:
            raise SystemExit(f"server never became healthy; log at {log.name}")

        snapshot = {"ingest": [], "tables": {}, "firewall": []}
        sends = [
            ("traces", ExportTraceServiceRequest, traces), ("logs", ExportLogsServiceRequest, logs), ("metrics", ExportMetricsServiceRequest, metrics),
        ]
        for signal, message, build in sends:
            json_body = build("json-svc", 0)
            proto = ParseDict(protobuf_ids(build("proto-svc", 10_000)), message())
            snapshot["ingest"].append([signal, "json", *post(f"http://127.0.0.1:{otlp}/v1/{signal}", json.dumps(json_body).encode(), "application/json")])
            snapshot["ingest"].append([signal, "protobuf", *post(f"http://127.0.0.1:{otlp}/v1/{signal}", proto.SerializeToString(), "application/x-protobuf")])
            snapshot["ingest"].append([signal, "malformed", *post(f"http://127.0.0.1:{otlp}/v1/{signal}", b"\x0a\xff\xff", "application/x-protobuf")])

        expected = {"sigiro_spans": 12, "sigiro_logs": 14, "sigiro_metrics_gauge": 4, "sigiro_metrics_sum": 2, "sigiro_metrics_histogram": 2, "sigiro_metrics_exp_histogram": 2}
        for _ in range(60):
            counts = {t: stable(post(f"http://127.0.0.1:{http}/v1/query", f"SELECT count(*) AS n FROM {t}".encode(), "text/plain")[1]) for t in TABLES}
            if all(json.dumps(expected[t]) in json.dumps(counts[t]) for t in TABLES):
                break
            time.sleep(1)
        else:
            raise SystemExit(f"rows never all arrived: {counts}")

        for table in TABLES:
            status, body = post(f"http://127.0.0.1:{http}/v1/query", f"SELECT * FROM {table} ORDER BY ALL".encode(), "text/plain")
            snapshot["tables"][table] = [status, stable(body)]
        for sql in FIREWALL:
            status, body = post(f"http://127.0.0.1:{http}/v1/query", sql.encode(), "text/plain")
            snapshot["firewall"].append([sql, status, stable(body)])
        with open(out_path, "w") as f:
            json.dump(snapshot, f, indent=1, sort_keys=True)
        print(f"wrote {out_path}: {sum(len(v[1]) if isinstance(v[1], list) else 1 for v in snapshot['tables'].values())} table payload entries, {len(FIREWALL)} firewall probes")
    finally:
        server.terminate()
        server.wait(timeout=30)
        log.close()
        if os.environ.get("KEEP_DATA") != "1":
            shutil.rmtree(data, ignore_errors=True)


if __name__ == "__main__":
    main()

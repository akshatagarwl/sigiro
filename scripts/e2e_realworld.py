#!/usr/bin/env python3
"""
Real-world end-to-end test suite for Sigiro.

Simulates actual user workflows an SRE/observability engineer would perform.
Each scenario is self-contained and can be run independently.

Usage:
    python scripts/e2e_realworld.py --all          # Run all scenarios
    python scripts/e2e_realworld.py --scenario 1   # Run specific scenario
"""

import argparse
import json
import os
import random
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import Any, Optional

SIGIRO_ENDPOINT = os.environ.get("SIGIRO_ENDPOINT", "http://localhost:9999")
SIGIRO_OTLP_ENDPOINT = os.environ.get("SIGIRO_OTLP_ENDPOINT", "http://localhost:4318")
AUTH_KEY = os.environ.get("SIGIRO_API_KEY", "sigiro_dev_e2e")


@dataclass
class TestResult:
    scenario: str
    passed: bool
    message: str
    details: dict = field(default_factory=dict)


def now_us() -> int:
    return int(time.time() * 1_000_000)


def curl_api(method: str, path: str, data: Any = None, timeout: int = 60) -> tuple[int, Any]:
    """Call sigiro HTTP API. Returns (status, parsed_json_or_none)."""
    cmd = [
        "curl", "-s", "-w", "\\nHTTP_CODE:%{http_code}\\n",
        "-X", method,
        f"{SIGIRO_ENDPOINT}{path}",
        "-H", f"Authorization: Bearer {AUTH_KEY}",
        "-H", "Content-Type: application/json",
    ]
    if data is not None:
        cmd += ["-d", json.dumps(data)]

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return 0, None

    lines = result.stdout.strip().split("\n")
    http_code = 0
    for line in reversed(lines):
        if line.startswith("HTTP_CODE:"):
            http_code = int(line.split(":", 1)[1])
            break

    body = "\n".join(lines[:-1]) if lines else ""
    try:
        parsed = json.loads(body) if body else None
    except json.JSONDecodeError:
        parsed = body

    return http_code, parsed


def curl_otlp(signal_path: str, data: Any, timeout: int = 10) -> int:
    """Send OTLP data to ingestion endpoint. Returns HTTP status."""
    cmd = [
        "curl", "-s", "-o", "/dev/null", "-w", "%{http_code}",
        "-X", "POST",
        f"{SIGIRO_OTLP_ENDPOINT}{signal_path}",
        "-H", f"Authorization: Bearer {AUTH_KEY}",
        "-H", "Content-Type: application/json",
        "-d", json.dumps(data),
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return int(result.stdout.strip()) if result.stdout.strip().isdigit() else 0
    except (subprocess.TimeoutExpired, ValueError):
        return 0

def send_trace_with_spans(spans: list[dict]) -> tuple[bool, str]:
    """Send a trace with multiple related spans. Each span dict must have:
       name, service, start_time_us, duration_us.
       Optional: status_code, attributes, parent_idx (index of parent in list).
       Returns (success, trace_id)."""
    trace_id = format(random.getrandbits(128), '032x')
    span_ids = [format(random.getrandbits(64), '016x') for _ in spans]

    otlp_spans = []
    for i, spec in enumerate(spans):
        span = {
            "traceId": trace_id,
            "spanId": span_ids[i],
            "name": spec["name"],
            "kind": spec.get("kind", 2),
            "startTimeUnixNano": spec["start_time_us"] * 1000,
            "endTimeUnixNano": (spec["start_time_us"] + spec["duration_us"]) * 1000,
            "status": {"code": spec.get("status_code", 0)},
        }
        parent_idx = spec.get("parent_idx")
        if parent_idx is not None:
            span["parentSpanId"] = span_ids[parent_idx]
        if spec.get("attributes"):
            span["attributes"] = spec["attributes"]

        otlp_spans.append({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": spec["service"]}},
                    ]
                },
                "scopeSpans": [{"spans": [span]}]
            }]
        })

    # Send all spans
    for trace in otlp_spans:
        code = curl_otlp("/v1/traces", trace, timeout=10)
        if code != 200:
            return False, trace_id
    return True, trace_id
def send_otlp_trace(service: str, trace_name: str, trace_id: str = "",
                    parent_span_id: str = "", status_code: int = 0,
                    attributes: list = None, start_time_us: int = None,
                    duration_us: int = 1000) -> tuple[bool, str]:
    """Send a single OTLP trace via HTTP JSON. Returns (success, span_id)."""
    if not trace_id:
        trace_id = format(random.getrandbits(128), '032x')
    span_id = format(random.getrandbits(64), '016x')
    if start_time_us is None:
        start_time_us = now_us()
    end_time_us = start_time_us + duration_us

    span = {
        "traceId": trace_id,
        "spanId": span_id,
        "name": trace_name,
        "kind": 2,
        "startTimeUnixNano": start_time_us * 1000,
        "endTimeUnixNano": end_time_us * 1000,
        "status": {"code": status_code},
    }
    if parent_span_id:
        span["parentSpanId"] = parent_span_id
    if attributes:
        span["attributes"] = attributes

    trace = {
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": service}},
                    {"key": "service.version", "value": {"stringValue": "2.3.1"}},
                    {"key": "deployment.environment", "value": {"stringValue": "production"}},
                ]
            },
            "scopeSpans": [{"spans": [span]}]
        }]
    }

    code = curl_otlp("/v1/traces", trace, timeout=10)
    return code == 200, span_id


def send_otlp_log(service: str, body: str, severity: int = 9,
                  trace_id: str = "", span_id: str = "",
                  timestamp_us: int = None) -> bool:
    """Send a single OTLP log via HTTP JSON."""
    if timestamp_us is None:
        timestamp_us = now_us()

    log = {
        "resourceLogs": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": service}},
                ]
            },
            "scopeLogs": [{
                "logRecords": [{
                    "timeUnixNano": timestamp_us * 1000,
                    "severityNumber": severity,
                    "severityText": "INFO" if severity < 13 else "ERROR",
                    "body": {"stringValue": body},
                    **({"traceId": trace_id} if trace_id else {}),
                    **({"spanId": span_id} if span_id else {}),
                }]
            }]
        }]
    }

    code = curl_otlp("/v1/logs", log, timeout=10)
    return code == 200


def send_otlp_metric(service: str, metric_name: str, value: float,
                     timestamp_us: int = None) -> bool:
    """Send a single gauge metric via HTTP JSON."""
    if timestamp_us is None:
        timestamp_us = now_us()

    metric = {
        "resourceMetrics": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": service}},
                ]
            },
            "scopeMetrics": [{
                "metrics": [{
                    "name": metric_name,
                    "gauge": {
                        "dataPoints": [{
                            "timeUnixNano": timestamp_us * 1000,
                            "asDouble": value,
                        }]
                    }
                }]
            }]
        }]
    }

    code = curl_otlp("/v1/metrics", metric, timeout=10)
    return code == 200


def investigate(service: str, from_ts: int, to_ts: int) -> Optional[dict]:
    """Call investigate endpoint."""
    code, body = curl_api("POST", "/v1/investigate",
                          {"service": service, "from_ts": from_ts, "to_ts": to_ts},
                          timeout=120)
    if code == 200 and isinstance(body, dict):
        return body
    return None


def query(sql: str) -> Optional[list]:
    """Run raw SQL query."""
    code, body = curl_api("POST", "/v1/query", sql, timeout=60)
    if code == 200 and isinstance(body, list):
        return body
    return None


# ---------------------------------------------------------------------------
# Scenario 1: Multi-service incident with cascading errors
# A request hits gateway -> calls payment-service (fails)
# User investigates payment-service to find the root cause.
# ---------------------------------------------------------------------------
def scenario_cascading_errors() -> TestResult:
    print("\n[Scenario 1] Cascading error: gateway -> payment (fails)")
    base_time = now_us() - 60_000_000

    ok, trace_id = send_trace_with_spans([
        {"name": "POST /checkout", "service": "gateway",
         "start_time_us": base_time, "duration_us": 150_000, "status_code": 1,
         "attributes": [
             {"key": "http.method", "value": {"stringValue": "POST"}},
             {"key": "http.route", "value": {"stringValue": "/checkout"}},
             {"key": "http.status_code", "value": {"intValue": "200"}},
         ]},
        {"name": "process_payment", "service": "payment-service",
         "start_time_us": base_time + 10_000, "duration_us": 50_000, "status_code": 2,
         "parent_idx": 0, "attributes": [
             {"key": "error.type", "value": {"stringValue": "ConnectionRefused"}},
             {"key": "error.message", "value": {"stringValue": "connection refused to payment-db:5432"}},
         ]},
    ])
    # Add error log from payment service
    send_otlp_log("payment-service",
                  "connection refused to payment-db:5432 - retrying...",
                  severity=17, trace_id=trace_id,
                  timestamp_us=base_time + 20_000)

    # Also send some successful inventory spans (unrelated)
    for i in range(3):
        send_otlp_trace("inventory-service", "check_stock",
                        status_code=1,
                        start_time_us=base_time + i * 30_000, duration_us=20_000)

    time.sleep(3)

    # Investigate payment-service (where the actual error is)
    result = investigate("payment-service", base_time - 30_000_000, base_time + 120_000_000)
    if not result:
        return TestResult("cascading_errors", False, "Investigate returned no data")

    block = result.get("payment-service", {})
    traces = block.get("traces", [])
    ops = block.get("operation_summaries", [])
    logs = block.get("logs", [])

    details = {
        "traces_count": len(traces),
        "ops_count": len(ops),
        "logs_count": len(logs),
        "error_ops": [op for op in ops if op.get("error_count", 0) > 0],
    }

    error_ops = [op for op in ops if op.get("error_count", 0) > 0]
    passed = len(error_ops) > 0
    msg = f"Found {len(traces)} traces, {len(error_ops)} ops with errors, {len(logs)} logs"
    return TestResult("cascading_errors", passed, msg, details)


# ---------------------------------------------------------------------------
# Scenario 2: Deployment correlation - error spike after version change
# ---------------------------------------------------------------------------
def scenario_deployment_correlation() -> TestResult:
    print("\n[Scenario 2] Deployment correlation: error spike after deploy")
    base_time = now_us() - 120_000_000

    # Phase 1: Stable period with v2.3.0, 1% error rate
    for i in range(20):
        send_otlp_trace("orders-service", "place_order",
                        status_code=0 if random.random() > 0.01 else 2,
                        start_time_us=base_time + i * 3_000_000,
                        duration_us=random.randint(50_000, 150_000),
                        attributes=[
                            {"key": "service.version", "value": {"stringValue": "2.3.0"}},
                        ])

    # Phase 2: Deploy v2.3.1
    deploy_time = base_time + 70_000_000
    send_otlp_trace("orders-service", "deploy_v2.3.1",
                    status_code=1,
                    start_time_us=deploy_time, duration_us=30_000_000,
                    attributes=[
                        {"key": "service.version", "value": {"stringValue": "2.3.1"}},
                        {"key": "deployment.id", "value": {"stringValue": "deploy-abc123"}},
                    ])

    # Phase 3: After deploy, 30% error rate
    for i in range(20):
        send_otlp_trace("orders-service", "place_order",
                        status_code=0 if random.random() > 0.30 else 2,
                        start_time_us=deploy_time + 5_000_000 + i * 2_000_000,
                        duration_us=random.randint(50_000, 500_000),
                        attributes=[
                            {"key": "service.version", "value": {"stringValue": "2.3.1"}},
                        ])

    time.sleep(3)

    result = investigate("orders-service", base_time - 10_000_000, deploy_time + 60_000_000)
    if not result:
        return TestResult("deployment_correlation", False, "Investigate returned no data")

    block = result.get("orders-service", {})
    ops = block.get("operation_summaries", [])
    events = block.get("events", [])

    place_order_op = next((op for op in ops if op.get("span_name") == "place_order"), None)

    details = {
        "ops_count": len(ops),
        "events_count": len(events),
        "place_order_count": place_order_op.get("count", 0) if place_order_op else 0,
        "place_order_errors": place_order_op.get("error_count", 0) if place_order_op else 0,
        "place_order_error_pct": place_order_op.get("error_pct", 0) if place_order_op else 0,
    }

    passed = place_order_op is not None and place_order_op.get("error_count", 0) > 3
    msg = f"place_order: {details['place_order_count']} spans, {details['place_order_errors']} errors ({details['place_order_error_pct']:.1f}%)"
    return TestResult("deployment_correlation", passed, msg, details)


# ---------------------------------------------------------------------------
# Scenario 3: Cross-service trace following
# ---------------------------------------------------------------------------
def scenario_cross_service_trace() -> TestResult:
    print("\n[Scenario 3] Cross-service trace following")
    base_time = now_us() - 60_000_000

    ok, trace_id = send_trace_with_spans([
        {"name": "GET /api/users/123", "service": "api-gateway",
         "start_time_us": base_time, "duration_us": 200_000, "status_code": 1},
        {"name": "get_user_profile", "service": "user-service",
         "start_time_us": base_time + 10_000, "duration_us": 100_000, "status_code": 1,
         "parent_idx": 0},
        {"name": "validate_token", "service": "auth-service",
         "start_time_us": base_time + 20_000, "duration_us": 50_000, "status_code": 2,
         "parent_idx": 1, "attributes": [
             {"key": "error.type", "value": {"stringValue": "TokenExpired"}},
         ]},
    ])
    time.sleep(3)

    code, trace_detail = curl_api("GET", f"/v1/traces/{trace_id}", timeout=30)
    if code != 200 or not isinstance(trace_detail, dict):
        return TestResult("cross_service_trace", False,
                          f"Trace detail failed: HTTP {code}")

    spans = trace_detail.get("spans", [])
    services = trace_detail.get("services", [])
    critical_path = trace_detail.get("critical_path", [])

    details = {
        "spans_count": len(spans),
        "services": services,
        "critical_path_len": len(critical_path),
        "error_count": trace_detail.get("error_count", 0),
    }

    passed = (len(spans) >= 3 and
              "api-gateway" in services and
              "user-service" in services and
              "auth-service" in services and
              trace_detail.get("error_count", 0) > 0)

    msg = f"Trace {trace_id[:16]}...: {len(spans)} spans across {len(services)} services, {details['error_count']} errors"
    return TestResult("cross_service_trace", passed, msg, details)


# ---------------------------------------------------------------------------
# Scenario 4: High-throughput stress test
# ---------------------------------------------------------------------------
def scenario_high_throughput() -> TestResult:
    print("\n[Scenario 4] High-throughput stress test (5000 spans)")
    base_time = now_us() - 30_000_000

    total = 5000
    batch_size = 50
    success = 0
    start = time.time()

    for batch_idx in range(total // batch_size):
        trace_id = format(random.getrandbits(128), '032x')
        spans = []
        for i in range(batch_size):
            span_id = format(random.getrandbits(64), '016x')
            spans.append({
                "traceId": trace_id,
                "spanId": span_id,
                "name": f"stress-span-{batch_idx}-{i}",
                "kind": 1,
                "startTimeUnixNano": (base_time + batch_idx * 1000) * 1000,
                "endTimeUnixNano": (base_time + batch_idx * 1000 + 500) * 1000,
                "status": {"code": 0},
            })

        trace = {
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": "stress-service"}},
                    ]
                },
                "scopeSpans": [{"spans": spans}]
            }]
        }

        code = curl_otlp("/v1/traces", trace, timeout=10)
        if code == 200:
            success += batch_size

    elapsed = time.time() - start
    throughput = success / elapsed if elapsed > 0 else 0

    time.sleep(3)

    result = investigate("stress-service", base_time - 10_000_000, base_time + 60_000_000)
    if not result:
        return TestResult("high_throughput", False,
                          f"Sent {success}/{total} spans at {throughput:.0f}/s but no data",
                          {"sent": success, "throughput": throughput})

    block = result.get("stress-service", {})
    ops_count = len(block.get("operation_summaries", []))

    details = {
        "sent": success,
        "target": total,
        "throughput": round(throughput, 0),
        "elapsed_sec": round(elapsed, 1),
        "ops_found": ops_count,
    }

    passed = success >= total * 0.8 and ops_count > 0
    msg = f"Sent {success}/{total} spans ({throughput:.0f}/s), found {ops_count} operation types"
    return TestResult("high_throughput", passed, msg, details)


# ---------------------------------------------------------------------------
# Scenario 5: Log pattern deduplication
# ---------------------------------------------------------------------------
def scenario_log_patterns() -> TestResult:
    print("\n[Scenario 5] Log pattern deduplication")
    base_time = now_us() - 60_000_000

    error_patterns = [
        ("Connection refused to payment-db:5432", 17),
        ("Timeout waiting for inventory-cache response after 5000ms", 17),
        ("Rate limit exceeded: 429 from auth-service", 13),
    ]

    for i in range(100):
        pattern, severity = error_patterns[i % 3]
        body = pattern + f" (req_id={format(random.getrandbits(32), '08x')})"
        send_otlp_log("notification-service", body, severity=severity,
                      timestamp_us=base_time + i * 500_000)

    time.sleep(3)

    result = investigate("notification-service", base_time - 10_000_000, base_time + 60_000_000)
    if not result:
        return TestResult("log_patterns", False, "Investigate returned no data")

    block = result.get("notification-service", {})
    patterns = block.get("log_patterns", [])
    logs = block.get("logs", [])

    details = {
        "patterns_count": len(patterns),
        "raw_logs_count": len(logs),
        "top_patterns": [p.get("template", "")[:60] for p in patterns[:3]],
    }

    passed = len(patterns) <= 10 and len(logs) <= 50
    msg = f"{len(patterns)} patterns from 100 logs, {len(logs)} raw logs shown"
    return TestResult("log_patterns", passed, msg, details)


# ---------------------------------------------------------------------------
# Scenario 6: Metric + trace + log correlation
# ---------------------------------------------------------------------------
def scenario_signal_correlation() -> TestResult:
    print("\n[Scenario 6] Metric + trace + log correlation")
    base_time = now_us() - 60_000_000

    service = "correlation-service"

    for i in range(10):
        send_otlp_metric(service, "http.request.duration", random.uniform(50, 200),
                         timestamp_us=base_time + i * 5_000_000)
        send_otlp_metric(service, "http.request.errors", 1 if i > 7 else 0,
                         timestamp_us=base_time + i * 5_000_000)

    for i in range(15):
        send_otlp_trace(service, "GET /api/data",
                        status_code=0 if i < 12 else 2,
                        start_time_us=base_time + i * 3_000_000,
                        duration_us=random.randint(50_000, 300_000))

    for i in range(10):
        send_otlp_log(service,
                      f"Request processed in {random.randint(50, 300)}ms",
                      severity=9,
                      timestamp_us=base_time + i * 5_000_000)

    time.sleep(3)

    result = investigate(service, base_time - 10_000_000, base_time + 60_000_000)
    if not result:
        return TestResult("signal_correlation", False, "Investigate returned no data")

    block = result.get(service, {})
    coverage = block.get("coverage", {})
    signals = coverage.get("signals", [])

    details = {
        "metrics_count": len(block.get("metrics", [])),
        "traces_count": len(block.get("traces", [])),
        "logs_count": len(block.get("logs", [])),
        "coverage_signals": {s.get("signal_type"): s.get("available") for s in signals},
    }

    spans_available = any(s.get("signal_type") == "spans" and s.get("available") for s in signals)
    logs_available = any(s.get("signal_type") == "logs" and s.get("available") for s in signals)
    metrics_available = any(s.get("signal_type") == "metrics" and s.get("available") for s in signals)

    passed = spans_available and logs_available and metrics_available
    msg = f"Spans: {details['traces_count']} traces, Logs: {details['logs_count']} entries, Metrics: {details['metrics_count']} buckets"
    return TestResult("signal_correlation", passed, msg, details)


# ---------------------------------------------------------------------------
# Scenario 7: Mandatory from/to rejection
# ---------------------------------------------------------------------------
def scenario_mandatory_timestamps() -> TestResult:
    print("\n[Scenario 7] Mandatory from/to timestamp validation")

    code1, _ = curl_api("POST", "/v1/investigate",
                        {"service": "test"}, timeout=10)

    code2, _ = curl_api("POST", "/v1/investigate",
                        {"service": "test", "from_ts": 0, "to_ts": 0}, timeout=10)

    code3, _ = curl_api("POST", "/v1/investigate",
                        {"service": "sigiro", "from_ts": now_us() - 300_000_000,
                         "to_ts": now_us()}, timeout=10)

    details = {
        "missing_ts_code": code1,
        "zero_ts_code": code2,
        "valid_ts_code": code3,
    }

    passed = code1 in (400, 422) and code2 == 400 and code3 == 200
    msg = f"Missing ts: {code1}, Zero ts: {code2}, Valid ts: {code3}"
    return TestResult("mandatory_timestamps", passed, msg, details)


# ---------------------------------------------------------------------------
# Scenario 8: Trace-tree CLI rendering
# ---------------------------------------------------------------------------
def scenario_trace_tree() -> TestResult:
    print("\n[Scenario 8] Trace-tree CLI rendering")
    base_time = now_us() - 60_000_000

    ok, trace_id = send_trace_with_spans([
        {"name": "POST /order", "service": "gateway",
         "start_time_us": base_time, "duration_us": 300_000, "status_code": 1},
        {"name": "create_order", "service": "orders",
         "start_time_us": base_time + 10_000, "duration_us": 200_000, "status_code": 1,
         "parent_idx": 0},
        {"name": "charge_card", "service": "payments",
         "start_time_us": base_time + 20_000, "duration_us": 100_000, "status_code": 2,
         "parent_idx": 1},
        {"name": "validate_fraud", "service": "payments",
         "start_time_us": base_time + 30_000, "duration_us": 50_000, "status_code": 1,
         "parent_idx": 2},
    ])
    time.sleep(3)

    code, detail = curl_api("GET", f"/v1/traces/{trace_id}", timeout=30)
    if code != 200:
        return TestResult("trace_tree", False, f"Trace detail failed: HTTP {code}")

    spans = detail.get("spans", [])
    root = detail.get("root_span", {})
    critical_path = detail.get("critical_path", [])
    services = detail.get("services", [])

    details = {
        "spans_count": len(spans),
        "root_span_name": root.get("span_name", ""),
        "critical_path_len": len(critical_path),
        "services": services,
        "error_count": detail.get("error_count", 0),
    }

    passed = (len(spans) == 4 and
              len(services) >= 2 and
              detail.get("error_count", 0) > 0)

    msg = f"{len(spans)} spans, root='{root.get('span_name')}', {len(critical_path)} critical path, {detail.get('error_count')} errors"
    return TestResult("trace_tree", passed, msg, details)

# ---------------------------------------------------------------------------
# Scenario 9: Self-instrumentation investigation
# ---------------------------------------------------------------------------
def scenario_self_instrumentation() -> TestResult:
    print("\n[Scenario 9] Self-instrumentation investigation")

    result = investigate("sigiro", now_us() - 300_000_000, now_us())
    if not result:
        return TestResult("self_instrumentation", False, "No self-instrumentation data")

    block = result.get("sigiro", {})
    traces = block.get("traces", [])
    ops = block.get("operation_summaries", [])
    coverage = block.get("coverage", {})

    details = {
        "traces_count": len(traces),
        "ops_count": len(ops),
        "coverage_signals": coverage.get("signals", []),
    }

    passed = len(traces) > 0 or len(ops) > 0
    msg = f"Found {len(traces)} traces, {len(ops)} operations from sigiro self-instrumentation"
    return TestResult("self_instrumentation", passed, msg, details)


# ---------------------------------------------------------------------------
# Main runner
# ---------------------------------------------------------------------------
SCENARIOS = [
    scenario_cascading_errors,
    scenario_deployment_correlation,
    scenario_cross_service_trace,
    scenario_high_throughput,
    scenario_log_patterns,
    scenario_signal_correlation,
    scenario_mandatory_timestamps,
    scenario_trace_tree,
    scenario_self_instrumentation,
]


def run_all():
    print("=" * 60)
    print("SIGIRO REAL-WORLD E2E TEST SUITE")
    print(f"Query: {SIGIRO_ENDPOINT}")
    print(f"OTLP:  {SIGIRO_OTLP_ENDPOINT}")
    print("=" * 60)

    results = []
    for scenario_fn in SCENARIOS:
        try:
            result = scenario_fn()
            results.append(result)
        except Exception as e:
            results.append(TestResult(
                scenario_fn.__name__,
                False,
                f"Exception: {e}",
                {}
            ))
            import traceback
            traceback.print_exc()

    print("\n" + "=" * 60)
    print("RESULTS")
    print("=" * 60)

    passed = sum(1 for r in results if r.passed)
    failed = sum(1 for r in results if not r.passed)

    for r in results:
        status = "PASS" if r.passed else "FAIL"
        print(f"  [{status}] {r.scenario:30s} — {r.message}")
        if r.details:
            for k, v in r.details.items():
                print(f"           {k}: {v}")

    print("=" * 60)
    print(f"Total: {len(results)} scenarios | {passed} passed | {failed} failed")
    print("=" * 60)

    return 0 if failed == 0 else 1


def main():
    parser = argparse.ArgumentParser(description="Sigiro real-world E2E tests")
    parser.add_argument("--all", action="store_true", help="Run all scenarios")
    parser.add_argument("--scenario", type=int, help="Run specific scenario (1-9)")
    args = parser.parse_args()

    if args.scenario:
        idx = args.scenario - 1
        if 0 <= idx < len(SCENARIOS):
            result = SCENARIOS[idx]()
            print(f"\nResult: {'PASS' if result.passed else 'FAIL'} — {result.message}")
            sys.exit(0 if result.passed else 1)
        else:
            print(f"Invalid scenario: {args.scenario}. Range: 1-{len(SCENARIOS)}")
            sys.exit(1)

    sys.exit(run_all())


if __name__ == "__main__":
    main()

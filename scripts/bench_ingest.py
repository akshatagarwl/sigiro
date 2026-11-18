#!/usr/bin/env python3
"""
Sigiro ingestion benchmark.

Generates synthetic OTLP traces and sends them to sigiro's OTLP HTTP endpoint,
measuring throughput and end-to-end latency.

Usage:
    python scripts/bench_ingest.py --rate 1000 --duration 60
    python scripts/bench_ingest.py --rate 10000 --services 5 --trace-depth 3

Requires: pip install requests opentelemetry-proto
"""

from __future__ import annotations

import argparse
import json
import random
import string
import time
import sys
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import List

def ensure_otlp_imports() -> None:
    global ResourceSpans, ScopeSpans, OtlpSpan, TracesData
    global OtlpResource, KeyValue, AnyValue, InstrumentationScope

    try:
        from opentelemetry.proto.common.v1.common_pb2 import (
            AnyValue,
            InstrumentationScope,
            KeyValue,
        )
        from opentelemetry.proto.resource.v1.resource_pb2 import Resource as OtlpResource
        from opentelemetry.proto.trace.v1.trace_pb2 import (
            ResourceSpans,
            ScopeSpans,
            Span as OtlpSpan,
            TracesData,
        )
    except ImportError:
        print("ERROR: opentelemetry-proto not installed.")
        print("Run: pip install opentelemetry-proto")
        sys.exit(1)


def random_id(length: int = 16) -> str:
    return "".join(random.choices(string.hexdigits.lower(), k=length))


def make_span(
    trace_id: str,
    span_id: str,
    parent_span_id: str,
    name: str,
    service: str,
    duration_us: int = 1000,
) -> OtlpSpan:
    span = OtlpSpan()
    span.trace_id = bytes.fromhex(trace_id)
    span.span_id = bytes.fromhex(span_id)
    if parent_span_id:
        span.parent_span_id = bytes.fromhex(parent_span_id)
    span.name = name
    span.kind = 1  # SPAN_KIND_INTERNAL
    span.start_time_unix_nano = int(time.time() * 1e9)
    span.end_time_unix_nano = span.start_time_unix_nano + duration_us * 1000
    span.status.code = 0  # STATUS_CODE_UNSET

    # Add some attributes
    span.attributes.append(KeyValue(key="http.method", value=AnyValue(string_value="GET")))
    span.attributes.append(KeyValue(key="http.route", value=AnyValue(string_value="/api/v1/test")))
    span.attributes.append(KeyValue(key="service.name", value=AnyValue(string_value=service)))

    return span


def make_trace(
    service: str,
    depth: int = 1,
    num_spans: int = 1,
) -> ResourceSpans:
    trace_id = random_id(32)
    resource_spans = ResourceSpans()

    resource = OtlpResource()
    resource.attributes.append(
        KeyValue(key="service.name", value=AnyValue(string_value=service))
    )
    resource.attributes.append(
        KeyValue(key="service.version", value=AnyValue(string_value="1.0.0"))
    )
    resource.attributes.append(
        KeyValue(key="deployment.environment", value=AnyValue(string_value="production"))
    )
    resource_spans.resource.CopyFrom(resource)

    scope_spans = ScopeSpans()
    scope = InstrumentationScope()
    scope.name = "bench-generator"
    scope.version = "1.0.0"
    scope_spans.scope.CopyFrom(scope)

    # Generate spans for this trace
    parent_ids = [""]  # root has no parent
    for i in range(num_spans):
        span_id = random_id(16)
        parent = random.choice(parent_ids)
        name = f"span-{i}"
        duration = random.randint(100, 10000)  # 0.1ms to 10ms
        span = make_span(trace_id, span_id, parent, name, service, duration)
        scope_spans.spans.append(span)
        parent_ids.append(span_id)

    resource_spans.scope_spans.append(scope_spans)
    return resource_spans


def make_batch(
    services: List[str],
    spans_per_trace: int = 5,
    traces_per_batch: int = 10,
) -> bytes:
    traces_data = TracesData()
    for _ in range(traces_per_batch):
        service = random.choice(services)
        resource_spans = make_trace(service, num_spans=spans_per_trace)
        traces_data.resource_spans.append(resource_spans)
    return traces_data.SerializeToString()


def send_batch(endpoint: str, batch: bytes, access_token: str | None) -> tuple[float, bool]:
    import requests

    headers = {"Content-Type": "application/x-protobuf"}
    if access_token:
        headers["Authorization"] = f"Bearer {access_token}"
    start = time.time()
    resp = requests.post(
        f"{endpoint}/v1/traces",
        data=batch,
        headers=headers,
        timeout=30,
    )
    latency = time.time() - start
    ok = resp.status_code == 200
    if not ok:
        print(f"  HTTP {resp.status_code}: {resp.text[:200]}")
    return latency, ok

@dataclass
class BenchmarkResult:
    total_spans: int
    total_batches: int
    elapsed_sec: float
    throughput: float  # spans/sec
    p50_latency_ms: float
    p95_latency_ms: float
    p99_latency_ms: float
    errors: int


def run_benchmark(args) -> BenchmarkResult:
    ensure_otlp_imports()

    services = [f"service-{i}" for i in range(args.services)]
    total_spans_target = args.rate * args.duration
    spans_per_batch = args.spans_per_trace * args.traces_per_batch
    total_batches = total_spans_target // spans_per_batch

    print(f"Benchmark config:")
    print(f"  Endpoint: {args.endpoint}")
    print(f"  Target rate: {args.rate} spans/sec")
    print(f"  Duration: {args.duration} sec")
    print(f"  Services: {args.services}")
    print(f"  Spans/trace: {args.spans_per_trace}")
    print(f"  Traces/batch: {args.traces_per_batch}")
    print(f"  Total batches: {total_batches}")
    print()

    latencies: List[float] = []
    errors = 0
    start_time = time.time()
    spans_sent = 0

    # Pre-generate all batches
    print("Generating batches...")
    batches = [make_batch(services, args.spans_per_trace, args.traces_per_batch) for _ in range(total_batches)]
    print(f"Generated {len(batches)} batches ({len(batches) * spans_per_batch} spans)")
    print()

    # Send at target rate
    print("Sending...")
    batch_interval = 1.0 / (args.rate / spans_per_batch)
    next_send = start_time

    for i, batch in enumerate(batches):
        now = time.time()
        if now < next_send:
            time.sleep(next_send - now)
        next_send += batch_interval

        try:
            latency, ok = send_batch(args.endpoint, batch, args.access_token)
            latencies.append(latency)
            spans_sent += spans_per_batch
            if not ok:
                errors += 1
        except Exception as e:
            errors += 1
            print(f"  Batch {i} failed: {e}")

        if (i + 1) % 100 == 0:
            elapsed = time.time() - start_time
            current_rate = spans_sent / elapsed
            print(f"  Sent {spans_sent} spans in {elapsed:.1f}s ({current_rate:.0f} spans/sec)")

    elapsed = time.time() - start_time

    # Compute percentiles
    if latencies:
        latencies.sort()
        p50 = latencies[len(latencies) // 2] * 1000
        p95 = latencies[int(len(latencies) * 0.95)] * 1000
        p99 = latencies[int(len(latencies) * 0.99)] * 1000
    else:
        p50 = p95 = p99 = 0.0

    throughput = spans_sent / elapsed if elapsed > 0 else 0

    return BenchmarkResult(
        total_spans=spans_sent,
        total_batches=total_batches,
        elapsed_sec=elapsed,
        throughput=throughput,
        p50_latency_ms=p50,
        p95_latency_ms=p95,
        p99_latency_ms=p99,
        errors=errors,
    )


def main():
    parser = argparse.ArgumentParser(description="Sigiro ingestion benchmark")
    parser.add_argument("--endpoint", default="http://localhost:4318", help="OTLP HTTP endpoint")
    parser.add_argument("--rate", type=int, default=1000, help="Target spans/sec")
    parser.add_argument("--duration", type=int, default=60, help="Duration in seconds")
    parser.add_argument("--services", type=int, default=3, help="Number of services")
    parser.add_argument("--spans-per-trace", type=int, default=5, help="Spans per trace")
    parser.add_argument("--traces-per-batch", type=int, default=10, help="Traces per batch")
    parser.add_argument("--access-token", help="OAuth access token for hosted Sigiro")
    args = parser.parse_args()

    result = run_benchmark(args)

    print()
    print("=" * 50)
    print("RESULTS")
    print("=" * 50)
    print(f"Total spans sent:   {result.total_spans:,}")
    print(f"Total batches:      {result.total_batches:,}")
    print(f"Elapsed time:       {result.elapsed_sec:.2f}s")
    print(f"Throughput:         {result.throughput:,.0f} spans/sec")
    print(f"Latency p50:        {result.p50_latency_ms:.2f}ms")
    print(f"Latency p95:        {result.p95_latency_ms:.2f}ms")
    print(f"Latency p99:        {result.p99_latency_ms:.2f}ms")
    print(f"Errors:             {result.errors}")
    print()

    # Save results
    results_file = f"bench_result_{int(time.time())}.json"
    with open(results_file, "w") as f:
        json.dump(
            {
                "config": vars(args),
                "results": {
                    "total_spans": result.total_spans,
                    "total_batches": result.total_batches,
                    "elapsed_sec": result.elapsed_sec,
                    "throughput": result.throughput,
                    "p50_latency_ms": result.p50_latency_ms,
                    "p95_latency_ms": result.p95_latency_ms,
                    "p99_latency_ms": result.p99_latency_ms,
                    "errors": result.errors,
                },
            },
            f,
            indent=2,
        )
    print(f"Results saved to: {results_file}")


if __name__ == "__main__":
    main()

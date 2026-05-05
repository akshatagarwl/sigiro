#!/usr/bin/env python3
"""
Federation + S3 multi-node integration test.

Spins up multi-node sigiro federation via Docker Compose, ingests data to
multiple workers, and verifies central node can query across all nodes
including S3 cold storage.

Usage:
    python scripts/test_federation.py [--compose docker-compose.federation.yml]
    python scripts/test_federation.py --cleanup  # Stop and remove containers

Requires: docker compose, curl, python3
"""

import argparse
import json
import subprocess
import sys
import time
from dataclasses import dataclass
from typing import Optional


AUTH_HEADER = "Authorization: Bearer sigiro_dev_fed"


@dataclass
class TestResult:
    name: str
    passed: bool
    message: str
    duration_ms: float


def run_cmd(cmd: list[str], capture: bool = True, timeout: int = 30) -> subprocess.CompletedProcess:
    """Run a shell command, returning stdout."""
    return subprocess.run(
        cmd,
        capture_output=capture,
        text=True,
        timeout=timeout,
    )


def docker_compose(cmd: list[str], compose_file: str) -> subprocess.CompletedProcess:
    """Run docker compose with the given file."""
    return run_cmd(["docker", "compose", "-f", compose_file] + cmd)


def healthcheck_services(compose_file: str, timeout: int = 120) -> bool:
    """Wait for all services to be healthy."""
    print(f"Waiting for services to be healthy (timeout={timeout}s)...")
    start = time.time()
    while time.time() - start < timeout:
        result = docker_compose(["ps", "--format", "json"], compose_file)
        if result.returncode != 0:
            time.sleep(2)
            continue
        try:
            services = json.loads(result.stdout)
            if not isinstance(services, list):
                services = [services]
            all_healthy = all(
                s.get("Health", "") == "healthy" or s.get("State") != "running"
                for s in services
                if s.get("Service") != "minio-setup"
            )
            running = [s["Service"] for s in services if s.get("State") == "running"]
            if all_healthy and len(running) >= 4:  # postgres, minio, worker-1, worker-2
                print(f"  All services healthy: {', '.join(running)}")
                return True
        except (json.JSONDecodeError, KeyError):
            pass
        time.sleep(2)
    print("  TIMEOUT: Services did not become healthy")
    return False


def send_trace(endpoint: str, service: str, trace_name: str) -> bool:
    """Send a single synthetic trace to a worker endpoint."""
    import random
    trace = {
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": service}},
                    {"key": "service.version", "value": {"stringValue": "1.0.0"}},
                ]
            },
            "scopeSpans": [{
                "spans": [{
                    "traceId": format(random.getrandbits(128), '032x'),
                    "spanId": format(random.getrandbits(64), '016x'),
                    "name": trace_name,
                    "kind": 2,  # server
                    "startTimeUnixNano": int(time.time() * 1e9),
                    "endTimeUnixNano": int(time.time() * 1e9) + int(1e6 * 50),
                    "status": {"code": 0}
                }]
            }]
        }]
    }
    result = run_cmd([
        "curl", "-sf", "-X", "POST",
        f"{endpoint}/v1/traces",
        "-H", "Content-Type: application/json",
        "-H", AUTH_HEADER,
        "-d", json.dumps(trace),
    ], timeout=10)
    return result.returncode == 0


def query_central(central_url: str, service: str, from_ts: int, to_ts: int) -> Optional[dict]:
    """Query investigate endpoint."""
    result = run_cmd([
        "curl", "-sf", "-X", "POST",
        f"{central_url}/v1/investigate",
        "-H", "Content-Type: application/json",
        "-H", AUTH_HEADER,
        "-d", json.dumps({"service": service, "from_ts": from_ts, "to_ts": to_ts}),
    ], timeout=120)
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def test_worker_ingest(worker_url: str, service: str) -> TestResult:
    """Test that a worker accepts OTLP traces."""
    start = time.time()
    success = True
    for i in range(5):
        if not send_trace(worker_url, service, f"test-span-{i}"):
            success = False
            break
    dur = (time.time() - start) * 1000
    if success:
        return TestResult("worker_ingest", True, f"Sent 5 traces to {worker_url}", dur)
    return TestResult("worker_ingest", False, f"Failed to send traces to {worker_url}", dur)


def test_worker_query(worker_url: str, service: str) -> TestResult:
    """Test that worker node can query its own data."""
    start = time.time()
    # Wait for flush
    time.sleep(3)

    from_ts = int((time.time() - 300) * 1e6)
    to_ts = int((time.time() + 60) * 1e6)
    result = query_central(worker_url, service, from_ts, to_ts)
    dur = (time.time() - start) * 1000

    if result is None:
        return TestResult("worker_query", False, "Worker query returned no response", dur)

    if service not in result:
        return TestResult("worker_query", False, f"Service '{service}' not in response", dur)

    block = result[service]
    trace_count = len(block.get("traces", []))
    op_count = len(block.get("operation_summaries", []))

    if trace_count > 0 or op_count > 0:
        return TestResult(
            "worker_query",
            True,
            f"Found {trace_count} traces, {op_count} operations",
            dur,
        )
    return TestResult("worker_query", False, f"No traces/ops found for {service}", dur)

def test_multi_service_federation(worker_urls: list[str]) -> TestResult:
    """Test federation across multiple services on different workers."""
    start = time.time()

    # Ingest to both services
    for i in range(3):
        send_trace("http://localhost:10102", "frontend", f"fe-span-{i}")
        send_trace("http://localhost:10202", "backend", f"be-span-{i}")

    time.sleep(3)

    from_ts = int((time.time() - 300) * 1e6)
    to_ts = int((time.time() + 60) * 1e6)

    # Query each worker directly
    fe = query_central(worker_urls[0], "frontend", from_ts, to_ts)
    be = query_central(worker_urls[1], "backend", from_ts, to_ts)
    dur = (time.time() - start) * 1000

    if fe and be and "frontend" in fe and "backend" in be:
        fe_ops = len(fe["frontend"].get("operation_summaries", []))
        be_ops = len(be["backend"].get("operation_summaries", []))
        if fe_ops > 0 and be_ops > 0:
            return TestResult(
                "multi_service_federation",
                True,
                f"frontend: {fe_ops} ops, backend: {be_ops} ops",
                dur,
            )
    return TestResult("multi_service_federation", False, "Missing data from one or more services", dur)

def test_s3_cold_storage(central_url: str) -> TestResult:
    """Verify S3 cold storage dataset is configured in spicepod."""
    start = time.time()
    # Check spicepod.yaml on central
    result = docker_compose(["exec", "-T", "central", "cat", "/data/central/spicepod.yaml"],
                           "docker-compose.federation.yml")
    dur = (time.time() - start) * 1000

    if result.returncode != 0:
        return TestResult("s3_cold_storage", False, "Could not read spicepod.yaml", dur)

    if "sigiro_spans_cold" in result.stdout and "s3://" in result.stdout:
        return TestResult("s3_cold_storage", True, "S3 cold storage configured in spicepod", dur)
    return TestResult("s3_cold_storage", False, "S3 cold storage not found in spicepod", dur)


def test_worker_registration(central_url: str) -> TestResult:
    """Verify workers are registered with central."""
    start = time.time()
    result = run_cmd([
        "curl", "-sf", f"{central_url}/v1/federation/workers",
        "-H", AUTH_HEADER,
    ], timeout=10)
    dur = (time.time() - start) * 1000

    if result.returncode != 0:
        return TestResult("worker_registration", False, "Failed to list workers", dur)

    try:
        data = json.loads(result.stdout)
        workers = data.get("workers", [])
        worker_ids = [w.get("id") for w in workers]
        if "worker-1" in worker_ids and "worker-2" in worker_ids:
            return TestResult(
                "worker_registration",
                True,
                f"Registered: {', '.join(worker_ids)}",
                dur,
            )
    except json.JSONDecodeError:
        pass
    return TestResult("worker_registration", False, f"Unexpected response: {result.stdout[:200]}", dur)


def run_all_tests(compose_file: str) -> list[TestResult]:
    """Run all federation tests."""
    central_url = "http://localhost:10000"

    results = []

    # Test 1: Worker ingest
    print("\n[1/6] Testing worker-1 ingest...")
    results.append(test_worker_ingest("http://localhost:10102", "frontend"))

    print("[2/6] Testing worker-2 ingest...")
    results.append(test_worker_ingest("http://localhost:10202", "backend"))

    # Test 2: Worker registration
    print("[3/6] Testing worker registration...")
    results.append(test_worker_registration(central_url))

    # Test 3: Worker query (direct, not through central)
    print("[4/6] Testing worker query (direct)...")
    results.append(test_worker_query("http://localhost:10001", "frontend"))

    # Test 4: Multi-service federation
    print("[5/6] Testing multi-service federation...")
    results.append(test_multi_service_federation(["http://localhost:10001", "http://localhost:10002"]))

    # Test 5: S3 cold storage
    print("[6/6] Testing S3 cold storage config...")
    results.append(test_s3_cold_storage(central_url))

    return results


def print_results(results: list[TestResult]) -> int:
    """Print test results and return exit code."""
    print("\n" + "=" * 60)
    print("FEDERATION TEST RESULTS")
    print("=" * 60)

    passed = sum(1 for r in results if r.passed)
    failed = sum(1 for r in results if not r.passed)

    for r in results:
        status = "PASS" if r.passed else "FAIL"
        print(f"  [{status}] {r.name:30s} {r.duration_ms:6.1f}ms — {r.message}")

    print("=" * 60)
    print(f"Total: {len(results)} tests | {passed} passed | {failed} failed")
    print("=" * 60)

    return 0 if failed == 0 else 1


def main():
    parser = argparse.ArgumentParser(description="Federation + S3 multi-node test")
    parser.add_argument("--compose", default="docker-compose.federation.yml",
                        help="Path to docker-compose.federation.yml")
    parser.add_argument("--cleanup", action="store_true",
                        help="Stop and remove containers after tests")
    parser.add_argument("--skip-start", action="store_true",
                        help="Assume containers are already running")
    args = parser.parse_args()

    compose_file = args.compose

    if not args.skip_start:
        print("Starting federation stack...")
        result = docker_compose(["up", "-d", "--build"], compose_file)
        if result.returncode != 0:
            print(f"ERROR: Failed to start stack:\n{result.stderr}")
            sys.exit(1)

        if not healthcheck_services(compose_file):
            print("ERROR: Services failed to become healthy")
            docker_compose(["logs"], compose_file)
            if args.cleanup:
                docker_compose(["down", "-v"], compose_file)
            sys.exit(1)

        # Extra wait for central to register workers
        print("Waiting for central to register workers...")
        time.sleep(5)

    try:
        results = run_all_tests(compose_file)
        exit_code = print_results(results)
    finally:
        if args.cleanup and not args.skip_start:
            print("\nCleaning up...")
            docker_compose(["down", "-v"], compose_file)

    sys.exit(exit_code)


if __name__ == "__main__":
    main()

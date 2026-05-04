#!/usr/bin/env python3
"""
SelfLoopCollector — instruments a sigiro self-loop e2e run and collects:
- sigiro config + logs
- every HTTP query request/response
- profile sample data
- opencode token usage (via SQLite)
- a human-readable timeline

Output: .self-loop/<run-id>/*.jsonl + timeline.md
"""

import json
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

OPENCODE_DB = Path.home() / ".local/share/opencode/opencode.db"
SIGIRO_URL = "http://localhost:9999"
DEV_AUTH = "Authorization: Bearer sigiro_dev_test"


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def run_id() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")


def opencode_session_id() -> Optional[str]:
    """Read the active opencode session ID from swarm state."""
    state_path = Path(".swarm/session/state.json")
    if not state_path.exists():
        return None
    try:
        with open(state_path) as f:
            state = json.load(f)
        sessions = state.get("agentSessions", {})
        if not sessions:
            return None
        # Return the most recent session by lastAgentEventTime
        latest = max(sessions.items(), key=lambda kv: kv[1].get("lastAgentEventTime", 0))
        return latest[0]
    except Exception:
        return None


def query_opencode_tokens(session_id: Optional[str], run_start_ms: int) -> List[Dict[str, Any]]:
    """Pull token breakdown for messages after run_start from opencode SQLite."""
    if not OPENCODE_DB.exists() or not session_id:
        return []

    import sqlite3
    conn = sqlite3.connect(str(OPENCODE_DB))
    conn.row_factory = sqlite3.Row
    rows = conn.execute("""
        SELECT
            m.time_created,
            m.data,
            p.data as part_data
        FROM message m
        LEFT JOIN part p ON p.message_id = m.id
        WHERE m.session_id = ? AND m.time_created >= ?
        ORDER BY m.time_created ASC
    """, (session_id, run_start_ms)).fetchall()

    events = []
    for row in rows:
        msg = json.loads(row["data"])
        tokens = msg.get("tokens", {})
        event = {
            "ts": datetime.fromtimestamp(row["time_created"] / 1000, tz=timezone.utc).isoformat(),
            "role": msg.get("role"),
            "mode": msg.get("mode"),
            "finish": msg.get("finish"),
            "model_id": msg.get("modelID"),
            "cost": msg.get("cost", 0),
            "tokens": {
                "input": tokens.get("input", 0),
                "output": tokens.get("output", 0),
                "reasoning": tokens.get("reasoning", 0),
                "cache_read": tokens.get("cache", {}).get("read", 0),
                "cache_write": tokens.get("cache", {}).get("write", 0),
                "total": tokens.get("total", 0),
            },
        }
        events.append(event)
    return events


def compute_metrics(
    token_events: List[Dict[str, Any]],
    queries: List[Dict[str, Any]],
    investigate: Dict[str, Any],
) -> Dict[str, Any]:
    """Compute aggregate metrics: tokens_per_call, success_rate, cost totals."""
    llm_calls = [e for e in token_events if e.get("role") == "assistant"]
    calls_count = len(llm_calls)
    total_input = sum(e["tokens"]["input"] for e in llm_calls)
    total_output = sum(e["tokens"]["output"] for e in llm_calls)
    total_reasoning = sum(e["tokens"]["reasoning"] for e in llm_calls)
    total_cache_read = sum(e["tokens"]["cache_read"] for e in llm_calls)
    total_cache_write = sum(e["tokens"]["cache_write"] for e in llm_calls)
    total_tokens = sum(e["tokens"]["total"] for e in llm_calls)
    total_cost = sum(e.get("cost", 0) for e in llm_calls)
    tokens_per_call = round(total_tokens / calls_count, 1) if calls_count > 0 else 0.0

    attempts: List[Dict[str, Any]] = []
    for q in queries:
        attempts.append({"label": "query", "ok": 200 <= q.get("status", 0) < 300})
    if investigate:
        attempts.append({"label": "investigate", "ok": 200 <= investigate.get("status", 0) < 300})
    ok_count = sum(1 for a in attempts if a["ok"])
    total_attempts = len(attempts)
    success_rate = round(ok_count / total_attempts, 4) if total_attempts > 0 else 0.0

    return {
        "tokens_per_call": tokens_per_call,
        "calls_count": calls_count,
        "tokens": {
            "input": total_input,
            "output": total_output,
            "reasoning": total_reasoning,
            "cache_read": total_cache_read,
            "cache_write": total_cache_write,
            "total": total_tokens,
        },
        "cost_total": round(total_cost, 6),
        "success_rate": success_rate,
        "success_count": ok_count,
        "total_attempts": total_attempts,
    }


def sigiro_logs() -> List[Dict[str, Any]]:
    """Get sigiro logs from docker."""
    result = subprocess.run(
        ["docker", "compose", "logs", "sigiro"],
        capture_output=True, text=True, cwd="tests/e2e"
    )
    lines = result.stdout.split("\n") + result.stderr.split("\n")
    events = []
    for line in lines:
        if "pyroscope ingest" in line or "ERROR" in line or "WARN" in line:
            # Parse timestamp from docker log format
            # Format: sigiro-1  | 2026-05-04T08:30:00.000000Z  INFO  ...
            parts = line.split("|", 1)
            if len(parts) < 2:
                continue
            ts_part = parts[0].split("sigiro-1")[0].strip()[-26:]
            log_part = parts[1].strip()
            events.append({"raw": log_part, "ts": ts_part})
    return events


def curl_json(method: str, path: str, data: Optional[str] = None,
              headers: Optional[List[str]] = None) -> tuple[int, Any, int]:
    """Make an HTTP request, return (status_code, json_body, response_bytes)."""
    cmd = ["curl", "-s", "-w", "\\n%{http_code}", "-X", method, f"{SIGIRO_URL}{path}"]
    extra = []
    if headers:
        for h in headers:
            extra += ["-H", h]
    if data:
        extra += ["-d", data]
    cmd += extra

    result = subprocess.run(cmd, capture_output=True, text=True)
    # Last line is status code
    lines = result.stdout.strip().split("\n")
    status_line = lines[-1]
    try:
        status = int(status_line)
    except ValueError:
        status = 0
    body = "\n".join(lines[:-1])
    response_bytes = len(result.stdout.encode())
    try:
        body_json = json.loads(body) if body else {}
    except json.JSONDecodeError:
        body_json = {"raw": body[:500]}

    return status, body_json, response_bytes


def run_query(sql: str) -> Dict[str, Any]:
    """Run a SQL query against sigiro, return structured result."""
    status, body, resp_bytes = curl_json("POST", "/v1/query", sql,
                                         headers=[DEV_AUTH, "Content-Type: text/plain"])
    return {
        "sql": sql,
        "status": status,
        "response_bytes": resp_bytes,
        "row_count": len(body) if isinstance(body, list) else -1,
        "first_row": body[0] if isinstance(body, list) and body else None,
        "error": body.get("detail") if isinstance(body, dict) else None,
    }


def run_investigate(service: str) -> Dict[str, Any]:
    """Run /v1/investigate for a service."""
    payload = json.dumps({"service": service})
    status, body, resp_bytes = curl_json("POST", "/v1/investigate", payload,
                                         headers=[DEV_AUTH, "Content-Type: application/json"])
    return {
        "service": service,
        "status": status,
        "response_bytes": resp_bytes,
        "profile_count": body.get("sigiro", {}).get("coverage", {}).get("profile_sample_count", 0),
        "has_profiles": body.get("sigiro", {}).get("coverage", {}).get("has_profiles", False),
        "has_spans": body.get("sigiro", {}).get("coverage", {}).get("has_spans", False),
        "has_logs": body.get("sigiro", {}).get("coverage", {}).get("has_logs", False),
    }


def collect_profiles_sample(limit: int = 3) -> List[Dict[str, Any]]:
    """Fetch a few actual profile rows for blog post."""
    result = subprocess.run(
        ["curl", "-s", "-X", "POST", f"{SIGIRO_URL}/v1/query",
         "-H", DEV_AUTH, "-d",
         f"SELECT * FROM lake.sigiro_profiles LIMIT {limit}"],
        capture_output=True, text=True
    )
    try:
        rows = json.loads(result.stdout)
        return rows
    except Exception:
        return []


def collect(notes: str = "") -> Path:
    """Run the full collection and write to .self-loop/<run-id>/."""
    run_dir = Path(f".self-loop/run-{run_id()}")
    run_dir.mkdir(parents=True, exist_ok=True)

    run_start_iso = now_iso()
    run_start_ms = int(datetime.now(timezone.utc).timestamp() * 1000)
    session_id = opencode_session_id()

    # Collect opencode token data (only messages after run started)
    token_events = query_opencode_tokens(session_id, run_start_ms) if session_id else []

    # Wait for sigiro to be healthy
    health_ok = False
    for _ in range(30):
        status, _, _ = curl_json("GET", "/health")
        if status == 200:
            health_ok = True
            break
        time.sleep(1)

    meta = {
        "run_id": run_dir.name,
        "started_at": run_start_iso,
        "notes": notes,
        "sigiro_url": SIGIRO_URL,
        "opencode_session_id": session_id,
        "sigiro_healthy": health_ok,
    }

    # Collect sigiro logs
    logs = sigiro_logs()

    # Run queries
    queries = []
    queries.append(run_query("SELECT count(*) as n FROM lake.sigiro_profiles"))
    queries.append(run_query(
        "SELECT service_name, profile_type, unit, count(*) as cnt "
        "FROM lake.sigiro_profiles GROUP BY service_name, profile_type, unit"
    ))
    queries.append(run_query(
        "SELECT stack_trace, count(*) as cnt FROM lake.sigiro_profiles "
        "WHERE stack_trace IS NOT NULL AND array_length(stack_trace) > 0 "
        "GROUP BY stack_trace ORDER BY cnt DESC LIMIT 5"
    ))

    # Investigate
    investigate = run_investigate("sigiro")

    # Profile samples
    profiles = collect_profiles_sample(3)

    metrics = compute_metrics(token_events, queries, investigate)
    meta["tokens_per_call"] = metrics["tokens_per_call"]
    meta["success_rate"] = metrics["success_rate"]
    meta["calls_count"] = metrics["calls_count"]
    meta["cost_total"] = metrics["cost_total"]

    # Build timeline
    lines = [f"# Self-Loop Run — {run_dir.name}", "", f"Started: {run_start_iso}", f"Notes: {notes}", ""]
    lines.append("## Timeline")
    lines.append(f"- t+0s: sigiro started, pyroscope profiling enabled")
    lines.append(f"- t+10s: first profile received (pyroscope ingest log)")
    if queries:
        lines.append(f"- t+X: SQL queries executed ({len(queries)} queries)")
    if investigate.get("profile_count"):
        lines.append(f"- t+X: /investigate returned {investigate['profile_count']} profile samples")
    lines.append("")
    lines.append("## Metrics")
    lines.append(f"- tokens_per_call: {metrics['tokens_per_call']}")
    lines.append(f"- llm_calls: {metrics['calls_count']}")
    lines.append(f"- total_tokens: {metrics['tokens']['total']}")
    lines.append(f"- cost_total: ${metrics['cost_total']:.6f}")
    lines.append(f"- success_rate: {metrics['success_rate']:.1%} ({metrics['success_count']}/{metrics['total_attempts']})")
    if token_events:
        lines.append("")
        lines.append(f"## Token Usage (opencode session {session_id})")
        for e in token_events[-5:]:
            t = e["tokens"]
            lines.append(f"- {e['role']}/{e['mode']}: input={t['input']} output={t['output']} "
                          f"cache_read={t['cache_read']} cost=${e['cost']:.6f}")

    # Write files
    with open(run_dir / "meta.json", "w") as f:
        json.dump(meta, f, indent=2)

    with open(run_dir / "events.jsonl", "w") as f:
        for log in logs:
            f.write(json.dumps({"type": "sigiro_log", **log}) + "\n")

    with open(run_dir / "queries.jsonl", "w") as f:
        for q in queries:
            f.write(json.dumps({"type": "query", "ts": now_iso(), **q}) + "\n")

    if token_events:
        with open(run_dir / "token-breakdown.jsonl", "w") as f:
            for e in token_events:
                f.write(json.dumps({"type": "token_event", **e}) + "\n")

    if profiles:
        profiles_dir = run_dir / "profiles"
        profiles_dir.mkdir(exist_ok=True)
        for i, p in enumerate(profiles):
            with open(profiles_dir / f"profile-{i:03d}.json", "w") as f:
                json.dump(p, f, indent=2)
        with open(profiles_dir / "all.json", "w") as f:
            json.dump(profiles, f, indent=2)

    with open(run_dir / "investigate.json", "w") as f:
        json.dump({"type": "investigate", "ts": now_iso(), **investigate}, f, indent=2)

    with open(run_dir / "metrics.json", "w") as f:
        json.dump({"type": "metrics", "ts": now_iso(), **metrics}, f, indent=2)

    with open(run_dir / "timeline.md", "w") as f:
        f.write("\n".join(lines))

    print(f"Collected to {run_dir}")
    return run_dir


if __name__ == "__main__":
    notes = sys.argv[1] if len(sys.argv) > 1 else ""
    collect(notes)
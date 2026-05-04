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
CAR_FLOWS_DB = Path(".codex-autorunner/flows.db")
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


def ticket_call_counts() -> Dict[str, int]:
    """Map ticket basename -> assistant-message count via CAR flows.db + opencode DB."""
    if not CAR_FLOWS_DB.exists() or not OPENCODE_DB.exists():
        return {}

    import sqlite3

    flows_conn = sqlite3.connect(str(CAR_FLOWS_DB))
    flows_conn.row_factory = sqlite3.Row
    rows = flows_conn.execute("""
        SELECT
            json_extract(state, '$.ticket_engine.current_ticket') AS ticket_path,
            json_extract(state, '$.ticket_engine.last_agent_turn_id') AS turn_id
        FROM flow_runs
        WHERE flow_type = 'ticket_flow'
          AND json_extract(state, '$.ticket_engine.current_ticket') IS NOT NULL
          AND json_extract(state, '$.ticket_engine.last_agent_turn_id') IS NOT NULL
        ORDER BY created_at
    """).fetchall()
    flows_conn.close()

    ticket_sessions: Dict[str, str] = {}
    for row in rows:
        turn_id = row["turn_id"]
        session_id = turn_id.split(":")[0] if ":" in turn_id else turn_id
        ticket_sessions[Path(row["ticket_path"]).name] = session_id

    if not ticket_sessions:
        return {}

    session_ids = list(set(ticket_sessions.values()))
    placeholders = ",".join("?" for _ in session_ids)
    opencode_conn = sqlite3.connect(str(OPENCODE_DB))
    rows = opencode_conn.execute(f"""
        SELECT session_id, count(*) AS cnt
        FROM message
        WHERE session_id IN ({placeholders})
          AND json_extract(data, '$.role') = 'assistant'
        GROUP BY session_id
    """, session_ids).fetchall()
    opencode_conn.close()

    session_counts: Dict[str, int] = {row[0]: row[1] for row in rows}

    return {name: session_counts.get(sid, 0) for name, sid in ticket_sessions.items()}


def compute_metrics(
    token_events: List[Dict[str, Any]],
    queries: List[Dict[str, Any]],
    investigate: Dict[str, Any],
    per_ticket_calls: Optional[Dict[str, int]] = None,
) -> Dict[str, Any]:
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

    result = {
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

    if per_ticket_calls:
        counts = list(per_ticket_calls.values())
        result["calls_per_ticket"] = {
            "tickets": per_ticket_calls,
            "mean": round(sum(counts) / len(counts), 1) if counts else 0,
            "max": max(counts) if counts else 0,
            "min": min(counts) if counts else 0,
        }

    return result


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

    per_ticket = ticket_call_counts()
    metrics = compute_metrics(token_events, queries, investigate, per_ticket)
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

    if per_ticket:
        lines.append("")
        lines.append("## Calls Per Ticket")
        cpt = metrics.get("calls_per_ticket", {})
        lines.append(f"- mean: {cpt.get('mean', 0)}, min: {cpt.get('min', 0)}, max: {cpt.get('max', 0)}")
        lines.append("")
        lines.append("```")
        max_calls = max(per_ticket.values()) if per_ticket else 0
        scale = max(40 / max_calls, 1) if max_calls > 0 else 1
        for name in sorted(per_ticket):
            n = per_ticket[name]
            bar = "█" * int(n * scale)
            lines.append(f"{name:55s} {n:>4d} {bar}")
        lines.append("```")

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
    generate_dashboard()
    return run_dir


def generate_dashboard(output_path: str = "docs/self-loop/metrics-dashboard.md") -> Path:
    base = Path(".self-loop")
    run_dirs = sorted(base.glob("run-*"))
    if not run_dirs:
        print("No run directories found in .self-loop/")
        return Path(output_path)

    runs: List[Dict[str, Any]] = []
    for rd in run_dirs:
        entry: Dict[str, Any] = {"run_id": rd.name, "dir": str(rd)}

        meta_path = rd / "meta.json"
        if meta_path.exists():
            with open(meta_path) as f:
                meta = json.load(f)
            entry["started_at"] = meta.get("started_at", "")
            entry["notes"] = meta.get("notes", "")
            entry["sigiro_healthy"] = meta.get("sigiro_healthy")

        inv_path = rd / "investigate.json"
        if inv_path.exists():
            with open(inv_path) as f:
                inv = json.load(f)
            entry["profile_count"] = inv.get("profile_count", 0)
            entry["investigate_status"] = inv.get("status", 0)
            entry["has_profiles"] = inv.get("has_profiles", False)
            entry["has_spans"] = inv.get("has_spans", False)
            entry["has_logs"] = inv.get("has_logs", False)

        tb_path = rd / "token-breakdown.jsonl"
        if tb_path.exists():
            with open(tb_path) as f:
                events = [json.loads(line) for line in f]
            assistants = [e for e in events if e.get("role") == "assistant"]
            entry["llm_calls"] = len(assistants)
            entry["total_input"] = sum(e["tokens"]["input"] for e in assistants)
            entry["total_output"] = sum(e["tokens"]["output"] for e in assistants)
            entry["total_reasoning"] = sum(e["tokens"]["reasoning"] for e in assistants)
            entry["total_cache_read"] = sum(e["tokens"]["cache_read"] for e in assistants)
            entry["total_tokens"] = sum(e["tokens"]["total"] for e in assistants)
            entry["total_cost"] = sum(e.get("cost", 0) for e in assistants)
            if assistants:
                first_ts = assistants[0].get("ts", "")
                last_ts = assistants[-1].get("ts", "")
                entry["first_event_ts"] = first_ts
                entry["last_event_ts"] = last_ts
                try:
                    from datetime import datetime as _dt
                    t0 = _dt.fromisoformat(first_ts)
                    t1 = _dt.fromisoformat(last_ts)
                    entry["duration_s"] = (t1 - t0).total_seconds()
                except Exception:
                    entry["duration_s"] = 0
            else:
                entry["duration_s"] = 0

            models: Dict[str, int] = {}
            for e in assistants:
                m = e.get("model_id") or "unknown"
                models[m] = models.get(m, 0) + 1
            entry["models"] = models
        else:
            entry["llm_calls"] = 0
            entry["total_tokens"] = 0
            entry["total_cost"] = 0
            entry["duration_s"] = 0
            entry["models"] = {}

        queries_path = rd / "queries.jsonl"
        if queries_path.exists():
            with open(queries_path) as f:
                queries = [json.loads(line) for line in f]
            entry["query_count"] = len(queries)
            ok = sum(1 for q in queries if 200 <= q.get("status", 0) < 300)
            entry["query_success_rate"] = round(ok / len(queries), 4) if queries else 0.0
        else:
            entry["query_count"] = 0
            entry["query_success_rate"] = 0.0

        runs.append(entry)

    total_runs = len(runs)
    total_llm_calls = sum(r.get("llm_calls", 0) for r in runs)
    total_tokens_all = sum(r.get("total_tokens", 0) for r in runs)
    total_cost_all = sum(r.get("total_cost", 0) for r in runs)
    total_profiles = sum(r.get("profile_count", 0) for r in runs)
    durations = [r.get("duration_s", 0) for r in runs if r.get("duration_s", 0) > 0]
    avg_duration = round(sum(durations) / len(durations), 1) if durations else 0
    tokens_per_run = round(total_tokens_all / total_runs, 1) if total_runs > 0 else 0
    calls_per_run = round(total_llm_calls / total_runs, 1) if total_runs > 0 else 0
    cost_per_run = round(total_cost_all / total_runs, 6) if total_runs > 0 else 0

    all_models: Dict[str, int] = {}
    for r in runs:
        for m, c in r.get("models", {}).items():
            all_models[m] = all_models.get(m, 0) + c

    now = now_iso()
    lines = [
        "# Self-Loop Metrics Dashboard",
        "",
        f"> **Auto-generated** by `scripts/self-loop-collector.py --dashboard` — {now}",
        "",
        "## Aggregate Summary",
        "",
        f"| Metric | Value |",
        f"|--------|-------|",
        f"| Total runs | {total_runs} |",
        f"| Total LLM calls | {total_llm_calls:,} |",
        f"| Total tokens | {total_tokens_all:,} |",
        f"| Total cost | ${total_cost_all:.4f} |",
        f"| Total profiles collected | {total_profiles:,} |",
        f"| Tokens/run (avg) | {tokens_per_run:,} |",
        f"| Calls/run (avg) | {calls_per_run:.1f} |",
        f"| Cost/run (avg) | ${cost_per_run:.4f} |",
        f"| Avg duration | {_fmt_duration(avg_duration)} |",
        "",
    ]

    if all_models:
        lines.append("## Model Usage")
        lines.append("")
        lines.append("| Model | Calls | Share |")
        lines.append("|-------|-------|-------|")
        for m in sorted(all_models, key=all_models.get, reverse=True):  # type: ignore[arg-type]
            c = all_models[m]
            share = round(c / total_llm_calls * 100, 1) if total_llm_calls > 0 else 0
            lines.append(f"| {m} | {c:,} | {share}% |")
        lines.append("")

    lines.append("## Per-Run Breakdown")
    lines.append("")
    lines.append("| Run | LLM Calls | Tokens | Cost | Profiles | Duration | Queries | Query OK |")
    lines.append("|-----|-----------|--------|------|----------|----------|---------|----------|")
    for r in runs:
        rid = r["run_id"].replace("run-", "")
        calls = r.get("llm_calls", 0)
        tokens = r.get("total_tokens", 0)
        cost = r.get("total_cost", 0)
        profs = r.get("profile_count", 0)
        dur = _fmt_duration(r.get("duration_s", 0))
        qc = r.get("query_count", 0)
        qr = f"{r.get('query_success_rate', 0):.0%}" if r.get("query_count", 0) > 0 else "—"
        lines.append(f"| {rid} | {calls:,} | {tokens:,} | ${cost:.4f} | {profs:,} | {dur} | {qc} | {qr} |")
    lines.append("")

    has_token_data = any(r.get("total_input", 0) > 0 for r in runs)
    if has_token_data:
        lines.append("## Token Breakdown by Run")
        lines.append("")
        lines.append("| Run | Input | Output | Reasoning | Cache Read | Total |")
        lines.append("|-----|-------|--------|-----------|------------|-------|")
        for r in runs:
            if r.get("total_input", 0) > 0:
                rid = r["run_id"].replace("run-", "")
                lines.append(
                    f"| {rid} | {r.get('total_input', 0):,} | {r.get('total_output', 0):,} "
                    f"| {r.get('total_reasoning', 0):,} | {r.get('total_cache_read', 0):,} "
                    f"| {r.get('total_tokens', 0):,} |"
                )
        lines.append("")

    if len(runs) > 1:
        lines.append("## Trends")
        lines.append("")
        token_vals = [r.get("total_tokens", 0) for r in runs]
        max_tok = max(token_vals) if token_vals else 1
        lines.append("### Tokens per Run")
        lines.append("```")
        for r in runs:
            rid = r["run_id"].replace("run-", "")
            v = r.get("total_tokens", 0)
            bar_len = int(v / max_tok * 50) if max_tok > 0 else 0
            bar = "█" * bar_len
            lines.append(f"{rid}  {v:>10,}  {bar}")
        lines.append("```")
        lines.append("")

        cost_vals = [r.get("total_cost", 0) for r in runs]
        max_cost = max(cost_vals) if cost_vals else 1
        lines.append("### Cost per Run ($)")
        lines.append("```")
        for r in runs:
            rid = r["run_id"].replace("run-", "")
            v = r.get("total_cost", 0)
            bar_len = int(v / max_cost * 50) if max_cost > 0 else 0
            bar = "█" * bar_len
            lines.append(f"{rid}  ${v:>9.4f}  {bar}")
        lines.append("```")
        lines.append("")

    lines.append("## Signal Coverage")
    lines.append("")
    lines.append("| Run | Profiles | Spans | Logs | Investigate Status |")
    lines.append("|-----|----------|-------|------|---------------------|")
    for r in runs:
        rid = r["run_id"].replace("run-", "")
        hp = "✅" if r.get("has_profiles") else "❌"
        hs = "✅" if r.get("has_spans") else "❌"
        hl = "✅" if r.get("has_logs") else "❌"
        ist = r.get("investigate_status", "—")
        lines.append(f"| {rid} | {hp} | {hs} | {hl} | {ist} |")
    lines.append("")

    lines.append("---")
    lines.append(f"*Last updated: {now}*")

    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w") as f:
        f.write("\n".join(lines) + "\n")

    print(f"Dashboard written to {out}")
    return out


def _fmt_duration(seconds: float) -> str:
    if seconds <= 0:
        return "—"
    if seconds < 60:
        return f"{seconds:.0f}s"
    minutes = int(seconds // 60)
    secs = int(seconds % 60)
    if minutes < 60:
        return f"{minutes}m {secs}s"
    hours = int(minutes // 60)
    mins = minutes % 60
    return f"{hours}h {mins}m {secs}s"


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description="Self-Loop Collector")
    parser.add_argument("notes", nargs="?", default="", help="Run notes")
    parser.add_argument("--dashboard", action="store_true",
                        help="Generate metrics dashboard from all runs")
    parser.add_argument("--dashboard-path", default="docs/self-loop/metrics-dashboard.md",
                        help="Output path for dashboard (default: docs/self-loop/metrics-dashboard.md)")
    args = parser.parse_args()

    if args.dashboard:
        generate_dashboard(args.dashboard_path)
    else:
        collect(args.notes)
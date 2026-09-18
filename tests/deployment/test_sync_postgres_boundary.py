"""The sync `postgres` client must never be reached from the async runtime.

`postgres::Client` drives its own runtime via `block_on`. Called from a thread
that already has a Tokio runtime context it does not return an error -- it is a
non-unwinding panic that ABORTS the process. Three separate cutover attempts
died this way, each in a different call site, because a fix covered only the one
site that happened to crash.

`tokio::task::block_in_place` does NOT solve this: it keeps the runtime context
on the thread, so the inner `block_on` still aborts. Only `spawn_blocking`
provides a context-free thread.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

SRC = Path(__file__).resolve().parents[2] / "src"

# Entry points into sync-postgres code. Async callers must wrap these.
ENTRY_POINTS = (
    r"AlertStore::new",
    r"admin::read_all_status",
    r"admin::write_instance_status_batch",
    r"admin::write_instance_status\b",
    r"alerts::create_postgres_tables",
    r"\.process_events\(",
    r"\.next_due\(",
    r"\.mark_sent\(",
    r"\.mark_retry\(",
)

FN_RE = re.compile(r"\s*(pub(?:\([^)]*\))?\s+)?(async\s+)?fn\s+(\w+)")
# How far back a spawn_blocking / with_admin_blocking wrapper may sit.
GUARD_WINDOW = 14


def _enclosing_fn(lines: list[str], index: int) -> tuple[str, bool]:
    for j in range(index, 0, -1):
        match = FN_RE.match(lines[j - 1])
        if match:
            return match.group(3), bool(match.group(2))
    return "<top-level>", False


class SyncPostgresBoundary(unittest.TestCase):
    def test_no_sync_postgres_call_runs_on_the_async_runtime(self) -> None:
        offenders: list[str] = []
        for path in sorted(SRC.rglob("*.rs")):
            lines = path.read_text().splitlines()
            for number, line in enumerate(lines, start=1):
                if line.lstrip().startswith("//"):
                    continue
                if not any(re.search(pattern, line) for pattern in ENTRY_POINTS):
                    continue
                name, is_async = _enclosing_fn(lines, number)
                if not is_async:
                    continue
                window = "\n".join(lines[max(0, number - GUARD_WINDOW) : number])
                if "spawn_blocking" in window or "with_admin_blocking" in window:
                    continue
                offenders.append(f"{path.name}:{number} in async fn {name}: {line.strip()}")

        self.assertEqual(
            offenders,
            [],
            "sync postgres client reached from async code without spawn_blocking; "
            "this aborts the process at runtime:\n  " + "\n  ".join(offenders),
        )

    def test_block_in_place_is_not_used_as_the_escape_hatch(self) -> None:
        # block_in_place keeps the runtime context, so it does not prevent the
        # abort and would silently look like a fix.
        for path in sorted(SRC.rglob("*.rs")):
            text = path.read_text()
            if "block_in_place" not in text:
                continue
            for pattern in ENTRY_POINTS:
                self.assertNotRegex(
                    text,
                    rf"block_in_place[^;]*{pattern}",
                    f"{path.name} guards a sync postgres call with block_in_place",
                )


if __name__ == "__main__":
    unittest.main()

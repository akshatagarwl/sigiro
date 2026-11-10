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
    r"\.mark_failed\(",
    r"\.replay_dead\(",
    r"\.delivery_summary\(",
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

    def test_the_hosted_client_is_never_held_as_a_bare_postgres_client(self) -> None:
        # Call-site discipline cannot reach `drop`. `postgres::Client::drop`
        # closes the connection through `block_on`, so a bare client stored in
        # long-lived state aborts the process when that state is dropped on a
        # runtime thread -- no call site is involved and the checks above see
        # nothing. Production died exactly this way: the drop panic also
        # replaced the real boot error, so the cause went unlogged.
        #
        # `HostedClient` owns that drop, so the invariant is that the sync
        # client is only ever stored behind it.
        admin = (SRC / "admin.rs").read_text()
        self.assertIn(
            "impl Drop for HostedClient",
            admin,
            "HostedClient must own the close so dropping it off-runtime is structural",
        )
        self.assertRegex(
            admin,
            r"Hosted\(HostedClient\)",
            "AdminConnection::Hosted must wrap the client in HostedClient",
        )

        offenders: list[str] = []
        for path in sorted(SRC.rglob("*.rs")):
            for number, line in enumerate(path.read_text().splitlines(), start=1):
                stripped = line.lstrip()
                if stripped.startswith("//"):
                    continue
                # A stored (not borrowed, not returned) bare sync client.
                if re.search(r"(Box<\s*postgres::Client\s*>|:\s*postgres::Client\b)", line):
                    offenders.append(f"{path.name}:{number}: {stripped}")

        # HostedClient itself is the one legitimate owner: it is the type whose
        # Drop moves the close off the runtime. Borrows and constructor returns
        # do not own the drop, so they cannot cause the abort.
        allowed = (
            "client: &mut postgres::Client",
            "-> Result<postgres::Client>",
            "pub struct HostedClient(",
            "fn new(client: postgres::Client)",
        )
        offenders = [o for o in offenders if not any(a in o for a in allowed)]
        self.assertEqual(
            offenders,
            [],
            "sync postgres client stored outside HostedClient; its drop calls "
            "block_on and aborts the process when dropped on a runtime thread:\n  "
            + "\n  ".join(offenders),
        )


if __name__ == "__main__":
    unittest.main()

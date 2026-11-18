from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any

from deploy.nightly_release import (
    Config,
    DurableLock,
    NATIVE_ARTIFACT_ENV,
    Release,
    TARGETS,
    prepare_release_worktree,
    validate_mbx_report,
    validate_production_shape,
)

SHA = "0123456789abcdef0123456789abcdef01234567"


def config(root: Path, *, dry_run: bool = False) -> Config:
    key = root / "key"
    key.write_text("test")
    toolchain = root / "toolchain"
    toolchain.write_text("test")
    duckdb_cache = root / "duckdb-static"
    duckdb_cache.mkdir()
    return Config(root, root, root / "state", root / "release.lock", toolchain, key, dry_run, not dry_run, 30.0, duckdb_cache)


class NightlyReleaseDriverTest(unittest.TestCase):
    def test_release_gate_accepts_content_addressed_task_cache_hit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            release = Release(config(Path(directory)), SHA)
            key = "a" * 64
            calls: list[list[str]] = []

            def run(args: list[str], **kwargs: Any) -> str:
                calls.append(args)
                return json.dumps({"task": "release", "cache_key": key}) if "--task-cache-explain-json" in args else "[release] sources up-to-date, skipping"

            release.runner.run = run  # type: ignore[method-assign]
            result = release.phase_gate()
            self.assertEqual(result, {"gate": "passed", "source": "mise-task-cache", "cache_key": key})
            self.assertEqual(calls, [["mise", "run", "--dry-run", "--task-cache-explain-json", "release"], ["mise", "run", "release"]])
            self.assertTrue(release.verify_gate(result))

    def test_native_deployment_passes_all_content_addressed_inputs_to_mise(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = Release(config(root), SHA)
            native = root / "sigiro-native.tar.gz"
            native.write_bytes(b"native")
            digest = hashlib.sha256(b"native").hexdigest()
            release.state["phases"] = {
                "binaries": {"native": {"path": str(native), "sha256": digest}},
                "preflight": {"deployment_head": SHA},
            }
            env = os.environ.copy()
            for artifact in NATIVE_ARTIFACT_ENV[1:]:
                path = root / artifact.lower()
                path.write_bytes(b"native")
                env[artifact] = str(path)
                env[f"{artifact}_SHA256"] = digest
            calls: list[tuple[list[str], dict[str, str] | None]] = []
            def run(args: list[str], **kwargs: Any) -> str:
                calls.append((args, kwargs.get("env")))
                return SHA if args[:3] == ["git", "rev-parse", "origin/main"] else ""

            release.runner.run = run  # type: ignore[method-assign]
            old = os.environ.copy()
            os.environ.update(env)
            try:
                result = release.phase_native_deployment()
            finally:
                os.environ.clear()
                os.environ.update(old)
            self.assertEqual(result["unit"], "sigiro.service")
            command, command_env = next((args, command_env) for args, command_env in calls if args[0] == "sops")
            self.assertEqual(command, ["sops", "exec-env", "ops/prod/secrets.sops.json", "mise run alchemy-workload-deploy"])
            assert command_env is not None
            self.assertEqual(command_env["SIGIRO_NATIVE_ACTION"], "deploy")
            self.assertEqual(command_env["SIGIRO_ARTIFACT_SHA256"], digest)
            self.assertEqual(command_env["NODE_ARTIFACT_SHA256"], digest)

    def test_native_deployment_rejects_missing_artifact_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            release = Release(config(Path(directory)), SHA)
            release.state["phases"] = {
                "binaries": {"native": {"path": "/tmp/sigiro-native.tar.gz", "sha256": "a" * 64}},
                "preflight": {"deployment_head": SHA},
            }
            with self.assertRaisesRegex(RuntimeError, "ARTIFACT"):
                release.native_artifacts()

    def test_binaries_use_mise_for_every_target(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            release = Release(config(Path(directory)), SHA)
            calls: list[list[str]] = []
            release.runner.run = lambda args, **kwargs: calls.append(args) or ""  # type: ignore[method-assign]
            with self.assertRaises(RuntimeError):
                release.phase_binaries()
            self.assertEqual(calls[: len(TARGETS)], [["mise", "run", "build"] for _ in TARGETS])

    def test_mbx_report_records_misses_and_rejects_unconsulted_work(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mbx.json"
            report = {"hits": 32, "misses": 3, "unconsulted": 0, "compiler": {"miss": {"invocations": 1}}, "bypasses": {"native-library": 6}}
            path.write_text(json.dumps(report))
            self.assertEqual(validate_mbx_report(path)["compiler_misses"], 1)
            report["unconsulted"] = 1
            path.write_text(json.dumps(report))
            with self.assertRaises(RuntimeError):
                validate_mbx_report(path)

    def test_production_data_directory_must_be_on_a_writable_volume(self) -> None:
        with self.assertRaises(RuntimeError):
            validate_production_shape("/opt/sigiro", ["/data"])

    def test_receipt_resume_requires_revalidation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            release = Release(config(Path(directory)), SHA)
            calls: list[int] = []
            release.phase("gate", lambda: calls.append(1) or {"gate": "passed"}, lambda _: True)
            release.phase("gate", lambda: calls.append(2) or {"gate": "passed"}, lambda _: True)
            self.assertEqual(calls, [1])

    def test_lock_is_exclusive_and_reusable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release.lock"
            with DurableLock(path):
                with self.assertRaises(RuntimeError):
                    with DurableLock(path):
                        pass
            with DurableLock(path):
                self.assertTrue(path.exists())
            self.assertFalse(path.exists())

    def test_supported_worktree_reset_preserves_ignored_target(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, worktree = root / "source", root / "nightly"
            source.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=source, check=True)
            subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=source, check=True)
            subprocess.run(["git", "config", "user.name", "Test"], cwd=source, check=True)
            (source / ".gitignore").write_text("target\n")
            (source / "value").write_text("one")
            subprocess.run(["git", "add", "."], cwd=source, check=True)
            subprocess.run(["git", "commit", "-qm", "one"], cwd=source, check=True)
            first = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
            subprocess.run(["git", "worktree", "add", "--detach", str(worktree), first], cwd=source, check=True, stdout=subprocess.DEVNULL)
            (source / "value").write_text("two")
            subprocess.run(["git", "commit", "-qam", "two"], cwd=source, check=True)
            second = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
            (worktree / "target").mkdir()
            (worktree / "target" / "warm").write_text("kept")
            cfg = config(worktree)
            object.__setattr__(cfg, "source_repo", source)
            prepare_release_worktree(cfg, second)
            self.assertEqual((worktree / "value").read_text(), "two")
            self.assertEqual((worktree / "target" / "warm").read_text(), "kept")


if __name__ == "__main__":
    unittest.main()

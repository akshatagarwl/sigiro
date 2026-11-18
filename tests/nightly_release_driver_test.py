from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from deploy.nightly_release import (
    Config,
    DurableLock,
    Release,
    parse_sigiro_deploy,
    prepare_release_worktree,
    update_sigiro_image,
    validate_mbx_report,
    validate_production_shape,
)

SHA = "0123456789abcdef0123456789abcdef01234567"


def config(root: Path, *, dry_run: bool = False) -> Config:
    executable = root / "tool"
    executable.write_text("#!/bin/sh\n")
    executable.chmod(0o755)
    key = root / "key"
    key.write_text("test")
    toolchain = root / "toolchain"
    toolchain.write_text("test")
    duckdb_cache = root / "duckdb-static"
    duckdb_cache.mkdir()
    return Config(
        repo=root,
        source_repo=root,
        state_root=root / "state",
        lock_path=root / "release.lock",
        toolchain_file=toolchain,
        kamal=executable,
        ssh_key=key,
        dry_run=dry_run,
        allow_writes=not dry_run,
        min_free_gib=30.0,
        duckdb_cache=duckdb_cache,
    )


def deploy_yaml(image: str = "old") -> str:
    return f"""accessories:
  sigiro-hosted:
    image: ghcr.io/sigiroai/sigiro:{image}
    env:
      clear:
        SIGIRO_DATA_DIR: /var/lib/sigiro-hosted
    volumes:
      - data:/var/lib/sigiro-hosted
    options:
      read-only: true
  registry:
    image: registry:2
"""


class NightlyReleaseDriverTest(unittest.TestCase):
    def test_release_gate_accepts_content_addressed_task_cache_hit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = Release(config(root), SHA)
            calls: list[tuple[list[str], bool]] = []
            cache_key = "a" * 64

            def run(args: list[str], **kwargs: object) -> str:
                calls.append((args, bool(kwargs.get("capture", False))))
                if "--task-cache-explain-json" in args:
                    return json.dumps({"task": "release", "cache_key": cache_key})
                return "[release] sources up-to-date, skipping"

            release.runner.run = run  # type: ignore[method-assign]

            result = release.phase_gate()

            self.assertEqual(calls, [
                (["mise", "run", "--dry-run", "--task-cache-explain-json", "release"], True),
                (["mise", "run", "release"], True),
            ])
            self.assertEqual(result, {"gate": "passed", "source": "mise-task-cache", "cache_key": cache_key})
            self.assertTrue(release.verify_gate(result))

    def test_website_uses_aube_and_alchemy_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = Release(config(root), SHA)
            commands: list[tuple[list[str], Path | None, bool]] = []
            release.runner.run = lambda args, **kwargs: commands.append((args, kwargs.get("cwd"), kwargs.get("write", False))) or ""  # type: ignore[method-assign]
            deployments = iter([{"deployment_id": "old", "version_id": "v1"}, {"deployment_id": "new", "version_id": "v2"}])
            release.worker_deployment = lambda: next(deployments)  # type: ignore[method-assign]
            release.verify_site = lambda: {"https://sigiro.com/": 200}  # type: ignore[method-assign]

            release.phase_website()

            self.assertEqual([command for command, _, _ in commands], [
                ["aube", "-C", "www", "install", "--frozen-lockfile"],
                ["aube", "-C", "www", "run", "check"],
                ["aube", "-C", "www", "run", "build"],
                ["mise", "run", "alchemy-deploy"],
            ])
            self.assertTrue(all("bun" not in part.lower() and "wrangler" not in part.lower() for command, _, _ in commands for part in command))

    def test_wrangler_is_local_aube_read_only_helper(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            release = Release(config(Path(directory)), SHA)
            self.assertEqual(release._wrangler("r2", "object", "get"), [
                "aube", "exec", "--", "wrangler", "r2", "object", "get",
            ])
            self.assertNotIn("deploy", release._wrangler("r2", "object", "get"))

    def test_worker_deployment_selects_newest_active_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            release = Release(config(Path(directory)), SHA)
            payload = [
                {"id": "old", "created_on": "2026-01-01T00:00:00Z", "versions": [{"version_id": "v1", "percentage": 100}]},
                {"id": "new", "created_on": "2026-01-02T00:00:00Z", "versions": [{"version_id": "v2", "percentage": 100}]},
            ]
            release.runner.run = lambda *args, **kwargs: json.dumps(payload)  # type: ignore[method-assign]
            self.assertEqual(release.worker_deployment(), {"deployment_id": "new", "version_id": "v2"})

    def test_mbx_report_rejects_cold_or_unconsulted_work(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "mbx.json"
            report = {"hits": 32, "misses": 3, "unconsulted": 0, "compiler": {"miss": {"invocations": 1}}, "bypasses": {"native-library": 6}}
            path.write_text(json.dumps(report))
            self.assertEqual(validate_mbx_report(path)["compiler_misses"], 1)
            report["unconsulted"] = 1
            path.write_text(json.dumps(report))
            with self.assertRaises(RuntimeError):
                validate_mbx_report(path)
            report["unconsulted"] = 0
            report["compiler"]["miss"]["invocations"] = 2
            path.write_text(json.dumps(report))
            with self.assertRaises(RuntimeError):
                validate_mbx_report(path)

    def test_production_shape_and_scoped_image_update(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "deploy.yml"
            path.write_text(deploy_yaml())
            parsed = parse_sigiro_deploy(path)
            self.assertEqual(parsed["image"], "ghcr.io/sigiroai/sigiro:old")
            update_sigiro_image(path, "abcdef123")
            self.assertEqual(parse_sigiro_deploy(path)["image"], "ghcr.io/sigiroai/sigiro:abcdef123")
            self.assertIn("image: registry:2", path.read_text())
        with self.assertRaises(RuntimeError):
            validate_production_shape("/opt/sigiro", ["/data"])

    def test_repository_production_config_parses(self) -> None:
        parsed = parse_sigiro_deploy(Path(__file__).parents[1] / "config/deploy.yml")
        self.assertEqual(parsed["data_dir"], "/var/lib/sigiro-hosted")
        self.assertEqual(parsed["mounts"], ["/var/lib/sigiro-hosted"])

    def test_receipt_resume_requires_revalidation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = Release(config(root), SHA)
            calls: list[int] = []
            release.phase("gate", lambda: calls.append(1) or {"gate": "passed"}, lambda _: True)
            release.phase("gate", lambda: calls.append(2) or {"gate": "passed"}, lambda _: True)
            self.assertEqual(calls, [1])
            events = release.events_path.read_text()
            self.assertIn('"kind": "phase-revalidated"', events)
            state = json.loads(release.state_path.read_text())
            self.assertEqual(state["phases"]["gate"]["status"], "complete")

    def test_failed_phase_preserves_external_checkpoint(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            release = Release(config(Path(directory)), SHA)

            def publish_then_fail() -> dict[str, object]:
                release.checkpoint("image", candidate_index_digest="sha256:" + "a" * 64)
                raise RuntimeError("verification failed")

            with self.assertRaises(RuntimeError):
                release.phase("image", publish_then_fail)
            state = json.loads(release.state_path.read_text())
            self.assertEqual(state["phases"]["image"]["status"], "failed")
            self.assertEqual(state["phases"]["image"]["candidate_index_digest"], "sha256:" + "a" * 64)

    def test_dry_run_records_planned_not_complete(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            release = Release(config(root, dry_run=True), SHA)
            release.phase("image", lambda: {"promotion": "planned"})
            state = json.loads(release.state_path.read_text())
            self.assertEqual(state["phases"]["image"]["status"], "planned")

    def test_lock_is_exclusive_and_reusable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            lock_path = Path(directory) / "release.lock"
            with DurableLock(lock_path):
                with self.assertRaises(RuntimeError):
                    with DurableLock(lock_path):
                        pass
            with DurableLock(lock_path):
                self.assertTrue(lock_path.exists())
            self.assertFalse(lock_path.exists())

    def test_supported_worktree_reset_preserves_ignored_target(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            worktree = root / "nightly"
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
            target = worktree / "target"
            target.mkdir()
            (target / "warm").write_text("kept")
            cfg = config(worktree)
            object.__setattr__(cfg, "source_repo", source)
            prepare_release_worktree(cfg, second)
            self.assertEqual((worktree / "value").read_text(), "two")
            self.assertEqual((target / "warm").read_text(), "kept")


if __name__ == "__main__":
    unittest.main()

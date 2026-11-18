#!/usr/bin/env python3
"""Deterministic Sigiro nightly release, promotion, and deployment driver."""
from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import fcntl
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable
from urllib.request import Request, urlopen

VERSION = "0.1.0"
IMAGE = "ghcr.io/sigiroai/sigiro"
BUCKET = "sigiro-dl"
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)
PHASES = (
    "source",
    "preflight",
    "gate",
    "image",
    "binaries",
    "r2",
    "website",
    "worldpol",
    "kamal",
    "health",
)
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
IMAGE_LINE_RE = re.compile(r"^(\s*image:\s*)ghcr\.io/sigiroai/sigiro:[^\s#]+(\s*(?:#.*)?)$")


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def atomic_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(value, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp, path)
    finally:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(tmp)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_sha(value: str) -> str:
    value = value.strip().lower()
    if not SHA_RE.fullmatch(value):
        raise RuntimeError(f"expected a full 40-character Git SHA, got {value!r}")
    return value


def validate_mbx_report(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise RuntimeError("mbx did not write its release statistics report")
    report = json.loads(path.read_text(encoding="utf-8"))
    if report.get("unconsulted") != 0:
        raise RuntimeError(f"mbx reported unconsulted compiler actions: {report.get('unconsulted')}")
    compiler_misses = report.get("compiler", {}).get("miss", {}).get("invocations", 0)
    if compiler_misses > 1:
        raise RuntimeError(f"cold release work detected: {compiler_misses} compiler misses")
    allowed_bypasses = {"compiler-query", "native-library", "rustdoc", "standard-input", "unsupported-search-path"}
    unexpected = set(report.get("bypasses", {})) - allowed_bypasses
    if unexpected:
        raise RuntimeError(f"unexpected mbx bypass categories: {sorted(unexpected)}")
    return {
        "hits": report.get("hits", 0),
        "misses": report.get("misses", 0),
        "compiler_misses": compiler_misses,
        "unconsulted": report.get("unconsulted", 0),
        "bypasses": report.get("bypasses", {}),
    }


def validate_production_shape(endpoint: str, extension_dir: str, mounts: Iterable[str]) -> None:
    if not re.fullmatch(r"https?://[^/\s]+(?:/[^\s]*)?", endpoint):
        raise RuntimeError("SIGIRO_S3_ENDPOINT must be an absolute HTTP(S) URL")
    normalized = [mount.rstrip("/") or "/" for mount in mounts]
    if not any(extension_dir == mount or extension_dir.startswith(mount + "/") for mount in normalized):
        raise RuntimeError("SIGIRO_EXTENSION_DIR must be inside a writable volume")


def parse_sigiro_deploy(path: Path) -> dict[str, Any]:
    lines = path.read_text(encoding="utf-8").splitlines()
    start = next((i for i, line in enumerate(lines) if line == "  sigiro-hosted:"), None)
    if start is None:
        raise RuntimeError("worldpol has no sigiro-hosted accessory")
    end = next((i for i in range(start + 1, len(lines)) if re.match(r"^  [A-Za-z0-9_-]+:$", lines[i])), len(lines))
    block = lines[start:end]
    image_matches = [(i, IMAGE_LINE_RE.match(line)) for i, line in enumerate(block)]
    image_matches = [(i, match) for i, match in image_matches if match]
    if len(image_matches) != 1:
        raise RuntimeError("sigiro-hosted must contain exactly one Sigiro image line")
    endpoint = _yaml_scalar(block, "SIGIRO_S3_ENDPOINT")
    extension_dir = _yaml_scalar(block, "SIGIRO_EXTENSION_DIR")
    read_only = _yaml_scalar(block, "read-only") == "true"
    mounts = []
    in_volumes = False
    for line in block:
        if line.strip() == "volumes:":
            in_volumes = True
            continue
        if in_volumes and re.match(r"^    \S", line):
            break
        if in_volumes:
            match = re.match(r"^\s*-\s*[^:]+:(/[^\s]+)", line)
            if match:
                mounts.append(match.group(1))
    if not read_only:
        raise RuntimeError("sigiro-hosted root filesystem must be read-only")
    validate_production_shape(endpoint, extension_dir, mounts)
    local_index = image_matches[0][0]
    image = block[local_index].split("image:", 1)[1].split("#", 1)[0].strip()
    return {
        "image": image,
        "endpoint": endpoint,
        "extension_dir": extension_dir,
        "mounts": mounts,
        "line_index": start + local_index,
        "lines": lines,
    }


def _yaml_scalar(lines: list[str], key: str) -> str:
    matches = []
    pattern = re.compile(rf"^\s*{re.escape(key)}:\s*([^#]+?)\s*(?:#.*)?$")
    for line in lines:
        match = pattern.match(line)
        if match:
            matches.append(match.group(1).strip().strip('"\''))
    if len(matches) != 1:
        raise RuntimeError(f"sigiro-hosted must contain exactly one {key}")
    return matches[0]


def update_sigiro_image(path: Path, tag: str) -> None:
    parsed = parse_sigiro_deploy(path)
    lines = parsed["lines"]
    index = parsed["line_index"]
    match = IMAGE_LINE_RE.match(lines[index])
    assert match
    lines[index] = f"{match.group(1)}{IMAGE}:{tag}{match.group(2)}"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    reparsed = parse_sigiro_deploy(path)
    if reparsed["image"] != f"{IMAGE}:{tag}":
        raise RuntimeError("failed to update the sigiro-hosted image")


@dataclass(frozen=True)
class Config:
    repo: Path
    source_repo: Path
    worldpol_repo: Path
    state_root: Path
    lock_path: Path
    toolchain_file: Path
    kamal: Path
    ssh_key: Path
    dry_run: bool
    allow_writes: bool
    min_free_gib: float
    duckdb_cache: Path


class CommandRunner:
    def __init__(self, log_path: Path, dry_run: bool) -> None:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        self.log_path = log_path
        self.dry_run = dry_run
        secret_names = re.compile(r"(?:TOKEN|KEY|SECRET|PASSWORD|CREDENTIAL)", re.I)
        self.secrets = [value for name, value in os.environ.items() if secret_names.search(name) and len(value) >= 8]

    def _redact(self, text: str) -> str:
        for secret in self.secrets:
            text = text.replace(secret, "[REDACTED]")
        return text

    def run(
        self,
        args: list[str],
        *,
        cwd: Path | None = None,
        env: dict[str, str] | None = None,
        write: bool = False,
        capture: bool = False,
    ) -> str:
        if write and self.dry_run:
            self.note(f"DRY-RUN write skipped: {shlex.join(args)}")
            return ""
        self.note(f"$ {shlex.join(args)} (cwd={cwd or Path.cwd()})")
        merged_env = os.environ.copy()
        if env:
            merged_env.update(env)
        if capture:
            result = subprocess.run(args, cwd=cwd, env=merged_env, text=True, capture_output=True, check=False)
            output = (result.stdout or "") + (result.stderr or "")
            with self.log_path.open("a", encoding="utf-8") as handle:
                handle.write(self._redact(output))
                if output and not output.endswith("\n"):
                    handle.write("\n")
        else:
            fd, raw_name = tempfile.mkstemp(prefix=".command.", dir=self.log_path.parent)
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as raw:
                    result = subprocess.run(args, cwd=cwd, env=merged_env, text=True, stdout=raw, stderr=subprocess.STDOUT, check=False)
                with open(raw_name, encoding="utf-8", errors="replace") as raw, self.log_path.open("a", encoding="utf-8") as handle:
                    for line in raw:
                        handle.write(self._redact(line))
            finally:
                with contextlib.suppress(FileNotFoundError):
                    os.unlink(raw_name)
        if result.returncode:
            raise RuntimeError(f"{args[0]} failed with exit {result.returncode}; see {self.log_path}")
        return result.stdout.strip() if capture else ""

    def note(self, text: str) -> None:
        with self.log_path.open("a", encoding="utf-8") as handle:
            handle.write(self._redact(text) + "\n")


class DurableLock:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.handle: Any = None

    def __enter__(self) -> "DurableLock":
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.handle = self.path.open("a+", encoding="utf-8")
        try:
            fcntl.flock(self.handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            self.handle.seek(0)
            owner = self.handle.read().strip() or "unknown owner"
            self.handle.close()
            raise RuntimeError(f"nightly release already running ({owner})") from exc
        self.handle.seek(0)
        self.handle.truncate()
        json.dump({"pid": os.getpid(), "hostname": socket.gethostname(), "started_at": utc_now()}, self.handle)
        self.handle.write("\n")
        self.handle.flush()
        return self

    def __exit__(self, *_: object) -> None:
        if self.handle:
            fcntl.flock(self.handle.fileno(), fcntl.LOCK_UN)
            self.handle.close()
            self.handle = None
        with contextlib.suppress(FileNotFoundError):
            self.path.unlink()


class Release:
    def __init__(self, config: Config, sha: str) -> None:
        self.config = config
        self.sha = require_sha(sha)
        self.tag = self.sha[:9]
        self.dir = config.state_root / "receipts" / self.sha
        self.dir.mkdir(parents=True, exist_ok=True)
        self.artifacts = self.dir / "artifacts"
        self.downloads = self.dir / "downloads"
        self.logs = self.dir / "logs"
        self.runner = CommandRunner(self.logs / "commands.log", config.dry_run)
        self.state_path = self.dir / "receipt.json"
        self.events_path = self.dir / "events.jsonl"
        self.state = self._load_state()

    def _load_state(self) -> dict[str, Any]:
        if not self.state_path.exists():
            return {
                "schema": 1,
                "driver_sha256": sha256_file(Path(__file__)),
                "source": self.sha,
                "tag": self.tag,
                "created_at": utc_now(),
                "phases": {},
            }
        state = json.loads(self.state_path.read_text(encoding="utf-8"))
        if state.get("source") != self.sha:
            raise RuntimeError("receipt source mismatch")
        if state.get("driver_sha256") != sha256_file(Path(__file__)):
            raise RuntimeError("driver changed; start a new receipt or explicitly migrate it")
        return state

    def event(self, kind: str, **fields: object) -> None:
        record = {"at": utc_now(), "kind": kind, "source": self.sha, **fields}
        with self.events_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, sort_keys=True) + "\n")
            handle.flush()
            os.fsync(handle.fileno())

    def checkpoint(self, phase: str, **fields: object) -> None:
        """Persist an externally visible intermediate result before continuing."""
        self.state["phases"][phase].update(fields)
        atomic_json(self.state_path, self.state)
        self.event("phase-checkpoint", phase=phase, fields=fields)

    def phase(self, name: str, action: Callable[[], dict[str, Any]], verify: Callable[[dict[str, Any]], bool] | None = None) -> None:
        old = self.state["phases"].get(name)
        if old and old.get("status") == "complete" and verify and verify(old):
            self.event("phase-revalidated", phase=name)
            return
        if self.config.dry_run:
            result = action()
            self.state["phases"][name] = {"status": "planned", "checked_at": utc_now(), **result}
            atomic_json(self.state_path, self.state)
            return
        self.state["phases"][name] = {"status": "running", "started_at": utc_now(), "previous_receipt": old}
        atomic_json(self.state_path, self.state)
        self.event("phase-start", phase=name)
        try:
            result = action()
            self.state["phases"][name] = {"status": "complete", "completed_at": utc_now(), **result}
            atomic_json(self.state_path, self.state)
            self.event("phase-complete", phase=name)
        except Exception as exc:
            self.state["phases"][name].update({"status": "failed", "failed_at": utc_now(), "error": str(exc)})
            atomic_json(self.state_path, self.state)
            self.event("phase-failed", phase=name, error=str(exc))
            raise

    def execute(self) -> None:
        for name in PHASES:
            action = getattr(self, f"phase_{name}")
            verify = getattr(self, f"verify_{name}", None)
            self.phase(name, action, verify)
        self.state["outcome"] = "planned" if self.config.dry_run else "deployed"
        self.state["finished_at"] = utc_now()
        atomic_json(self.state_path, self.state)

    def phase_source(self) -> dict[str, Any]:
        head = self.runner.run(["git", "rev-parse", "HEAD"], cwd=self.config.repo, capture=True)
        dirty = self.runner.run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=self.config.repo, capture=True)
        if head != self.sha or dirty:
            raise RuntimeError("dedicated release worktree is not clean at the resolved source SHA")
        common = Path(self.runner.run(["git", "rev-parse", "--git-common-dir"], cwd=self.config.repo, capture=True)).resolve()
        expected_common = (self.config.source_repo / ".git").resolve()
        if common != expected_common:
            raise RuntimeError("release checkout is not registered to the expected source repository")
        return {"head": head, "worktree": str(self.config.repo)}

    def verify_source(self, _: dict[str, Any]) -> bool:
        return self._git_head(self.config.repo) == self.sha and not self._git_dirty(self.config.repo)

    def phase_preflight(self) -> dict[str, Any]:
        deploy = self.deploy_at_ref("origin/main") if self.config.dry_run else None
        source_remote = self.runner.run(["git", "remote", "get-url", "origin"], cwd=self.config.source_repo, capture=True)
        worldpol_remote = self.runner.run(["git", "remote", "get-url", "origin"], cwd=self.config.worldpol_repo, capture=True)
        if source_remote != "https://github.com/akshatagarwl/sigiro.git":
            raise RuntimeError(f"unexpected Sigiro origin: {source_remote}")
        if worldpol_remote != "https://github.com/akshatagarwl/linguabench.git":
            raise RuntimeError(f"unexpected worldpol origin: {worldpol_remote}")
        for command in ("git", "mise", "mbx", "docker", "gh", "curl", "ssh"):
            if shutil.which(command) is None:
                raise RuntimeError(f"required command not found: {command}")
        if not self.config.kamal.is_file() or not os.access(self.config.kamal, os.X_OK):
            raise RuntimeError(f"Kamal is unavailable: {self.config.kamal}")
        if not self.config.ssh_key.is_file():
            raise RuntimeError(f"SSH key is unavailable: {self.config.ssh_key}")
        if not self.config.duckdb_cache.is_dir():
            raise RuntimeError(f"stable DuckDB static cache is unavailable: {self.config.duckdb_cache}")
        free = shutil.disk_usage("/").free
        if free < self.config.min_free_gib * 1024**3:
            if self.config.dry_run:
                raise RuntimeError(f"less than {self.config.min_free_gib:g} GiB free ({free / 1024**3:.1f} GiB)")
            self.runner.run(["docker", "buildx", "prune", "--builder", "multiplatform", "--force"], write=True)
            free = shutil.disk_usage("/").free
            if free < self.config.min_free_gib * 1024**3:
                raise RuntimeError(f"less than {self.config.min_free_gib:g} GiB free after builder-cache prune ({free / 1024**3:.1f} GiB)")
        target = self.config.repo / "target"
        resolved_target = target.resolve(strict=True)
        target_root = Path.home() / "Library/Caches/mbx/targets"
        if not resolved_target.is_relative_to(target_root) or not (resolved_target / "release/deps").is_dir():
            raise RuntimeError("warm release target is unavailable")
        expected_toolchain = self.config.toolchain_file.read_text(encoding="utf-8").strip()
        actual_toolchain = self.runner.run(["rustc", "--version", "--verbose"], cwd=self.config.repo, capture=True)
        if actual_toolchain != expected_toolchain:
            raise RuntimeError("Rust toolchain differs from the recorded warm-cache toolchain")
        doctor_raw = self.runner.run(["mbx", "doctor", "--json"], cwd=self.config.repo, capture=True)
        doctor = json.loads(doctor_raw)
        if doctor.get("failures") != 0:
            raise RuntimeError("mbx doctor reported failures")
        config_checks = [item for item in doctor.get("checks", []) if item.get("name") == "config" and item.get("severity") == "pass"]
        cache_checks = [item for item in doctor.get("checks", []) if item.get("name") == "cache" and item.get("severity") == "pass"]
        if len(config_checks) != 1 or "35.0 GiB budget" not in config_checks[0].get("detail", "") or not cache_checks:
            raise RuntimeError("mbx must report a writable cache with the 35 GiB action-store budget")
        bake_raw = self.runner.run(["docker", "buildx", "bake", "--print", "sigiro"], cwd=self.config.repo, capture=True)
        bake = json.loads(bake_raw[bake_raw.index("{"):])
        image_target = bake.get("target", {}).get("sigiro", {})
        if set(image_target.get("platforms", [])) != {"linux/amd64", "linux/arm64"}:
            raise RuntimeError("docker-bake sigiro platforms changed")
        if f"{IMAGE}:latest" not in image_target.get("tags", []):
            raise RuntimeError("docker-bake sigiro registry target changed")
        if not self.config.dry_run:
            self.runner.run(["docker", "info"])
            builder = self.runner.run(["docker", "buildx", "inspect", "multiplatform"], capture=True)
            if "linux/amd64" not in builder or "linux/arm64" not in builder:
                raise RuntimeError("multiplatform builder lacks required platforms")
            self.runner.run(["gh", "auth", "status"])
            self.docker_login()
            self.runner.run(["ssh", "-i", str(self.config.ssh_key), "-o", "BatchMode=yes", "root@91.98.118.235", "true"])
            self.runner.run(self._wrangler("whoami"), cwd=self.config.repo / "www")
            buckets = self.runner.run(self._wrangler("r2", "bucket", "list", "--json"), cwd=self.config.repo / "www", capture=True)
            if BUCKET not in buckets:
                raise RuntimeError(f"R2 bucket is unavailable: {BUCKET}")
            if self.http_get("https://api.sigiro.com/health")[0] != 200:
                raise RuntimeError("production API baseline is not healthy")
            self.runner.run(["git", "fetch", "origin", "main"], cwd=self.config.worldpol_repo)
            worldpol_head = self._git_ref(self.config.worldpol_repo, "origin/main")
            deploy = self.deploy_at_ref("origin/main")
            live = self.live_container()
        else:
            worldpol_head = self._git_ref(self.config.worldpol_repo, "origin/main")
            assert deploy is not None
            live = {"image": deploy["image"], "digest": "not-read-in-dry-run", "health": "not-read-in-dry-run"}
        assert deploy is not None
        return {
            "free_bytes": free,
            "target": str(resolved_target),
            "toolchain": actual_toolchain,
            "production_image": deploy["image"],
            "production_shape": "valid",
            "worldpol_head": worldpol_head,
            "live_baseline": live,
            "source_remote": source_remote,
            "worldpol_remote": worldpol_remote,
        }

    def phase_gate(self) -> dict[str, Any]:
        report = self.logs / "mbx-release-stats.json"
        self.runner.run(["mise", "run", "release"], cwd=self.config.repo, env={"MBX_STATS_REPORT": str(report)}, write=True)
        if self.config.dry_run:
            return {"mbx_report": str(report), "gate": "planned"}
        stats = validate_mbx_report(report)
        return {"mbx_report": str(report), "gate": "passed", "mbx": stats}

    def verify_gate(self, receipt: dict[str, Any]) -> bool:
        if receipt.get("gate") != "passed":
            return False
        try:
            validate_mbx_report(Path(receipt.get("mbx_report", "")))
            return True
        except Exception:
            return False

    def phase_image(self) -> dict[str, Any]:
        immutable = f"{IMAGE}:{self.tag}"
        if self.config.dry_run:
            return {"immutable": immutable, "promotion": "planned"}
        self.runner.run(["git", "fetch", "origin", "main"], cwd=self.config.source_repo)
        if self._git_ref(self.config.source_repo, "origin/main") != self.sha:
            raise RuntimeError("origin/main moved during the release gate; refusing publication")
        existing = self.inspect_index(immutable, optional=True)
        if not existing:
            self.runner.run(
                ["mise", "run", "image", "--", "--set", f"*.tags={immutable}", "--set", f"*.labels=org.opencontainers.image.revision={self.sha}", "--push"],
                cwd=self.config.repo,
                write=True,
            )
            existing = self.inspect_index(immutable)
        assert existing is not None
        if set(existing["platforms"]) != {"linux/amd64", "linux/arm64"}:
            raise RuntimeError("immutable image has unexpected platforms")
        self.smoke_image(existing["platforms"]["linux/amd64"])
        self.checkpoint(
            "image",
            candidate_index_digest=existing["digest"],
            candidate_amd64_digest=existing["platforms"]["linux/amd64"],
        )
        self.runner.run(
            ["docker", "buildx", "imagetools", "create", "--tag", f"{IMAGE}:{VERSION}", "--tag", f"{IMAGE}:latest", immutable],
            write=True,
        )
        checked = self.verify_registry()
        return {"immutable": immutable, **checked}

    def verify_image(self, receipt: dict[str, Any]) -> bool:
        if not DIGEST_RE.fullmatch(str(receipt.get("index_digest", ""))):
            return False
        if self.config.dry_run:
            return False
        try:
            checked = self.verify_registry()
        except Exception:
            return False
        return checked["index_digest"] == receipt["index_digest"]

    def phase_binaries(self) -> dict[str, Any]:
        self.artifacts.mkdir(parents=True, exist_ok=True)
        if self.config.dry_run:
            return {"targets": list(TARGETS), "artifacts": "planned"}
        self.runner.run(["docker", "buildx", "bake", "dist"], cwd=self.config.repo, write=True)
        for target in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
            self.runner.run(["mise", "run", "build"], cwd=self.config.repo, env={"TARGET": target}, write=True)
        sources = {
            "x86_64-unknown-linux-gnu": self.config.repo / "dist-bin/linux_amd64/sigiro",
            "aarch64-unknown-linux-gnu": self.config.repo / "dist-bin/linux_arm64/sigiro",
            "aarch64-apple-darwin": self.config.repo / "target/aarch64-apple-darwin/release/sigiro",
            "x86_64-apple-darwin": self.config.repo / "target/x86_64-apple-darwin/release/sigiro",
        }
        checksums: dict[str, str] = {}
        for target, binary in sources.items():
            self.require_executable(binary)
            description = self.runner.run(["file", "-b", str(binary)], capture=True)
            expected = "arm64" if target.startswith("aarch64-apple") else "x86_64" if target.startswith("x86_64-apple") else "aarch64" if target.startswith("aarch64") else "x86-64"
            if expected.lower() not in description.lower():
                raise RuntimeError(f"wrong architecture for {target}: {description}")
            archive = self.artifacts / f"sigiro-{VERSION}-{target}.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                bundle.add(binary, arcname="sigiro")
            self.verify_archive(archive)
            checksums[archive.name] = sha256_file(archive)
        sums = self.artifacts / "SHA256SUMS"
        sums.write_text("".join(f"{digest}  {name}\n" for name, digest in sorted(checksums.items())), encoding="utf-8")
        return {"checksums": checksums, "sha256sums": sha256_file(sums)}

    def verify_binaries(self, receipt: dict[str, Any]) -> bool:
        checksums = receipt.get("checksums", {})
        return len(checksums) == 4 and all((self.artifacts / name).is_file() and sha256_file(self.artifacts / name) == digest for name, digest in checksums.items())

    def phase_r2(self) -> dict[str, Any]:
        checksums = self.state["phases"]["binaries"].get("checksums", {})
        if len(checksums) != 4 and not self.config.dry_run:
            raise RuntimeError("binary receipt is incomplete")
        stage = f"staging/{self.sha}"
        if self.config.dry_run:
            return {"stage": stage, "version": VERSION, "promotion": "planned"}
        previous_sums = self.downloads / "previous-0.1.0-SHA256SUMS"
        self.r2_get(f"{VERSION}/SHA256SUMS", previous_sums)
        previous_checksum = sha256_file(previous_sums)
        self.checkpoint(
            "r2",
            previous_sha256sums=previous_checksum,
            previous_sha256sums_path=str(previous_sums),
        )
        names = list(checksums) + (["SHA256SUMS"] if checksums else [])
        for prefix in (stage, VERSION):
            for name in names:
                self.r2_put(f"{prefix}/{name}", self.artifacts / name)
            if not self.config.dry_run:
                self.verify_r2_prefix(prefix, checksums)
        latest = self.artifacts / "LATEST"
        latest.write_text(VERSION, encoding="utf-8")
        self.r2_put("LATEST", latest, content_type="text/plain")
        public = self.verify_public_downloads(checksums)
        return {
            "stage": stage,
            "version": VERSION,
            "checksums": checksums,
            "previous_sha256sums": previous_checksum,
            "previous_sha256sums_path": str(previous_sums),
            **public,
        }

    def verify_r2(self, receipt: dict[str, Any]) -> bool:
        if self.config.dry_run or receipt.get("version") != VERSION:
            return False
        try:
            self.verify_public_downloads(receipt.get("checksums", {}))
            return True
        except Exception:
            return False

    def phase_website(self) -> dict[str, Any]:
        www = self.config.repo / "www"
        if self.config.dry_run:
            return {"previous": "not-read-in-dry-run", "deployment": "planned"}
        previous = self.worker_deployment()
        self.runner.run(["mise", "exec", "bun@1.4.0", "--", "bun", "install", "--frozen-lockfile"], cwd=www, write=True)
        self.runner.run(["mise", "exec", "bun@1.4.0", "--", "bun", "run", "check"], cwd=www, write=True)
        self.runner.run(["mise", "exec", "bun@1.4.0", "--", "bun", "run", "build"], cwd=www, write=True)
        generated = www / "dist/server/wrangler.json"
        if not self.config.dry_run:
            self.verify_wrangler_config(generated)
        self.runner.run(["mise", "exec", "bun@1.4.0", "--", "bun", "run", "deploy"], cwd=www, write=True)
        try:
            current = self.worker_deployment()
            if current == previous:
                raise RuntimeError("Worker deployment did not change")
            probes = self.verify_site()
            return {"previous": previous, "deployment": current, "probes": probes}
        except Exception:
            if previous:
                self.runner.run(self._wrangler("rollback", previous["version_id"], "--name", "sigiro-www", "--yes"), cwd=www, write=True)
            raise

    def verify_website(self, receipt: dict[str, Any]) -> bool:
        if self.config.dry_run:
            return False
        try:
            return self.worker_deployment() == receipt.get("deployment") and bool(self.verify_site())
        except Exception:
            return False

    def phase_worldpol(self) -> dict[str, Any]:
        if self.config.dry_run:
            deploy = self.deploy_at_ref("origin/main")
            return {"base": self._git_ref(self.config.worldpol_repo, "origin/main"), "current_image": deploy["image"], "commit": "planned", "image": f"{IMAGE}:{self.tag}"}
        self.runner.run(["git", "fetch", "origin", "main"], cwd=self.config.worldpol_repo)
        base = require_sha(self.runner.run(["git", "rev-parse", "origin/main"], cwd=self.config.worldpol_repo, capture=True))
        baseline = self.state["phases"]["preflight"]["worldpol_head"]
        if base != baseline:
            raise RuntimeError("worldpol origin/main moved during the release; refusing cutover")
        if self.live_container() != self.state["phases"]["preflight"]["live_baseline"]:
            raise RuntimeError("live Sigiro container changed during the release; refusing cutover")
        with tempfile.TemporaryDirectory(prefix="sigiro-worldpol-") as tmp:
            worktree = Path(tmp) / "checkout"
            self.runner.run(["git", "worktree", "add", "--detach", str(worktree), base], cwd=self.config.worldpol_repo, write=True)
            try:
                deploy = worktree / "config/deploy.yml"
                current_deploy = parse_sigiro_deploy(deploy)
                if current_deploy["image"] == f"{IMAGE}:{self.tag}":
                    return {"base": base, "commit": base, "image": f"{IMAGE}:{self.tag}", "already_declared": True}
                update_sigiro_image(deploy, self.tag)
                changed = self.runner.run(["git", "diff", "--name-only"], cwd=worktree, capture=True)
                if changed != "config/deploy.yml":
                    raise RuntimeError(f"worldpol change escaped config/deploy.yml: {changed}")
                self.runner.run(["git", "diff", "--check"], cwd=worktree)
                diff = self.runner.run(["git", "diff", "--unified=0", "--", "config/deploy.yml"], cwd=worktree, capture=True)
                semantic = [line for line in diff.splitlines() if line.startswith(("+", "-")) and not line.startswith(("+++", "---"))]
                if len(semantic) != 2 or not all("image: ghcr.io/sigiroai/sigiro:" in line for line in semantic):
                    raise RuntimeError("worldpol semantic diff is not exactly one image replacement")
                if self.config.dry_run:
                    return {"base": base, "commit": "planned", "image": f"{IMAGE}:{self.tag}"}
                self.runner.run(["git", "add", "config/deploy.yml"], cwd=worktree, write=True)
                self.runner.run(["git", "commit", "-m", f"chore(deploy): bump sigiro-hosted to {self.tag}"], cwd=worktree, write=True)
                commit = require_sha(self.runner.run(["git", "rev-parse", "HEAD"], cwd=worktree, capture=True))
                self.runner.run(["git", "push", "origin", "HEAD:main"], cwd=worktree, write=True)
                self.runner.run(["git", "fetch", "origin", "main"], cwd=self.config.worldpol_repo)
                if self._git_ref(self.config.worldpol_repo, "origin/main") != commit:
                    raise RuntimeError("worldpol push readback does not match the created commit")
                return {"base": base, "commit": commit, "image": f"{IMAGE}:{self.tag}"}
            finally:
                self.runner.run(["git", "worktree", "remove", "--force", str(worktree)], cwd=self.config.worldpol_repo, write=True)

    def verify_worldpol(self, receipt: dict[str, Any]) -> bool:
        if self.config.dry_run:
            return False
        try:
            self.runner.run(["git", "fetch", "origin", "main"], cwd=self.config.worldpol_repo)
            if self._git_ref(self.config.worldpol_repo, "origin/main") != receipt.get("commit"):
                return False
            return self.deploy_at_ref("origin/main")["image"] == receipt.get("image")
        except Exception:
            return False

    def phase_kamal(self) -> dict[str, Any]:
        if self.config.dry_run:
            return {"accessory": "sigiro-hosted", "cutover": "planned"}
        commit = self.state["phases"]["worldpol"]["commit"]
        with tempfile.TemporaryDirectory(prefix="sigiro-kamal-") as tmp:
            worktree = Path(tmp) / "checkout"
            self.runner.run(["git", "worktree", "add", "--detach", str(worktree), commit], cwd=self.config.worldpol_repo, write=True)
            try:
                self.runner.run([str(self.config.kamal), "accessory", "reboot", "sigiro-hosted"], cwd=worktree, write=True)
            finally:
                self.runner.run(["git", "worktree", "remove", "--force", str(worktree)], cwd=self.config.worldpol_repo, write=True)
        return {"accessory": "sigiro-hosted", "cutover": "planned" if self.config.dry_run else "started"}

    def verify_kamal(self, _: dict[str, Any]) -> bool:
        if self.config.dry_run:
            return False
        try:
            state = self.live_container()
            return state["image"] == f"{IMAGE}:{self.tag}" and state["health"] == "healthy"
        except Exception:
            return False

    def phase_health(self) -> dict[str, Any]:
        if self.config.dry_run:
            return {"expected_image": f"{IMAGE}:{self.tag}", "health": "planned"}
        expected_digest = self.state["phases"]["image"]["amd64_digest"]
        reads = []
        for attempt in range(30):
            state = self.live_container()
            status, body = self.http_get("https://api.sigiro.com/health")
            if state["image"] == f"{IMAGE}:{self.tag}" and state["digest"] == expected_digest and state["health"] == "healthy" and status == 200:
                reads.append({"container": state, "http": status, "body": body})
                if len(reads) == 2:
                    return {"reads": reads, "image": state["image"], "digest": state["digest"], "health": "healthy", "http": 200}
                time.sleep(5)
            else:
                reads.clear()
                time.sleep(5)
        raise RuntimeError("new API did not become and remain healthy within 150 seconds")

    def verify_health(self, receipt: dict[str, Any]) -> bool:
        if self.config.dry_run:
            return False
        try:
            state = self.live_container()
            status, _ = self.http_get("https://api.sigiro.com/health")
            return state["image"] == f"{IMAGE}:{self.tag}" and state["digest"] == receipt.get("digest") and state["health"] == "healthy" and status == 200
        except Exception:
            return False

    def inspect_index(self, reference: str, optional: bool = False) -> dict[str, Any] | None:
        try:
            raw = self.runner.run(["docker", "buildx", "imagetools", "inspect", "--raw", reference], capture=True)
        except RuntimeError:
            if optional:
                return None
            raise
        manifest = json.loads(raw)
        digest = self.runner.run(["docker", "buildx", "imagetools", "inspect", "--format", "{{json .Manifest.Digest}}", reference], capture=True).strip('"')
        platforms: dict[str, str] = {}
        for item in manifest.get("manifests", []):
            platform_spec = item.get("platform", {})
            if platform_spec.get("os") == "unknown":
                continue
            platforms[f"{platform_spec.get('os')}/{platform_spec.get('architecture')}"] = item.get("digest")
        return {"digest": digest, "platforms": platforms}

    def verify_registry(self) -> dict[str, Any]:
        refs = [f"{IMAGE}:{self.tag}", f"{IMAGE}:{VERSION}", f"{IMAGE}:latest"]
        inspected = [self.inspect_index(ref) for ref in refs]
        assert all(inspected)
        digests = {item["digest"] for item in inspected if item}
        if len(digests) != 1:
            raise RuntimeError("registry tags do not resolve to one index digest")
        platforms = inspected[0]["platforms"]
        if set(platforms) != {"linux/amd64", "linux/arm64"}:
            raise RuntimeError(f"unexpected OCI platforms: {sorted(platforms)}")
        return {"index_digest": digests.pop(), "amd64_digest": platforms["linux/amd64"], "arm64_digest": platforms["linux/arm64"]}

    def smoke_image(self, amd64_digest: str) -> None:
        ref = f"{IMAGE}@{amd64_digest}"
        self.runner.run(["docker", "pull", "--platform", "linux/amd64", ref])
        revision = self.runner.run(["docker", "image", "inspect", "--format", '{{index .Config.Labels "org.opencontainers.image.revision"}}', ref], capture=True)
        if revision != self.sha:
            raise RuntimeError(f"published image revision label mismatch: {revision!r}")
        output = self.runner.run(["docker", "run", "--rm", "--platform", "linux/amd64", "--entrypoint", "/usr/local/bin/sigiro", ref, "--version"], capture=True)
        if VERSION not in output:
            raise RuntimeError(f"published image reported an unexpected version: {output}")

    def verify_archive(self, archive: Path) -> None:
        with tarfile.open(archive, "r:gz") as bundle:
            members = bundle.getmembers()
            if len(members) != 1 or members[0].name != "sigiro" or not members[0].isfile() or not members[0].mode & stat.S_IXUSR:
                raise RuntimeError(f"invalid release archive: {archive.name}")

    def require_executable(self, path: Path) -> None:
        if not path.is_file() or not os.access(path, os.X_OK):
            raise RuntimeError(f"release binary is missing or not executable: {path}")

    def r2_put(self, key: str, source: Path, content_type: str | None = None) -> None:
        args = self._wrangler("r2", "object", "put", f"{BUCKET}/{key}", "--remote", "--file", str(source), "--force")
        if content_type:
            args += ["--content-type", content_type]
        self.runner.run(args, cwd=self.config.repo / "www", write=True)

    def r2_get(self, key: str, destination: Path) -> None:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.unlink(missing_ok=True)
        self.runner.run(self._wrangler("r2", "object", "get", f"{BUCKET}/{key}", "--remote", "--file", str(destination)), cwd=self.config.repo / "www")

    def verify_r2_prefix(self, prefix: str, checksums: dict[str, str]) -> None:
        root = self.downloads / prefix.replace("/", "-")
        for name, expected in checksums.items():
            destination = root / name
            self.r2_get(f"{prefix}/{name}", destination)
            if sha256_file(destination) != expected:
                raise RuntimeError(f"R2 checksum mismatch: {prefix}/{name}")
        sums = root / "SHA256SUMS"
        self.r2_get(f"{prefix}/SHA256SUMS", sums)
        if sha256_file(sums) != sha256_file(self.artifacts / "SHA256SUMS"):
            raise RuntimeError(f"R2 SHA256SUMS mismatch: {prefix}")

    def verify_public_downloads(self, checksums: dict[str, str]) -> dict[str, Any]:
        status, latest = self.http_get("https://sigiro.com/dl/LATEST")
        if status != 200 or latest != VERSION:
            raise RuntimeError("public LATEST is incorrect")
        status, sums = self.http_get(f"https://sigiro.com/dl/{VERSION}/SHA256SUMS")
        if status != 200 or sums != (self.artifacts / "SHA256SUMS").read_text(encoding="utf-8").strip():
            raise RuntimeError("public SHA256SUMS is incorrect")
        for name, expected in checksums.items():
            destination = self.downloads / "public" / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            request = Request(f"https://sigiro.com/dl/{VERSION}/{name}", headers={"User-Agent": "sigiro-nightly-release"})
            with urlopen(request, timeout=60) as response, destination.open("wb") as handle:
                shutil.copyfileobj(response, handle)
            if sha256_file(destination) != expected:
                raise RuntimeError(f"public artifact checksum mismatch: {name}")
        installer_ok = self.verify_installer(checksums)
        return {"public_latest": latest, "installer": installer_ok}

    def verify_installer(self, checksums: dict[str, str]) -> str:
        machine = platform.machine().lower()
        target = "aarch64-apple-darwin" if machine in {"arm64", "aarch64"} else "x86_64-apple-darwin"
        archive_name = f"sigiro-{VERSION}-{target}.tar.gz"
        if archive_name not in checksums:
            raise RuntimeError(f"no installer artifact for host architecture {machine}")
        with tempfile.TemporaryDirectory(prefix="sigiro-install-") as tmp:
            install_dir = Path(tmp)
            script = install_dir / "install"
            status, content_bytes = self.http_bytes("https://sigiro.com/install")
            if status != 200:
                raise RuntimeError("public installer is unavailable")
            source_bytes = (self.config.repo / "www/public/install").read_bytes()
            if content_bytes != source_bytes:
                raise RuntimeError("public installer bytes differ from the release source")
            script.write_bytes(content_bytes)
            self.runner.run(["sh", str(script)], env={"SIGIRO_INSTALL_DIR": str(install_dir)})
            installed = install_dir / "sigiro"
            self.require_executable(installed)
            with tarfile.open(self.downloads / "public" / archive_name, "r:gz") as bundle:
                expected = bundle.extractfile("sigiro")
                assert expected
                if hashlib.sha256(expected.read()).hexdigest() != sha256_file(installed):
                    raise RuntimeError("installed binary differs from the published archive")
            output = self.runner.run([str(installed), "--version"], capture=True)
            if VERSION not in output:
                raise RuntimeError("installed binary reports an unexpected version")
        return "passed"

    def worker_deployment(self, optional: bool = False) -> dict[str, str] | None:
        if self.config.dry_run:
            return None
        try:
            raw = self.runner.run(self._wrangler("deployments", "list", "--name", "sigiro-www", "--json"), cwd=self.config.repo / "www", capture=True)
            data = json.loads(raw)
            if isinstance(data, list):
                if not data:
                    raise RuntimeError("no sigiro-www deployments found")
                latest = max(data, key=lambda item: item.get("created_on", ""))
            elif isinstance(data, dict):
                latest = data
            else:
                raise RuntimeError("unexpected Wrangler deployments response")
            versions = latest.get("versions", [])
            active = [item for item in versions if float(item.get("percentage", 0)) == 100]
            if len(active) != 1:
                raise RuntimeError("sigiro-www does not have one 100%-active version")
            version_id = active[0].get("version_id") or active[0].get("versionId")
            deployment_id = latest.get("id")
            if not version_id or not deployment_id:
                raise RuntimeError("active Worker deployment lacks IDs")
            return {"deployment_id": deployment_id, "version_id": version_id}
        except Exception:
            if optional:
                return None
            raise

    def verify_wrangler_config(self, path: Path) -> None:
        config = json.loads(path.read_text(encoding="utf-8"))
        if config.get("name") != "sigiro-www":
            raise RuntimeError("generated Worker name changed")
        patterns = {route.get("pattern") for route in config.get("routes", []) if isinstance(route, dict)}
        if patterns != {"sigiro.com/*", "www.sigiro.com/*"}:
            raise RuntimeError(f"generated Worker routes changed: {patterns}")
        bindings = {(item.get("binding"), item.get("bucket_name")) for item in config.get("r2_buckets", [])}
        if bindings != {("DOWNLOADS", BUCKET)}:
            raise RuntimeError(f"generated R2 binding changed: {bindings}")

    def verify_site(self) -> dict[str, int]:
        probes = {}
        for url in ("https://sigiro.com/", "https://sigiro.com/docs", "https://www.sigiro.com/", "https://sigiro.com/install"):
            status, _ = self.http_get(url)
            if status != 200:
                raise RuntimeError(f"website probe failed: {url} -> {status}")
            probes[url] = status
        status, latest = self.http_get("https://sigiro.com/dl/LATEST")
        if status != 200 or latest != VERSION:
            raise RuntimeError("website download route returned the wrong LATEST")
        probes["https://sigiro.com/dl/LATEST"] = status
        return probes

    def live_container(self) -> dict[str, str]:
        script = (
            "set -e; id=$(docker ps -q --filter name=^/linguabench-sigiro-hosted$); "
            "test -n \"$id\"; docker inspect --format "
            "'{{.Config.Image}}|{{json .RepoDigests}}|{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' \"$id\""
        )
        output = self.runner.run(["ssh", "-i", str(self.config.ssh_key), "-o", "BatchMode=yes", "root@91.98.118.235", script], capture=True)
        image, repo_digests_json, health = output.split("|", 2)
        repo_digests = json.loads(repo_digests_json)
        digest_matches = [value.rsplit("@", 1)[1] for value in repo_digests if value.startswith(f"{IMAGE}@")]
        if len(digest_matches) != 1:
            raise RuntimeError(f"live container has unexpected RepoDigests: {repo_digests}")
        return {"image": image, "digest": digest_matches[0], "health": health}

    def http_get(self, url: str) -> tuple[int, str]:
        status, body = self.http_bytes(url)
        return status, body.decode("utf-8").strip()

    def http_bytes(self, url: str) -> tuple[int, bytes]:
        request = Request(url, headers={"User-Agent": "sigiro-nightly-release"})
        with urlopen(request, timeout=30) as response:
            return response.status, response.read()

    def _wrangler(self, *args: str) -> list[str]:
        return ["mise", "exec", "bun@1.4.0", "--", "bunx", "wrangler", *args]

    def docker_login(self) -> None:
        token = subprocess.check_output(["gh", "auth", "token"], text=True).strip()
        result = subprocess.run(
            ["docker", "login", "ghcr.io", "--username", "akshatagarwl", "--password-stdin"],
            input=token + "\n",
            text=True,
            capture_output=True,
            check=False,
        )
        self.runner.note(self.runner._redact((result.stdout or "") + (result.stderr or "")))
        if result.returncode:
            raise RuntimeError("Docker login to ghcr.io failed")

    def deploy_at_ref(self, ref: str) -> dict[str, Any]:
        content = self.runner.run(["git", "show", f"{ref}:config/deploy.yml"], cwd=self.config.worldpol_repo, capture=True)
        with tempfile.NamedTemporaryFile("w", suffix=".yml", delete=False) as handle:
            handle.write(content + "\n")
            path = Path(handle.name)
        try:
            return parse_sigiro_deploy(path)
        finally:
            path.unlink()

    def _git_head(self, path: Path) -> str:
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=path, text=True).strip()

    def _git_ref(self, path: Path, ref: str) -> str:
        return subprocess.check_output(["git", "rev-parse", ref], cwd=path, text=True).strip()

    def _git_dirty(self, path: Path) -> bool:
        return bool(subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=path, text=True).strip())


def resolve_source(config: Config, requested: str | None) -> str:
    if requested:
        return require_sha(requested)
    if not config.dry_run:
        subprocess.run(["git", "fetch", "origin", "main"], cwd=config.source_repo, check=True)
    sha = subprocess.check_output(["git", "rev-parse", "origin/main"], cwd=config.source_repo, text=True).strip()
    return require_sha(sha)


def prepare_release_worktree(config: Config, sha: str) -> None:
    """Move only the dedicated release worktree to the fetched source commit."""
    if not config.repo.is_dir():
        config.repo.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "worktree", "add", "--detach", str(config.repo), sha],
            cwd=config.source_repo,
            check=True,
        )
        return
    common = subprocess.check_output(
        ["git", "rev-parse", "--git-common-dir"], cwd=config.repo, text=True
    ).strip()
    if Path(common).resolve() != (config.source_repo / ".git").resolve():
        raise RuntimeError("release checkout is not registered to the expected source repository")
    if config.dry_run:
        if subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=config.repo, text=True).strip() != sha:
            raise RuntimeError("dry-run release checkout is not at the requested source SHA")
        return
    subprocess.run(["git", "reset", "--hard", sha], cwd=config.repo, check=True)
    # No -x/-X: ignored mbx targets and node_modules must remain warm.
    subprocess.run(["git", "clean", "-fd"], cwd=config.repo, check=True)


def build_config(args: argparse.Namespace) -> Config:
    allow = os.environ.get("SIGIRO_RELEASE_ALLOW_WRITES") == "1"
    if not args.dry_run and not allow:
        raise RuntimeError("real release requires SIGIRO_RELEASE_ALLOW_WRITES=1")
    return Config(
        repo=args.repo.resolve(),
        source_repo=args.source_repo.resolve(),
        worldpol_repo=args.worldpol_repo.resolve(),
        state_root=args.state_dir.resolve(),
        lock_path=args.lock.resolve(),
        toolchain_file=args.toolchain_file.resolve(),
        kamal=args.kamal.resolve(),
        ssh_key=args.ssh_key.resolve(),
        dry_run=args.dry_run,
        allow_writes=allow,
        min_free_gib=args.min_free_gib,
        duckdb_cache=args.duckdb_cache.resolve(),
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    home = Path.home()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--source")
    parser.add_argument("--repo", type=Path, default=home / ".hermes/workflows/sigiro/worktrees/nightly-release")
    parser.add_argument("--source-repo", type=Path, default=home / "workspace/sigiro")
    parser.add_argument("--worldpol-repo", type=Path, default=home / "workspace/worldpol")
    parser.add_argument("--state-dir", type=Path, default=home / ".hermes/workflows/sigiro/nightly-release")
    parser.add_argument("--lock", type=Path, default=home / ".hermes/workflows/sigiro/nightly-release.lock")
    parser.add_argument("--toolchain-file", type=Path, default=home / ".hermes/workflows/sigiro/nightly-release-cache-toolchain.txt")
    parser.add_argument("--kamal", type=Path, default=Path("/opt/homebrew/lib/ruby/gems/4.0.0/bin/kamal"))
    parser.add_argument("--ssh-key", type=Path, default=home / ".ssh/linguabench")
    parser.add_argument("--min-free-gib", type=float, default=30.0, help=argparse.SUPPRESS)
    parser.add_argument("--duckdb-cache", type=Path, default=home / "Library/Caches/sigiro/duckdb-static")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    try:
        args = parse_args(argv)
        config = build_config(args)
        os.environ["DUCKDB_STATIC_CACHE"] = str(config.duckdb_cache)
        with DurableLock(config.lock_path):
            sha = resolve_source(config, args.source)
            prepare_release_worktree(config, sha)
            release = Release(config, sha)
            release.execute()
        outcome = "DRY-RUN" if config.dry_run else "DEPLOYED"
        if config.dry_run:
            print(f"{outcome}: Sigiro {sha[:9]} ({sha}); all phases planned; receipt={release.state_path}")
        else:
            phases = release.state["phases"]
            image = phases["image"]
            binaries = phases["binaries"]
            website = phases["website"]
            health = phases["health"]
            print(f"{outcome}: Sigiro {sha[:9]} ({sha})")
            print(f"registry: {image['index_digest']} amd64={image['amd64_digest']}")
            print(f"R2 {VERSION}: " + " ".join(f"{name}={digest}" for name, digest in sorted(binaries["checksums"].items())))
            print(f"installer: {phases['r2']['installer']}; worker={website['deployment']['deployment_id']} version={website['deployment']['version_id']}")
            print(f"worldpol={phases['worldpol']['commit']}; live={health['image']} {health['digest']} {health['health']} HTTP {health['http']}")
            print(f"receipt={release.state_path}")
        return 0
    except Exception as exc:
        print(f"ABORTED: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Run one controlled A/A + A/B/B/A inference experiment.

AIPerf owns workload replay, warm-up, goodput, and confidence intervals. This
script owns process isolation, immutable fingerprints, Sigiro evidence, and the
six-period reversal sequence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import signal
import string
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


PERIODS = (
    ("A0-1", "control"),
    ("A0-2", "control"),
    ("A1", "control"),
    ("B1", "candidate"),
    ("B2", "candidate"),
    ("A2", "control"),
)
AGGREGATE_NAME = "profile_export_aiperf_aggregate.json"
# Collectors whose state must be declared. AIPerf defaults both to ON — its
# `no_gpu_telemetry` and `no_server_metrics` flags are `= False` — so an
# undeclared manifest is silently instrumented, and two arms that differ here
# are not measuring the same machine.
REQUIRED_COLLECTORS = {"gpu_telemetry", "server_metrics"}
# Declared state must match the flags actually passed, or the declaration is
# decoration. Maps a collector to the AIPerf flag that disables it.
COLLECTOR_DISABLE_FLAGS = {
    "gpu_telemetry": "--no-gpu-telemetry",
    "server_metrics": "--no-server-metrics",
}
RESERVED_AIPERF_FLAGS = {
    "--artifact-dir",
    "--custom-dataset-type",
    "--dataset-sampling-strategy",
    "--failed-request-threshold",
    "--input-file",
    "--model",
    "--no-vary-seed-per-trial",
    "--num-profile-runs",
    "--otel-resource-attributes",
    "--otel-url",
    "--output-artifact-dir",
    "--streaming",
    "--url",
    "--vary-seed-per-trial",
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def expect_object(value: Any, name: str, keys: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{name} must be an object")
    unknown = set(value) - keys
    missing = keys - set(value)
    if unknown:
        fail(f"{name} has unknown fields: {', '.join(sorted(unknown))}")
    if missing:
        fail(f"{name} is missing fields: {', '.join(sorted(missing))}")
    return value


def expect_string(value: Any, name: str) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{name} must be a non-empty string")
    return value


def expect_number(value: Any, name: str, minimum: float = 0.0) -> float:
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(value)
        or value < minimum
    ):
        fail(f"{name} must be a number >= {minimum}")
    return float(value)


def expect_port(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 65535:
        fail(f"{name} must be an integer from 1 to 65535")
    return value


def expect_argv(value: Any, name: str) -> list[str]:
    if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
        fail(f"{name} must be a non-empty string array")
    return value


def expect_string_array(value: Any, name: str) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        fail(f"{name} must be a string array")
    return value


def expect_env(value: Any, name: str) -> dict[str, str]:
    if not isinstance(value, dict) or not all(
        isinstance(key, str) and key and isinstance(item, str) for key, item in value.items()
    ):
        fail(f"{name} must be a string-to-string object")
    secret_markers = ("TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL")
    secret_names = sorted(key for key in value if any(marker in key.upper() for marker in secret_markers))
    if secret_names:
        fail(
            f"{name} contains credential-like fields: {', '.join(secret_names)}; "
            "pass credentials through the inherited environment instead of the persisted manifest"
        )
    return value


def expect_telemetry(value: Any, name: str) -> dict[str, bool]:
    """Which collectors were active. Enabling one changes the performance being
    measured, so this belongs in the run identity rather than in tribal memory."""
    if not isinstance(value, dict) or not all(
        isinstance(key, str) and key and isinstance(item, bool) for key, item in value.items()
    ):
        fail(f"{name} must be a string-to-boolean object")
    missing = REQUIRED_COLLECTORS - set(value)
    if missing:
        fail(f"{name} must declare every known collector; missing: {', '.join(sorted(missing))}")
    return value


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def resolve_path(base: Path, value: Any, name: str) -> Path:
    path = Path(expect_string(value, name)).expanduser()
    return path.resolve() if path.is_absolute() else (base / path).resolve()


def parse_manifest(path: Path) -> dict[str, Any]:
    try:
        raw = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read manifest {path}: {error}")
    root = expect_object(
        raw,
        "manifest",
        {
            "schema_version",
            "experiment_id",
            "output_root",
            "model",
            "hardware",
            "sigiro",
            "aiperf",
            "control",
            "candidate",
            "telemetry",
            "telemetry_flush_seconds",
        },
    )
    if root["schema_version"] != 1:
        fail("manifest.schema_version must be 1")
    experiment_id = expect_string(root["experiment_id"], "manifest.experiment_id")
    allowed_id = set(string.ascii_letters + string.digits + "._-")
    if len(experiment_id) > 128 or any(character not in allowed_id for character in experiment_id):
        fail("manifest.experiment_id must be 1-128 ASCII letters, digits, dots, underscores, or hyphens")
    base = path.parent.resolve()
    root["output_root"] = resolve_path(base, root["output_root"], "manifest.output_root")

    model = expect_object(root["model"], "manifest.model", {"id", "revision"})
    expect_string(model["id"], "manifest.model.id")
    expect_string(model["revision"], "manifest.model.revision")

    if not isinstance(root["hardware"], dict):
        fail("manifest.hardware must be an object")
    hardware = expect_object(root["hardware"], "manifest.hardware", set(root["hardware"]))
    if set(hardware) not in ({"command"}, {"file"}):
        fail("manifest.hardware must contain exactly one of command or file")
    if "command" in hardware:
        hardware["command"] = expect_argv(hardware["command"], "manifest.hardware.command")
    else:
        hardware["file"] = resolve_path(base, hardware["file"], "manifest.hardware.file")

    sigiro = expect_object(
        root["sigiro"],
        "manifest.sigiro",
        {"binary", "http_port", "otlp_http_port", "otlp_grpc_port", "startup_timeout_seconds"},
    )
    expect_string(sigiro["binary"], "manifest.sigiro.binary")
    for key in ("http_port", "otlp_http_port", "otlp_grpc_port"):
        sigiro[key] = expect_port(sigiro[key], f"manifest.sigiro.{key}")
    if len({sigiro["http_port"], sigiro["otlp_http_port"], sigiro["otlp_grpc_port"]}) != 3:
        fail("manifest.sigiro ports must be distinct")
    sigiro["startup_timeout_seconds"] = expect_number(
        sigiro["startup_timeout_seconds"], "manifest.sigiro.startup_timeout_seconds", 0.1
    )

    aiperf = expect_object(
        root["aiperf"],
        "manifest.aiperf",
        {"binary", "endpoint", "inputs_file", "num_profile_runs", "common_args", "timeout_seconds"},
    )
    expect_string(aiperf["binary"], "manifest.aiperf.binary")
    expect_string(aiperf["endpoint"], "manifest.aiperf.endpoint")
    aiperf["inputs_file"] = resolve_path(base, aiperf["inputs_file"], "manifest.aiperf.inputs_file")
    if (
        isinstance(aiperf["num_profile_runs"], bool)
        or not isinstance(aiperf["num_profile_runs"], int)
        or not 2 <= aiperf["num_profile_runs"] <= 10
    ):
        fail("manifest.aiperf.num_profile_runs must be an integer from 2 to 10")
    aiperf["common_args"] = expect_string_array(aiperf["common_args"], "manifest.aiperf.common_args")
    for argument in aiperf["common_args"]:
        option = argument.split("=", 1)[0]
        if option in RESERVED_AIPERF_FLAGS:
            fail(f"manifest.aiperf.common_args cannot override runner flag {option}")
    aiperf["timeout_seconds"] = expect_number(
        aiperf["timeout_seconds"], "manifest.aiperf.timeout_seconds", 1.0
    )

    for arm_name in ("control", "candidate"):
        arm = expect_object(root[arm_name], f"manifest.{arm_name}", {"argv", "env"})
        arm["argv"] = expect_argv(arm["argv"], f"manifest.{arm_name}.argv")
        arm["env"] = expect_env(arm["env"], f"manifest.{arm_name}.env")

    root["telemetry"] = expect_telemetry(root["telemetry"], "manifest.telemetry")
    for collector, disable_flag in COLLECTOR_DISABLE_FLAGS.items():
        disabled = disable_flag in aiperf["common_args"]
        if root["telemetry"][collector] == disabled:
            fail(
                f"manifest.telemetry.{collector} is {root['telemetry'][collector]} but "
                f"{disable_flag} is {'present' if disabled else 'absent'} in "
                "manifest.aiperf.common_args; the declaration must match the command"
            )

    root["telemetry_flush_seconds"] = expect_number(
        root["telemetry_flush_seconds"], "manifest.telemetry_flush_seconds", 0.0
    )
    if not aiperf["inputs_file"].is_file():
        fail(f"AIPerf inputs file does not exist: {aiperf['inputs_file']}")
    try:
        inputs = json.loads(aiperf["inputs_file"].read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"invalid AIPerf inputs file {aiperf['inputs_file']}: {error}")
    if not isinstance(inputs, dict) or not isinstance(inputs.get("data"), list) or not inputs["data"]:
        fail("AIPerf inputs file must contain a non-empty data array")
    return root


def process_output(command: list[str], cwd: Path, timeout: float) -> tuple[int, str, str]:
    process = subprocess.Popen(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        stdout, stderr = process.communicate()
        fail(f"command timed out after {timeout}s: {' '.join(command)}\n{stderr}")
    return process.returncode, stdout, stderr


def stop_process(process: subprocess.Popen[Any] | None) -> None:
    if process is None or process.poll() is not None:
        return
    for sig, timeout in ((signal.SIGINT, 20), (signal.SIGTERM, 5), (signal.SIGKILL, 5)):
        try:
            os.killpg(process.pid, sig)
            process.wait(timeout=timeout)
            return
        except ProcessLookupError:
            return
        except subprocess.TimeoutExpired:
            continue


def wait_ready(base_url: str, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    last_error = ""
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{base_url}/health", timeout=2) as response:
                body = json.loads(response.read())
                if body.get("status") == "ready":
                    return
                last_error = json.dumps(body)
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            last_error = str(error)
        time.sleep(0.25)
    fail(f"Sigiro did not become ready within {timeout}s: {last_error}")


def read_hardware(manifest: dict[str, Any], cwd: Path) -> bytes:
    hardware = manifest["hardware"]
    if "file" in hardware:
        try:
            return hardware["file"].read_bytes()
        except OSError as error:
            fail(f"cannot read hardware fingerprint file {hardware['file']}: {error}")
    code, stdout, stderr = process_output(hardware["command"], cwd, 30)
    if code != 0:
        fail(f"hardware fingerprint command failed with {code}:\n{stderr}")
    if not stdout:
        fail("hardware fingerprint command produced no stdout")
    return stdout.encode()


def aiperf_version(binary: str, cwd: Path) -> str:
    code, stdout, stderr = process_output([binary, "--version"], cwd, 30)
    if code != 0:
        fail(f"AIPerf version check failed with {code}:\n{stderr}")
    version = stdout.strip()
    if version != "0.12.0":
        fail(f"AIPerf 0.12.0 is required, got: {version or stderr.strip()}")
    return version


def aiperf_command(
    manifest: dict[str, Any],
    period_dir: Path,
    run_id: str,
    arm: str,
    fingerprints: dict[str, str],
) -> list[str]:
    config = manifest["aiperf"]
    attrs = [
        f"optimization.experiment.id={manifest['experiment_id']}",
        f"optimization.run.id={run_id}",
        f"optimization.arm={arm}",
        f"optimization.config.sha256={fingerprints[arm]}",
        f"optimization.model.sha256={fingerprints['model']}",
        f"optimization.hardware.sha256={fingerprints['hardware']}",
        f"optimization.workload.sha256={fingerprints['workload']}",
        f"optimization.benchmark.sha256={fingerprints['benchmark']}",
    ]
    return [
        config["binary"],
        "profile",
        "--model",
        manifest["model"]["id"],
        "--url",
        config["endpoint"],
        "--streaming",
        "--input-file",
        str(config["inputs_file"]),
        "--custom-dataset-type",
        "inputs_json",
        "--dataset-sampling-strategy",
        "sequential",
        "--num-profile-runs",
        str(config["num_profile_runs"]),
        "--no-vary-seed-per-trial",
        "--failed-request-threshold",
        "0",
        "--otel-url",
        f"http://127.0.0.1:{manifest['sigiro']['otlp_http_port']}",
        "--otel-resource-attributes",
        *attrs,
        "--output-artifact-dir",
        str(period_dir / "aiperf"),
        *config["common_args"],
    ]


def locate_aggregate(period_dir: Path) -> Path:
    matches = list((period_dir / "aiperf").rglob(AGGREGATE_NAME))
    if len(matches) != 1:
        fail(f"expected one {AGGREGATE_NAME} for {period_dir.name}, found {len(matches)}")
    return matches[0]


def reject_request_errors(aggregate: dict[str, Any], run_id: str) -> None:
    metrics = aggregate.get("metrics")
    if not isinstance(metrics, dict):
        return
    for name in ("request_error_rate_avg", "error_request_count_avg"):
        metric = metrics.get(name)
        if isinstance(metric, dict) and metric.get("mean") != 0:
            fail(f"{run_id} AIPerf artifact reports nonzero {name}")


def append_ledger(path: Path, experiment_id: str, record: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        for number, line in enumerate(path.read_text().splitlines(), start=1):
            if not line.strip():
                continue
            try:
                existing = json.loads(line)
            except json.JSONDecodeError as error:
                fail(f"invalid ledger JSON at {path}:{number}: {error}")
            if existing.get("experiment_id") == experiment_id:
                fail(f"experiment_id {experiment_id} already exists in {path}")
    with path.open("a") as ledger:
        ledger.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n")


def ledger_contains(path: Path, experiment_id: str) -> bool:
    if not path.exists():
        return False
    for number, line in enumerate(path.read_text().splitlines(), start=1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid ledger JSON at {path}:{number}: {error}")
        if record.get("experiment_id") == experiment_id:
            return True
    return False


def run(manifest_path: Path) -> dict[str, Any]:
    manifest_path = manifest_path.resolve()
    manifest = parse_manifest(manifest_path)
    cwd = manifest_path.parent
    output_root: Path = manifest["output_root"]
    ledger_path = output_root / "experiments.jsonl"
    # Check the durable ID before creating a partial artifact directory.
    if ledger_contains(ledger_path, manifest["experiment_id"]):
        fail(f"experiment_id {manifest['experiment_id']} already exists in {ledger_path}")
    experiment_dir = output_root / manifest["experiment_id"]
    if experiment_dir.exists():
        fail(f"experiment artifact directory already exists: {experiment_dir}")
    experiment_dir.mkdir(parents=True)
    status_path = experiment_dir / "status.json"

    observer: subprocess.Popen[Any] | None = None
    target: subprocess.Popen[Any] | None = None
    observer_log = None
    target_log = None
    try:
        hardware = read_hardware(manifest, cwd)
        version = aiperf_version(manifest["aiperf"]["binary"], cwd)
        fingerprints = {
            "model": sha256_bytes(canonical_bytes(manifest["model"])),
            "hardware": sha256_bytes(hardware),
            "workload": sha256_bytes(manifest["aiperf"]["inputs_file"].read_bytes()),
            "benchmark": sha256_bytes(
                canonical_bytes(
                    {
                        "version": version,
                        "model": manifest["model"]["id"],
                        "endpoint": manifest["aiperf"]["endpoint"],
                        "num_profile_runs": manifest["aiperf"]["num_profile_runs"],
                        "runner_contract": [
                            "profile",
                            "streaming",
                            "inputs_json_sequential",
                            "consistent_seed",
                            "failed_request_threshold_zero",
                            "otel_metrics",
                        ],
                        "common_args": manifest["aiperf"]["common_args"],
                        # Enabling a collector changes the performance being
                        # recorded, so two runs instrumented differently are
                        # different configurations and must not compare equal.
                        "telemetry": manifest["telemetry"],
                    }
                )
            ),
            "control": sha256_bytes(canonical_bytes(manifest["control"])),
            "candidate": sha256_bytes(canonical_bytes(manifest["candidate"])),
        }
        if fingerprints["control"] == fingerprints["candidate"]:
            fail("control and candidate launch fingerprints are identical")
        (experiment_dir / "hardware.txt").write_bytes(hardware)
        snapshot = json.loads(json.dumps(manifest, default=str))
        (experiment_dir / "manifest.json").write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
        status_path.write_text(json.dumps({"status": "running"}, indent=2) + "\n")

        data_dir = experiment_dir / "sigiro-data"
        extension_dir = output_root / ".sigiro-extensions"
        data_dir.mkdir()
        extension_dir.mkdir(parents=True, exist_ok=True)
        sigiro_env = os.environ.copy()
        for key in ("SIGIRO_API_KEY", "SIGIRO_CATALOG_URL", "SIGIRO_ADMIN_TOKEN", "SIGIRO_DATA_DIR"):
            sigiro_env.pop(key, None)
        sigiro_env["SIGIRO_DATA_DIR"] = str(data_dir)
        sigiro_env["SIGIRO_EXTENSION_DIR"] = str(extension_dir)
        sigiro = manifest["sigiro"]
        prewarm = subprocess.run(
            [sigiro["binary"], "fetch-extensions"],
            cwd=cwd,
            env=sigiro_env,
            text=True,
            capture_output=True,
            timeout=180,
        )
        (experiment_dir / "fetch-extensions.stdout").write_text(prewarm.stdout)
        (experiment_dir / "fetch-extensions.stderr").write_text(prewarm.stderr)
        if prewarm.returncode != 0:
            fail(f"Sigiro extension prewarm failed with status {prewarm.returncode}: {prewarm.stderr}")

        observer_log = (experiment_dir / "sigiro.log").open("w")
        observer = subprocess.Popen(
            [
                sigiro["binary"],
                "serve",
                "--data-dir",
                str(data_dir),
                "--http-port",
                str(sigiro["http_port"]),
                "--otlp-http-port",
                str(sigiro["otlp_http_port"]),
                "--otlp-grpc-port",
                str(sigiro["otlp_grpc_port"]),
            ],
            cwd=cwd,
            env=sigiro_env,
            stdout=observer_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        base_url = f"http://127.0.0.1:{sigiro['http_port']}"
        wait_ready(base_url, sigiro["startup_timeout_seconds"])

        period_results: dict[str, dict[str, Any]] = {}
        for run_id, arm in PERIODS:
            period_dir = experiment_dir / run_id
            period_dir.mkdir()
            arm_config = manifest[arm]
            target_log = (period_dir / "target.log").open("w")
            target_env = os.environ.copy()
            target_env.update(arm_config["env"])
            from_ts = time.time_ns() // 1_000
            target = subprocess.Popen(
                arm_config["argv"],
                cwd=cwd,
                env=target_env,
                stdout=target_log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            time.sleep(0.1)
            if target.poll() is not None:
                fail(f"{run_id} target exited before AIPerf with status {target.returncode}")
            command = aiperf_command(manifest, period_dir, run_id, arm, fingerprints)
            code, stdout, stderr = process_output(command, cwd, manifest["aiperf"]["timeout_seconds"])
            (period_dir / "aiperf.stdout").write_text(stdout)
            (period_dir / "aiperf.stderr").write_text(stderr)
            if code != 0:
                fail(f"{run_id} AIPerf failed with status {code}; see {period_dir}")
            time.sleep(manifest["telemetry_flush_seconds"])
            to_ts = time.time_ns() // 1_000
            aggregate_path = locate_aggregate(period_dir)
            try:
                aggregate = json.loads(aggregate_path.read_text())
            except (OSError, json.JSONDecodeError) as error:
                fail(f"{run_id} has invalid aggregate artifact {aggregate_path}: {error}")
            if not isinstance(aggregate, dict):
                fail(f"{run_id} aggregate artifact must be a JSON object")
            reject_request_errors(aggregate, run_id)
            period_results[run_id] = {
                "run_id": run_id,
                "from_ts": from_ts,
                "to_ts": to_ts,
                "aiperf": aggregate,
            }
            stop_process(target)
            target = None
            target_log.close()
            target_log = None

        request = {
            "experiment_id": manifest["experiment_id"],
            "model_fingerprint": fingerprints["model"],
            "hardware_fingerprint": fingerprints["hardware"],
            "workload_fingerprint": fingerprints["workload"],
            "benchmark_fingerprint": fingerprints["benchmark"],
            "control_config_fingerprint": fingerprints["control"],
            "candidate_config_fingerprint": fingerprints["candidate"],
            "calibration_control": [period_results["A0-1"], period_results["A0-2"]],
            "control_before": period_results["A1"],
            "candidate": [period_results["B1"], period_results["B2"]],
            "control_after": period_results["A2"],
        }
        request_path = experiment_dir / "comparison-request.json"
        request_path.write_text(json.dumps(request, indent=2, sort_keys=True) + "\n")
        cli_env = os.environ.copy()
        cli_env["SIGIRO_ENDPOINT"] = base_url
        cli_env.pop("SIGIRO_API_KEY", None)
        compare = subprocess.run(
            [sigiro["binary"], "compare-experiment", "--file", str(request_path)],
            cwd=cwd,
            env=cli_env,
            text=True,
            capture_output=True,
            timeout=120,
        )
        (experiment_dir / "comparison.stdout").write_text(compare.stdout)
        (experiment_dir / "comparison.stderr").write_text(compare.stderr)
        if compare.returncode != 0:
            fail(f"Sigiro comparison failed with status {compare.returncode}: {compare.stderr}")
        try:
            comparison = json.loads(compare.stdout)
        except json.JSONDecodeError as error:
            fail(f"Sigiro comparison returned invalid JSON: {error}")
        if comparison.get("experiment_id") != manifest["experiment_id"]:
            fail("Sigiro comparison returned the wrong experiment_id")
        if len(comparison.get("periods", [])) != 6:
            fail("Sigiro comparison did not verify all six periods")
        (experiment_dir / "comparison.json").write_text(
            json.dumps(comparison, indent=2, sort_keys=True) + "\n"
        )
        record = {
            "experiment_id": manifest["experiment_id"],
            "completed_at": time.time_ns() // 1_000,
            "artifact_dir": str(experiment_dir),
            "fingerprints": fingerprints,
            "outcome": comparison["outcome"],
            "winner": comparison.get("winner"),
            "comparison": comparison,
        }
        append_ledger(ledger_path, manifest["experiment_id"], record)
        status_path.write_text(json.dumps({"status": "completed"}, indent=2) + "\n")
        return record
    except BaseException as error:
        message = str(error) or type(error).__name__
        status_path.write_text(
            json.dumps({"status": "failed", "error": message}, indent=2, sort_keys=True) + "\n"
        )
        raise
    finally:
        stop_process(target)
        if target_log is not None:
            target_log.close()
        stop_process(observer)
        if observer_log is not None:
            observer_log.close()


def main() -> int:
    def interrupted(_signum: int, _frame: Any) -> None:
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description="Run one paired inference configuration experiment")
    parser.add_argument("manifest", type=Path)
    args = parser.parse_args()
    try:
        record = run(args.manifest)
    except KeyboardInterrupt:
        print("error: experiment interrupted", file=sys.stderr)
        return 130
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(json.dumps(record, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

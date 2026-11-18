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


# One running sidecar: its manifest name, the process, and its open log file.
Sidecar = tuple[str, subprocess.Popen[Any], Any]

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
# Characters allowed in an identifier the runner turns into a path component.
ALLOWED_ID_CHARS = frozenset(string.ascii_letters + string.digits + "._-")
# Deployment facts the runner cannot observe, split by the shape they must have.
# Every one is a column of `sigiro_experiment_runs`; the names must match
# exactly because they are passed straight through to `sigiro record-run`, which
# rejects unknown fields.
TOPOLOGY_STRINGS = ("engine", "engine_version", "gpu_model", "quantization")
TOPOLOGY_COUNTS = ("gpu_count", "tp_size")
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


def expect_roofline(value: Any, name: str) -> dict[str, Any] | None:
    """Inputs to an analytic decode floor, in an accelerator-neutral shape.

    Optional, because a mock target has no floor to be near; complete when present.

    The floor is **the minimum over resources**, not a memory-bandwidth figure. An
    earlier version of this block took a single `peak_memory_bandwidth_bytes_per_sec`
    and so assumed two things that only hold for HBM-resident GPUs: that weights live
    in device memory, and that device memory is what binds. On SRAM-resident silicon
    (Groq, Cerebras) weights sit on-chip at orders of magnitude more bandwidth and the
    **interconnect** binds instead, because the model must shard across many chips.
    So `resources` names each supply and `weight_residence` says which one the weight
    read is served from.

    Only inputs are recorded, never a derived headroom ratio. The ratio needs three
    choices this runner cannot make — the engine's decode batch (client concurrency is
    not the same thing), the context length, and tokens-versus-requests — so it belongs
    in a query over the ledger where those can vary without re-running anything.

    **What is and is not derivable from these inputs alone.** Only the
    `weight_residence` ceiling is: `resources[weight_residence] / (weight_bytes +
    kv_bytes_per_token * ctx)`. It is tempting to take the minimum over every resource,
    but that is wrong — it charges the whole weight read to every tier, and a
    single-GPU decode step puts zero bytes on NVLink. An interconnect ceiling needs a
    **sharding descriptor** this block deliberately does not carry yet (TP/EP degree,
    and bytes crossing per step, which for MoE means the expert traffic matrix). The
    other resources are recorded anyway so the supply side is on file for when that
    descriptor arrives; until then, treat them as context, not as ceilings.
    """
    if value is None:
        return None
    known = {
        "weight_bytes",
        "kv_bytes_per_token",
        "resources",
        "weight_residence",
        "source",
    }
    if not isinstance(value, dict):
        fail(f"{name} must be an object")
    # peak_flops_per_sec is optional: it only matters where compute binds (prefill,
    # training), and several accelerators do not publish a comparable figure.
    root = expect_object(value, name, known | ({"peak_flops_per_sec"} if "peak_flops_per_sec" in value else set()))

    for key in ("weight_bytes", "kv_bytes_per_token"):
        raw = root[key]
        if isinstance(raw, bool) or not isinstance(raw, int) or raw <= 0:
            fail(f"{name}.{key} must be a positive integer count of bytes")

    resources = root["resources"]
    if not isinstance(resources, dict) or not resources:
        fail(f"{name}.resources must be a non-empty object of name -> peak bytes/sec")
    for rname, peak in resources.items():
        if not isinstance(rname, str) or not rname:
            fail(f"{name}.resources keys must be non-empty resource names")
        if isinstance(peak, bool) or not isinstance(peak, int) or peak <= 0:
            fail(f"{name}.resources.{rname} must be a positive integer of bytes/sec")

    # The tier the weight read is served from. Without this the floor silently
    # assumes HBM, which is wrong on any SRAM-resident part.
    residence = expect_string(root["weight_residence"], f"{name}.weight_residence")
    if residence not in resources:
        fail(
            f"{name}.weight_residence is {residence!r}, which is not a declared "
            f"resource; known: {', '.join(sorted(resources))}"
        )

    if "peak_flops_per_sec" in root:
        raw = root["peak_flops_per_sec"]
        if isinstance(raw, bool) or not isinstance(raw, int) or raw <= 0:
            fail(f"{name}.peak_flops_per_sec must be a positive integer of FLOP/sec")

    # A stale figure silently corrupts every number derived from it, so provenance
    # is mandatory rather than a nicety.
    expect_string(root["source"], f"{name}.source")
    return root


def expect_topology(value: Any, name: str) -> dict[str, Any]:
    """Deployment facts the runner cannot observe, for the run ledger.

    Optional, and every field inside it is optional too, because this is the one
    place where a wrong value is worse than a missing one: the runner writes
    these straight into `sigiro_experiment_runs`, where a guessed `gpu_model` or
    `tp_size` is indistinguishable from a measured one. Absent stays NULL.

    Field names are the ledger's column names on purpose — they are passed
    through verbatim, and `sigiro record-run` rejects an unknown field, so a typo
    here fails the period instead of silently leaving a column null.
    """
    if value is None:
        return {}
    if not isinstance(value, dict) or not value:
        fail(f"{name} must be a non-empty object")
    known = set(TOPOLOGY_STRINGS) | set(TOPOLOGY_COUNTS) | {"price_per_hour_usd"}
    unknown = set(value) - known
    if unknown:
        fail(f"{name} has unknown fields: {', '.join(sorted(unknown))}; known: {', '.join(sorted(known))}")
    for key in TOPOLOGY_STRINGS:
        if key in value:
            expect_string(value[key], f"{name}.{key}")
    for key in TOPOLOGY_COUNTS:
        if key in value:
            raw = value[key]
            if isinstance(raw, bool) or not isinstance(raw, int) or raw <= 0:
                fail(f"{name}.{key} must be a positive integer")
    if "price_per_hour_usd" in value:
        value["price_per_hour_usd"] = expect_number(value["price_per_hour_usd"], f"{name}.price_per_hour_usd")
    return value


def expect_sidecars(value: Any, name: str) -> list[dict[str, Any]]:
    """Processes started before the target in every period and stopped after it.

    Both arms get the same sidecars, so a collector or profiler cannot be the
    difference between them. A sidecar changes what is being measured, so its
    name and argv are part of the benchmark fingerprint.
    """
    if value is None:
        return []
    if not isinstance(value, list) or not value:
        fail(f"{name} must be a non-empty array of sidecar objects")
    seen: set[str] = set()
    for index, item in enumerate(value):
        entry = expect_object(item, f"{name}[{index}]", {"name", "argv", "env"})
        # The name becomes a log filename, and it identifies the sidecar in the
        # fingerprint, so it must be both unique and path-safe.
        sidecar_name = expect_string(entry["name"], f"{name}[{index}].name")
        if len(sidecar_name) > 64 or any(character not in ALLOWED_ID_CHARS for character in sidecar_name):
            fail(f"{name}[{index}].name must be 1-64 ASCII letters, digits, dots, underscores, or hyphens")
        if sidecar_name in seen:
            fail(f"{name} has duplicate sidecar name {sidecar_name}")
        seen.add(sidecar_name)
        entry["argv"] = expect_argv(entry["argv"], f"{name}[{index}].argv")
        entry["env"] = expect_env(entry["env"], f"{name}[{index}].env")
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
        }
        | ({"roofline"} if "roofline" in raw else set())
        | ({"topology"} if "topology" in raw else set())
        | ({"sidecars"} if "sidecars" in raw else set()),
    )
    if root["schema_version"] != 1:
        fail("manifest.schema_version must be 1")
    experiment_id = expect_string(root["experiment_id"], "manifest.experiment_id")
    if len(experiment_id) > 128 or any(character not in ALLOWED_ID_CHARS for character in experiment_id):
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

    root["roofline"] = expect_roofline(root.get("roofline"), "manifest.roofline")
    root["topology"] = expect_topology(root.get("topology"), "manifest.topology")
    root["sidecars"] = expect_sidecars(root.get("sidecars"), "manifest.sidecars")
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


def start_process(
    argv: list[str], cwd: Path, env: dict[str, str], log_path: Path
) -> tuple[subprocess.Popen[Any], Any]:
    """Start a long-lived process with its output on disk.

    Its own session, so `stop_process` can signal the whole group: an engine or
    a collector spawns workers, and killing only the leader leaves them holding
    the port the next period needs. The caller owns closing the returned log.
    """
    log = log_path.open("w")
    try:
        process = subprocess.Popen(
            argv,
            cwd=cwd,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except BaseException:
        log.close()
        raise
    return process, log


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


def start_sidecars(
    manifest: dict[str, Any], cwd: Path, period_dir: Path
) -> list[Sidecar]:
    """Bring up every sidecar for one period, before the target starts."""
    started: list[Sidecar] = []
    try:
        for sidecar in manifest["sidecars"]:
            env = os.environ.copy()
            env.update(sidecar["env"])
            process, log = start_process(sidecar["argv"], cwd, env, sidecar_log(period_dir, sidecar["name"]))
            started.append((sidecar["name"], process, log))
    except BaseException:
        # A sidecar that fails to launch must not leave its predecessors holding
        # the ports the next attempt needs.
        stop_sidecars(started)
        raise
    return started


def sidecar_log(period_dir: Path, name: str) -> Path:
    return period_dir / f"sidecar-{name}.log"


def check_sidecars(
    sidecars: list[Sidecar],
    period_dir: Path,
    run_id: str,
    when: str,
) -> None:
    """Fail the experiment if a sidecar is no longer running.

    A dead collector does not fail AIPerf — it just stops recording, and the
    period would look clean while measuring something the other arm did not.
    """
    for name, process, _log in sidecars:
        if process.poll() is not None:
            fail(
                f"{run_id} sidecar {name} exited {when} with status {process.returncode}; "
                f"see {sidecar_log(period_dir, name)}"
            )


def stop_sidecars(sidecars: list[Sidecar]) -> None:
    """Stop every sidecar in reverse start order and empty the list."""
    while sidecars:
        _name, process, log = sidecars.pop()
        stop_process(process)
        log.close()


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


def aiperf_goodput(aggregate: dict[str, Any], run_id: str) -> float:
    """The goodput AIPerf measured for one period, or a loud failure.

    The same metric `sigiro compare-experiment` judges on, so a period whose row
    carried a different number than the verdict would be a second, silently
    disagreeing source of truth.
    """
    metrics = aggregate.get("metrics")
    metric = metrics.get("goodput_avg") if isinstance(metrics, dict) else None
    if not isinstance(metric, dict):
        fail(
            f"{run_id} AIPerf aggregate has no goodput_avg metric; "
            "manifest.aiperf.common_args must set --goodput"
        )
    mean = metric.get("mean")
    if isinstance(mean, bool) or not isinstance(mean, (int, float)) or not math.isfinite(mean):
        fail(f"{run_id} AIPerf goodput_avg.mean is not a finite number: {mean!r}")
    return float(mean)


def period_run_record(
    manifest: dict[str, Any],
    fingerprints: dict[str, str],
    run_id: str,
    arm: str,
    parent_run_id: str | None,
    from_ts: int,
    to_ts: int,
    aggregate: dict[str, Any],
) -> dict[str, Any]:
    """One `sigiro_experiment_runs` row for a finished period.

    Only what this runner observed: the identity of the period, the window it
    ran in, the fingerprints it computed, the arm's launch configuration, and
    the goodput AIPerf measured. Everything else the row can hold — engine,
    GPU model, tensor parallelism, quantization, price — is a deployment fact
    the runner cannot see, so it stays absent unless `manifest.topology`
    declares it. A guessed value there would be indistinguishable from a
    measured one for every query that later reads this table.

    `config_json` carries the arm's argv *and* env because both are what was
    launched and both are inside `config_sha`; recording only argv would make
    two rows with different `config_sha` look identical. `expect_env` has
    already refused credential-like keys, so nothing secret lands here.
    """
    record: dict[str, Any] = {
        "experiment_id": manifest["experiment_id"],
        "run_id": run_id,
        "arm": arm,
        "started_at": from_ts,
        "ended_at": to_ts,
        "config_sha": fingerprints[arm],
        "model_sha": fingerprints["model"],
        "hardware_sha": fingerprints["hardware"],
        "benchmark_sha": fingerprints["benchmark"],
        "model_id": manifest["model"]["id"],
        "model_revision": manifest["model"]["revision"],
        "config_json": {"argv": manifest[arm]["argv"], "env": manifest[arm]["env"]},
        "goodput": aiperf_goodput(aggregate, run_id),
    }
    # The first period has no predecessor; the rest chain in execution order, so
    # the ledger can walk one experiment's periods as a lineage.
    if parent_run_id is not None:
        record["parent_run_id"] = parent_run_id
    record.update(manifest["topology"])
    return record


def ledger_command(
    binary: str, env: dict[str, str], cwd: Path, out_dir: Path, args: list[str], what: str
) -> None:
    """Run one batch ledger subcommand, keeping its output beside the artifacts.

    Every ledger write goes through here — the catalog is a local file DuckDB
    holds a single-writer lock on, so it is written by `sigiro` subprocesses
    rather than over HTTP. `args[0]` is the subcommand and becomes the log
    filename; a nonzero status fails the experiment, because a ledger that
    silently lacks a row or a verdict is worse than no ledger at all.
    """
    completed = subprocess.run(
        [binary, *args],
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        timeout=120,
    )
    (out_dir / f"{args[0]}.stdout").write_text(completed.stdout)
    (out_dir / f"{args[0]}.stderr").write_text(completed.stderr)
    if completed.returncode != 0:
        fail(f"{what} {args[0]} failed with status {completed.returncode}: {completed.stderr}")


def record_period_run(
    binary: str, env: dict[str, str], cwd: Path, period_dir: Path, record: dict[str, Any]
) -> None:
    """Write one finished period into `sigiro_experiment_runs`."""
    record_path = period_dir / "run-record.json"
    record_path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    ledger_command(
        binary, env, cwd, period_dir, ["record-run", "--file", str(record_path)], record["run_id"]
    )


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
    sidecars: list[Sidecar] = []
    try:
        hardware = read_hardware(manifest, cwd)
        version = aiperf_version(manifest["aiperf"]["binary"], cwd)
        fingerprints = {
            "model": sha256_bytes(canonical_bytes(manifest["model"])),
            # The roofline inputs describe this machine, so they belong in its
            # identity: two runs asserting different peak bandwidth for the same
            # box are making different claims and must not compare as equal.
            "hardware": sha256_bytes(
                hardware
                if manifest["roofline"] is None
                else hardware + canonical_bytes(manifest["roofline"])
            ),
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
                        # Same argument for sidecars: a profiler or a scraping
                        # collector alongside the target changes what is being
                        # measured. Name and argv only — env is host-specific
                        # wiring, not a change in what is measured.
                        "sidecars": [
                            {"name": sidecar["name"], "argv": sidecar["argv"]}
                            for sidecar in manifest["sidecars"]
                        ],
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
        # `sigiro record-run` writes a second, separate catalog, for two reasons.
        # DuckDB holds a single-writer lock on a catalog file, so nothing can
        # insert into the observer's catalog while it is serving — a per-period
        # record-run against `data_dir` fails with "Conflicting lock is held".
        # And `data_dir` is created fresh per experiment, so a run ledger inside
        # it could never carry lineage past the experiment that wrote it, which
        # is the only reason `parent_run_id` exists.
        ledger_dir = output_root / "ledger"
        ledger_dir.mkdir(parents=True, exist_ok=True)
        ledger_env = sigiro_env.copy()
        ledger_env["SIGIRO_DATA_DIR"] = str(ledger_dir)
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

        observer, observer_log = start_process(
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
            cwd,
            sigiro_env,
            experiment_dir / "sigiro.log",
        )
        base_url = f"http://127.0.0.1:{sigiro['http_port']}"
        wait_ready(base_url, sigiro["startup_timeout_seconds"])

        period_results: dict[str, dict[str, Any]] = {}
        parent_run_id: str | None = None
        for run_id, arm in PERIODS:
            period_dir = experiment_dir / run_id
            period_dir.mkdir()
            arm_config = manifest[arm]
            target_env = os.environ.copy()
            target_env.update(arm_config["env"])
            from_ts = time.time_ns() // 1_000
            # Sidecars come up first and in every period, so both arms are
            # measured under the same instrumentation rather than the candidate
            # inheriting whatever the control left running.
            sidecars = start_sidecars(manifest, cwd, period_dir)
            target, target_log = start_process(
                arm_config["argv"], cwd, target_env, period_dir / "target.log"
            )
            time.sleep(0.1)
            check_sidecars(sidecars, period_dir, run_id, "before the target")
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
            check_sidecars(sidecars, period_dir, run_id, "before the period ended")
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
            stop_sidecars(sidecars)
            # Recorded now rather than at the end: the row is complete the
            # moment the period is, and a run that dies in period four must
            # leave the three finished periods in the ledger.
            record_period_run(
                sigiro["binary"],
                ledger_env,
                cwd,
                period_dir,
                period_run_record(
                    manifest, fingerprints, run_id, arm, parent_run_id, from_ts, to_ts, aggregate
                ),
            )
            parent_run_id = run_id

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
        if not isinstance(comparison.get("outcome"), str):
            fail("Sigiro comparison returned no outcome to record in the ledger")
        (experiment_dir / "comparison.json").write_text(
            json.dumps(comparison, indent=2, sort_keys=True) + "\n"
        )
        # The verdict only exists now, after all six periods: each period's
        # ledger row was written the moment it ended, with `outcome` NULL. Write
        # it onto every row of the experiment, or the ledger records what was
        # run and never whether it helped.
        ledger_command(
            sigiro["binary"],
            ledger_env,
            cwd,
            experiment_dir,
            [
                "finalize-experiment",
                "--experiment-id",
                manifest["experiment_id"],
                "--outcome",
                comparison["outcome"],
            ],
            manifest["experiment_id"],
        )
        record = {
            "experiment_id": manifest["experiment_id"],
            "completed_at": time.time_ns() // 1_000,
            "artifact_dir": str(experiment_dir),
            "fingerprints": fingerprints,
            "roofline": manifest["roofline"],
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
        stop_sidecars(sidecars)
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

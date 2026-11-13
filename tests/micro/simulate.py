# /// script
# requires-python = ">=3.11"
# dependencies = ["paho-mqtt==2.1.0", "opentelemetry-proto==1.38.0"]
# ///
"""Execute actual Cortex-M instructions in QEMU; no host substitute for firmware.
Semihost files are a simulator transport/NOR model, not MCU TCP or physical flash.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import queue
import sqlite3
import struct
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

import paho.mqtt.client as mqtt
from google.protobuf.message import DecodeError
from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import (
    ExportLogsServiceRequest,
    ExportLogsServiceResponse,
)
from paho.mqtt.packettypes import PacketTypes
from paho.mqtt.properties import Properties

ROOT = Path(__file__).resolve().parents[2]
ELF = (
    ROOT / "firmware/qemu-recorder/target/thumbv7m-none-eabi/release/sigiro-micro-qemu"
)
MOSQUITTO = "eclipse-mosquitto:2@sha256:75d6662dee52c947dc9dd9c29f1d8d7ffef78ddaf38001caa09755681e8bbc98"
COLLECTOR = "otel/opentelemetry-collector:0.140.0@sha256:6852803128c97a37fd2bafb989a04a10e3af920737d8eee998eefdfcebc698be"
TOPIC = "sigiro/experimental/v1/qemu-lab/logs"
RESPONSE = "sigiro/experimental/v1/qemu-lab/ack"
BASE_TIME = 1_794_495_600_123_456_789


def command(args, **kwargs):
    try:
        return subprocess.run(
            args, check=True, text=True, capture_output=True, timeout=120, **kwargs
        ).stdout.strip()
    except subprocess.CalledProcessError as error:
        print(error.stdout, error.stderr, flush=True)
        raise


class Lab:
    def __init__(self, artifacts: Path):
        self.root = artifacts
        self.checks = []
        self.boots = 0

    def case(self, name, image=None):
        path = self.root / name
        path.mkdir(parents=True)
        (path / "flash.bin").write_bytes(image if image is not None else b"\xff" * 8192)
        return path

    def boot(
        self, path, action, ordinal=0, cut=0xFFFFFFFF, io_error=False, unknown=False
    ):
        for name in ("status.bin", "record.bin", "dump.bin"):
            (path / name).unlink(missing_ok=True)
        (path / "command.bin").write_bytes(
            struct.pack("<BBBxII", action, io_error, unknown, ordinal, cut)
        )
        result = subprocess.run(
            [
                "qemu-system-arm",
                "-M",
                "mps2-an385",
                "-nographic",
                "-monitor",
                "none",
                "-serial",
                "none",
                "-semihosting-config",
                "enable=on,target=native",
                "-kernel",
                str(ELF),
            ],
            cwd=path,
            check=False,
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.boots += 1
        with (path / "qemu.log").open("a") as log:
            log.write(
                f"action={action} ordinal={ordinal} cut={cut} exit={result.returncode}\n{result.stdout}{result.stderr}\n"
            )
        if result.returncode == 42:
            assert cut != 0xFFFFFFFF, "unexpected injected reset"
            assert not (path / "status.bin").exists(), (
                "reset falsely reported completion"
            )
            return None
        assert result.returncode == 0, (
            f"firmware failed in {path}: {result.stdout}{result.stderr}"
        )
        raw = (path / "status.bin").read_bytes()
        assert len(raw) == 9
        status, mutations, count = struct.unpack("<BII", raw)
        return {"status": status, "mutations": mutations, "count": count}

    def peek(self, path):
        result = self.boot(path, 2)
        assert result["status"] in (0, 1)
        return (path / "record.bin").read_bytes()

    def ack(self, path, wire, outcome=1, batch_id=None, signal=None):
        ident, actual_signal, _ = parse_record(wire)
        ack = (
            b"SAK1"
            + (batch_id if batch_id is not None else ident)
            + bytes([signal or actual_signal, outcome, 0, 0])
        )
        (path / "ack.bin").write_bytes(ack)
        return self.boot(path, 3)

    def dump(self, name, image):
        path = self.case(name, image)
        result = self.boot(path, 7)
        assert result["status"] == 0
        raw = (path / "dump.bin").read_bytes()
        records = []
        while raw:
            assert len(raw) >= 4
            length = struct.unpack("<I", raw[:4])[0]
            assert 28 <= length <= 512 and len(raw) >= 4 + length
            records.append(raw[4 : 4 + length])
            raw = raw[4 + length :]
        assert len(records) == result["count"]
        for record in records:
            parse_record(record)
        return records

    def passed(self, name, **details):
        self.checks.append({"name": name, "passed": True, **details})
        print(f"PASS {name}" + (f" {details}" if details else ""), flush=True)


def parse_record(wire):
    assert len(wire) >= 28 and wire[:4] == b"SOB1" and wire[5:8] == b"\0\0\0"
    assert wire[4] in (1, 2, 3)
    assert struct.unpack(">I", wire[24:28])[0] == len(wire) - 28
    return wire[8:24], wire[4], wire[28:]


def verify_otlp(wire, ordinal, unknown=False):
    ident, signal, payload = parse_record(wire)
    assert ident == b"qemu-lab-id-" + struct.pack(">I", ordinal) and signal == 1
    request = ExportLogsServiceRequest.FromString(payload)
    assert len(request.resource_logs) == 1
    group = request.resource_logs[0]
    assert group.resource.attributes[0].value.string_value == "sigiro-micro-qemu"
    log = group.scope_logs[0].log_records[0]
    assert (
        log.body.string_value == "QEMU: π\0🍵"
        and log.time_unix_nano == BASE_TIME + ordinal
    )
    if unknown:
        assert payload.endswith(b"\xba\x3e\x04raw!")
        known = ExportLogsServiceRequest.FromString(payload)
        known.DiscardUnknownFields()
        assert known.SerializeToString() != payload
    return payload


def simulator_tests(lab, exhaustive):
    path = lab.case("policy")
    assert lab.boot(path, 1, 7, unknown=True)["status"] == 0
    wire = lab.peek(path)
    verify_otlp(wire, 7, unknown=True)
    assert lab.peek(path) == wire
    lab.passed(
        "Cortex-M encode, NOR persist, process restart, exact OTLP and unknown fields"
    )
    for outcome, name in [
        (2, "broker PUBACK"),
        (3, "retry"),
        (4, "partial"),
        (5, "permanent rejection"),
    ]:
        assert lab.ack(path, wire, outcome)["status"] == 3
        assert lab.peek(path) == wire
        lab.passed(f"{name} does not delete queued data")
    assert lab.ack(path, wire, batch_id=b"wrong-identifier")["status"] == 4
    assert lab.ack(path, wire, signal=2)["status"] == 4
    assert lab.peek(path) == wire
    assert lab.boot(path, 1, 8)["status"] == 0
    assert lab.ack(path, wire)["status"] == 0
    second = lab.peek(path)
    verify_otlp(second, 8)
    assert lab.ack(path, wire)["status"] == 4
    assert lab.peek(path) == second
    lab.passed(
        "matching acceptance pops once; wrong/stale identities and signals cannot pop next batch"
    )

    full = lab.case("full")
    result = lab.boot(full, 4)
    assert result["status"] == 2 and result["count"] > 0
    oldest = lab.peek(full)
    assert lab.boot(full, 1, 900)["status"] == 2 and lab.peek(full) == oldest
    lab.passed(
        "full NOR queue rejects new telemetry without overwriting oldest",
        capacity_batches=result["count"],
    )

    cycle = lab.case("cycle")
    result = lab.boot(cycle, 5, 160)
    assert (
        result["status"] == 0 and result["count"] == 160 and result["mutations"] > 8192
    )
    assert lab.peek(cycle) == b""
    lab.passed(
        "160 enqueue/accept/pop cycles cross NOR page wrap and erase",
        mutations=result["mutations"],
    )

    seed = lab.case("append-seed")
    assert lab.boot(seed, 1, 10)["status"] == 0
    first = lab.peek(seed)
    image = (seed / "flash.bin").read_bytes()
    reference = lab.case("append-reference", image)
    mutations = lab.boot(reference, 1, 11)["mutations"]
    expected = lab.dump("append-expected", (reference / "flash.bin").read_bytes())
    assert len(expected) == 2 and expected[0] == first
    cuts = (
        range(mutations + 1)
        if exhaustive
        else sorted({0, 1, 2, mutations // 2, mutations - 1, mutations})
    )
    for cut in cuts:
        case = lab.case(f"append-cut-{cut}", image)
        assert lab.boot(case, 1, 11, cut=cut) is None
        recovered = lab.dump(f"append-recover-{cut}", (case / "flash.bin").read_bytes())
        assert recovered in ([first], expected), (cut, recovered)
    lab.passed(
        "append torn-write reset recovery preserves prior data; exposes only complete new batches",
        cuts=len(cuts),
    )

    image = (reference / "flash.bin").read_bytes()
    pop_ref = lab.case("pop-reference", image)
    pop_mutations = lab.ack(pop_ref, first)["mutations"]
    for cut in range(pop_mutations + 1):
        case = lab.case(f"pop-cut-{cut}", image)
        (case / "ack.bin").write_bytes(
            b"SAK1" + parse_record(first)[0] + bytes([1, 1, 0, 0])
        )
        assert lab.boot(case, 3, cut=cut) is None
        recovered = lab.dump(f"pop-recover-{cut}", (case / "flash.bin").read_bytes())
        assert recovered in (expected, expected[1:]), (cut, recovered)
    lab.passed(
        "accept/pop torn-write reset recovery never removes the next batch",
        cuts=pop_mutations + 1,
    )

    io_case = lab.case("io-error", image)
    assert lab.boot(io_case, 1, 12, cut=8, io_error=True)["status"] == 5
    assert lab.peek(io_case) == first
    lab.passed("program I/O failure is not reported as successful recording")

    erase_seed = lab.case("erase-seed")
    lab.boot(erase_seed, 6)
    image = (erase_seed / "flash.bin").read_bytes()
    survivors = lab.dump("erase-survivors", image)
    erase_ref = lab.case("erase-reference", image)
    mutations = lab.boot(erase_ref, 1, 1000)["mutations"]
    assert mutations >= 4096
    expected = lab.dump("erase-expected", (erase_ref / "flash.bin").read_bytes())
    assert expected[:-1] == survivors
    cuts = sorted({0, 1, 2, 255, 256, 2047, 4095, 4096, 4097, mutations - 1, mutations})
    for cut in cuts:
        case = lab.case(f"erase-cut-{cut}", image)
        assert lab.boot(case, 1, 1000, cut=cut) is None
        recovered = lab.dump(f"erase-recover-{cut}", (case / "flash.bin").read_bytes())
        assert recovered in (survivors, expected), (cut, len(recovered))
    lab.passed(
        "partial erase and page-reuse reset recovery preserves all pending batches",
        cuts=len(cuts),
        protected_batches=len(survivors),
    )


@contextlib.contextmanager
def services(lab):
    suffix = f"{os.getpid()}-{time.time_ns()}"
    names = []
    try:
        broker = f"sigiro-qemu-mqtt-{suffix}"
        command(
            [
                "docker",
                "run",
                "--rm",
                "-d",
                "--name",
                broker,
                "--user",
                "1883",
                "--cap-drop",
                "ALL",
                "--read-only",
                "--tmpfs",
                "/tmp",
                "--memory",
                "64m",
                "-p",
                "127.0.0.1::1883",
                "--entrypoint",
                "sh",
                MOSQUITTO,
                "-c",
                'printf "listener 1883\\nallow_anonymous true\\npersistence false\\n" > /tmp/lab.conf; exec /usr/sbin/mosquitto -c /tmp/lab.conf',
            ]
        )
        names.append(broker)
        config = lab.root / "collector.yaml"
        config.write_text(
            "receivers:\n  otlp:\n    protocols:\n      http:\n        endpoint: 0.0.0.0:4318\nexporters:\n  debug:\n    verbosity: detailed\nservice:\n  pipelines:\n    logs:\n      receivers: [otlp]\n      exporters: [debug]\n"
        )
        collector = f"sigiro-qemu-collector-{suffix}"
        command(
            [
                "docker",
                "run",
                "--rm",
                "-d",
                "--name",
                collector,
                "--cap-drop",
                "ALL",
                "--read-only",
                "--memory",
                "256m",
                "-p",
                "127.0.0.1::4318",
                "--mount",
                f"type=bind,source={config},target=/etc/otelcol/config.yaml,readonly",
                COLLECTOR,
            ]
        )
        names.append(collector)

        def port(name, internal):
            value = json.loads(command(["docker", "inspect", name]))[0]
            return int(
                value["NetworkSettings"]["Ports"][f"{internal}/tcp"][0]["HostPort"]
            )

        endpoint = f"http://127.0.0.1:{port(collector, 4318)}/v1/logs"
        for _ in range(100):
            try:
                request = urllib.request.Request(
                    endpoint,
                    data=b"",
                    headers={"Content-Type": "application/x-protobuf"},
                )
                with urllib.request.urlopen(request, timeout=1) as response:
                    assert response.status == 200
                break
            except (OSError, urllib.error.URLError):
                time.sleep(0.1)
        else:
            raise AssertionError("Collector never became ready")
        yield port(broker, 1883), endpoint, collector
    finally:
        for name in reversed(names):
            logs = subprocess.run(
                ["docker", "logs", name],
                check=False,
                text=True,
                capture_output=True,
                timeout=15,
            )
            (
                lab.root / ("collector.log" if "collector" in name else "mosquitto.log")
            ).write_text(logs.stdout + logs.stderr)
            subprocess.run(
                ["docker", "stop", "-t", "2", name],
                check=False,
                text=True,
                capture_output=True,
                timeout=15,
            )


class Peer:
    def __init__(self, port, name, topic):
        self.messages = queue.Queue()
        self.ready = threading.Event()
        self.client = mqtt.Client(
            mqtt.CallbackAPIVersion.VERSION2, client_id=name, protocol=mqtt.MQTTv5
        )

        def connected(client, userdata, flags, reason, properties):
            if reason.is_failure:
                return
            client.subscribe(topic, qos=1)

        self.client.on_connect = connected
        self.client.on_subscribe = lambda *args: self.ready.set()
        self.client.on_message = lambda client, userdata, message: self.messages.put(
            message
        )
        self.client.connect("127.0.0.1", port, keepalive=30)
        self.client.loop_start()
        assert self.ready.wait(5), f"MQTT subscription failed: {name}"

    def publish(self, topic, payload, properties=None):
        info = self.client.publish(
            topic, payload, qos=1, retain=False, properties=properties
        )
        info.wait_for_publish(timeout=5)
        assert info.is_published(), "no broker PUBACK"

    def close(self):
        self.client.disconnect()
        self.client.loop_stop()


def http_outcome(status, raw):
    """Lab ACK outcome: full=1, retry=3, partial/warning=4, permanent=5."""
    assert len(raw) <= 4096, "response exceeds configured bound"
    if status == 200:
        result = ExportLogsServiceResponse.FromString(raw)
        # A present-but-empty partial_success is explicitly equivalent to unset.
        partial = result.partial_success
        return 4 if partial.rejected_log_records or partial.error_message else 1
    return 3 if status in (429, 502, 503, 504) else 5


def response_tests(lab):
    assert http_outcome(200, b"") == http_outcome(200, b"\x0a\x00") == 1
    response = ExportLogsServiceResponse()
    response.partial_success.rejected_log_records = 1
    assert http_outcome(200, response.SerializeToString()) == 4
    response.partial_success.rejected_log_records = 0
    response.partial_success.error_message = "warning" * 100
    assert http_outcome(200, response.SerializeToString()) == 4
    for status in (429, 502, 503, 504):
        assert http_outcome(status, b"") == 3
    for status in (400, 401, 403, 404, 500):
        assert http_outcome(status, b"") == 5
    for raw in (b"\xff", b"x" * 4097):
        try:
            http_outcome(200, raw)
        except (AssertionError, DecodeError):
            pass
        else:
            raise AssertionError("invalid/oversized response accepted")
    lab.passed(
        "OTLP HTTP outcomes: empty partial success, warnings/rejections, retry codes, malformed/oversized responses"
    )


class Gateway:
    def __init__(self, port, endpoint, database, forwarded):
        self.peer = Peer(port, "sigiro-qemu-bridge", TOPIC)
        self.endpoint = endpoint
        self.database = Path(database)
        self.forwarded = forwarded
        self.db = sqlite3.connect(database)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.execute(
            "CREATE TABLE IF NOT EXISTS accepted(id BLOB PRIMARY KEY, payload BLOB NOT NULL)"
        )
        self.db.commit()

    def process(self, drop_ack=False, crash_after_http=False):
        message = self.peer.messages.get(timeout=5)
        assert message.topic == TOPIC and message.properties.ResponseTopic == RESPONSE
        assert message.properties.ContentType == "application/x-protobuf"
        ident = message.properties.CorrelationData
        assert len(ident) == 16 and len(message.payload) <= 384
        existing = self.db.execute(
            "SELECT payload FROM accepted WHERE id=?", (ident,)
        ).fetchone()
        outcome = 1
        if existing is not None:
            assert existing[0] == message.payload, (
                "batch ID reused with different bytes"
            )
        else:
            request = urllib.request.Request(
                self.endpoint,
                data=message.payload,
                headers={"Content-Type": "application/x-protobuf"},
            )
            try:
                response = urllib.request.urlopen(request, timeout=5)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                assert response.headers.get_content_type() == "application/x-protobuf"
                raw = response.read(4097)
                (
                    self.database.parent / f"http-response-{len(self.forwarded)}.bin"
                ).write_bytes(raw)
                outcome = http_outcome(response.status, raw)
            if outcome == 1:
                self.forwarded.append(message.payload)
                if crash_after_http:
                    return None  # Simulated crash in the irreducible HTTP/ledger gap.
                self.db.execute(
                    "INSERT INTO accepted VALUES (?,?)", (ident, message.payload)
                )
                self.db.commit()
        ack = b"SAK1" + ident + bytes([1, outcome, 0, 0])
        if not drop_ack:
            properties = Properties(PacketTypes.PUBLISH)
            properties.CorrelationData = ident
            self.peer.publish(RESPONSE, ack, properties)
        return ack

    def close(self):
        self.peer.close()
        self.db.close()


def mqtt_request(peer, wire):
    ident, _, payload = parse_record(wire)
    props = Properties(PacketTypes.PUBLISH)
    props.ContentType = "application/x-protobuf"
    props.ResponseTopic = RESPONSE
    props.CorrelationData = ident
    peer.publish(TOPIC, payload, props)


def export_tests(lab):
    with services(lab) as (port, endpoint, collector):
        device = Peer(port, "sigiro-qemu-device-host-adapter", RESPONSE)
        forwarded = []
        gateway = Gateway(port, endpoint, lab.root / "gateway.sqlite", forwarded)
        try:
            for ordinal, gap in [(200, False), (201, True)]:
                path = lab.case(f"export-{ordinal}")
                lab.boot(path, 1, ordinal, unknown=True)
                wire = lab.peek(path)
                payload = verify_otlp(wire, ordinal, unknown=True)
                mqtt_request(device, wire)
                assert lab.ack(path, wire, outcome=2)["status"] == 3
                assert lab.peek(path) == wire
                before = len(forwarded)
                gateway.process(drop_ack=not gap, crash_after_http=gap)
                assert device.messages.empty(), "ACK should have been lost/not created"
                assert lab.peek(path) == wire
                gateway.close()
                gateway = Gateway(
                    port, endpoint, lab.root / "gateway.sqlite", forwarded
                )
                mqtt_request(device, wire)
                gateway.process()
                message = device.messages.get(timeout=5)
                assert message.properties.CorrelationData == parse_record(wire)[0]
                assert message.payload == b"SAK1" + parse_record(wire)[0] + bytes(
                    [1, 1, 0, 0]
                )
                (path / "ack.bin").write_bytes(message.payload)
                assert lab.boot(path, 3)["status"] == 0 and lab.peek(path) == b""
                requests = len(forwarded) - before
                assert requests == (2 if gap else 1)
                assert all(body == payload for body in forwarded[before:])
                lab.passed(
                    "HTTP acceptance / ledger crash can duplicate but never falsely delete"
                    if gap
                    else "lost application ACK + gateway restart replays durable receipt without re-export",
                    downstream_requests=requests,
                )
            rejected = lab.case("export-rejected")
            assert lab.boot(rejected, 1, 202, unknown=2)["status"] == 0
            wire = lab.peek(rejected)
            assert parse_record(wire)[2] == b"\xff"
            mqtt_request(device, wire)
            gateway.process()
            receipt = device.messages.get(timeout=5)
            assert receipt.payload[21] == 5
            (rejected / "ack.bin").write_bytes(receipt.payload)
            assert lab.boot(rejected, 3)["status"] == 3 and lab.peek(rejected) == wire
            lab.passed(
                "real Collector HTTP 400 yields permanent rejection, not successful deletion"
            )
            logs = command(["docker", "logs", collector])
            # Collector debug output may be written to stderr instead of stdout.
            logs_process = subprocess.run(
                ["docker", "logs", collector],
                check=True,
                capture_output=True,
                text=True,
                timeout=15,
            )
            logs += logs_process.stderr
            assert "sigiro-micro-qemu" in logs and "QEMU:" in logs
            lab.passed(
                "QEMU-produced original OTLP crosses real MQTT 5 and reaches official Collector",
                collector_image=COLLECTOR,
                qos=1,
                unknown_fields_preserved_to_HTTP=True,
            )
        finally:
            gateway.close()
            device.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--quick",
        action="store_true",
        help="sample append cut points instead of every byte",
    )
    parser.add_argument("--no-network", action="store_true")
    args = parser.parse_args()
    env = dict(os.environ, RUSTUP_TOOLCHAIN="stable")
    print(
        command(
            [
                "rustup",
                "run",
                "stable",
                "cargo",
                "test",
                "--locked",
                "-p",
                "otlp-outbox",
            ],
            cwd=ROOT,
            env=env,
        ),
        flush=True,
    )
    print(
        command(
            [
                "rustup",
                "run",
                "stable",
                "cargo",
                "build",
                "--locked",
                "--release",
                "-p",
                "sigiro-micro-qemu",
                "--target",
                "thumbv7m-none-eabi",
            ],
            cwd=ROOT / "firmware/qemu-recorder",
            env=env,
        ),
        flush=True,
    )
    artifacts = Path(tempfile.mkdtemp(prefix="run-", dir=ROOT / "artifacts"))
    lab = Lab(artifacts)
    print(f"Artifacts: {artifacts}", flush=True)
    started = time.monotonic()
    completed = False
    try:
        response_tests(lab)
        simulator_tests(lab, exhaustive=not args.quick)
        if not args.no_network:
            export_tests(lab)
        completed = True
    finally:
        report = {
            "completed": completed,
            "network_tested": not args.no_network,
            "exhaustive_append_cuts": not args.quick,
            "checks": lab.checks,
            "qemu_boots": lab.boots,
            "elapsed_seconds": round(time.monotonic() - started, 2),
            "elf_sha256": hashlib.sha256(ELF.read_bytes()).hexdigest(),
            "qemu": command(["qemu-system-arm", "--version"]).splitlines()[0],
            "physical_flash_tested": False,
            "mcu_network_stack_tested": False,
            "rustc": command(["rustc", "--version"], env=env),
            "target": "thumbv7m-none-eabi",
            "machine": "mps2-an385",
            "firmware_reset_method": "semihosting SYS_EXIT at storage-byte budget; fresh QEMU process",
            "gateway_crash_method": "control-flow fault before ledger commit/receipt; reopen SQLite",
            "collector_image": COLLECTOR if not args.no_network else None,
            "mosquitto_image": MOSQUITTO if not args.no_network else None,
        }
        (artifacts / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        f"PASS {len(lab.checks)} acceptance checks across {lab.boots} actual QEMU boots; {artifacts / 'results.json'}",
        flush=True,
    )


if __name__ == "__main__":
    (ROOT / "artifacts").mkdir(exist_ok=True)
    main()

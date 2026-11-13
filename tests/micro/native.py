# /// script
# requires-python = ">=3.11"
# dependencies = ["paho-mqtt==2.1.0", "opentelemetry-proto==1.38.0"]
# ///
# SPDX-License-Identifier: MIT OR Apache-2.0
"""QEMU storage -> host TLS/MQTT adapter -> actual Sigiro -> durable raw + SQL.
No Python gateway, no downstream Collector. MCU networking/physical NOR remain untested.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import queue
import secrets
import socket
import sqlite3
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

import paho.mqtt.client as mqtt
from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import (
    ExportMetricsServiceRequest,
)
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
)
from paho.mqtt.packettypes import PacketTypes
from paho.mqtt.properties import Properties
from simulate import ELF, MOSQUITTO, ROOT, Lab, command, parse_record

NAMESPACE = "sigiro/micro/v1"
DEVICE = "qemu-lab"
ACK_TOPIC = f"{NAMESPACE}/{DEVICE}/ack"


def wait_for(predicate, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError("condition did not become true before timeout")


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


@contextlib.contextmanager
def broker(root):
    config = root / "broker"
    config.mkdir()
    credentials = {user: secrets.token_hex(24) for user in ("server", DEVICE, "other")}
    password_file = config / "passwords"
    password_file.write_text(
        "".join(f"{user}:{password}\n" for user, password in credentials.items())
    )
    command(
        [
            "docker",
            "run",
            "--rm",
            "--user",
            f"{os.getuid()}:{os.getgid()}",
            "--mount",
            f"type=bind,source={config},target=/cfg",
            "--entrypoint",
            "mosquitto_passwd",
            MOSQUITTO,
            "-U",
            "/cfg/passwords",
        ]
    )
    password_file.chmod(0o644)
    command(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-keyout",
            str(config / "ca-key.pem"),
            "-out",
            str(config / "ca.pem"),
            "-subj",
            "/CN=Sigiro Test CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "subjectKeyIdentifier=hash",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ]
    )
    command(
        [
            "openssl",
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            str(config / "key.pem"),
            "-out",
            str(config / "server.csr"),
            "-subj",
            "/CN=localhost",
        ]
    )
    (config / "extensions").write_text(
        "basicConstraints=critical,CA:FALSE\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n"
    )
    command(
        [
            "openssl",
            "x509",
            "-req",
            "-in",
            str(config / "server.csr"),
            "-CA",
            str(config / "ca.pem"),
            "-CAkey",
            str(config / "ca-key.pem"),
            "-CAcreateserial",
            "-out",
            str(config / "server.pem"),
            "-days",
            "1",
            "-extfile",
            str(config / "extensions"),
        ]
    )
    # Throwaway fixture key, accessible to container UID 1883. Never production credentials.
    (config / "key.pem").chmod(0o644)
    (config / "acl").write_text(
        f"user server\ntopic read {NAMESPACE}/+/logs\ntopic read {NAMESPACE}/+/traces\ntopic read {NAMESPACE}/+/metrics\ntopic write {NAMESPACE}/+/ack\nuser {DEVICE}\ntopic write {NAMESPACE}/{DEVICE}/logs\ntopic write {NAMESPACE}/{DEVICE}/traces\ntopic write {NAMESPACE}/{DEVICE}/metrics\ntopic read {ACK_TOPIC}\nuser other\ntopic write {NAMESPACE}/other/logs\ntopic read {NAMESPACE}/other/ack\n"
    )
    (config / "mosquitto.conf").write_text(
        "listener 8883\nallow_anonymous false\npersistence false\npassword_file /cfg/passwords\nacl_file /cfg/acl\ncertfile /cfg/server.pem\nkeyfile /cfg/key.pem\nlog_type all\n"
    )
    name = f"sigiro-native-micro-{os.getpid()}-{time.time_ns()}"
    mqtt_port = port()  # Fixed binding survives stop/start; Docker's :: port does not.
    command(
        [
            "docker",
            "run",
            "-d",
            "--name",
            name,
            "--user",
            "1883",
            "--cap-drop",
            "ALL",
            "--read-only",
            "--memory",
            "64m",
            "-p",
            f"127.0.0.1:{mqtt_port}:8883",
            "--mount",
            f"type=bind,source={config},target=/cfg,readonly",
            MOSQUITTO,
            "mosquitto",
            "-c",
            "/cfg/mosquitto.conf",
        ]
    )
    try:
        endpoint = command(["docker", "port", name, "8883"]).splitlines()[0]
        mqtt_port = int(endpoint.rsplit(":", 1)[1])
        context = ssl.create_default_context(cafile=str(config / "ca.pem"))

        def ready():
            try:
                with (
                    socket.create_connection(
                        ("127.0.0.1", mqtt_port), timeout=1
                    ) as raw,
                    context.wrap_socket(raw, server_hostname="127.0.0.1"),
                ):
                    return True
            except ssl.SSLCertVerificationError:
                raise
            except OSError:
                return False  # Docker mapping can be visible before its listener.

        wait_for(ready, 15)
        yield mqtt_port, credentials, config / "ca.pem", name
    finally:
        logs = subprocess.run(
            ["docker", "logs", name], capture_output=True, text=True, check=False
        )
        (root / "broker.log").write_text(logs.stdout + logs.stderr)
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)


class Device:
    def __init__(self, mqtt_port, password, ca, user=DEVICE):
        self.receipts = queue.Queue()
        self.published = queue.Queue()
        self.ready = threading.Event()
        self.client = mqtt.Client(
            mqtt.CallbackAPIVersion.VERSION2,
            client_id=f"test-{user}",
            protocol=mqtt.MQTTv5,
        )
        self.client.username_pw_set(user, password)
        self.client.tls_set_context(ssl.create_default_context(cafile=str(ca)))

        def connected(client, _data, _flags, reason, _props):
            assert not reason.is_failure, reason
            client.subscribe(
                ACK_TOPIC if user == DEVICE else f"{NAMESPACE}/other/ack", qos=1
            )

        def subscribed(_client, _data, _mid, reasons, _props):
            assert all(not reason.is_failure for reason in reasons)
            self.ready.set()

        self.client.on_connect = connected
        self.client.on_subscribe = subscribed
        self.client.on_message = lambda _c, _u, msg: self.receipts.put(msg)
        self.client.on_publish = lambda _c, _u, mid, reason, _p: self.published.put(
            (mid, reason)
        )
        self.client.connect("127.0.0.1", mqtt_port)
        self.client.loop_start()
        assert self.ready.wait(15), "device never authenticated/subscribed"

    def close(self):
        self.client.disconnect()
        self.client.loop_stop()

    def send(self, ident, signal, payload, *, response=ACK_TOPIC, retain=False):
        properties = Properties(PacketTypes.PUBLISH)
        properties.ContentType = "application/x-protobuf"
        properties.ResponseTopic = response
        properties.CorrelationData = ident
        return self.client.publish(
            f"{NAMESPACE}/{DEVICE}/{signal}",
            payload,
            qos=1,
            retain=retain,
            properties=properties,
        )

    def receipt(self, ident, signal=1, expected=1, timeout=15):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            msg = self.receipts.get(timeout=max(0.01, deadline - time.monotonic()))
            if msg.payload[4:20] != ident:
                continue
            assert msg.topic == ACK_TOPIC and not msg.retain
            assert msg.payload == b"SAK1" + ident + bytes([signal, expected, 0, 0]), (
                msg.payload
            )
            assert msg.properties.CorrelationData == ident
            assert (
                msg.properties.ContentType == "application/vnd.sigiro.micro-receipt.v1"
            )
            return msg.payload
        raise AssertionError("no matching application receipt")

    def exchange(self, ident, signal, payload, expected=1):
        self.send(ident, signal, payload)
        return self.receipt(
            ident, {"logs": 1, "traces": 2, "metrics": 3}[signal], expected
        )

    def drain(self):
        while not self.receipts.empty():
            self.receipts.get_nowait()


class Host:
    def __init__(self, binary, root, mqtt_port, password, ca):
        self.binary, self.root = binary, root
        self.data = root / "sigiro-data"
        self.http, self.otlp, self.grpc = port(), port(), port()
        secret = root / "mqtt-password"
        secret.write_text(password)
        secret.chmod(0o600)
        self.config = {
            "broker": {
                "host": "127.0.0.1",
                "port": mqtt_port,
                "client_id": "sigiro-native-test",
                "tls": True,
                "username": "server",
                "password_file": str(secret),
                "ca_file": str(ca),
            },
            "namespace": NAMESPACE,
            "devices": [{"id": DEVICE, "project_id": "default"}],
            "max_batch_bytes": 4096,
            "max_inbox_bytes": 65536,
            "max_batches": 5,
        }
        self.config_path = root / "micro.json"
        self.process = None

    def start(self):
        self.config_path.write_text(json.dumps(self.config))
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("SIGIRO_", "OTEL_"))
        }
        env.update(
            SIGIRO_DATA_DIR=str(self.data),
            SIGIRO_HTTP_PORT=str(self.http),
            SIGIRO_OTLP_HTTP_PORT=str(self.otlp),
            SIGIRO_GRPC_PORT=str(self.grpc),
            SIGIRO_FLUSH_INTERVAL_SECS="1",
            RUST_LOG="sigiro=info,warn",
        )
        # Explicit opt-in preinstalled extensions keep offline experiments
        # reproducible; never inherit unrelated hosted/auth settings.
        if extension_dir := os.environ.get("SIGIRO_EXTENSION_DIR"):
            env["SIGIRO_EXTENSION_DIR"] = extension_dir
        self.log = (self.root / "sigiro.log").open("a")
        self.process = subprocess.Popen(
            [self.binary, "serve", "--micro-config", str(self.config_path)],
            env=env,
            stdout=self.log,
            stderr=subprocess.STDOUT,
        )

        def healthy():
            assert self.process.poll() is None, (
                f"Sigiro exited: {self.root / 'sigiro.log'}"
            )
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{self.http}/health", timeout=1
                ) as response:
                    return response.status == 200
            except (OSError, ValueError):
                return False

        wait_for(healthy, 120)

    def stop(self, crash=False):
        if self.process is not None:
            if self.process.poll() is None:
                (self.process.kill if crash else self.process.terminate)()
                self.process.wait(timeout=45)
            self.log.close()
            self.process = None

    def rows(self):
        database = self.data / ".micro/inbox.sqlite"
        if not database.exists():
            return []
        with sqlite3.connect(database) as conn:
            return conn.execute(
                "SELECT batch_id,signal,payload,state FROM batches ORDER BY row"
            ).fetchall()

    def indexed(self, count):
        return len([row for row in self.rows() if row[3] == "indexed"]) == count

    def query_count(self, table):
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.http}/v1/query",
            data=f"SELECT count(*) AS n FROM {table}".encode(),
            headers={"content-type": "text/plain"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=30) as response:
            return json.load(response)[0]["n"]


def run(binary, root):
    lab = Lab(root)
    complete = False
    with broker(root) as (mqtt_port, credentials, ca, container):
        host = Host(binary, root, mqtt_port, credentials["server"], ca)
        device = Device(mqtt_port, credentials[DEVICE], ca)
        try:
            host.start()
            case = lab.case("qemu-raw")
            assert lab.boot(case, 1, 7, unknown=True)["status"] == 0
            wire = lab.peek(case)
            pending_image = (case / "flash.bin").read_bytes()
            ident, signal, payload = parse_record(wire)
            assert signal == 1

            # Wait for reconnect/subscription without presuming broker PUBACK is acceptance.
            def deliver():
                assert host.process.poll() is None, (
                    f"Sigiro exited: {root / 'sigiro.log'}"
                )
                device.send(ident, "logs", payload)
                try:
                    return device.receipt(ident, timeout=0.5)
                except queue.Empty:
                    return None

            receipt = wait_for(deliver, 30)
            rows = host.rows()
            assert len(rows) == 1 and rows[0][:3] == (ident, signal, payload)
            (case / "ack.bin").write_bytes(receipt)
            assert lab.boot(case, 3)["status"] == 0 and lab.peek(case) == b""
            wait_for(lambda: host.indexed(1))
            assert host.query_count("sigiro_logs") == 1
            lab.passed(
                "QEMU -> authenticated TLS broker -> actual Sigiro -> durable exact raw bytes -> SQL -> matching MCU pop",
                raw_sha256=hashlib.sha256(payload).hexdigest(),
            )
            device.exchange(ident, "logs", payload)
            assert len(host.rows()) == 1 and host.query_count("sigiro_logs") == 1
            lab.passed(
                "redelivery after lost application receipt is raw-deduplicated and not reprojected"
            )
            offline = lab.case("broker-offline", pending_image)
            device.drain()
            device.ready.clear()
            command(["docker", "stop", "--time", "1", container])
            device.send(ident, "logs", payload)
            time.sleep(0.5)
            assert device.receipts.empty() and lab.peek(offline) == wire
            command(["docker", "start", container])
            assert device.ready.wait(20)
            recovered = wait_for(deliver, 35)
            (offline / "ack.bin").write_bytes(recovered)
            assert lab.boot(offline, 3)["status"] == 0 and lab.peek(offline) == b""
            assert len(host.rows()) == 1
            lab.passed(
                "real broker outage retains pending MCU data; verified reconnect and idempotent replay recover"
            )
            host.stop(crash=True)
            assert host.rows()[0][:3] == (ident, signal, payload)
            host.start()
            receipt = wait_for(deliver, 30)
            assert (
                receipt
                and len(host.rows()) == 1
                and host.query_count("sigiro_logs") == 1
            )
            lab.passed(
                "real host SIGKILL/restart preserves acknowledged raw data and deduplication"
            )
            device.exchange(ident, "logs", payload + b"\xa8\x06\x01", expected=5)
            device.exchange(ident, "traces", payload, expected=5)
            assert host.rows()[0][2] == payload and len(host.rows()) == 1
            lab.passed(
                "reused device/batch ID with different bytes or signal cannot receive success"
            )
            malformed = bytes([90]) * 16
            device.exchange(malformed, "logs", b"\x0a\xff\xff", expected=5)
            assert len(host.rows()) == 1
            lab.passed("malformed OTLP never commits a success receipt")
            device.drain()
            spoof = bytes([91]) * 16
            device.send(spoof, "logs", payload, response=f"{NAMESPACE}/other/ack")
            time.sleep(0.5)
            assert len(host.rows()) == 1 and device.receipts.empty()
            lab.passed("unprovisioned response topic is rejected without reflection")
            attacker = Device(mqtt_port, credentials["other"], ca, user="other")
            try:
                forged = attacker.client.publish(
                    ACK_TOPIC, b"SAK1" + spoof + bytes([1, 1, 0, 0]), qos=1
                )
                mid, reason = attacker.published.get(timeout=10)
                assert mid == forged.mid and reason.is_failure
                forbidden = attacker.send(spoof, "logs", payload)
                mid, reason = attacker.published.get(timeout=10)
                assert mid == forbidden.mid and reason.is_failure
                assert device.receipts.empty() and len(host.rows()) == 1
            finally:
                attacker.close()
            probe = mqtt.Client(
                mqtt.CallbackAPIVersion.VERSION2,
                client_id="wrong-server-credential",
                protocol=mqtt.MQTTv5,
            )
            probe.username_pw_set("server", credentials["other"])
            probe.tls_set_context(ssl.create_default_context(cafile=str(ca)))
            refused = queue.Queue()
            probe.on_connect = lambda _c, _u, _f, reason, _p: refused.put(reason)
            probe.connect("127.0.0.1", mqtt_port)
            probe.loop_start()
            try:
                assert refused.get(timeout=10).is_failure
            finally:
                probe.disconnect()
                probe.loop_stop()
            lab.passed(
                "distinct credentials and broker ACLs deny server impersonation, cross-device ingress and forged receipts"
            )
            traces = ExportTraceServiceRequest()
            resource = traces.resource_spans.add()
            attribute = resource.resource.attributes.add()
            attribute.key = "service.name"
            attribute.value.string_value = "micro-native"
            span = resource.scope_spans.add().spans.add()
            span.trace_id = b"t" * 16
            span.span_id = b"s" * 8
            span.name = "micro trace"
            span.start_time_unix_nano = time.time_ns()
            span.end_time_unix_nano = span.start_time_unix_nano + 1
            metrics = ExportMetricsServiceRequest()
            metric = metrics.resource_metrics.add().scope_metrics.add().metrics.add()
            metric.name = "micro.gauge"
            point = metric.gauge.data_points.add()
            point.time_unix_nano = time.time_ns()
            point.as_int = 42
            device.exchange(bytes([92]) * 16, "traces", traces.SerializeToString())
            device.exchange(bytes([93]) * 16, "metrics", metrics.SerializeToString())
            wait_for(lambda: host.indexed(3))
            assert (
                host.query_count("sigiro_spans") == 1
                and host.query_count("sigiro_metrics_gauge") == 1
            )
            lab.passed("native logs/traces/metrics use real Sigiro query projections")
            # Quota is lifetime raw retention; neither indexing nor quarantine erases it.
            for value in [94, 95]:
                device.exchange(bytes([value]) * 16, "logs", payload)
            wait_for(lambda: host.indexed(5))
            full_case = lab.case("qemu-quota")
            lab.boot(full_case, 1, 9)
            full_wire = lab.peek(full_case)
            full_id, _, full_payload = parse_record(full_wire)
            retry = device.exchange(full_id, "logs", full_payload, expected=3)
            (full_case / "ack.bin").write_bytes(retry)
            assert (
                lab.boot(full_case, 3)["status"] == 3
                and lab.peek(full_case) == full_wire
            )
            device.exchange(ident, "logs", payload)
            assert len(host.rows()) == 5
            lab.passed(
                "full raw inbox returns retry, protects pending MCU bytes, and still recognizes old duplicates"
            )
            host.stop()
            # Increase configured capacity; retry the same persistent MCU record.
            host.config["max_batches"] = 7
            host.start()

            def deliver_full():
                assert host.process.poll() is None, (
                    f"Sigiro exited: {root / 'sigiro.log'}"
                )
                device.send(full_id, "logs", full_payload)
                try:
                    return device.receipt(full_id, timeout=0.5)
                except queue.Empty:
                    return None

            accepted = wait_for(deliver_full, 30)
            (full_case / "ack.bin").write_bytes(accepted)
            assert lab.boot(full_case, 3)["status"] == 0 and lab.peek(full_case) == b""
            wait_for(lambda: host.indexed(6))
            assert host.query_count("sigiro_logs") == 4
            lab.passed(
                "capacity recovery replays retained MCU record and only then permits deletion"
            )
            # Well-formed protobuf may still have unsupported/invalid semantic
            # content. A recording receipt does not promise query visibility.
            span.trace_id = b"x"
            device.exchange(bytes([96]) * 16, "traces", traces.SerializeToString())
            wait_for(
                lambda: len(host.rows()) == 7 and host.rows()[-1][3] == "quarantined"
            )
            assert host.query_count("sigiro_spans") == 1
            report = json.loads(
                command([str(binary), "micro", "status", "--data-dir", str(host.data)])
            )
            assert (
                report["batches"] == 7
                and report["receipt_boundary"] == "durably_recorded"
            )
            assert {row["state"]: row["batches"] for row in report["devices"]} == {
                "indexed": 6,
                "quarantined": 1,
            }
            lab.passed(
                "recording and query visibility are distinct; semantic rejection is durably quarantined and inspectable in the same binary"
            )
            host.stop()
            # Deterministic model of the DuckLake-commit/inbox-status crash gap,
            # NOT an actual process kill at a precisely instrumented instruction.
            with sqlite3.connect(host.data / ".micro/inbox.sqlite") as connection:
                connection.execute(
                    "UPDATE batches SET state='pending' WHERE batch_id=?", [ident]
                )
            host.start()
            wait_for(lambda: host.indexed(6))
            assert len(host.rows()) == 7 and host.query_count("sigiro_logs") == 5
            lab.passed(
                "modeled projection commit/status gap repeats a query row, never original raw storage; no exactly-once indexing claim"
            )
            host.stop()
            # Native trust store cannot trust our randomly generated private CA.
            host.config["broker"]["ca_file"] = None
            try:
                host.start()
            except AssertionError:
                pass  # May fail before the HTTP listener becomes healthy.
            wait_for(lambda: host.process.poll() is not None, 30)
            assert (
                host.process.returncode != 0
                and "invalid peer certificate" in (root / "sigiro.log").read_text()
            )
            lab.passed(
                "unknown TLS issuer fails closed instead of accepting unverifiable broker receipts"
            )
            host.stop()
            complete = True
        finally:
            host.stop()
            device.close()
            (root / "results.json").write_text(
                json.dumps(
                    {
                        "completed": complete,
                        "checks": lab.checks,
                        "qemu_boots": lab.boots,
                        "physical_flash_tested": False,
                        "mcu_network_stack_tested": False,
                        "receipt_boundary": "SQLite WAL synchronous=FULL raw inbox commit; query visibility is separate",
                        "projection_delivery": "at-least-once; DuckLake commit -> inbox status gap can repeat projected rows",
                        "broker_image": MOSQUITTO,
                        "firmware": str(ELF),
                        "binary": str(binary),
                    },
                    indent=2,
                )
                + "\n"
            )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/sigiro")
    args = parser.parse_args()
    assert args.binary.exists() and ELF.exists(), (
        "build Sigiro and the independent firmware workspace first"
    )
    (ROOT / "artifacts").mkdir(exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="native-", dir=ROOT / "artifacts"))
    print(f"Artifacts: {root}", flush=True)
    run(args.binary.resolve(), root)


if __name__ == "__main__":
    main()

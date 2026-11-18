#!/usr/bin/env python3
"""Black-box invariants for the native shared-host deployment contract."""
from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import stat
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
UNITS = ROOT / "ops/prod/systemd"


class NativeContractTests(unittest.TestCase):
    def test_alchemy_is_the_only_native_mutation_boundary(self) -> None:
        source = (ROOT / "www/alchemy.workload.run.ts").read_text()
        # The host runs no container engine, so the stack has no Docker provider.
        self.assertNotIn("Docker", source)
        self.assertIn("Command.Exec", source)
        self.assertIn("SIGIRO_NATIVE_ACTION", source)
        self.assertIn("memo:", source)
        self.assertIn("SIGIRO_AUTH_DB_TLS_KEY_B64", source)

    def test_units_have_hardening_health_and_resource_limits(self) -> None:
        expected = {
            "sigiro.service": ("CPUQuota=100%", "MemoryMax=1792M", "ProtectSystem=strict", "ExecStartPost="),
            "sigiro-edge.service": ("CPUQuota=5%", "MemoryMax=64M", "ExecStartPost="),
            "kamal-proxy.service": ("MemoryMax=256M", "AmbientCapabilities=CAP_NET_BIND_SERVICE"),
        }
        for name, directives in expected.items():
            text = (UNITS / name).read_text()
            for directive in directives:
                self.assertIn(directive, text, name)
            self.assertNotIn("--password", text)
            self.assertIn("NoNewPrivileges=yes", text)

    def test_postgres_is_18_tls_only_and_role_isolated(self) -> None:
        hba = (ROOT / "ops/prod/pg_hba.conf").read_text()
        for role in ("linguabench_app", "sigiro"):
            self.assertIn(role, hba)
        # The Better Auth database and role were retired with the service; a
        # provisioning run that recreated them would resurrect dead state.
        self.assertNotIn("sigiro_auth", hba)
        self.assertIn("hostssl", hba)
        self.assertIn("reject", hba)
        config = (ROOT / "ops/prod/postgresql.conf").read_text()
        self.assertIn("ssl = on", config)
        self.assertIn("ssl_min_protocol_version = 'TLSv1.3'", config)
        self.assertIn("shared_preload_libraries = 'pg_cron'", config)
        self.assertIn("cron.database_name = 'pipeline'", config)

    def test_routes_are_registered_by_a_unit_that_joins_the_proxy_namespace(self) -> None:
        # kamal-proxy runs with PrivateTmp=yes and creates its control socket
        # under TMPDIR, so the socket exists only inside that namespace. Calling
        # the CLI from a deploy shell fails with
        # "dial unix /run/user/0/kamal-proxy.sock: no such file or directory".
        unit = (UNITS / "kamal-proxy-routes.service").read_text()
        self.assertIn("JoinsNamespaceOf=kamal-proxy.service", unit)
        self.assertIn("ExecStart=/usr/local/sbin/configure-native-routes", unit)
        self.assertIn("WantedBy=multi-user.target", unit)

    def test_proxy_is_not_ordered_after_the_service_that_depends_on_it(self) -> None:
        # kamal-proxy shipped with After=...sigiro.service, but sigiro fetches
        # the Better Auth JWKS over its PUBLIC issuer URL during startup, which
        # only resolves once the proxy serves auth-origin. That ordering is a
        # boot deadlock: every reboot fails sigiro's first start attempt.
        proxy = (UNITS / "kamal-proxy.service").read_text()
        after = [line for line in proxy.splitlines() if line.startswith("After=")]
        self.assertEqual(len(after), 1, "expected exactly one After= line")
        self.assertNotIn(
            "sigiro.service",
            after[0],
            "kamal-proxy must not be ordered after sigiro; sigiro needs the proxy to boot",
        )

    def test_routes_must_be_reregistered_because_the_proxy_persists_nothing(self) -> None:
        # The native proxy writes no route state, so boot recovery depends
        # entirely on the oneshot unit re-registering. Verified on the host: the
        # only state file predates the native deployment and still names retired
        # Docker container IDs.
        unit = (UNITS / "kamal-proxy-routes.service").read_text()
        self.assertIn("WantedBy=multi-user.target", unit)
        self.assertIn("Requires=kamal-proxy.service", unit)
        self.assertIn("After=kamal-proxy.service", unit)

    def test_no_unit_depends_on_a_docker_path(self) -> None:
        # The host runs no container engine. kamal-proxy
        # shipped with XDG_CONFIG_HOME and ReadWritePaths pointing into
        # /var/lib/docker/volumes, so the native ingress read its live TLS
        # certificates from a Docker volume: Docker stayed load-bearing and
        # `docker volume prune` could break production ingress.
        offenders = []
        for unit in sorted(UNITS.glob("*.service")) + sorted(UNITS.glob("*.timer")):
            # Only directives matter; a comment may legitimately mention the old
            # Docker path to explain why the migration happened.
            directives = [
                line
                for line in unit.read_text().splitlines()
                if not line.lstrip().startswith("#")
            ]
            if any("/var/lib/docker" in line for line in directives):
                offenders.append(unit.name)
        self.assertEqual(
            offenders,
            [],
            "unit files must not reference Docker paths: " + ", ".join(offenders),
        )
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        verify = host[host.index("verify() {") :]
        self.assertIn("a container engine is installed on the native host", verify)

    def test_reboot_recovery_requires_every_unit_to_be_enabled(self) -> None:
        # The proxy's persisted state survives from the Docker deployment and
        # still names retired container IDs, so routes must be re-registered on
        # boot rather than restored. Only an enabled unit does that.
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        verify = host[host.index("verify() {") :]
        self.assertIn("systemctl is-enabled --quiet", verify)
        self.assertIn("would not survive reboot", verify)
        self.assertIn("kamal-proxy-routes", verify)

    def test_sandbox_paths_survive_reboot(self) -> None:
        # /run is tmpfs. sigiro-edge listed ReadWritePaths=/run/sigiro-edge,
        # which only staging created, so after the first reboot systemd could
        # not build its mount namespace and the edge crash-looped (226/NAMESPACE).
        for unit in sorted(UNITS.glob("*.service")):
            for line in unit.read_text().splitlines():
                if line.startswith(("ReadWritePaths=", "ReadOnlyPaths=")):
                    for path in line.split("=", 1)[1].split():
                        self.assertFalse(path.lstrip("-").startswith("/run/"), f"{unit.name}: {path}")

    def test_routes_preserve_every_public_host_and_path(self) -> None:
        routes = (ROOT / "ops/prod/configure-native-routes.sh").read_text()
        # auth-origin.sigiro.com is deliberately absent: that route served the
        # retired Node auth service and its DNS record no longer resolves, so
        # re-registering it would publish a permanently unhealthy target.
        self.assertNotIn("auth-origin.sigiro.com", routes)
        for token in (
            "linguabench.akshat.dev",
            "linguabench-origin.akshat.dev",
            "api-origin.sigiro.com",
            "/v1development/profiles",
            "/push.v1.PusherService/Push",
            "--strip-path-prefix=false",
            "--max-request-body 8388608",
        ):
            self.assertIn(token, routes)

    def test_artifact_is_deterministic_and_checksum_verified(self) -> None:
        package = ROOT / "scripts/package-native-artifact.py"
        install = ROOT / "scripts/install-verified-artifact.py"
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            binary = temp / "sigiro"
            binary.write_bytes(b"#!/bin/sh\nprintf native\\n")
            binary.chmod(0o755)
            out = temp / "out"
            command = [
                "python3",
                str(package),
                "--input",
                str(binary),
                "--service",
                "sigiro",
                "--version",
                "0.1.0",
                "--output",
                str(out),
            ]
            subprocess.run(command, check=True, capture_output=True)
            first = next(out.glob("*.tar.gz")).read_bytes()
            subprocess.run(command, check=True, capture_output=True)
            archive = next(out.glob("*.tar.gz"))
            self.assertEqual(first, archive.read_bytes())
            digest = hashlib.sha256(first).hexdigest()
            target = temp / "opt"
            subprocess.run(
                [
                    "python3",
                    str(install),
                    "--archive",
                    str(archive),
                    "--sha256",
                    digest,
                    "--service",
                    "sigiro",
                    "--root",
                    str(target),
                ],
                check=True,
                capture_output=True,
            )
            current = target / "sigiro/current"
            self.assertTrue(current.is_symlink())
            self.assertEqual((current / "sigiro").read_bytes(), binary.read_bytes())
            with tarfile.open(archive) as bundle:
                self.assertEqual(bundle.getnames(), ["payload/sigiro"])

    def test_secret_renderer_fails_closed_and_preserves_tls_verification(self) -> None:
        renderer = ROOT / "scripts/render-native-secrets.py"
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            env = {
                key: value
                for key, value in os.environ.items()
                if not key.startswith(("SIGIRO_", "BETTER_AUTH_", "CLOUDFLARE_"))
            } | {
                "SIGIRO_CATALOG_PASSWORD": "catalog p@ss",
                "SIGIRO_AUTH_DB_TLS_CERT_B64": base64.b64encode(b"-----BEGIN CERTIFICATE-----\nx\n").decode(),
                "SIGIRO_AUTH_DB_TLS_KEY_B64": base64.b64encode(b"-----BEGIN PRIVATE KEY-----\nx\n").decode(),
                "SIGIRO_ADMIN_TOKEN": "admin-token",
                "SIGIRO_INGEST_CLIENT_ID": "test-only-client",
                "SIGIRO_INGEST_CLIENT_SECRET": "test-only-secret",
                "SIGIRO_INGEST_RELAY_KEY": "a" * 64,
            }
            subprocess.run(["python3", str(renderer), "--output", str(target)], env=env, check=True)
            for name in ("sigiro.env", "database.env", "server.crt", "server.key", "ingest-relay.env", "linguabench-relay.env"):
                self.assertEqual(stat.S_IMODE((target / name).stat().st_mode), 0o600)
            self.assertIn("sslmode=verify-full", (target / "sigiro.env").read_text())
            self.assertIn("sslrootcert=/etc/sigiro/postgres-ca.crt", (target / "sigiro.env").read_text())
            self.assertNotIn("SIGIRO_OPTIMIZER", (target / "sigiro.env").read_text())
            self.assertNotIn("SIGIRO_JEV", (target / "sigiro.env").read_text())
            # The retired auth service's EnvironmentFiles must not be produced:
            # writing them would re-supply credentials for a dead unit.
            for dead in ("auth.env", "cloudflared.env"):
                self.assertFalse((target / dead).exists(), dead)

    def test_profiler_endpoint_is_a_versioned_late_environment_override(self) -> None:
        env = (ROOT / "ops/prod/environment/linguabench-profiler.env").read_text()
        dropin = (UNITS / "linguabench-profiler.conf").read_text()
        deploy = (ROOT / "ops/prod/deploy-native-host.sh").read_text()
        native = (ROOT / "ops/prod/native-host.sh").read_text()
        self.assertIn("PYROSCOPE_SERVER_ADDRESS=http://127.0.0.1:14318", env)
        self.assertNotIn("api.sigiro.com", env.splitlines()[-1])
        self.assertIn('cp "$repo/ops/optimizer/read-bounded-body.mjs"', deploy)
        self.assertIn('cp "$repo/ops/optimizer/relay-target.mjs"', deploy)
        self.assertIn('install -m 0644 "$stage/relay-target.mjs" /usr/local/libexec/relay-target.mjs', native)
        self.assertIn("EnvironmentFile=/etc/linguabench/profiler.env", dropin)
        self.assertIn('cp "$repo/ops/prod/systemd/linguabench-profiler.conf"', deploy)
        self.assertIn('cp "$repo/ops/prod/environment/"*.env', deploy)
        self.assertIn('install -m 0644 "$stage/environment/linguabench-profiler.env" /etc/linguabench/profiler.env', native)
        self.assertIn('"/etc/systemd/system/$unit.service.d/profiler.conf"', native)
        self.assertIn('if [[ "$unit" != linguabench-analysis ]]; then', native)

    def test_staging_uses_disk_backed_storage_and_reclaims_space(self) -> None:
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        deploy = (ROOT / "ops/prod/deploy-native-host.sh").read_text()
        self.assertIn("stage=/var/cache/sigiro-native-stage", host)
        self.assertNotIn("/run/sigiro-native-stage", host)
        self.assertNotIn("/run/sigiro-native-stage", deploy)
        self.assertIn('rm -f "$stage/artifacts/$archive"', host)
        self.assertIn("insufficient disk for artifact installation", host)
        self.assertIn("COPYFILE_DISABLE=1", deploy)
        self.assertIn("AppleDouble", deploy)

    def test_bulk_artifacts_transfer_resumably_with_ssh_keepalive(self) -> None:
        deploy = (ROOT / "ops/prod/deploy-native-host.sh").read_text()
        self.assertIn("ServerAliveInterval=15", deploy)
        self.assertIn("rsync", deploy)
        self.assertIn("--partial", deploy)
        # The artifact payload must not ride the contract tar over a single
        # SSH stdin pipe, which cannot resume after a stalled transfer.
        self.assertIn('cp "$archive" "$tmp/artifacts/$filename"', deploy)
        self.assertNotIn('cp "$archive" "$tmp/contract/artifacts/$filename"', deploy)

    def test_no_bare_test_and_as_a_trailing_statement(self) -> None:
        # `[[ cond ]] && cmd` as the last statement of a function returns the
        # test's exit status, so a false test makes the FUNCTION fail under
        # `set -e`. This silently aborted staging after the smoke boot had
        # already passed, with no error message at all.
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        offenders = [
            line.strip()
            for line in host.splitlines()
            if re.match(r"\s*\[\[.*\]\]\s+&&\s+\S", line)
            and "||" not in line
        ]
        self.assertEqual(offenders, [], f"bare '[[ ]] &&' statements: {offenders}")

    def test_artifact_installation_is_idempotent(self) -> None:
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        # Archives are deleted after install to cap peak disk, and
        # install_artifacts runs in both the stage and deploy paths. A second
        # pass must accept an already-installed digest instead of failing.
        self.assertIn('if [[ ! -f "$stage/artifacts/$archive" ]]; then', host)
        self.assertIn('if [[ -d "/opt/$service/releases/$digest" ]]; then', host)
        # But a missing archive for a digest that is NOT installed must still fail.
        self.assertIn("is not installed", host)

    def test_sync_admin_queries_never_run_on_the_async_runtime(self) -> None:
        admin = (ROOT / "src/admin.rs").read_text()
        main = (ROOT / "src/main.rs").read_text()
        server = (ROOT / "src/server.rs").read_text()
        # The sync postgres client calls block_on internally and ABORTS the
        # process from a runtime thread. block_in_place does not help: it keeps
        # the runtime context. Only spawn_blocking gives a context-free thread.
        self.assertIn("pub async fn with_admin_blocking", admin)
        self.assertIn("spawn_blocking", admin)
        self.assertIn("with_admin_blocking(admin_conn", main)
        # The /admin/status handler must not touch the sync client inline.
        self.assertIn("spawn_blocking", server)
        self.assertNotIn(
            "let mut guard = state.admin_conn.lock();\n    let mut map =", server
        )

    def test_hosted_admin_connect_does_not_block_the_async_runtime(self) -> None:
        admin = (ROOT / "src/admin.rs").read_text()
        main = (ROOT / "src/main.rs").read_text()
        # The sync postgres client calls block_on internally, which panics with
        # "Cannot start a runtime from within a runtime" on a #[tokio::main] thread.
        self.assertIn("spawn_blocking", admin)
        self.assertIn("pub async fn open_hosted_admin_db", admin)
        self.assertIn("open_hosted_admin_db(config.catalog_url()).await", main)

    def test_retired_auth_service_cannot_be_reprovisioned(self) -> None:
        # The Node auth service and its Cloudflare tunnel were deleted from the
        # host: the artifact, the unit files, /opt/sigiro-auth, the DNS record,
        # and the sigiro_auth database and role are all gone. A provisioning run
        # that still referenced them would either fail on the missing artifact
        # or resurrect a retired public surface.
        self.assertFalse((UNITS / "sigiro-auth.service").exists())
        self.assertFalse((UNITS / "cloudflared.service").exists())
        self.assertFalse((ROOT / "ops/prod/environment/sigiro-auth.env").exists())
        for path in (
            "ops/prod/native-host.sh",
            "ops/prod/deploy-native-host.sh",
            "ops/prod/configure-native-routes.sh",
            "scripts/render-native-secrets.py",
        ):
            text = (ROOT / path).read_text()
            for token in ("sigiro-auth", "sigiro_auth", "cloudflared", "auth-origin"):
                self.assertNotIn(token, text, f"{path} still references {token}")

    def test_otlp_chunked_body_reader_is_packaged_with_the_relay(self) -> None:
        deploy = (ROOT / "ops/prod/deploy-native-host.sh").read_text()
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        alchemy = (ROOT / "www/alchemy.workload.run.ts").read_text()
        relay = (ROOT / "ops/optimizer/ingest-relay.mjs").read_text()
        self.assertIn('cp "$repo/ops/optimizer/read-bounded-body.mjs"', deploy)
        self.assertIn('"$stage/read-bounded-body.mjs"', host)
        self.assertIn('"ops/optimizer/read-bounded-body.mjs"', alchemy)
        self.assertIn('from "./read-bounded-body.mjs"', relay)

if __name__ == "__main__":
    unittest.main()

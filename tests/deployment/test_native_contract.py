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
        # The Docker provider stays registered only so Alchemy can tear down the
        # legacy Docker state rows. Declaring any Docker resource would restore a
        # second deployment path, so only `Docker.providers()` may appear.
        self.assertEqual(source.count("Docker."), 1)
        self.assertIn("Docker.providers()", source)
        for resource in ("Docker.Context", "Docker.Network", "Docker.Container", "Docker.Image"):
            self.assertNotIn(resource, source)
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
        migration = (ROOT / "ops/prod/migrate-postgresql18.sh").read_text()
        self.assertIn("PostgreSQL 18 pg_restore is required", migration)
        self.assertIn("diff -u", migration)
        self.assertIn("pg_dump -U postgres -d pipeline", migration)
        # Sigiro's catalog MUST migrate too. This test previously asserted the
        # opposite -- that it was left empty -- and that contract is what let a
        # cutover destroy the DuckLake metadata for every tenant while reporting
        # success.
        self.assertIn("pg_dump -U sigiro -d sigiro ", migration)
        # Better Auth moved to a Cloudflare Worker on D1 and its PostgreSQL
        # database and role are gone from this host.
        self.assertNotIn("sigiro_auth", migration)
        self.assertIn("LinguaBench and Sigiro both preserved", migration)
        self.assertNotIn("Sigiro initialized empty", migration)

    def test_migration_fails_when_sigiro_state_did_not_survive(self) -> None:
        # An empty restore must be a hard failure, not a silent success: health
        # checks pass against an empty catalog, so nothing else catches it.
        migration = (ROOT / "ops/prod/migrate-postgresql18.sh").read_text()
        self.assertIn("select count(*) from sigiro_control.tenants", migration)
        self.assertIn("DuckLake telemetry would be orphaned", migration)

    def test_sigiro_starts_after_ingress_because_it_fetches_public_jwks(self) -> None:
        # sigiro resolves SIGIRO_AUTH_JWKS_URL over its PUBLIC issuer during
        # startup, so it must not race the rest of the cutover. Starting it
        # alongside the other services risks failing the whole cutover with
        # "failed to fetch Better Auth JWKS during startup".
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        cutover = host[host.index("cutover() {") :]
        proxy_at = cutover.index("systemctl enable --now kamal-proxy.service")
        sigiro_at = cutover.index("systemctl enable --now sigiro.service")
        self.assertLess(
            proxy_at,
            sigiro_at,
            "sigiro must start after kamal-proxy; it needs the public JWKS URL to boot",
        )

    def test_routes_are_registered_by_a_unit_that_joins_the_proxy_namespace(self) -> None:
        # kamal-proxy runs with PrivateTmp=yes and creates its control socket
        # under TMPDIR, so the socket exists only inside that namespace. Calling
        # the CLI from the cutover shell fails with
        # "dial unix /run/user/0/kamal-proxy.sock: no such file or directory".
        unit = (UNITS / "kamal-proxy-routes.service").read_text()
        self.assertIn("JoinsNamespaceOf=kamal-proxy.service", unit)
        self.assertIn("ExecStart=/usr/local/sbin/configure-native-routes", unit)
        self.assertIn("WantedBy=multi-user.target", unit)
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        cutover = host[host.index("cutover() {") :]
        self.assertIn("systemctl enable --now kamal-proxy-routes.service", cutover)
        self.assertNotIn("\n  /usr/local/sbin/configure-native-routes\n", cutover)

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
        # The cutover is only complete if Docker can be uninstalled. kamal-proxy
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
        self.assertIn("/var/lib/docker", verify)
        self.assertIn("still references a Docker path", verify)

    def test_reboot_recovery_requires_every_unit_to_be_enabled(self) -> None:
        # The proxy's persisted state survives from the Docker deployment and
        # still names retired container IDs, so routes must be re-registered on
        # boot rather than restored. Only an enabled unit does that.
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        verify = host[host.index("verify() {") :]
        self.assertIn("systemctl is-enabled --quiet", verify)
        self.assertIn("would not survive reboot", verify)
        self.assertIn("kamal-proxy-routes", verify)

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
            }
            subprocess.run(["python3", str(renderer), "--output", str(target)], env=env, check=True)
            for name in ("sigiro.env", "database.env", "server.crt", "server.key"):
                self.assertEqual(stat.S_IMODE((target / name).stat().st_mode), 0o600)
            self.assertIn("sslmode=verify-full", (target / "sigiro.env").read_text())
            self.assertIn("sslrootcert=/etc/sigiro/postgres-ca.crt", (target / "sigiro.env").read_text())
            # The retired auth service's EnvironmentFiles must not be produced:
            # writing them would re-supply credentials for a dead unit.
            for dead in ("auth.env", "cloudflared.env"):
                self.assertFalse((target / dead).exists(), dead)

    def test_cutover_preserves_docker_volumes_and_requires_backup(self) -> None:
        deploy = (ROOT / "ops/prod/deploy-native-host.sh").read_text()
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        rollback = (ROOT / "scripts/rollback-native-host.sh").read_text()
        combined = deploy + host + rollback
        for forbidden in ("down -v", "docker volume rm", "rm -rf /var/lib/docker/volumes"):
            self.assertNotIn(forbidden, combined)
        self.assertIn("LINGUABENCH_BACKUP_RECEIPT", deploy)
        self.assertIn("backup-verified", host)
        self.assertNotIn("migrate_telemetry", host)
        self.assertNotIn("linguabench_sigiro_hosted_data", host)
        self.assertIn("--accept-native-write-loss", rollback)

    def test_backup_preserves_linguabench_without_copying_sigiro_data(self) -> None:
        backup = (ROOT / "scripts/backup-native-host.sh").read_text()
        self.assertIn("linguabench-pipeline.pgdump.age", backup)
        self.assertIn("linguabench-globals.sql.age", backup)
        self.assertIn("kamal-proxy-config.tar.age", backup)
        for discarded in (
            "sigiro.pgdump.age",
            "sigiro-auth.pgdump.age",
            "sigiro-globals.sql.age",
            "sigiro-hosted-data.tar.age",
        ):
            self.assertNotIn(discarded, backup)

    def test_linguabench_environment_conversion_requires_verify_full(self) -> None:
        renderer = ROOT / "scripts/render-linguabench-environment.py"
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            inspect = temp / "inspect.json"
            output = temp / "service.env"
            inspect.write_text(
                json.dumps([{"Config": {"Env": ["DATABASE_URL=postgresql://linguabench_app:p%40ss@postgres:5432/pipeline", "DB_SSL=false", "TOKEN=value"]}}])
            )
            subprocess.run(["python3", str(renderer), "--inspect", str(inspect), "--output", str(output)], check=True)
            text = output.read_text()
            self.assertIn("auth-db.sigiro.com:5432/pipeline?sslmode=verify-full", text)
            self.assertIn("p%40ss", text)
            self.assertNotIn("p%2540ss", text)
            self.assertIn('DB_SSL="true"', text)
            self.assertIn('DB_SSL_ROOT_CERT="/etc/linguabench/postgres-ca.crt"', text)

    def test_native_releases_are_disabled_until_successful_cutover(self) -> None:
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        rollback = (ROOT / "scripts/rollback-native-host.sh").read_text()
        self.assertIn("native deployment is disabled until cutover completes", host)
        self.assertIn("/var/lib/sigiro-migration/native-active", host)
        self.assertIn("install_linguabench_units", host)
        self.assertIn("/opt/linguabench/current/deploy", host)
        self.assertIn("rm -f /var/lib/sigiro-migration/native-active", rollback)

    def test_postgresql_rehearsal_cannot_claim_the_live_database_port(self) -> None:
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        migration = (ROOT / "ops/prod/migrate-postgresql18.sh").read_text()
        self.assertIn("port = 55432", host)
        self.assertIn("rm -f /etc/postgresql/18/main/rehearsal.conf", host)
        self.assertIn("SIGIRO_POSTGRESQL_MIGRATION_PORT:-55432", migration)
        self.assertIn('psql -p "$target_port"', migration)
        self.assertIn('pg_restore -p "$target_port"', migration)
        self.assertIn("native host is already active; use deploy instead of stage", host)
        self.assertIn("rehearsal is disabled after native cutover", host)

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

    def test_staging_boots_sigiro_not_just_version(self) -> None:
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        # `sigiro --version` returns before runtime init, so it cannot catch a
        # SIGABRT on the hosted boot path. Staging must run the real server.
        self.assertIn("smoke_sigiro_boot", host)
        self.assertIn("sigiro serve", host)
        self.assertIn("Cannot start a runtime from within a runtime", host)
        # The smoke instance must not collide with, or serve, real traffic.
        self.assertIn("SIGIRO_HTTP_PORT", host)
        self.assertIn("SIGIRO_OTLP_HTTP_PORT", host)
        self.assertIn("SIGIRO_GRPC_PORT", host)

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

    def test_staging_initializes_a_missing_postgres_cluster(self) -> None:
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        # apt-get install is a no-op when postgresql-18 is already present, so a
        # cluster whose datadir was removed stays registered but unstartable.
        self.assertIn("PG_VERSION", host)
        self.assertIn("initdb", host)
        # Must match the live PG17 source locale or restored collations diverge.
        self.assertIn("--locale=en_US.utf8", host)
        self.assertIn("--data-checksums", host)

    def test_staging_proves_entrypoints_load_before_cutover(self) -> None:
        host = (ROOT / "ops/prod/native-host.sh").read_text()
        nginx = (ROOT / "ops/prod/sigiro-edge.nginx.conf").read_text()
        # systemd-analyze only parses units; staging must also execute entrypoints.
        self.assertIn("smoke_entrypoints", host)
        self.assertIn("--import tsx", host)
        self.assertIn("nginx -t", host)
        # /dev/stderr is not openable under systemd's journal socket.
        self.assertNotIn("error_log /dev/stderr", nginx)
        self.assertIn("error_log stderr", nginx)

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
            "ops/prod/migrate-postgresql18.sh",
            "scripts/rollback-native-host.sh",
            "scripts/render-native-secrets.py",
        ):
            text = (ROOT / path).read_text()
            for token in ("sigiro-auth", "sigiro_auth", "cloudflared", "auth-origin"):
                self.assertNotIn(token, text, f"{path} still references {token}")

    def test_alchemy_uses_the_linguabench_backup_receipt_contract(self) -> None:
        alchemy = (ROOT / "www/alchemy.workload.run.ts").read_text()
        deploy = (ROOT / "ops/prod/deploy-native-host.sh").read_text()
        self.assertIn("LINGUABENCH_BACKUP_RECEIPT", alchemy)
        self.assertIn("LINGUABENCH_BACKUP_RECEIPT", deploy)
        self.assertNotIn("SIGIRO_BACKUP_RECEIPT", alchemy)


if __name__ == "__main__":
    unittest.main()

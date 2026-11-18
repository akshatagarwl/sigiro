#!/usr/bin/env python3
"""Render root-owned systemd EnvironmentFiles from deployment secrets."""
from __future__ import annotations

import argparse
import base64
import os
import stat
from pathlib import Path
from urllib.parse import quote

# SIGIRO_AUTH_DB_TLS_* is misleadingly named: it is the PostgreSQL server
# certificate and key for the LIVE catalog cluster, not auth material. It stays.
# The retired Better Auth service's own secrets (tunnel token, database URL,
# signing secret, email token) are not rendered here any more: auth runs as a
# Cloudflare Worker on D1 and is configured by www/alchemy.run.ts.
REQUIRED = (
    "SIGIRO_CATALOG_PASSWORD",
    "SIGIRO_AUTH_DB_TLS_CERT_B64",
    "SIGIRO_AUTH_DB_TLS_KEY_B64",
    "SIGIRO_ADMIN_TOKEN",
)


def encode(value: str) -> str:
    if "\n" in value or "\r" in value or "\x00" in value:
        raise ValueError("environment values may not contain control characters")
    return value.replace("\\", "\\\\").replace('"', '\\"').replace("$", "\\$").replace("`", "\\`")


def write_environment(path: Path, values: dict[str, str]) -> None:
    path.write_text("".join(f'{key}="{encode(value)}"\n' for key, value in values.items()), encoding="utf-8")
    path.chmod(stat.S_IRUSR | stat.S_IWUSR)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    missing = [key for key in REQUIRED if not os.environ.get(key)]
    if missing:
        raise SystemExit(f"missing required deployment secrets: {', '.join(missing)}")

    certificate = base64.b64decode(os.environ["SIGIRO_AUTH_DB_TLS_CERT_B64"], validate=True)
    key = base64.b64decode(os.environ["SIGIRO_AUTH_DB_TLS_KEY_B64"], validate=True)
    if b"BEGIN CERTIFICATE" not in certificate or b"BEGIN" not in key or b"PRIVATE KEY" not in key:
        raise SystemExit("database TLS material is not PEM")

    args.output.mkdir(parents=True, exist_ok=True)
    args.output.chmod(stat.S_IRWXU)
    catalog_password = quote(os.environ["SIGIRO_CATALOG_PASSWORD"], safe="")
    catalog_url = (
        f"postgresql://sigiro:{catalog_password}@auth-db.sigiro.com:5432/sigiro"
        "?sslmode=verify-full&sslrootcert=/etc/sigiro/postgres-ca.crt"
    )
    write_environment(
        args.output / "sigiro.env",
        {
            "SIGIRO_CATALOG_URL": catalog_url,
            "SIGIRO_ADMIN_TOKEN": os.environ["SIGIRO_ADMIN_TOKEN"],
        },
    )
    write_environment(
        args.output / "database.env",
        {"SIGIRO_CATALOG_PASSWORD": os.environ["SIGIRO_CATALOG_PASSWORD"]},
    )
    (args.output / "server.crt").write_bytes(certificate)
    (args.output / "server.key").write_bytes(key)
    (args.output / "server.crt").chmod(stat.S_IRUSR | stat.S_IWUSR)
    (args.output / "server.key").chmod(stat.S_IRUSR | stat.S_IWUSR)


if __name__ == "__main__":
    main()

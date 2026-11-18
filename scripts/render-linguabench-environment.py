#!/usr/bin/env python3
"""Convert a live LinguaBench container inspect document to a systemd EnvironmentFile."""
from __future__ import annotations

import argparse
import json
import stat
from pathlib import Path
from urllib.parse import quote, unquote, urlsplit, urlunsplit

DROP = {
    "KAMAL_CONTAINER_NAME",
    "KAMAL_HOST",
    "KAMAL_VERSION",
    "NODE_VERSION",
    "PNPM_HOME",
    "YARN_VERSION",
    "PATH",
}


def encode(value: str) -> str:
    if "\n" in value or "\r" in value or "\x00" in value:
        raise ValueError("container environment contains a control character")
    return value.replace("\\", "\\\\").replace('"', '\\"').replace("$", "\\$").replace("`", "\\`")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--inspect", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    document = json.loads(args.inspect.read_text(encoding="utf-8"))
    if not isinstance(document, list) or len(document) != 1:
        raise SystemExit("expected one Docker inspect document")
    entries = document[0].get("Config", {}).get("Env", [])
    values = dict(entry.split("=", 1) for entry in entries if "=" in entry)
    for key in DROP:
        values.pop(key, None)
    raw_url = values.get("DATABASE_URL")
    if not raw_url:
        raise SystemExit("live LinguaBench environment has no DATABASE_URL")
    parsed = urlsplit(raw_url)
    if not parsed.password:
        raise SystemExit("live LinguaBench DATABASE_URL has no password")
    values["DATABASE_URL"] = urlunsplit(
        (
            "postgresql",
            f"linguabench_app:{quote(unquote(parsed.password), safe='')}@auth-db.sigiro.com:5432",
            "/pipeline",
            "sslmode=verify-full",
            "",
        )
    )
    values["DB_SSL"] = "true"
    values["DB_SSL_ROOT_CERT"] = "/etc/linguabench/postgres-ca.crt"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        "".join(f'{key}="{encode(values[key])}"\n' for key in sorted(values)),
        encoding="utf-8",
    )
    args.output.chmod(stat.S_IRUSR | stat.S_IWUSR)


if __name__ == "__main__":
    main()

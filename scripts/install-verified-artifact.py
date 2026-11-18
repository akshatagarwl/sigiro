#!/usr/bin/env python3
"""Verify and atomically install a native service artifact."""
from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import tarfile
import tempfile
from pathlib import Path, PurePosixPath


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--service", required=True)
    parser.add_argument("--root", required=True, type=Path)
    return parser.parse_args()


def link_stays_in_payload(member: PurePosixPath, target: PurePosixPath) -> bool:
    parts: list[str] = []
    for part in (member.parent / target).parts:
        if part in ("", "."):
            continue
        if part == "..":
            if not parts:
                return False
            parts.pop()
        else:
            parts.append(part)
    return bool(parts) and parts[0] == "payload"


def validate_member(member: tarfile.TarInfo) -> None:
    path = PurePosixPath(member.name)
    if path.is_absolute() or not path.parts or path.parts[0] != "payload" or ".." in path.parts:
        raise ValueError(f"unsafe artifact member: {member.name}")
    if member.ischr() or member.isblk() or member.isfifo():
        raise ValueError(f"unsupported artifact member: {member.name}")
    if member.issym() or member.islnk():
        target = PurePosixPath(member.linkname)
        if target.is_absolute() or not link_stays_in_payload(path, target):
            raise ValueError(f"unsafe artifact link: {member.name} -> {member.linkname}")


def main() -> None:
    args = parse_args()
    expected = args.sha256.lower()
    if len(expected) != 64 or any(char not in "0123456789abcdef" for char in expected):
        raise SystemExit("sha256 must be 64 lowercase hexadecimal characters")
    if not args.archive.is_file():
        raise SystemExit(f"artifact archive does not exist: {args.archive}")
    actual = hashlib.sha256(args.archive.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"checksum mismatch for {args.archive}")

    service_root = args.root / args.service
    releases = service_root / "releases"
    release = releases / actual
    releases.mkdir(parents=True, exist_ok=True)
    if not release.exists():
        with tempfile.TemporaryDirectory(dir=releases, prefix=f".{actual}.") as staging_name:
            staging = Path(staging_name)
            with tarfile.open(args.archive, "r:gz") as bundle:
                members = bundle.getmembers()
                if not members:
                    raise SystemExit("artifact is empty")
                for member in members:
                    validate_member(member)
                bundle.extractall(staging, filter="data")
            payload = staging / "payload"
            if not payload.is_dir():
                raise SystemExit("artifact has no payload directory")
            os.rename(payload, release)

    current = service_root / "current"
    temporary_link = service_root / f".current.{os.getpid()}"
    temporary_link.symlink_to(release)
    os.replace(temporary_link, current)
    print(release)


if __name__ == "__main__":
    main()

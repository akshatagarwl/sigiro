#!/usr/bin/env python3
"""Create a deterministic, content-addressed native service artifact."""
from __future__ import annotations

import argparse
import gzip
import hashlib
import os
import stat
import tarfile
from pathlib import Path, PurePosixPath


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--service", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def normalized(info: tarfile.TarInfo) -> tarfile.TarInfo:
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    info.pax_headers = {}
    return info


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


def add_path(bundle: tarfile.TarFile, source: Path, name: PurePosixPath) -> None:
    info = normalized(bundle.gettarinfo(str(source), str(name)))
    if info.ischr() or info.isblk() or info.isfifo():
        raise ValueError(f"unsupported artifact entry: {source}")
    if info.issym():
        target = PurePosixPath(info.linkname)
        if target.is_absolute() or not link_stays_in_payload(name, target):
            raise ValueError(f"unsafe symlink: {source} -> {info.linkname}")
        bundle.addfile(info)
    elif info.isfile():
        with source.open("rb") as handle:
            bundle.addfile(info, handle)
    else:
        bundle.addfile(info)


def main() -> None:
    args = parse_args()
    if not args.input.exists():
        raise SystemExit(f"artifact input does not exist: {args.input}")
    if not args.service.replace("-", "").isalnum() or args.service.lower() != args.service:
        raise SystemExit("service must contain only lowercase letters, digits, and hyphens")

    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / f"{args.service}-{args.version}-x86_64-linux.tar.gz"
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as bundle:
                if args.input.is_dir():
                    add_path(bundle, args.input, PurePosixPath("payload"))
                    for source in sorted(args.input.rglob("*"), key=lambda value: value.as_posix()):
                        relative = source.relative_to(args.input)
                        add_path(bundle, source, PurePosixPath("payload") / PurePosixPath(relative.as_posix()))
                else:
                    add_path(bundle, args.input, PurePosixPath("payload") / args.input.name)

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    checksum = archive.with_suffix(archive.suffix + ".sha256")
    checksum.write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    os.chmod(archive, stat.S_IRUSR | stat.S_IWUSR | stat.S_IRGRP | stat.S_IROTH)
    os.chmod(checksum, stat.S_IRUSR | stat.S_IWUSR | stat.S_IRGRP | stat.S_IROTH)
    print(archive)


if __name__ == "__main__":
    main()

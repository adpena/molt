#!/usr/bin/env python3
"""Fetch one release tool from the checked-in digest and size authority."""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import tempfile
import tomllib
import urllib.request


ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "config" / "release_supply_chain.toml"
CHUNK_SIZE = 1024 * 1024


def _entry(tool: str, target: str) -> dict[str, object]:
    with MANIFEST.open("rb") as handle:
        document = tomllib.load(handle)
    if document.get("schema") != "molt.release-supply-chain.v1":
        raise ValueError("release supply-chain manifest schema is not supported")
    try:
        raw = document["downloads"][tool]["targets"][target]
    except (KeyError, TypeError) as exc:
        raise ValueError(f"no pinned release tool for {tool!r} on {target!r}") from exc
    if not isinstance(raw, dict):
        raise ValueError(f"invalid pinned release tool entry for {tool!r}/{target!r}")
    return raw


def fetch(tool: str, target: str, output: Path) -> None:
    raw = _entry(tool, target)
    url = str(raw.get("url", ""))
    sha256 = str(raw.get("sha256", ""))
    size = raw.get("size")
    if not url.startswith("https://github.com/"):
        raise ValueError(f"pinned release tool URL must use GitHub HTTPS: {url!r}")
    if len(sha256) != 64 or any(ch not in "0123456789abcdef" for ch in sha256):
        raise ValueError(f"pinned release tool digest is invalid: {sha256!r}")
    if not isinstance(size, int) or size <= 0:
        raise ValueError(f"pinned release tool size is invalid: {size!r}")

    output = output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary_name = tempfile.mkstemp(prefix=f".{output.name}.", dir=output.parent)
    temporary = Path(temporary_name)
    digest = hashlib.sha256()
    received = 0
    request = urllib.request.Request(url, headers={"User-Agent": "molt-release/1"})
    try:
        with os.fdopen(fd, "wb") as target_handle:
            with urllib.request.urlopen(request, timeout=120) as response:  # noqa: S310
                while chunk := response.read(CHUNK_SIZE):
                    target_handle.write(chunk)
                    digest.update(chunk)
                    received += len(chunk)
            target_handle.flush()
            os.fsync(target_handle.fileno())
        if received != size:
            raise ValueError(
                f"pinned release tool size mismatch: expected {size}, got {received}"
            )
        actual_sha256 = digest.hexdigest()
        if actual_sha256 != sha256:
            raise ValueError(
                "pinned release tool digest mismatch: "
                f"expected {sha256}, got {actual_sha256}"
            )
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tool", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    fetch(args.tool, args.target, args.output)


if __name__ == "__main__":
    main()

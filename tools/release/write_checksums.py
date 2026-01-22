#!/usr/bin/env python3
"""Generate SHA256 checksums for release artifacts."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("artifacts", nargs="+")
    args = parser.parse_args()

    lines: list[str] = []
    for item in args.artifacts:
        path = Path(item)
        digest = _sha256(path)
        lines.append(f"{digest}  {path.name}")
    Path(args.out).write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()

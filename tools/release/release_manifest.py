#!/usr/bin/env python3
"""Generate a release manifest JSON for Molt artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[2]
CONFIG_PATH = ROOT / "packaging" / "config.toml"

FILENAME_RE = re.compile(
    r"^(?P<name>molt|molt-worker)-(?P<version>\d+\.\d+\.\d+)-(?P<platform>macos|linux|windows)-(?P<arch>[A-Za-z0-9_\-]+)\.(?P<ext>tar\.gz|zip)$"
)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _load_config() -> dict:
    if not CONFIG_PATH.exists():
        return {}
    return tomllib.loads(CONFIG_PATH.read_text())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("artifacts", nargs="+")
    args = parser.parse_args()

    config = _load_config()
    owner = config.get("github", {}).get("owner", "adpena")
    repo = config.get("github", {}).get("repo", "molt")

    artifacts = []
    for item in args.artifacts:
        path = Path(item)
        match = FILENAME_RE.match(path.name)
        if not match:
            raise SystemExit(f"Unrecognized artifact name: {path.name}")
        info = match.groupdict()
        arch = info["arch"]
        libc = None
        if info["platform"] == "linux" and "-" in arch:
            arch, libc = arch.split("-", 1)
        sha = _sha256(path)
        url = f"https://github.com/{owner}/{repo}/releases/download/v{args.version}/{path.name}"
        artifacts.append(
            {
                "name": info["name"],
                "version": args.version,
                "platform": info["platform"],
                "arch": arch,
                "libc": libc,
                "filename": path.name,
                "sha256": sha,
                "url": url,
            }
        )

    payload = {
        "version": args.version,
        "repo": f"{owner}/{repo}",
        "artifacts": artifacts,
    }

    Path(args.out).write_text(json.dumps(payload, indent=2) + "\n")


if __name__ == "__main__":
    main()

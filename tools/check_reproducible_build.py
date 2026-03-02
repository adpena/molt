#!/usr/bin/env python3
"""Verify that two independent builds produce bit-identical artifacts.

Usage:
    python tools/check_reproducible_build.py build1.json build2.json

Each JSON file should be the output of `molt.cli build --json`, containing
an "output" or "artifact" key with the path to the compiled binary.

Exit codes:
    0 — builds are reproducible (SHA256 match)
    1 — builds differ (SHA256 mismatch)
    2 — usage error
"""

import hashlib
import json
import sys
from pathlib import Path


def sha256_file(path: str) -> str:
    """Compute SHA256 hex digest of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def extract_artifact_path(build_json: dict, prefer_object: bool = False) -> str:
    """Extract the artifact path from build JSON output.

    When *prefer_object* is True, prefer the ``.o`` file over the linked binary
    because the linker (especially on macOS) injects nondeterministic UUIDs.
    """
    data = build_json
    # Unwrap "data" envelope (molt.cli build --json wraps output in data)
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]

    # If prefer_object, try to find the object file first
    if prefer_object:
        artifacts = data.get("artifacts", {})
        if isinstance(artifacts, dict) and "object" in artifacts:
            return artifacts["object"]

    # Try standard keys
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    # Try nested under "build"
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    raise KeyError(
        f"Cannot find artifact path in build JSON. Available keys: {list(data.keys())}"
    )


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2

    path1, path2 = sys.argv[1], sys.argv[2]
    # --object flag: compare .o files instead of linked binaries to avoid
    # linker-injected nondeterminism (macOS ld injects random UUIDs)
    prefer_object = "--object" in sys.argv

    try:
        with open(path1) as f:
            build1 = json.load(f)
        with open(path2) as f:
            build2 = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError) as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2

    try:
        artifact1 = extract_artifact_path(build1, prefer_object=prefer_object)
        artifact2 = extract_artifact_path(build2, prefer_object=prefer_object)
    except KeyError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2

    if not Path(artifact1).exists():
        print(f"ERROR: Artifact not found: {artifact1}", file=sys.stderr)
        return 2
    if not Path(artifact2).exists():
        print(f"ERROR: Artifact not found: {artifact2}", file=sys.stderr)
        return 2

    hash1 = sha256_file(artifact1)
    hash2 = sha256_file(artifact2)

    print(f"Build 1: {artifact1}")
    print(f"  SHA256: {hash1}")
    print(f"Build 2: {artifact2}")
    print(f"  SHA256: {hash2}")

    if hash1 == hash2:
        print("\nREPRODUCIBLE: Artifacts are bit-identical.")
        return 0
    else:
        print("\nFAILED: Artifacts differ!")
        print(f"  Size 1: {Path(artifact1).stat().st_size} bytes")
        print(f"  Size 2: {Path(artifact2).stat().st_size} bytes")
        return 1


if __name__ == "__main__":
    sys.exit(main())

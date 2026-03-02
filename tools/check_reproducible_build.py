#!/usr/bin/env python3
"""Verify that two independent builds produce bit-identical artifacts.

Usage:
    python tools/check_reproducible_build.py [--object] build1.json build2.json

Each JSON file should be the output of `molt.cli build --json`, containing
an "output" or "artifact" key with the path to the compiled binary.

Flags:
    --object  Compare .o object files instead of linked binaries.
              Use this to avoid linker-injected nondeterminism (macOS LC_UUID).

Exit codes:
    0 — builds are reproducible (SHA256 match)
    1 — builds differ (SHA256 mismatch)
    2 — usage error
"""

import argparse
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

    # Check status field — bail early if build failed
    status = build_json.get("status") or data.get("status")
    if status and status != "ok":
        raise KeyError(f"Build reported non-ok status: {status}")

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
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("build1", help="First build JSON file")
    parser.add_argument("build2", help="Second build JSON file")
    parser.add_argument(
        "--object",
        action="store_true",
        help="Compare .o files instead of linked binaries (avoids linker UUID nondeterminism)",
    )
    args = parser.parse_args()

    for label, path in [("Build 1", args.build1), ("Build 2", args.build2)]:
        if not Path(path).exists():
            print(f"ERROR: {label} JSON file not found: {path}", file=sys.stderr)
            return 2

    try:
        with open(args.build1) as f:
            build1 = json.load(f)
        with open(args.build2) as f:
            build2 = json.load(f)
    except json.JSONDecodeError as e:
        print(f"ERROR: Invalid JSON: {e}", file=sys.stderr)
        return 2

    try:
        artifact1 = extract_artifact_path(build1, prefer_object=args.object)
        artifact2 = extract_artifact_path(build2, prefer_object=args.object)
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

    size1 = Path(artifact1).stat().st_size
    size2 = Path(artifact2).stat().st_size

    print(f"Build 1: {artifact1}")
    print(f"  SHA256: {hash1}  ({size1} bytes)")
    print(f"Build 2: {artifact2}")
    print(f"  SHA256: {hash2}  ({size2} bytes)")

    if hash1 == hash2:
        print("\nREPRODUCIBLE: Artifacts are bit-identical.")
        return 0
    else:
        print("\nFAILED: Artifacts differ!")
        if size1 != size2:
            print(
                f"  Size differs: {size1} vs {size2} bytes ({abs(size1 - size2)} byte delta)"
            )
        else:
            print(f"  Same size ({size1} bytes) but different content")
        return 1


if __name__ == "__main__":
    sys.exit(main())

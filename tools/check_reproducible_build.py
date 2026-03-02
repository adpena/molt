#!/usr/bin/env python3
"""Verify that two independent builds produce bit-identical artifacts.

Usage:
    python tools/check_reproducible_build.py [--object] build1.json build2.json
    python tools/check_reproducible_build.py --build source.py
    python tools/check_reproducible_build.py --batch examples/*.py

Each JSON file should be the output of `molt.cli build --json`, containing
an "output" or "artifact" key with the path to the compiled binary.

Modes:
    build1.json build2.json   Compare two pre-built JSON artifacts.
    --build source.py         Self-contained: build source.py twice in
                              isolated caches and compare the artifacts.
    --batch sources...        Build each source file twice and report results.

Flags:
    --object  Compare .o object files instead of linked binaries.
              Use this to avoid linker-injected nondeterminism (macOS LC_UUID).
    --json-out FILE  Write JSON results (for CI integration).

Exit codes:
    0 — builds are reproducible (SHA256 match)
    1 — builds differ (SHA256 mismatch)
    2 — usage error
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def sha256_file(path: str) -> str:
    """Compute SHA256 hex digest of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def find_first_diff(path1: str, path2: str) -> tuple[int, int, int] | None:
    """Find the byte offset of the first difference between two files.

    Returns (offset, byte1, byte2) or None if files are identical.
    """
    with open(path1, "rb") as f1, open(path2, "rb") as f2:
        offset = 0
        while True:
            chunk1 = f1.read(4096)
            chunk2 = f2.read(4096)
            if not chunk1 and not chunk2:
                return None
            if not chunk1 or not chunk2:
                return (
                    offset,
                    -1 if not chunk1 else chunk1[0],
                    -1 if not chunk2 else chunk2[0],
                )
            for i, (b1, b2) in enumerate(zip(chunk1, chunk2)):
                if b1 != b2:
                    return (offset + i, b1, b2)
            if len(chunk1) != len(chunk2):
                return (offset + min(len(chunk1), len(chunk2)), -1, -1)
            offset += len(chunk1)
    return None


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


def _build_once(
    source: str,
    cache_dir: str,
    profile: str,
    prefer_object: bool,
) -> tuple[str | None, str]:
    """Build a source file once, returning (artifact_path, error_msg)."""
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", "src")
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    env["MOLT_CACHE"] = cache_dir
    # Clear any cached state
    if "MOLT_BUILD_CACHE" in env:
        del env["MOLT_BUILD_CACHE"]

    emit_args = ["--emit", "obj"] if prefer_object else []
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--deterministic",
        "--json",
        *emit_args,
        source,
    ]
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        return None, "build timed out"

    if result.returncode != 0:
        return None, f"build failed (exit {result.returncode}): {result.stderr[:500]}"

    stdout = result.stdout.strip()
    json_str = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            json_str = line
            break

    if json_str is None:
        return None, f"no JSON in build output: {stdout[:300]}"

    try:
        build_info = json.loads(json_str)
    except json.JSONDecodeError as e:
        return None, f"invalid build JSON: {e}"

    try:
        artifact = extract_artifact_path(build_info, prefer_object=prefer_object)
    except KeyError as e:
        return None, str(e)

    if not Path(artifact).exists():
        return None, f"artifact not found: {artifact}"

    return artifact, ""


def compare_artifacts(
    artifact1: str,
    artifact2: str,
    label: str = "",
) -> tuple[bool, dict]:
    """Compare two artifact files. Returns (match, details_dict)."""
    hash1 = sha256_file(artifact1)
    hash2 = sha256_file(artifact2)
    size1 = Path(artifact1).stat().st_size
    size2 = Path(artifact2).stat().st_size

    details = {
        "artifact1": artifact1,
        "artifact2": artifact2,
        "sha256_1": hash1,
        "sha256_2": hash2,
        "size_1": size1,
        "size_2": size2,
        "match": hash1 == hash2,
    }

    if label:
        details["source"] = label

    if hash1 != hash2:
        diff = find_first_diff(artifact1, artifact2)
        if diff is not None:
            offset, b1, b2 = diff
            details["first_diff_offset"] = offset
            details["first_diff_byte1"] = b1
            details["first_diff_byte2"] = b2

    return hash1 == hash2, details


def _build_twice_and_compare(
    source: str,
    profile: str,
    prefer_object: bool,
    verbose: bool,
) -> tuple[bool, dict]:
    """Build a source file twice in isolated caches and compare."""
    with (
        tempfile.TemporaryDirectory(prefix="repro_a_") as cache_a,
        tempfile.TemporaryDirectory(prefix="repro_b_") as cache_b,
    ):
        art1, err1 = _build_once(source, cache_a, profile, prefer_object)
        if art1 is None:
            return False, {"source": source, "error": f"build A: {err1}"}

        art2, err2 = _build_once(source, cache_b, profile, prefer_object)
        if art2 is None:
            return False, {"source": source, "error": f"build B: {err2}"}

        match, details = compare_artifacts(art1, art2, label=source)

        if verbose:
            print(f"  Build A: {art1}")
            print(f"    SHA256: {details['sha256_1']}  ({details['size_1']} bytes)")
            print(f"  Build B: {art2}")
            print(f"    SHA256: {details['sha256_2']}  ({details['size_2']} bytes)")

        return match, details


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "build_jsons",
        nargs="*",
        metavar="BUILD_JSON",
        help="Build JSON files to compare (exactly 2 required unless --build/--batch used)",
    )
    parser.add_argument(
        "--object",
        action="store_true",
        help="Compare .o files instead of linked binaries (avoids linker UUID nondeterminism)",
    )
    parser.add_argument(
        "--build",
        metavar="SOURCE",
        help="Self-contained mode: build SOURCE twice in isolated caches and compare",
    )
    parser.add_argument(
        "--batch",
        nargs="+",
        metavar="SOURCE",
        help="Batch mode: build each source twice and report reproducibility for all",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile for --build/--batch modes (default: dev)",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
    )
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write JSON results to FILE (for CI integration)",
    )
    args = parser.parse_args()

    # Mode: --batch (multiple sources)
    if args.batch:
        results = []
        passed = 0
        failed = 0
        errors = 0

        for source in args.batch:
            if not Path(source).exists():
                print(f"  SKIP {source} (not found)")
                errors += 1
                results.append({"source": source, "error": "not found"})
                continue

            print(f"  Testing {source} ...")
            match, details = _build_twice_and_compare(
                source,
                args.build_profile,
                args.object,
                args.verbose,
            )
            results.append(details)

            if "error" in details:
                print(f"  ERROR {source}: {details['error']}")
                errors += 1
            elif match:
                print(f"  PASS  {source}")
                passed += 1
            else:
                offset = details.get("first_diff_offset", "?")
                print(f"  FAIL  {source}  (first diff at byte {offset})")
                failed += 1

        total = passed + failed + errors
        print(
            f"\nReproducible build sweep: {total} files | {passed} pass | {failed} fail | {errors} error"
        )

        if args.json_out:
            out = {
                "passed": passed,
                "failed": failed,
                "errors": errors,
                "results": results,
            }
            Path(args.json_out).parent.mkdir(parents=True, exist_ok=True)
            Path(args.json_out).write_text(json.dumps(out, indent=2) + "\n")

        return 1 if failed > 0 else 0

    # Mode: --build (single source, self-contained)
    if args.build:
        source = args.build
        if not Path(source).exists():
            print(f"ERROR: Source file not found: {source}", file=sys.stderr)
            return 2

        print(f"Reproducible build test: {source}")
        match, details = _build_twice_and_compare(
            source,
            args.build_profile,
            args.object,
            verbose=True,
        )

        if "error" in details:
            print(f"\nERROR: {details['error']}", file=sys.stderr)
            return 2

        if match:
            print("\nREPRODUCIBLE: Both builds are bit-identical.")
            return 0
        else:
            offset = details.get("first_diff_offset", "?")
            print(
                f"\nFAILED: Artifacts differ! First difference at byte offset {offset}."
            )
            return 1

    # Mode: compare two pre-built JSON files
    if len(args.build_jsons) != 2:
        parser.error("Exactly 2 build JSON files required (or use --build/--batch)")

    for label, path in [
        ("Build 1", args.build_jsons[0]),
        ("Build 2", args.build_jsons[1]),
    ]:
        if not Path(path).exists():
            print(f"ERROR: {label} JSON file not found: {path}", file=sys.stderr)
            return 2

    try:
        with open(args.build_jsons[0]) as f:
            build1 = json.load(f)
        with open(args.build_jsons[1]) as f:
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

    match, details = compare_artifacts(artifact1, artifact2)

    print(f"Build 1: {artifact1}")
    print(f"  SHA256: {details['sha256_1']}  ({details['size_1']} bytes)")
    print(f"Build 2: {artifact2}")
    print(f"  SHA256: {details['sha256_2']}  ({details['size_2']} bytes)")

    if match:
        print("\nREPRODUCIBLE: Artifacts are bit-identical.")
        return 0
    else:
        print("\nFAILED: Artifacts differ!")
        size1, size2 = details["size_1"], details["size_2"]
        if size1 != size2:
            print(
                f"  Size differs: {size1} vs {size2} bytes ({abs(size1 - size2)} byte delta)"
            )
        else:
            print(f"  Same size ({size1} bytes) but different content")
        if "first_diff_offset" in details:
            print(
                f"  First byte difference at offset {details['first_diff_offset']} "
                f"(0x{details['first_diff_offset']:x})"
            )
        return 1


if __name__ == "__main__":
    sys.exit(main())

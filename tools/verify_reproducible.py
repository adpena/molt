#!/usr/bin/env python3
"""Verify Molt produces bit-identical outputs across repeated compilations.

Compiles a set of test programs N times each and verifies SHA256 digests match.
This is a lightweight wrapper around check_reproducible_build.py for quick
determinism smoke tests.

Usage:
    uv run --python 3.12 python3 tools/verify_reproducible.py
    uv run --python 3.12 python3 tools/verify_reproducible.py --runs 5
    uv run --python 3.12 python3 tools/verify_reproducible.py --programs examples/hello.py tests/differential/basic/core_types/int_basic.py
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Default test programs to verify reproducibility.
DEFAULT_PROGRAMS = [
    "examples/hello.py",
]

# Additional programs to include if they exist.
OPTIONAL_PROGRAMS = [
    "tests/differential/basic/core_types/int_basic.py",
    "tests/differential/basic/core_types/float_basic.py",
    "tests/differential/basic/core_types/string_basic.py",
]

IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(t: str) -> str:
    return _c("32", t)


def red(t: str) -> str:
    return _c("31", t)


def bold(t: str) -> str:
    return _c("1", t)


def sha256_file(path: str | Path) -> str:
    """Compute SHA256 hex digest of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()


def build_once(
    source: str,
    cache_dir: str,
    profile: str,
    prefer_object: bool,
) -> tuple[str | None, str]:
    """Build a source file once, returning (artifact_path, error_msg)."""
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", str(ROOT / "src"))
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    env["MOLT_CACHE"] = cache_dir
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
        return None, "build timed out (>120s)"

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

    # Extract artifact path
    data = build_info
    if "data" in build_info and isinstance(build_info["data"], dict):
        data = build_info["data"]

    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            artifact = data[key]
            if Path(artifact).exists():
                return artifact, ""
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                artifact = data["build"][key]
                if Path(artifact).exists():
                    return artifact, ""

    return None, f"cannot find artifact in build JSON: {list(data.keys())}"


def verify_program(
    source: str,
    runs: int,
    profile: str,
    prefer_object: bool,
    verbose: bool,
) -> tuple[bool, dict]:
    """Build a program N times and verify all outputs are identical."""
    hashes: list[str] = []
    sizes: list[int] = []

    for i in range(runs):
        with tempfile.TemporaryDirectory(prefix=f"repro_{i}_") as cache_dir:
            artifact, err = build_once(source, cache_dir, profile, prefer_object)
            if artifact is None:
                return False, {"source": source, "error": f"run {i + 1}: {err}"}
            h = sha256_file(artifact)
            sz = Path(artifact).stat().st_size
            hashes.append(h)
            sizes.append(sz)
            if verbose:
                print(f"    Run {i + 1}: SHA256={h[:16]}... ({sz} bytes)")

    unique = set(hashes)
    identical = len(unique) == 1

    return identical, {
        "source": source,
        "runs": runs,
        "identical": identical,
        "hashes": hashes,
        "sizes": sizes,
        "unique_hashes": len(unique),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify Molt produces bit-identical outputs across repeated compilations.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--programs",
        nargs="+",
        metavar="FILE",
        help="Python source files to test (default: examples/hello.py + common test files)",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=3,
        help="Number of compilation runs per program (default: 3)",
    )
    parser.add_argument(
        "--profile",
        default="dev",
        help="Build profile (default: dev)",
    )
    parser.add_argument(
        "--object",
        action="store_true",
        help="Compare .o files instead of linked binaries (avoids linker UUID nondeterminism)",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show per-run hashes",
    )
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write JSON results to FILE",
    )
    args = parser.parse_args()

    # Resolve programs list
    if args.programs:
        programs = args.programs
    else:
        programs = []
        for p in DEFAULT_PROGRAMS:
            full = ROOT / p
            if full.exists():
                programs.append(str(full))
            else:
                print(f"  SKIP (not found): {p}")
        for p in OPTIONAL_PROGRAMS:
            full = ROOT / p
            if full.exists():
                programs.append(str(full))

    if not programs:
        print("ERROR: No test programs found.", file=sys.stderr)
        return 2

    print(
        bold(
            f"Reproducibility verification: {len(programs)} programs x {args.runs} runs"
        )
    )
    print()

    passed = 0
    failed = 0
    errors = 0
    all_results: list[dict] = []

    for source in programs:
        if not Path(source).exists():
            print(f"  SKIP {source} (not found)")
            errors += 1
            all_results.append({"source": source, "error": "not found"})
            continue

        rel = (
            Path(source).relative_to(ROOT)
            if Path(source).is_relative_to(ROOT)
            else Path(source)
        )
        print(f"  Testing {rel} ...")

        t0 = time.monotonic()
        identical, details = verify_program(
            source, args.runs, args.profile, args.object, args.verbose
        )
        elapsed = time.monotonic() - t0
        all_results.append(details)

        if "error" in details:
            print(f"    {red('ERROR')}: {details['error']}")
            errors += 1
        elif identical:
            print(
                f"    {green('PASS')}: {args.runs} runs, all identical ({elapsed:.1f}s)"
            )
            passed += 1
        else:
            print(
                f"    {red('FAIL')}: {args.runs} runs, "
                f"{details['unique_hashes']} distinct hashes ({elapsed:.1f}s)"
            )
            failed += 1

    print()
    print(bold("-" * 60))
    print(
        f"  Reproducibility: {len(programs)} programs | "
        f"{green(f'{passed} pass')} | "
        f"{red(f'{failed} fail') if failed else f'{failed} fail'} | "
        f"{errors} error"
    )
    print(bold("-" * 60))

    if args.json_out:
        out = {
            "passed": passed,
            "failed": failed,
            "errors": errors,
            "runs_per_program": args.runs,
            "results": all_results,
        }
        Path(args.json_out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.json_out).write_text(json.dumps(out, indent=2) + "\n")
        print(f"  Results written to {args.json_out}")

    if failed > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

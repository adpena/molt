#!/usr/bin/env python3
"""Translation validation: verify Molt-compiled binaries match CPython output.

This is a paranoid verification mode that catches silent miscompilations by
compiling a Python program with Molt, running both the compiled binary and the
original program under CPython, and comparing their stdout, stderr, and exit
codes.

Unlike the full differential test harness (tests/molt_diff.py), this is a
lightweight standalone tool with no external dependencies beyond the Python
standard library. It is intended for quick spot-checks, CI smoke tests, and
ad-hoc validation of individual files or small directories.

Usage:
    python tools/check_translation_validation.py [OPTIONS] source.py
    python tools/check_translation_validation.py --batch DIR [OPTIONS]

Examples:
    # Validate a single file
    python tools/check_translation_validation.py examples/hello.py

    # Validate all .py files in a directory
    python tools/check_translation_validation.py --batch tests/differential/basic/

    # With custom build profile and timeout
    python tools/check_translation_validation.py --build-profile release --timeout 60 examples/hello.py

Exit codes:
    0 -- all validated files match CPython
    1 -- at least one file produced mismatched output
    2 -- usage error, missing file, or build infrastructure failure
"""

import argparse
import difflib
import json
import os
import subprocess
import sys
import time
from pathlib import Path


def _repo_root() -> Path:
    """Return the repository root (parent of tools/)."""
    return Path(__file__).resolve().parents[1]


def _make_env() -> dict[str, str]:
    """Build a clean environment for both CPython and Molt runs.

    Sets PYTHONPATH to include the Molt source tree and pins
    PYTHONHASHSEED=0 for reproducible hash ordering.
    """
    env = os.environ.copy()
    existing = env.get("PYTHONPATH", "")
    src_dir = str(_repo_root() / "src")
    if existing:
        # Prepend src if not already present
        parts = existing.split(os.pathsep)
        if src_dir not in parts:
            env["PYTHONPATH"] = src_dir + os.pathsep + existing
    else:
        env["PYTHONPATH"] = src_dir
    env["PYTHONHASHSEED"] = "0"
    return env


def _extract_binary(build_json: dict) -> str | None:
    """Extract the binary path from Molt build JSON output.

    Handles the ``data`` envelope: ``d.get("data", d)`` then looks for
    the ``output`` key (or fallback keys used by various build versions).
    """
    data = build_json.get("data", build_json)
    if isinstance(data, dict):
        pass
    else:
        data = build_json
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    return None


def run_cpython(
    source: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[str, str, int]:
    """Run a Python file under CPython and return (stdout, stderr, exit_code)."""
    cmd = [sys.executable, source]
    if verbose:
        print(f"  CPython: {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return "", f"CPython timed out after {timeout}s", -1
    return result.stdout, result.stderr, result.returncode


def build_molt(
    source: str,
    profile: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[str | None, str]:
    """Compile a Python file with Molt and return (binary_path, error_message).

    Returns (path, "") on success or (None, reason) on failure.
    """
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--json",
        "--capabilities",
        "fs,env,time,random",
        source,
    ]
    if verbose:
        print(f"  Build: {' '.join(cmd)}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return None, f"Molt build timed out after {timeout}s"

    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        return None, f"Molt build failed (exit {result.returncode}): {detail[:500]}"

    # Parse JSON from stdout -- the build may emit non-JSON lines before the
    # JSON payload, so try each line from the end.
    stdout = result.stdout.strip()
    build_info = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            try:
                build_info = json.loads(line)
                break
            except json.JSONDecodeError:
                continue
    if build_info is None:
        # Try the entire stdout as a single JSON blob
        try:
            build_info = json.loads(stdout)
        except json.JSONDecodeError:
            return None, f"Build produced no valid JSON. stdout: {stdout[:500]}"

    binary = _extract_binary(build_info)
    if binary is None:
        return None, (
            f"Cannot find binary in build JSON. Keys: {list(build_info.keys())}"
        )
    if not Path(binary).exists():
        return None, f"Binary path does not exist: {binary}"
    return binary, ""


def run_molt(
    binary: str,
    timeout: float,
    env: dict[str, str],
    verbose: bool = False,
) -> tuple[str, str, int]:
    """Run a Molt-compiled binary and return (stdout, stderr, exit_code)."""
    cmd = [binary]
    if verbose:
        print(f"  Molt run: {binary}")
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
        )
    except subprocess.TimeoutExpired:
        return "", f"Molt binary timed out after {timeout}s", -1
    return result.stdout, result.stderr, result.returncode


def _unified_diff(label_a: str, label_b: str, text_a: str, text_b: str) -> str:
    """Return a unified diff between two strings, or empty if identical."""
    lines_a = text_a.splitlines(keepends=True)
    lines_b = text_b.splitlines(keepends=True)
    diff = difflib.unified_diff(lines_a, lines_b, fromfile=label_a, tofile=label_b)
    return "".join(diff)


class ValidationResult:
    """Result of validating a single file."""

    __slots__ = ("source", "status", "detail", "elapsed")

    PASS = "pass"
    FAIL = "fail"
    ERROR = "error"
    SKIP = "skip"

    def __init__(
        self,
        source: str,
        status: str,
        detail: str = "",
        elapsed: float = 0.0,
    ):
        self.source = source
        self.status = status
        self.detail = detail
        self.elapsed = elapsed


def validate_file(
    source: str,
    profile: str,
    timeout: float,
    verbose: bool = False,
) -> ValidationResult:
    """Validate a single Python file: compile with Molt, run both, compare.

    Returns a ValidationResult with status pass/fail/error.
    """
    t0 = time.monotonic()
    env = _make_env()

    # Step 1: Run under CPython
    cpython_stdout, cpython_stderr, cpython_rc = run_cpython(
        source, timeout, env, verbose
    )
    if cpython_rc == -1:
        return ValidationResult(
            source,
            ValidationResult.ERROR,
            f"CPython timed out after {timeout}s",
            time.monotonic() - t0,
        )

    # Step 2: Build with Molt
    binary, build_error = build_molt(source, profile, timeout, env, verbose)
    if binary is None:
        return ValidationResult(
            source,
            ValidationResult.ERROR,
            f"Build error: {build_error}",
            time.monotonic() - t0,
        )

    # Step 3: Run Molt binary
    molt_stdout, molt_stderr, molt_rc = run_molt(binary, timeout, env, verbose)
    if molt_rc == -1 and "timed out" in molt_stderr:
        return ValidationResult(
            source,
            ValidationResult.ERROR,
            f"Molt binary timed out after {timeout}s",
            time.monotonic() - t0,
        )

    # Step 4: Compare
    mismatches: list[str] = []

    if cpython_stdout != molt_stdout:
        diff = _unified_diff(
            "cpython/stdout", "molt/stdout", cpython_stdout, molt_stdout
        )
        mismatches.append(f"STDOUT MISMATCH:\n{diff}")

    if cpython_stderr != molt_stderr:
        diff = _unified_diff(
            "cpython/stderr", "molt/stderr", cpython_stderr, molt_stderr
        )
        mismatches.append(f"STDERR MISMATCH:\n{diff}")

    if cpython_rc != molt_rc:
        mismatches.append(f"EXIT CODE MISMATCH: cpython={cpython_rc}, molt={molt_rc}")

    elapsed = time.monotonic() - t0

    if mismatches:
        detail = "\n".join(mismatches)
        return ValidationResult(source, ValidationResult.FAIL, detail, elapsed)

    return ValidationResult(source, ValidationResult.PASS, "", elapsed)


def collect_sources(batch_dir: str) -> list[str]:
    """Collect all .py files in a directory, sorted for deterministic ordering."""
    root = Path(batch_dir)
    if not root.is_dir():
        return []
    sources = sorted(str(p) for p in root.rglob("*.py") if p.is_file())
    return sources


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "source",
        nargs="?",
        help="Python source file to validate (required unless --batch is used)",
    )
    parser.add_argument(
        "--batch",
        metavar="DIR",
        help="Validate all .py files in DIR (recursively)",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile (default: dev)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=30.0,
        help="Timeout in seconds for each CPython/Molt run (default: 30)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print commands and intermediate details",
    )
    args = parser.parse_args()

    # Determine which files to validate
    if args.batch and args.source:
        print(
            "ERROR: Provide either a source file or --batch DIR, not both.",
            file=sys.stderr,
        )
        return 2
    if args.batch:
        batch_dir = args.batch
        if not Path(batch_dir).is_dir():
            print(
                f"ERROR: Batch directory not found: {batch_dir}",
                file=sys.stderr,
            )
            return 2
        sources = collect_sources(batch_dir)
        if not sources:
            print(
                f"ERROR: No .py files found in {batch_dir}",
                file=sys.stderr,
            )
            return 2
    elif args.source:
        if not Path(args.source).exists():
            print(
                f"ERROR: Source file not found: {args.source}",
                file=sys.stderr,
            )
            return 2
        sources = [args.source]
    else:
        print(
            "ERROR: Provide a source file or --batch DIR.",
            file=sys.stderr,
        )
        parser.print_usage(sys.stderr)
        return 2

    # Run validation
    results: list[ValidationResult] = []
    total = len(sources)

    print(
        f"Translation validation: {total} file(s), profile={args.build_profile}, timeout={args.timeout}s"
    )
    print()

    for i, source in enumerate(sources, 1):
        label = Path(source).name
        if args.verbose:
            print(f"[{i}/{total}] {source}")
        else:
            print(f"[{i}/{total}] {label} ... ", end="", flush=True)

        result = validate_file(
            source,
            profile=args.build_profile,
            timeout=args.timeout,
            verbose=args.verbose,
        )
        results.append(result)

        if not args.verbose:
            if result.status == ValidationResult.PASS:
                print(f"PASS ({result.elapsed:.1f}s)")
            elif result.status == ValidationResult.FAIL:
                print(f"FAIL ({result.elapsed:.1f}s)")
            elif result.status == ValidationResult.ERROR:
                print(f"ERROR ({result.elapsed:.1f}s)")
            else:
                print(f"SKIP ({result.elapsed:.1f}s)")

        # Show detail for failures and errors in both modes
        if result.status == ValidationResult.FAIL:
            for line in result.detail.splitlines():
                print(f"    {line}")
        elif result.status == ValidationResult.ERROR and args.verbose:
            print(f"    {result.detail[:300]}")

    # Summary
    n_pass = sum(1 for r in results if r.status == ValidationResult.PASS)
    n_fail = sum(1 for r in results if r.status == ValidationResult.FAIL)
    n_error = sum(1 for r in results if r.status == ValidationResult.ERROR)
    n_skip = sum(1 for r in results if r.status == ValidationResult.SKIP)
    total_time = sum(r.elapsed for r in results)

    print()
    print(
        f"Results: {n_pass} pass, {n_fail} fail, {n_error} error, {n_skip} skip  ({total_time:.1f}s)"
    )

    if n_fail > 0:
        print()
        print("Failed files:")
        for r in results:
            if r.status == ValidationResult.FAIL:
                print(f"  {r.source}")
        return 1

    if n_error > 0 and n_pass == 0:
        # All files errored (build infra issue) -- exit 2
        return 2

    return 0


if __name__ == "__main__":
    sys.exit(main())

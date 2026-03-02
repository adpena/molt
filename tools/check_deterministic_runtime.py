#!/usr/bin/env python3
"""Verify that a Molt-compiled binary produces deterministic output.

Builds a test program, runs it N times, and asserts all outputs are identical.

Usage:
    python tools/check_deterministic_runtime.py [--runs N] [--build-profile PROFILE] <source.py>

Exit codes:
    0 — all runs produced identical output
    1 — outputs differ across runs
    2 — build or execution error
"""

import argparse
import hashlib
import os
import subprocess
import sys
from pathlib import Path


def build_program(source: str, profile: str = "dev") -> str:
    """Build a Molt program and return the output binary path."""
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"

    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--deterministic",
        "--json",
        source,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, env=env)
    if result.returncode != 0:
        print(f"Build failed:\n{result.stderr}", file=sys.stderr)
        sys.exit(2)

    import json

    build_info = json.loads(result.stdout)

    # Extract artifact path
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in build_info:
            return build_info[key]
    if "build" in build_info:
        for key in ("output", "artifact", "binary", "path"):
            if key in build_info["build"]:
                return build_info["build"][key]

    print(
        f"Cannot find artifact in build output: {list(build_info.keys())}",
        file=sys.stderr,
    )
    sys.exit(2)


def run_binary(binary: str, run_index: int) -> tuple[str, str]:
    """Run a binary and return (stdout, stderr)."""
    env = os.environ.copy()
    env["MOLT_DETERMINISTIC"] = "1"
    env["PYTHONHASHSEED"] = "0"

    result = subprocess.run(
        [binary],
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    )
    if result.returncode != 0:
        print(
            f"Run {run_index} failed (exit {result.returncode}):\n{result.stderr}",
            file=sys.stderr,
        )
        sys.exit(2)

    return result.stdout, result.stderr


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("source", help="Python source file to test")
    parser.add_argument(
        "--runs", type=int, default=3, help="Number of runs to compare (default: 3)"
    )
    parser.add_argument(
        "--build-profile", default="dev", help="Molt build profile (default: dev)"
    )
    args = parser.parse_args()

    if not Path(args.source).exists():
        print(f"ERROR: Source file not found: {args.source}", file=sys.stderr)
        return 2

    print(f"Building {args.source} with --profile {args.build_profile} --deterministic")
    binary = build_program(args.source, args.build_profile)
    print(f"Binary: {binary}")

    outputs: list[tuple[str, str]] = []
    for i in range(args.runs):
        stdout, stderr = run_binary(binary, i + 1)
        outputs.append((stdout, stderr))
        h = hashlib.sha256(stdout.encode()).hexdigest()[:16]
        print(f"  Run {i + 1}: stdout hash={h} ({len(stdout)} chars)")

    # Compare all outputs against the first
    reference_stdout, reference_stderr = outputs[0]
    all_match = True

    for i, (stdout, stderr) in enumerate(outputs[1:], 2):
        if stdout != reference_stdout:
            print(f"\nFAILED: Run {i} stdout differs from run 1")
            # Show first difference
            lines_ref = reference_stdout.splitlines()
            lines_cur = stdout.splitlines()
            for j, (lr, lc) in enumerate(zip(lines_ref, lines_cur)):
                if lr != lc:
                    print(f"  First diff at line {j + 1}:")
                    print(f"    Run 1: {lr[:120]}")
                    print(f"    Run {i}: {lc[:120]}")
                    break
            all_match = False

        if stderr != reference_stderr:
            print(f"\nWARNING: Run {i} stderr differs from run 1 (non-fatal)")

    if all_match:
        print(f"\nDETERMINISTIC: All {args.runs} runs produced identical stdout.")
        return 0
    else:
        print(f"\nFAILED: Nondeterministic output detected across {args.runs} runs.")
        return 1


if __name__ == "__main__":
    sys.exit(main())

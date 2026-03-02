#!/usr/bin/env python3
"""Run the IR structure verifier across a suite of Python source files.

Compiles each file to TIR JSON via the frontend, then pipes through
check_ir_structure.py to validate well-formedness.

Usage:
    python tools/verify_ir_suite.py [--dir DIR] [--glob PATTERN] [--fail-fast] [--quiet]

Exit codes:
    0 — all files pass verification
    1 — one or more files have IR errors
    2 — usage error
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path


def compile_to_tir_json(source_path: Path) -> dict | None:
    """Compile a Python file to TIR JSON via the frontend."""
    cmd = [
        sys.executable,
        "-c",
        f"from molt.frontend import compile_to_tir; "
        f"import json, sys; "
        f"tir = compile_to_tir(open({str(source_path)!r}).read()); "
        f"json.dump(tir, sys.stdout)",
    ]
    env = {"PYTHONPATH": "src", "PATH": "/usr/bin:/bin:/usr/local/bin"}
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, env=env, timeout=60
        )
    except subprocess.TimeoutExpired:
        return None
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


def verify_tir(tir_json: dict) -> tuple[int, str]:
    """Run check_ir_structure on TIR JSON. Returns (exit_code, output)."""
    cmd = [sys.executable, "tools/check_ir_structure.py", "--stdin", "--quiet"]
    result = subprocess.run(
        cmd,
        input=json.dumps(tir_json),
        capture_output=True,
        text=True,
        timeout=30,
    )
    return result.returncode, result.stdout + result.stderr


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--dir",
        default="tests/differential/basic",
        help="Directory to scan for .py files (default: tests/differential/basic)",
    )
    parser.add_argument(
        "--glob",
        default="**/*.py",
        help="Glob pattern within --dir (default: **/*.py)",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop on first verification failure",
    )
    parser.add_argument(
        "--quiet", "-q", action="store_true", help="Only print failures"
    )
    parser.add_argument(
        "--examples",
        action="store_true",
        help="Also verify examples/*.py",
    )
    args = parser.parse_args()

    base = Path(args.dir)
    if not base.exists():
        print(f"ERROR: Directory not found: {base}", file=sys.stderr)
        return 2

    files = sorted(base.glob(args.glob))
    if args.examples:
        examples = Path("examples")
        if examples.exists():
            files.extend(sorted(examples.glob("*.py")))

    if not files:
        print(f"No .py files found in {base} with pattern {args.glob}")
        return 2

    total = 0
    passed = 0
    failed = 0
    errors = 0
    failure_details: list[tuple[str, str]] = []

    for f in files:
        total += 1
        tir = compile_to_tir_json(f)
        if tir is None:
            errors += 1
            if not args.quiet:
                print(f"  SKIP {f} (compile error)")
            continue

        exit_code, output = verify_tir(tir)
        if exit_code == 0:
            passed += 1
            if not args.quiet:
                print(f"  PASS {f}")
        else:
            failed += 1
            failure_details.append((str(f), output.strip()))
            print(f"  FAIL {f}")
            if output.strip():
                for line in output.strip().splitlines()[:5]:
                    print(f"       {line}")
            if args.fail_fast:
                break

    print(
        f"\nIR verification suite: {total} files | {passed} pass | {failed} fail | {errors} skip"
    )
    if failure_details:
        print("\nFailed files:")
        for path, detail in failure_details:
            print(f"  {path}")

    return 1 if failed > 0 else 0


if __name__ == "__main__":
    sys.exit(main())

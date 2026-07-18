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
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402
from tools.proof_counts import fail_closed_proof_exit_code  # noqa: E402


def compile_to_tir_json(source_path: Path) -> tuple[dict | None, dict[str, object]]:
    """Compile a Python file to TIR JSON via the frontend."""
    cmd = [
        sys.executable,
        "-c",
        f"from molt.frontend import compile_to_tir; "
        f"import json, sys; "
        f"tir = compile_to_tir(open({str(source_path)!r}, encoding='utf-8').read()); "
        f"json.dump(tir, sys.stdout)",
    ]
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)
    try:
        result = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            env=env,
            cwd=ROOT,
            timeout=60,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return None, {"status": "timeout", "returncode": None, "stderr": ""}
    if result.returncode != 0:
        return None, {
            "status": "error",
            "returncode": result.returncode,
            "stderr": result.stderr,
        }
    try:
        return json.loads(result.stdout), {
            "status": "pass",
            "returncode": result.returncode,
            "stderr": result.stderr,
        }
    except json.JSONDecodeError as exc:
        return None, {
            "status": "error",
            "returncode": result.returncode,
            "stderr": result.stderr,
            "error": f"invalid TIR JSON: {exc}",
        }


def verify_tir(tir_json: dict) -> tuple[int, str]:
    """Run check_ir_structure on TIR JSON. Returns (exit_code, output)."""
    cmd = [
        sys.executable,
        str(ROOT / "tools" / "check_ir_structure.py"),
        "--stdin",
        "--quiet",
    ]
    env = os.environ.copy()
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)
    result = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_TEST_SUITE",
        input=json.dumps(tir_json),
        capture_output=True,
        text=True,
        env=env,
        cwd=ROOT,
        timeout=30,
        limits=limits,
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
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write the complete fail-closed sweep result to FILE",
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

    selected = len(files)
    attempted = 0
    passed = 0
    failed = 0
    errors = 0
    failure_details: list[tuple[str, str]] = []
    results: list[dict[str, object]] = []

    for f in files:
        attempted += 1
        tir, compile_observable = compile_to_tir_json(f)
        if tir is None:
            errors += 1
            results.append(
                {
                    "source": str(f),
                    "status": "error",
                    "error": compile_observable.get("error", "compile failed"),
                    "compile": compile_observable,
                }
            )
            if not args.quiet:
                print(f"  ERROR {f} (compile failed)")
            if args.fail_fast:
                break
            continue

        exit_code, output = verify_tir(tir)
        if exit_code == 0:
            passed += 1
            results.append(
                {
                    "source": str(f),
                    "status": "pass",
                    "compile": compile_observable,
                }
            )
            if not args.quiet:
                print(f"  PASS {f}")
        else:
            failed += 1
            results.append(
                {
                    "source": str(f),
                    "status": "fail",
                    "returncode": exit_code,
                    "detail": output.strip(),
                }
            )
            failure_details.append((str(f), output.strip()))
            print(f"  FAIL {f}")
            if output.strip():
                for line in output.strip().splitlines()[:5]:
                    print(f"       {line}")
            if args.fail_fast:
                break

    print(
        f"\nIR verification suite: {selected} selected | {attempted} attempted | "
        f"{passed} pass | {failed} fail | {errors} error"
    )
    if failure_details:
        print("\nFailed files:")
        for path, detail in failure_details:
            print(f"  {path}")

    if args.json_out:
        out = {
            "schema": "molt.ir-verification-sweep.v1",
            "status": (
                "success"
                if passed > 0 and failed == 0 and errors == 0
                else "failure"
            ),
            "selected": selected,
            "attempted": attempted,
            "unexecuted": selected - attempted,
            "executed": passed + failed,
            "passed": passed,
            "failed": failed,
            "errors": errors,
            "results": results,
        }
        output_path = Path(args.json_out)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")

    return fail_closed_proof_exit_code(
        executed=passed + failed,
        failed=failed,
        errors=errors,
    )


if __name__ == "__main__":
    sys.exit(main())

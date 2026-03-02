#!/usr/bin/env python3
"""Verify that a Molt-compiled binary produces deterministic output.

Builds a test program, runs it N times, and asserts all outputs are identical.

Usage:
    python tools/check_deterministic_runtime.py [--runs N] [--build-profile PROFILE] <source.py>
    python tools/check_deterministic_runtime.py --batch examples/*.py --runs 5

Exit codes:
    0 — all runs produced identical output
    1 — outputs differ across runs
    2 — build or execution error
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path


def _extract_binary(build_json: dict) -> str | None:
    """Extract the binary path from build JSON, unwrapping data envelope."""
    data = build_json
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return data[key]
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return data["build"][key]
    return None


def build_program(source: str, profile: str = "dev") -> tuple[str | None, str]:
    """Build a Molt program. Returns (binary_path, error_msg).

    Returns (None, error) on failure instead of sys.exit().
    """
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", "src")
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
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, env=env, timeout=120
        )
    except subprocess.TimeoutExpired:
        return None, "build timed out"

    if result.returncode != 0:
        return None, f"build failed (exit {result.returncode}): {result.stderr[:1000]}"

    stdout = result.stdout.strip()
    json_str = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            json_str = line
            break

    if json_str is None:
        try:
            build_info = json.loads(stdout)
        except json.JSONDecodeError as e:
            return None, f"invalid build JSON: {e}"
    else:
        try:
            build_info = json.loads(json_str)
        except json.JSONDecodeError as e:
            return None, f"invalid build JSON: {e}"

    binary = _extract_binary(build_info)
    if binary is None:
        return None, f"no binary in build output (keys: {list(build_info.keys())})"
    if not Path(binary).exists():
        return None, f"binary not found: {binary}"

    return binary, ""


def run_binary(
    binary: str, run_index: int, timeout: int = 60
) -> tuple[str, str, int | None]:
    """Run a binary. Returns (stdout, stderr, returncode). returncode=None on timeout."""
    env = os.environ.copy()
    env["MOLT_DETERMINISTIC"] = "1"
    env["PYTHONHASHSEED"] = "0"

    try:
        result = subprocess.run(
            [binary],
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return "", "", None

    return result.stdout, result.stderr, result.returncode


def check_determinism(
    source: str,
    runs: int,
    profile: str,
    timeout: int = 60,
    verbose: bool = False,
) -> dict:
    """Check determinism for a single source file. Returns result dict."""
    result = {
        "source": source,
        "runs": runs,
        "deterministic": False,
        "status": "unknown",
    }

    if not Path(source).exists():
        result["status"] = "error"
        result["error"] = "source file not found"
        return result

    if verbose:
        print(f"Building {source} with --profile {profile} --deterministic")

    binary, err = build_program(source, profile)
    if binary is None:
        result["status"] = "build_error"
        result["error"] = err
        if verbose:
            print(f"  BUILD ERROR: {err[:200]}", file=sys.stderr)
        return result

    result["binary"] = binary
    result["binary_hash"] = hashlib.sha256(Path(binary).read_bytes()).hexdigest()

    if verbose:
        print(f"Binary: {binary}  (SHA256: {result['binary_hash'][:16]}...)")

    outputs: list[tuple[str, str, int | None]] = []
    for i in range(runs):
        stdout, stderr, rc = run_binary(binary, i + 1, timeout)
        outputs.append((stdout, stderr, rc))
        h = hashlib.sha256(stdout.encode()).hexdigest()[:16]
        if verbose:
            print(f"  Run {i + 1}: stdout hash={h} ({len(stdout)} chars) rc={rc}")

        if rc is None:
            result["status"] = "timeout"
            result["error"] = f"run {i + 1} timed out"
            return result

        if rc != 0:
            result["status"] = "run_error"
            result["error"] = f"run {i + 1} exited with rc={rc}: {stderr[:500]}"
            return result

    ref_stdout, ref_stderr, _ = outputs[0]
    all_match = True
    diff_details = []

    for i, (stdout, stderr, _) in enumerate(outputs[1:], 2):
        if stdout != ref_stdout:
            all_match = False
            lines_ref = ref_stdout.splitlines()
            lines_cur = stdout.splitlines()
            first_diff_line = None
            for j, (lr, lc) in enumerate(zip(lines_ref, lines_cur)):
                if lr != lc:
                    first_diff_line = j + 1
                    break
            if first_diff_line is None and len(lines_ref) != len(lines_cur):
                first_diff_line = min(len(lines_ref), len(lines_cur)) + 1
            diff_details.append(
                {
                    "run": i,
                    "first_diff_line": first_diff_line,
                    "ref_line": lines_ref[first_diff_line - 1][:200]
                    if first_diff_line and first_diff_line <= len(lines_ref)
                    else None,
                    "cur_line": lines_cur[first_diff_line - 1][:200]
                    if first_diff_line and first_diff_line <= len(lines_cur)
                    else None,
                }
            )

        if stderr != ref_stderr and verbose:
            print(f"\n  WARNING: Run {i} stderr differs from run 1 (non-fatal)")

    result["deterministic"] = all_match
    result["status"] = "pass" if all_match else "fail"
    result["stdout_hash"] = hashlib.sha256(ref_stdout.encode()).hexdigest()
    if diff_details:
        result["diffs"] = diff_details

    return result


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "source",
        nargs="?",
        help="Python source file to test (use --batch for multiple files)",
    )
    parser.add_argument(
        "--batch",
        nargs="+",
        metavar="SOURCE",
        help="Test multiple source files for determinism",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=3,
        help="Number of runs to compare (default: 3)",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile (default: dev)",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=60,
        help="Timeout in seconds per run (default: 60)",
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

    # Batch mode
    if args.batch:
        t0 = time.monotonic()
        results = []
        passed = 0
        failed = 0
        errors = 0

        for source in args.batch:
            r = check_determinism(
                source,
                args.runs,
                args.build_profile,
                args.timeout,
                args.verbose,
            )
            results.append(r)

            if r["status"] == "pass":
                passed += 1
                print(f"  PASS  {source}  (stdout hash: {r['stdout_hash'][:16]})")
            elif r["status"] == "fail":
                failed += 1
                print(f"  FAIL  {source}")
                for d in r.get("diffs", []):
                    print(
                        f"        run {d['run']} differs at line {d['first_diff_line']}"
                    )
            else:
                errors += 1
                print(f"  ERROR {source}: {r.get('error', 'unknown')[:100]}")

        elapsed = time.monotonic() - t0
        total = passed + failed + errors
        print(
            f"\nDeterminism sweep: {total} files | {passed} pass | {failed} fail | {errors} error  ({elapsed:.1f}s)"
        )

        if args.json_out:
            out = {
                "passed": passed,
                "failed": failed,
                "errors": errors,
                "elapsed_s": round(elapsed, 1),
                "results": results,
            }
            Path(args.json_out).parent.mkdir(parents=True, exist_ok=True)
            Path(args.json_out).write_text(json.dumps(out, indent=2) + "\n")
            print(f"JSON report written to {args.json_out}")

        return 1 if failed > 0 else 0

    # Single source mode
    if not args.source:
        parser.error("Either provide a source file or use --batch")

    r = check_determinism(
        args.source,
        args.runs,
        args.build_profile,
        args.timeout,
        verbose=True,
    )

    if r["status"] == "pass":
        print(f"\nDETERMINISTIC: All {args.runs} runs produced identical stdout.")
        print(f"  stdout hash: {r['stdout_hash']}")
        print(f"  binary hash: {r['binary_hash']}")
        return 0
    elif r["status"] == "fail":
        print(f"\nFAILED: Nondeterministic output detected across {args.runs} runs.")
        for d in r.get("diffs", []):
            print(f"  Run {d['run']} first diff at line {d['first_diff_line']}:")
            if d.get("ref_line"):
                print(f"    Run 1: {d['ref_line']}")
            if d.get("cur_line"):
                print(f"    Run {d['run']}: {d['cur_line']}")
        return 1
    else:
        print(f"\nERROR: {r.get('error', 'unknown')}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())

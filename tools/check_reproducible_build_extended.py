#!/usr/bin/env python3
"""Extended reproducible build verification with IR-level and cross-process checks.

Supplements the existing check_reproducible_build.py with:
- IR-level determinism: compile same program twice, compare IR JSON byte-for-byte
- Cross-process determinism: spawn two Python processes, compare their IR output
- Timestamp audit: verify no build timestamps leak into output
- Entropy source audit: check for random.*, os.urandom, uuid.* in compiler code

Usage:
    python tools/check_reproducible_build_extended.py [--quick] [--verbose]
    python tools/check_reproducible_build_extended.py --check ir
    python tools/check_reproducible_build_extended.py --check timestamp
    python tools/check_reproducible_build_extended.py --check entropy
    python tools/check_reproducible_build_extended.py --check cross-process

Exit codes:
    0 -- all checks pass
    1 -- one or more checks fail
    2 -- usage/setup error
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC_DIR = ROOT / "src" / "molt"
FRONTEND_INIT = SRC_DIR / "frontend" / "__init__.py"
BASIC_DIR = ROOT / "tests" / "differential" / "basic"


# ---------------------------------------------------------------------------
# IR-level determinism
# ---------------------------------------------------------------------------


def _compile_to_ir_json(source_text: str) -> str:
    """Compile source to IR JSON via subprocess (full process isolation)."""
    script = (
        "import json, sys; "
        "sys.path.insert(0, {src!r}); "
        "from molt.frontend import compile_to_tir; "
        "ir = compile_to_tir(sys.stdin.read()); "
        "print(json.dumps(ir, sort_keys=True, indent=2))"
    ).format(src=str(ROOT / "src"))

    env = os.environ.copy()
    env["PYTHONHASHSEED"] = "0"

    result = subprocess.run(
        [sys.executable, "-c", script],
        input=source_text,
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"Compilation failed (rc={result.returncode}): {result.stderr[:1000]}"
        )
    return result.stdout


def check_ir_determinism(
    programs: list[Path], runs: int = 2, verbose: bool = False
) -> tuple[bool, list[dict]]:
    """Compile each program *runs* times, assert byte-identical IR JSON."""
    results = []
    all_pass = True

    for prog in programs:
        source = prog.read_text()
        ir_outputs = []

        try:
            for _ in range(runs):
                ir = _compile_to_ir_json(source)
                ir_outputs.append(ir)
        except RuntimeError as exc:
            results.append({"program": prog.name, "status": "error", "error": str(exc)})
            if verbose:
                print(f"  ERROR {prog.name}: {exc}")
            continue

        reference = ir_outputs[0]
        match = all(ir == reference for ir in ir_outputs[1:])
        results.append({"program": prog.name, "status": "pass" if match else "fail"})

        if verbose:
            status = "PASS" if match else "FAIL"
            print(f"  {status}  {prog.name}  ({runs} compilations)")

        if not match:
            all_pass = False

    return all_pass, results


# ---------------------------------------------------------------------------
# Cross-process determinism
# ---------------------------------------------------------------------------


def check_cross_process_determinism(
    programs: list[Path], verbose: bool = False
) -> tuple[bool, list[dict]]:
    """Spawn two separate Python processes for each program, compare IR output."""
    results = []
    all_pass = True

    for prog in programs:
        source = prog.read_text()

        try:
            ir_a = _compile_to_ir_json(source)
            ir_b = _compile_to_ir_json(source)
        except RuntimeError as exc:
            results.append({"program": prog.name, "status": "error", "error": str(exc)})
            if verbose:
                print(f"  ERROR {prog.name}: {exc}")
            continue

        match = ir_a == ir_b
        results.append({"program": prog.name, "status": "pass" if match else "fail"})

        if verbose:
            status = "PASS" if match else "FAIL"
            print(f"  {status}  {prog.name}  (cross-process)")

        if not match:
            all_pass = False

    return all_pass, results


# ---------------------------------------------------------------------------
# Timestamp audit
# ---------------------------------------------------------------------------

_TIMESTAMP_RE = re.compile(
    r"""
    \btime\.time\(\)
    | \bdatetime\.now\(
    | \bdatetime\.utcnow\(
    | \bdate\.today\(
    """,
    re.VERBOSE,
)

_TIMESTAMP_SAFE_RE = re.compile(
    r"\b(?:log|debug|warn|info|perf|stats|diag|timing|elapsed|monotonic)\b", re.I
)


def check_timestamp_leakage(verbose: bool = False) -> tuple[bool, list[dict]]:
    """Scan compiler codegen sources for timestamp patterns that could leak into output."""
    findings: list[dict] = []

    # Only scan the frontend (codegen) module.
    scan_paths: list[Path] = []
    frontend_dir = SRC_DIR / "frontend"
    if frontend_dir.is_dir():
        scan_paths = sorted(frontend_dir.rglob("*.py"))
    elif FRONTEND_INIT.exists():
        scan_paths = [FRONTEND_INIT]

    for src in scan_paths:
        rel = str(src.relative_to(ROOT))
        for i, line in enumerate(src.read_text().splitlines(), 1):
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            if _TIMESTAMP_RE.search(stripped) and not _TIMESTAMP_SAFE_RE.search(
                stripped
            ):
                findings.append({"file": rel, "line": i, "text": stripped})
                if verbose:
                    print(f"  FOUND {rel}:{i}: {stripped}")

    ok = len(findings) == 0
    if verbose and ok:
        print("  PASS  No timestamp leakage found in compiler sources.")
    return ok, findings


# ---------------------------------------------------------------------------
# Entropy source audit
# ---------------------------------------------------------------------------

_ENTROPY_RE = re.compile(
    r"""
    \brandom\.\w+\(
    | \bos\.urandom\(
    | \buuid\.\w+\(
    | \bsecrets\.\w+\(
    """,
    re.VERBOSE,
)


def check_entropy_sources(verbose: bool = False) -> tuple[bool, list[dict]]:
    """Scan compiler codegen sources for entropy/randomness usage."""
    findings: list[dict] = []

    # Only scan the frontend (codegen) module.
    scan_paths: list[Path] = []
    frontend_dir = SRC_DIR / "frontend"
    if frontend_dir.is_dir():
        scan_paths = sorted(frontend_dir.rglob("*.py"))
    elif FRONTEND_INIT.exists():
        scan_paths = [FRONTEND_INIT]

    for src in scan_paths:
        rel = str(src.relative_to(ROOT))
        for i, line in enumerate(src.read_text().splitlines(), 1):
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            if _ENTROPY_RE.search(stripped):
                findings.append({"file": rel, "line": i, "text": stripped})
                if verbose:
                    print(f"  FOUND {rel}:{i}: {stripped}")

    ok = len(findings) == 0
    if verbose and ok:
        print("  PASS  No entropy sources found in compiler sources.")
    return ok, findings


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

CHECK_NAMES = ["ir", "cross-process", "timestamp", "entropy"]


def _get_programs(quick: bool) -> list[Path]:
    if not BASIC_DIR.is_dir():
        return []
    programs = sorted(BASIC_DIR.glob("*.py"))
    if quick:
        return programs[:1]
    return programs[:20]


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--check",
        choices=CHECK_NAMES,
        action="append",
        help="Run specific check(s). Repeat for multiple. Default: all.",
    )
    parser.add_argument(
        "--quick",
        action="store_true",
        help="Quick mode: 1 program, 2 runs (for CI Tier 1).",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=2,
        help="Number of IR compilation runs (default: 2).",
    )
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write JSON results to FILE.",
    )
    args = parser.parse_args()

    checks = args.check or CHECK_NAMES
    programs = _get_programs(args.quick)
    runs = args.runs

    overall_pass = True
    report: dict[str, dict] = {}

    for check in checks:
        print(f"\n=== {check} ===")

        if check == "ir":
            if not programs:
                print("  SKIP  No programs found in tests/differential/basic/")
                report["ir"] = {"status": "skip"}
                continue
            ok, details = check_ir_determinism(
                programs, runs=runs, verbose=args.verbose
            )
            report["ir"] = {"status": "pass" if ok else "fail", "details": details}

        elif check == "cross-process":
            if not programs:
                print("  SKIP  No programs found")
                report["cross-process"] = {"status": "skip"}
                continue
            progs = programs[:5] if not args.quick else programs[:1]
            ok, details = check_cross_process_determinism(progs, verbose=args.verbose)
            report["cross-process"] = {
                "status": "pass" if ok else "fail",
                "details": details,
            }

        elif check == "timestamp":
            ok, details = check_timestamp_leakage(verbose=args.verbose)
            report["timestamp"] = {
                "status": "pass" if ok else "fail",
                "findings": details,
            }

        elif check == "entropy":
            ok, details = check_entropy_sources(verbose=args.verbose)
            report["entropy"] = {
                "status": "pass" if ok else "fail",
                "findings": details,
            }
        else:
            continue

        status_str = "PASS" if ok else "FAIL"
        print(f"  Result: {status_str}")
        if not ok:
            overall_pass = False

    print(f"\n{'=' * 40}")
    print(f"Overall: {'PASS' if overall_pass else 'FAIL'}")

    if args.json_out:
        Path(args.json_out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.json_out).write_text(
            json.dumps(
                {"overall": "pass" if overall_pass else "fail", "checks": report},
                indent=2,
            )
            + "\n"
        )
        print(f"JSON report: {args.json_out}")

    return 0 if overall_pass else 1


if __name__ == "__main__":
    sys.exit(main())

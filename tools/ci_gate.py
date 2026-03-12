#!/usr/bin/env python3
"""Unified CI gate with tiered verification pipeline.

Runs ALL correctness verification tools in dependency order across three tiers:

  Tier 1 — Fast (< 60s, every commit):
    Linting, formatting, correspondence checks, layout checks, coverage
    analysis, property/mutation/fuzz smoke tests.

  Tier 2 — Medium (< 10min, on PR):
    Quint simulation, translation validation, full property tests,
    reproducible build spot-check.

  Tier 3 — Heavy (< 60min, nightly/weekly):
    Full formal methods (Lean + Quint), deep reproducibility sweep,
    extended fuzzing, mutation testing, model-based tests.

Usage:
    uv run --python 3.12 python3 tools/ci_gate.py
    uv run --python 3.12 python3 tools/ci_gate.py --tier 2
    uv run --python 3.12 python3 tools/ci_gate.py --tier all --parallel
    uv run --python 3.12 python3 tools/ci_gate.py --tier 1 --json
    uv run --python 3.12 python3 tools/ci_gate.py --dry-run
    uv run --python 3.12 python3 tools/ci_gate.py --tier 2 --fail-fast
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "tools"
TESTS = ROOT / "tests"

IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(t: str) -> str:
    return _c("32", t)


def red(t: str) -> str:
    return _c("31", t)


def yellow(t: str) -> str:
    return _c("33", t)


def bold(t: str) -> str:
    return _c("1", t)


def dim(t: str) -> str:
    return _c("2", t)


# ---------------------------------------------------------------------------
# Check definition
# ---------------------------------------------------------------------------


@dataclass
class Check:
    """A single verification step."""

    name: str
    tier: int
    cmd: list[str]
    cwd: str | None = None
    env_extra: dict[str, str] = field(default_factory=dict)
    timeout: int = 300  # seconds
    required: bool = True  # False = continue-on-error
    needs_rust: bool = False
    needs_lean: bool = False
    needs_quint: bool = False
    needs_pytest: bool = False


@dataclass
class CheckResult:
    name: str
    tier: int
    status: str  # "pass", "fail", "skip", "error"
    duration_s: float = 0.0
    returncode: int = 0
    stdout: str = ""
    stderr: str = ""
    skip_reason: str = ""


# ---------------------------------------------------------------------------
# UV / tool helpers
# ---------------------------------------------------------------------------

_UV = shutil.which("uv") or "uv"
_PYTHON = "3.12"


def _uv_run(*args: str) -> list[str]:
    """Build a 'uv run --python 3.12 python3 ...' command."""
    return [_UV, "run", "--python", _PYTHON, "python3", *args]


def _uv_pytest(*args: str) -> list[str]:
    """Build a 'uv run --python 3.12 pytest ...' command."""
    return [_UV, "run", "--python", _PYTHON, "pytest", *args]


def _has_tool(name: str) -> bool:
    return shutil.which(name) is not None


# ---------------------------------------------------------------------------
# Check registry
# ---------------------------------------------------------------------------


def _build_checks() -> list[Check]:
    """Return all checks, all tiers."""
    checks: list[Check] = []

    # ── Tier 1: Fast (< 60s, every commit) ─────────────────────────────

    checks.append(
        Check(
            name="ruff-check",
            tier=1,
            cmd=[_UV, "run", "--python", _PYTHON, "ruff", "check", "."],
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="ruff-format",
            tier=1,
            cmd=[_UV, "run", "--python", _PYTHON, "ruff", "format", "--check", "."],
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="cargo-fmt",
            tier=1,
            cmd=["cargo", "fmt", "--check"],
            timeout=60,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="cargo-clippy",
            tier=1,
            cmd=["cargo", "clippy", "--", "-D", "warnings"],
            timeout=120,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="correspondence-check",
            tier=1,
            cmd=_uv_run(str(TOOLS / "check_correspondence.py"), "--json"),
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="differential-suite-layout",
            tier=1,
            cmd=_uv_run(str(TOOLS / "check_differential_suite_layout.py")),
            timeout=30,
        )
    )
    checks.append(
        Check(
            name="diff-coverage-analysis",
            tier=1,
            cmd=_uv_run(str(TOOLS / "diff_coverage_analysis.py"), "--json"),
            timeout=60,
        )
    )
    checks.append(
        Check(
            name="property-smoke",
            tier=1,
            cmd=_uv_pytest(
                str(TESTS / "property"),
                "-x",
                "--max-examples=10",
                "-q",
            ),
            timeout=60,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="mutation-smoke",
            tier=1,
            cmd=_uv_pytest(
                str(TESTS / "mutation" / "test_mutation_smoke.py"),
                "-x",
                "-q",
            ),
            timeout=60,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="fuzz-smoke",
            tier=1,
            cmd=_uv_pytest(
                str(TESTS / "fuzz" / "test_fuzz_smoke.py"),
                "-x",
                "-q",
            ),
            timeout=60,
            needs_pytest=True,
        )
    )

    # ── Tier 2: Medium (< 10min, on PR) ────────────────────────────────

    checks.append(
        Check(
            name="formal-methods-quint-only",
            tier=2,
            cmd=_uv_run(
                str(TOOLS / "check_formal_methods.py"),
                "--skip-build",
            ),
            timeout=120,
            needs_quint=True,
        )
    )
    checks.append(
        Check(
            name="translation-validate-core",
            tier=2,
            cmd=_uv_run(
                str(TOOLS / "translation_validate.py"),
                "--json",
                str(TESTS / "differential" / "basic" / "core_types"),
            ),
            timeout=300,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="property-tests-full",
            tier=2,
            cmd=_uv_pytest(
                str(TESTS / "property"),
                "--max-examples=200",
                "-q",
            ),
            timeout=300,
            needs_pytest=True,
        )
    )
    checks.append(
        Check(
            name="reproducible-build-spot",
            tier=2,
            cmd=_uv_run(
                str(TOOLS / "verify_reproducible.py"),
                "--runs",
                "2",
                "--programs",
                "examples/hello.py",
                "--object",
            ),
            timeout=300,
            needs_rust=True,
        )
    )

    # ── Tier 3: Heavy (< 60min, nightly/weekly) ────────────────────────

    checks.append(
        Check(
            name="formal-methods-full",
            tier=3,
            cmd=_uv_run(str(TOOLS / "check_formal_methods.py")),
            timeout=1200,
            needs_lean=True,
            needs_quint=True,
        )
    )
    checks.append(
        Check(
            name="reproducible-build-sweep",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "verify_reproducible.py"),
                "--runs",
                "5",
                "--object",
            ),
            timeout=600,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="fuzz-compiler-extended",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "fuzz_compiler.py"),
                "--count",
                "100",
                "--timeout",
                "300",
            ),
            timeout=600,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="mutation-test-extended",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "mutation_test.py"),
                "--max-mutations",
                "50",
                "--timeout",
                "60",
                "--no-fail",
            ),
            timeout=3600,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="translation-validate-full",
            tier=3,
            cmd=_uv_run(
                str(TOOLS / "translation_validate.py"),
                "--json",
                str(TESTS / "differential"),
            ),
            timeout=1800,
            needs_rust=True,
        )
    )
    checks.append(
        Check(
            name="model-based-tests",
            tier=3,
            cmd=_uv_pytest(
                str(TESTS / "model_based"),
                "-x",
                "-q",
            ),
            timeout=600,
            needs_pytest=True,
        )
    )

    return checks


# ---------------------------------------------------------------------------
# Execution engine
# ---------------------------------------------------------------------------


def _skip_reason(check: Check) -> str | None:
    """Return a skip reason if prerequisites are missing, else None."""
    if check.needs_rust and not _has_tool("cargo"):
        return "cargo not found"
    if check.needs_lean and not _has_tool("lake"):
        return "lake (Lean 4) not found"
    if check.needs_quint and not _has_tool("quint"):
        return "quint not found"
    # Check that tool script exists for uv-run checks
    if check.cmd and len(check.cmd) > 4 and check.cmd[0] == _UV:
        script = check.cmd[4] if len(check.cmd) > 4 else None
        if script and script.startswith(str(TOOLS)) and not Path(script).exists():
            return f"script not found: {script}"
    # Check test directories for pytest checks — find the first arg that
    # looks like a path (after the "pytest" token in the command list).
    if check.needs_pytest:
        try:
            pytest_idx = check.cmd.index("pytest")
            for arg in check.cmd[pytest_idx + 1 :]:
                if arg.startswith("-"):
                    continue
                if not Path(arg).exists():
                    return f"test path not found: {arg}"
                break
        except ValueError:
            pass
    return None


def _run_check(check: Check, dry_run: bool = False) -> CheckResult:
    """Execute a single check and return the result."""
    skip = _skip_reason(check)
    if skip:
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="skip",
            skip_reason=skip,
        )

    if dry_run:
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="skip",
            skip_reason="dry-run",
        )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env["PYTHONUNBUFFERED"] = "1"
    env.update(check.env_extra)

    start = time.monotonic()
    try:
        proc = subprocess.run(
            check.cmd,
            cwd=check.cwd or str(ROOT),
            env=env,
            capture_output=True,
            text=True,
            timeout=check.timeout,
        )
        duration = time.monotonic() - start
        status = "pass" if proc.returncode == 0 else "fail"
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status=status,
            duration_s=round(duration, 2),
            returncode=proc.returncode,
            stdout=proc.stdout[-4096:] if len(proc.stdout) > 4096 else proc.stdout,
            stderr=proc.stderr[-4096:] if len(proc.stderr) > 4096 else proc.stderr,
        )
    except subprocess.TimeoutExpired:
        duration = time.monotonic() - start
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="error",
            duration_s=round(duration, 2),
            returncode=-1,
            stderr=f"timeout after {check.timeout}s",
        )
    except Exception as exc:  # noqa: BLE001
        duration = time.monotonic() - start
        return CheckResult(
            name=check.name,
            tier=check.tier,
            status="error",
            duration_s=round(duration, 2),
            returncode=-1,
            stderr=str(exc),
        )


def _status_icon(status: str) -> str:
    icons = {
        "pass": green("PASS"),
        "fail": red("FAIL"),
        "skip": yellow("SKIP"),
        "error": red("ERR "),
    }
    return icons.get(status, status)


def _print_result(result: CheckResult, verbose: bool = False) -> None:
    icon = _status_icon(result.status)
    timing = dim(f"({result.duration_s:.1f}s)") if result.duration_s > 0 else ""
    skip_info = dim(f" [{result.skip_reason}]") if result.skip_reason else ""
    print(f"  {icon}  {result.name} {timing}{skip_info}")
    if verbose and result.status in ("fail", "error"):
        if result.stderr:
            for line in result.stderr.strip().splitlines()[-10:]:
                print(f"         {dim(line)}")


# ---------------------------------------------------------------------------
# Main orchestrator
# ---------------------------------------------------------------------------


def run_gate(
    tiers: list[int],
    fail_fast: bool = False,
    parallel: bool = False,
    dry_run: bool = False,
    json_out: bool = False,
    verbose: bool = False,
) -> list[CheckResult]:
    """Run all checks for the requested tiers and return results."""
    all_checks = _build_checks()
    selected = [c for c in all_checks if c.tier in tiers]

    if not selected:
        print("No checks selected.")
        return []

    results: list[CheckResult] = []

    # Group by tier for ordered execution
    for tier in sorted(set(tiers)):
        tier_checks = [c for c in selected if c.tier == tier]
        if not tier_checks:
            continue

        if not json_out:
            print(f"\n{bold(f'=== Tier {tier} ===')} ({len(tier_checks)} checks)")

        if parallel and len(tier_checks) > 1:
            # Run checks within a tier concurrently
            with ThreadPoolExecutor(max_workers=min(4, len(tier_checks))) as pool:
                futures = {
                    pool.submit(_run_check, check, dry_run): check
                    for check in tier_checks
                }
                for future in as_completed(futures):
                    result = future.result()
                    results.append(result)
                    if not json_out:
                        _print_result(result, verbose)
                    if (
                        fail_fast
                        and result.status in ("fail", "error")
                        and futures[future].required
                    ):
                        # Cancel remaining futures
                        for f in futures:
                            f.cancel()
                        break
        else:
            for check in tier_checks:
                result = _run_check(check, dry_run)
                results.append(result)
                if not json_out:
                    _print_result(result, verbose)
                if fail_fast and result.status in ("fail", "error") and check.required:
                    break

        # Check for fail-fast across tiers
        if fail_fast and any(
            r.status in ("fail", "error") for r in results if r.tier == tier
        ):
            required_failures = [
                r
                for r in results
                if r.tier == tier
                and r.status in ("fail", "error")
                and any(c.required for c in tier_checks if c.name == r.name)
            ]
            if required_failures:
                break

    return results


def _results_to_dict(results: list[CheckResult]) -> dict[str, Any]:
    """Convert results to a JSON-serializable dict."""
    passed = sum(1 for r in results if r.status == "pass")
    failed = sum(1 for r in results if r.status == "fail")
    errored = sum(1 for r in results if r.status == "error")
    skipped = sum(1 for r in results if r.status == "skip")
    total_time = sum(r.duration_s for r in results)

    return {
        "summary": {
            "total": len(results),
            "passed": passed,
            "failed": failed,
            "errored": errored,
            "skipped": skipped,
            "total_time_s": round(total_time, 2),
            "success": failed == 0 and errored == 0,
        },
        "checks": [
            {
                "name": r.name,
                "tier": r.tier,
                "status": r.status,
                "duration_s": r.duration_s,
                "returncode": r.returncode,
                **({"skip_reason": r.skip_reason} if r.skip_reason else {}),
                **(
                    {"stderr_tail": r.stderr[-500:]}
                    if r.status in ("fail", "error") and r.stderr
                    else {}
                ),
            }
            for r in results
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Unified CI gate with tiered verification pipeline.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--tier",
        choices=["1", "2", "3", "all"],
        default="1",
        help="Which tier to run (default: 1). 'all' runs tiers 1-3.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output results as JSON",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop on first required-check failure",
    )
    parser.add_argument(
        "--parallel",
        action="store_true",
        help="Run independent checks within each tier concurrently",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would run without executing",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show stderr tail on failures",
    )
    args = parser.parse_args()

    if args.tier == "all":
        tiers = [1, 2, 3]
    else:
        tier_num = int(args.tier)
        # Running tier N implies running all tiers <= N
        tiers = list(range(1, tier_num + 1))

    if not args.json:
        tier_label = "all" if args.tier == "all" else args.tier
        mode_flags = []
        if args.fail_fast:
            mode_flags.append("fail-fast")
        if args.parallel:
            mode_flags.append("parallel")
        if args.dry_run:
            mode_flags.append("dry-run")
        mode_str = f" [{', '.join(mode_flags)}]" if mode_flags else ""
        print(bold(f"Molt CI Gate -- tier {tier_label}{mode_str}"))

    results = run_gate(
        tiers=tiers,
        fail_fast=args.fail_fast,
        parallel=args.parallel,
        dry_run=args.dry_run,
        json_out=args.json,
        verbose=args.verbose,
    )

    if args.json:
        output = _results_to_dict(results)
        print(json.dumps(output, indent=2))
    else:
        # Print summary
        passed = sum(1 for r in results if r.status == "pass")
        failed = sum(1 for r in results if r.status == "fail")
        errored = sum(1 for r in results if r.status == "error")
        skipped = sum(1 for r in results if r.status == "skip")
        total_time = sum(r.duration_s for r in results)

        print(f"\n{bold('Summary:')}")
        parts = []
        if passed:
            parts.append(green(f"{passed} passed"))
        if failed:
            parts.append(red(f"{failed} failed"))
        if errored:
            parts.append(red(f"{errored} errored"))
        if skipped:
            parts.append(yellow(f"{skipped} skipped"))
        print(f"  {', '.join(parts)} in {total_time:.1f}s")

        if failed > 0 or errored > 0:
            failures = [r for r in results if r.status in ("fail", "error")]
            print(f"\n{bold('Failures:')}")
            for r in failures:
                print(f"  {red(r.name)} (tier {r.tier}, rc={r.returncode})")

    # Exit code: 0 if no required failures
    all_checks = _build_checks()
    required_names = {c.name for c in all_checks if c.required}
    has_required_failure = any(
        r.status in ("fail", "error") and r.name in required_names for r in results
    )
    sys.exit(1 if has_required_failure else 0)


if __name__ == "__main__":
    main()

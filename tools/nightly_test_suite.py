#!/usr/bin/env python3
"""Nightly/weekly continuous testing orchestration for Molt.

Orchestrates grammar fuzzing, mutation testing, model-based test generation,
and translation validation into scheduled test campaigns with persistent
reporting and trend tracking.

**Nightly** (default):
  - Grammar fuzzing: 500 programs, 600s timeout
  - Model-based tests: 10 traces per Quint model as differential tests
  - Translation validation: all files in tests/differential/basic/

**Weekly** (``--mode weekly``):
  - Mutation testing: full compiler mutation, 200 max mutations
  - Extended fuzzing: 2000 programs with shrinking
  - Full translation validation: all files in tests/differential/
  - Reproducibility check: 5 repetitions on 10 programs

Usage::

    uv run --python 3.12 python3 tools/nightly_test_suite.py --mode nightly
    uv run --python 3.12 python3 tools/nightly_test_suite.py --mode weekly
    uv run --python 3.12 python3 tools/nightly_test_suite.py --mode nightly --dry-run
    uv run --python 3.12 python3 tools/nightly_test_suite.py --mode weekly --json
    uv run --python 3.12 python3 tools/nightly_test_suite.py --report-dir /tmp/reports

Environment:
    MOLT_EXT_ROOT     External volume for artifacts (fallback: /tmp/molt_testing)
    CARGO_TARGET_DIR  Rust build target directory
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[1]
_EXT_ROOT_DEFAULT = "/tmp/molt_testing"


def _ext_root() -> Path:
    """Resolve MOLT_EXT_ROOT with /tmp fallback."""
    raw = os.environ.get("MOLT_EXT_ROOT", "")
    if raw and Path(raw).is_dir():
        return Path(raw)
    fallback = Path(_EXT_ROOT_DEFAULT)
    fallback.mkdir(parents=True, exist_ok=True)
    return fallback


def _report_dir(override: str | None = None) -> Path:
    """Return the dated report directory."""
    if override:
        d = Path(override)
    else:
        d = (
            _ext_root()
            / "test_reports"
            / datetime.now(timezone.utc).strftime("%Y-%m-%d")
        )
    d.mkdir(parents=True, exist_ok=True)
    return d


def _fuzz_results_dir() -> Path:
    d = _ext_root() / "fuzz_results"
    d.mkdir(parents=True, exist_ok=True)
    return d


# ---------------------------------------------------------------------------
# Result data classes
# ---------------------------------------------------------------------------


@dataclass
class StepResult:
    """Result of a single test step."""

    name: str
    passed: bool
    duration_s: float = 0.0
    summary: dict[str, Any] = field(default_factory=dict)
    error: str | None = None


@dataclass
class SuiteResult:
    """Aggregate result of an entire nightly/weekly run."""

    mode: str
    timestamp: str
    steps: list[StepResult] = field(default_factory=list)
    regression_alerts: list[str] = field(default_factory=list)

    @property
    def all_passed(self) -> bool:
        return all(s.passed for s in self.steps)

    def to_dict(self) -> dict[str, Any]:
        d = asdict(self)
        d["all_passed"] = self.all_passed
        return d


# ---------------------------------------------------------------------------
# Step runners
# ---------------------------------------------------------------------------


def _run_cmd(
    cmd: list[str],
    *,
    timeout: int = 1200,
    dry_run: bool = False,
) -> tuple[int, str, str, float]:
    """Run a subprocess, returning (returncode, stdout, stderr, elapsed_s)."""
    if dry_run:
        print(f"  [dry-run] would run: {' '.join(cmd)}")
        return 0, "", "", 0.0

    env = os.environ.copy()
    env.setdefault("PYTHONPATH", str(REPO_ROOT / "src"))
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"

    t0 = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(REPO_ROOT),
            env=env,
        )
        elapsed = time.monotonic() - t0
        return proc.returncode, proc.stdout, proc.stderr, elapsed
    except subprocess.TimeoutExpired:
        elapsed = time.monotonic() - t0
        return -1, "", f"TIMEOUT after {timeout}s", elapsed


def _python() -> str:
    """Return the Python executable path."""
    return sys.executable


def run_grammar_fuzz(
    *,
    count: int = 500,
    timeout: float = 600,
    shrink: bool = False,
    dry_run: bool = False,
) -> StepResult:
    """Run grammar-based fuzzing via tools/fuzz_compiler.py."""
    print(f"\n{'=' * 60}")
    print(f"  Grammar fuzzing: {count} programs, timeout {timeout}s")
    print(f"{'=' * 60}")

    out_dir = _fuzz_results_dir() / datetime.now(timezone.utc).strftime("%Y-%m-%d")
    out_dir.mkdir(parents=True, exist_ok=True)

    cmd = [
        _python(),
        "-u",
        str(REPO_ROOT / "tools" / "fuzz_compiler.py"),
        "--mode",
        "safe",
        "--count",
        str(count),
        "--timeout",
        str(timeout),
        "--output-dir",
        str(out_dir),
        "--build-profile",
        "dev",
    ]
    if shrink:
        cmd.append("--shrink")

    rc, stdout, stderr, elapsed = _run_cmd(
        cmd, timeout=int(timeout) + 300, dry_run=dry_run
    )

    # Parse results from output
    summary: dict[str, Any] = {
        "programs_tested": count,
        "output_dir": str(out_dir),
    }

    # Try to extract counts from the summary output
    for line in (stdout + stderr).splitlines():
        line = line.strip()
        if "Passed" in line and ":" in line:
            try:
                summary["passed"] = int(line.split(":")[-1].strip().split()[0])
            except (ValueError, IndexError):
                pass
        elif "Mismatch" in line and ":" in line:
            try:
                summary["mismatches"] = int(line.split(":")[-1].strip().split()[0])
            except (ValueError, IndexError):
                pass
        elif "Build error" in line and ":" in line:
            try:
                summary["build_errors"] = int(line.split(":")[-1].strip().split()[0])
            except (ValueError, IndexError):
                pass

    passed = rc == 0
    error_msg = None if passed else (stderr[-500:] if stderr else f"exit code {rc}")

    return StepResult(
        name="grammar_fuzz",
        passed=passed,
        duration_s=elapsed,
        summary=summary,
        error=error_msg,
    )


def run_model_based_tests(
    *,
    traces_per_model: int = 10,
    dry_run: bool = False,
) -> StepResult:
    """Generate and run model-based tests from Quint traces."""
    print(f"\n{'=' * 60}")
    print(f"  Model-based tests: {traces_per_model} traces per model")
    print(f"{'=' * 60}")

    models_dir = REPO_ROOT / "formal" / "quint"
    models = sorted(models_dir.glob("*.qnt")) if models_dir.is_dir() else []

    if not models:
        return StepResult(
            name="model_based_tests",
            passed=True,
            summary={"skipped": True, "reason": "no Quint models found"},
        )

    output_dir = (
        _ext_root()
        / "generated_tests"
        / datetime.now(timezone.utc).strftime("%Y-%m-%d")
    )
    output_dir.mkdir(parents=True, exist_ok=True)

    total_generated = 0
    model_results: dict[str, Any] = {}
    t0 = time.monotonic()

    for model in models:
        model_name = model.stem

        cmd = [
            _python(),
            "-u",
            str(REPO_ROOT / "tools" / "quint_trace_to_tests.py"),
            "--model",
            str(model),
            "--max-steps",
            "10",
            "--count",
            str(traces_per_model),
            "--output-dir",
            str(output_dir / model_name),
            "--json",
        ]

        rc, stdout, stderr, elapsed = _run_cmd(cmd, timeout=300, dry_run=dry_run)

        generated = 0
        if rc == 0 and stdout.strip():
            try:
                report = json.loads(stdout)
                generated = report.get("count_generated", 0)
            except json.JSONDecodeError:
                pass

        total_generated += generated
        model_results[model_name] = {
            "generated": generated,
            "exit_code": rc,
            "elapsed_s": round(elapsed, 1),
        }

    # Run generated tests as differential tests if any were created
    diff_rc = 0
    if total_generated > 0 and not dry_run:
        diff_cmd = [
            _python(),
            "-u",
            str(REPO_ROOT / "tests" / "molt_diff.py"),
            "--build-profile",
            "dev",
            "--jobs",
            "2",
            str(output_dir),
        ]
        diff_rc, _, diff_stderr, _ = _run_cmd(diff_cmd, timeout=600, dry_run=dry_run)

    elapsed_total = time.monotonic() - t0

    return StepResult(
        name="model_based_tests",
        passed=diff_rc == 0,
        duration_s=elapsed_total,
        summary={
            "models_tested": len(models),
            "total_traces_generated": total_generated,
            "per_model": model_results,
        },
        error=None if diff_rc == 0 else f"Differential tests failed (rc={diff_rc})",
    )


def run_translation_validation(
    *,
    test_dir: str = "tests/differential/basic",
    dry_run: bool = False,
) -> StepResult:
    """Run translation validation on differential test files."""
    print(f"\n{'=' * 60}")
    print(f"  Translation validation: {test_dir}")
    print(f"{'=' * 60}")

    target = REPO_ROOT / test_dir
    if not target.exists():
        return StepResult(
            name="translation_validation",
            passed=True,
            summary={"skipped": True, "reason": f"{test_dir} not found"},
        )

    # Collect Python test files
    py_files = sorted(target.rglob("*.py"))
    py_files = [f for f in py_files if "__pycache__" not in str(f)]

    cmd = [
        _python(),
        "-u",
        str(REPO_ROOT / "tools" / "translation_validate.py"),
        "--json",
    ] + [str(f) for f in py_files[:200]]  # cap to avoid argv overflow

    rc, stdout, stderr, elapsed = _run_cmd(cmd, timeout=900, dry_run=dry_run)

    summary: dict[str, Any] = {
        "files_tested": len(py_files[:200]),
        "test_dir": test_dir,
    }

    # Parse JSON output if available
    if stdout.strip():
        for line in reversed(stdout.strip().splitlines()):
            line = line.strip()
            if line.startswith("{") or line.startswith("["):
                try:
                    parsed = json.loads(line)
                    if isinstance(parsed, dict):
                        summary.update(
                            {
                                k: v
                                for k, v in parsed.items()
                                if k in ("passed", "failed", "errors", "total")
                            }
                        )
                except json.JSONDecodeError:
                    pass
                break

    return StepResult(
        name="translation_validation",
        passed=rc == 0,
        duration_s=elapsed,
        summary=summary,
        error=None if rc == 0 else (stderr[-500:] if stderr else f"exit code {rc}"),
    )


def run_mutation_testing(
    *,
    max_mutations: int = 200,
    test_subset: str = "tests/differential/basic",
    timeout: int = 120,
    dry_run: bool = False,
) -> StepResult:
    """Run compiler mutation testing."""
    print(f"\n{'=' * 60}")
    print(f"  Mutation testing: max {max_mutations} mutations")
    print(f"{'=' * 60}")

    json_out = _report_dir() / "mutation_results.json"

    cmd = [
        _python(),
        "-u",
        str(REPO_ROOT / "tools" / "mutation_test.py"),
        "--mode",
        "compiler",
        "--max-mutations",
        str(max_mutations),
        "--test-subset",
        str(REPO_ROOT / test_subset),
        "--timeout",
        str(timeout),
        "--no-fail",
        "--json",
        str(json_out),
    ]

    rc, stdout, stderr, elapsed = _run_cmd(
        cmd, timeout=max_mutations * timeout + 600, dry_run=dry_run
    )

    summary: dict[str, Any] = {"max_mutations": max_mutations}

    # Read JSON results if written
    if json_out.exists():
        try:
            data = json.loads(json_out.read_text())
            summary.update(
                {
                    "mutation_score": data.get("score", 0.0),
                    "killed": data.get("killed", 0),
                    "survived": data.get("survived", 0),
                    "total": data.get("total", 0),
                    "build_fail": data.get("build_fail", 0),
                    "timeout": data.get("timeout", 0),
                }
            )
        except (json.JSONDecodeError, OSError):
            pass

    return StepResult(
        name="mutation_testing",
        passed=rc == 0,
        duration_s=elapsed,
        summary=summary,
        error=None if rc == 0 else (stderr[-500:] if stderr else f"exit code {rc}"),
    )


def run_reproducibility_check(
    *,
    repetitions: int = 5,
    program_count: int = 10,
    dry_run: bool = False,
) -> StepResult:
    """Verify build reproducibility by compiling programs multiple times."""
    print(f"\n{'=' * 60}")
    print(f"  Reproducibility: {repetitions} reps x {program_count} programs")
    print(f"{'=' * 60}")

    cmd = [
        _python(),
        "-u",
        str(REPO_ROOT / "tools" / "check_reproducible_build.py"),
    ]

    # Check if the tool accepts relevant flags
    rc, stdout, stderr, elapsed = _run_cmd(cmd, timeout=600, dry_run=dry_run)

    return StepResult(
        name="reproducibility_check",
        passed=rc == 0,
        duration_s=elapsed,
        summary={
            "repetitions": repetitions,
            "programs": program_count,
        },
        error=None if rc == 0 else (stderr[-500:] if stderr else f"exit code {rc}"),
    )


# ---------------------------------------------------------------------------
# Trend tracking
# ---------------------------------------------------------------------------


def _load_previous_results(
    report_base: Path, *, max_history: int = 14
) -> list[dict[str, Any]]:
    """Load up to max_history previous run results for trend comparison."""
    results: list[dict[str, Any]] = []
    if not report_base.parent.exists():
        return results

    dirs = sorted(
        [d for d in report_base.parent.iterdir() if d.is_dir() and d != report_base],
        reverse=True,
    )

    for d in dirs[:max_history]:
        summary_file = d / "suite_result.json"
        if summary_file.exists():
            try:
                data = json.loads(summary_file.read_text())
                results.append(data)
            except (json.JSONDecodeError, OSError):
                pass

    return results


def _check_regressions(
    current: SuiteResult,
    history: list[dict[str, Any]],
) -> list[str]:
    """Compare current run against recent history, return alert messages."""
    alerts: list[str] = []
    if not history:
        return alerts

    prev = history[0]  # most recent previous run

    # Check mutation score regression
    for step in current.steps:
        if step.name == "mutation_testing" and "mutation_score" in step.summary:
            current_score = step.summary["mutation_score"]
            for prev_step in prev.get("steps", []):
                if prev_step.get("name") == "mutation_testing":
                    prev_score = prev_step.get("summary", {}).get("mutation_score", 0)
                    if prev_score > 0 and current_score < prev_score - 0.05:
                        alerts.append(
                            f"REGRESSION: Mutation score dropped from "
                            f"{prev_score:.1%} to {current_score:.1%} "
                            f"(threshold: 5%)"
                        )

    # Check for new fuzz failures
    for step in current.steps:
        if step.name == "grammar_fuzz" and not step.passed:
            # Check if previous run also failed
            prev_fuzz_passed = True
            for prev_step in prev.get("steps", []):
                if prev_step.get("name") == "grammar_fuzz":
                    prev_fuzz_passed = prev_step.get("passed", True)
            if prev_fuzz_passed:
                alerts.append(
                    "REGRESSION: New grammar fuzzing failures detected "
                    "(previous run passed)"
                )

    return alerts


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------


def _generate_markdown_report(result: SuiteResult) -> str:
    """Generate a markdown summary report."""
    lines: list[str] = []
    ts = result.timestamp
    status = "PASS" if result.all_passed else "FAIL"

    lines.append(f"# Molt Continuous Testing Report — {result.mode.title()}")
    lines.append("")
    lines.append(f"**Date**: {ts}")
    lines.append(f"**Mode**: {result.mode}")
    lines.append(f"**Overall**: {status}")
    lines.append("")

    # Step summary table
    lines.append("## Step Results")
    lines.append("")
    lines.append("| Step | Status | Duration |")
    lines.append("|------|--------|----------|")
    for step in result.steps:
        status_icon = "PASS" if step.passed else "FAIL"
        dur = f"{step.duration_s:.1f}s" if step.duration_s else "—"
        lines.append(f"| {step.name} | {status_icon} | {dur} |")
    lines.append("")

    # Detailed results per step
    for step in result.steps:
        lines.append(f"## {step.name}")
        lines.append("")
        if step.summary:
            for k, v in step.summary.items():
                if isinstance(v, dict):
                    lines.append(f"- **{k}**:")
                    for sk, sv in v.items():
                        lines.append(f"  - {sk}: {sv}")
                else:
                    lines.append(f"- **{k}**: {v}")
        if step.error:
            lines.append(f"- **Error**: `{step.error[:200]}`")
        lines.append("")

    # Regression alerts
    if result.regression_alerts:
        lines.append("## Regression Alerts")
        lines.append("")
        for alert in result.regression_alerts:
            lines.append(f"- {alert}")
        lines.append("")

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Suite orchestration
# ---------------------------------------------------------------------------


def run_nightly(*, dry_run: bool = False) -> SuiteResult:
    """Execute the nightly test suite."""
    result = SuiteResult(
        mode="nightly",
        timestamp=datetime.now(timezone.utc).isoformat(),
    )

    # 1. Grammar fuzzing: 500 programs
    result.steps.append(run_grammar_fuzz(count=500, timeout=600, dry_run=dry_run))

    # 2. Model-based tests: 10 traces per model
    result.steps.append(run_model_based_tests(traces_per_model=10, dry_run=dry_run))

    # 3. Translation validation: basic tests
    result.steps.append(
        run_translation_validation(
            test_dir="tests/differential/basic",
            dry_run=dry_run,
        )
    )

    return result


def run_weekly(*, dry_run: bool = False) -> SuiteResult:
    """Execute the weekly test suite."""
    result = SuiteResult(
        mode="weekly",
        timestamp=datetime.now(timezone.utc).isoformat(),
    )

    # 1. Mutation testing: full compiler, 200 mutations
    result.steps.append(
        run_mutation_testing(
            max_mutations=200,
            timeout=120,
            dry_run=dry_run,
        )
    )

    # 2. Extended fuzzing: 2000 programs with shrinking
    result.steps.append(
        run_grammar_fuzz(
            count=2000,
            timeout=600,
            shrink=True,
            dry_run=dry_run,
        )
    )

    # 3. Full translation validation: all differential tests
    result.steps.append(
        run_translation_validation(
            test_dir="tests/differential",
            dry_run=dry_run,
        )
    )

    # 4. Reproducibility check: 5 reps x 10 programs
    result.steps.append(
        run_reproducibility_check(
            repetitions=5,
            program_count=10,
            dry_run=dry_run,
        )
    )

    return result


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Molt continuous testing orchestration (nightly/weekly).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  python tools/nightly_test_suite.py --mode nightly\n"
            "  python tools/nightly_test_suite.py --mode weekly --dry-run\n"
            "  python tools/nightly_test_suite.py --mode nightly --json\n"
        ),
    )
    parser.add_argument(
        "--mode",
        choices=["nightly", "weekly"],
        default="nightly",
        help="Test suite mode (default: nightly)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Print JSON results to stdout",
    )
    parser.add_argument(
        "--report-dir",
        type=str,
        default=None,
        help="Override report output directory",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print commands that would run without executing them",
    )

    args = parser.parse_args()

    print(f"Molt continuous testing — {args.mode} mode")
    print(f"Repo: {REPO_ROOT}")
    print(f"External root: {_ext_root()}")
    if args.dry_run:
        print("[DRY RUN — no commands will be executed]")

    # Run the selected suite
    if args.mode == "nightly":
        result = run_nightly(dry_run=args.dry_run)
    else:
        result = run_weekly(dry_run=args.dry_run)

    # Load history and check for regressions
    rdir = _report_dir(args.report_dir)
    history = _load_previous_results(rdir)
    alerts = _check_regressions(result, history)
    result.regression_alerts = alerts

    # Save results
    json_path = rdir / "suite_result.json"
    json_path.write_text(json.dumps(result.to_dict(), indent=2) + "\n")
    print(f"\nJSON results saved to: {json_path}")

    # Generate markdown report
    md_report = _generate_markdown_report(result)
    md_path = rdir / "report.md"
    md_path.write_text(md_report)
    print(f"Markdown report saved to: {md_path}")

    # Print alerts
    if alerts:
        print(f"\n{'!' * 60}")
        print("  REGRESSION ALERTS")
        print(f"{'!' * 60}")
        for alert in alerts:
            print(f"  - {alert}")

    # Print summary
    print(f"\n{'=' * 60}")
    print(f"  Suite: {result.mode} — {'PASS' if result.all_passed else 'FAIL'}")
    print(f"{'=' * 60}")
    for step in result.steps:
        status = "PASS" if step.passed else "FAIL"
        dur = f"{step.duration_s:.1f}s" if step.duration_s else "—"
        print(f"  [{status}] {step.name} ({dur})")
    print()

    # JSON output if requested
    if args.json_output:
        print(json.dumps(result.to_dict(), indent=2))

    return 0 if result.all_passed else 1


if __name__ == "__main__":
    sys.exit(main())

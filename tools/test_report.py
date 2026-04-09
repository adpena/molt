#!/usr/bin/env python3
"""Human-readable report generator for Molt nightly/weekly test runs.

Reads JSON results from ``$MOLT_EXT_ROOT/test_reports/`` or the repo-local
``tmp/molt_testing/test_reports`` canonical root and produces markdown
summaries with trend analysis.

Usage::

    # Show most recent report
    uv run --python 3.12 python3 tools/test_report.py

    # Show last 5 reports
    uv run --python 3.12 python3 tools/test_report.py --last 5

    # Compare two specific dates
    uv run --python 3.12 python3 tools/test_report.py --compare 2026-03-10 2026-03-12

    # Custom report directory
    uv run --python 3.12 python3 tools/test_report.py --report-dir tmp/reports
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[1]
_REPO_EXT_ROOT_DEFAULT = REPO_ROOT / "tmp" / "molt_testing"


def _reports_root(override: str | None = None) -> Path:
    if override:
        return Path(override).expanduser()
    raw = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser() / "test_reports"
    return _REPO_EXT_ROOT_DEFAULT / "test_reports"


# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------


@dataclass
class RunSnapshot:
    """A single test run loaded from disk."""

    date: str
    path: Path
    data: dict[str, Any]

    @property
    def mode(self) -> str:
        return self.data.get("mode", "unknown")

    @property
    def passed(self) -> bool:
        return self.data.get("all_passed", False)

    @property
    def steps(self) -> list[dict[str, Any]]:
        return self.data.get("steps", [])

    @property
    def alerts(self) -> list[str]:
        return self.data.get("regression_alerts", [])

    def step_by_name(self, name: str) -> dict[str, Any] | None:
        for s in self.steps:
            if s.get("name") == name:
                return s
        return None


def load_runs(
    reports_dir: Path,
    *,
    last: int = 0,
    dates: list[str] | None = None,
) -> list[RunSnapshot]:
    """Load run snapshots from the reports directory."""
    if not reports_dir.exists():
        return []

    dirs = sorted(
        [d for d in reports_dir.iterdir() if d.is_dir()],
        reverse=True,
    )

    if dates:
        dirs = [d for d in dirs if d.name in dates]

    if last > 0:
        dirs = dirs[:last]

    runs: list[RunSnapshot] = []
    for d in dirs:
        result_file = d / "suite_result.json"
        if not result_file.exists():
            continue
        try:
            data = json.loads(result_file.read_text())
        except (json.JSONDecodeError, OSError):
            continue
        runs.append(RunSnapshot(date=d.name, path=d, data=data))

    return runs


# ---------------------------------------------------------------------------
# ASCII trend rendering
# ---------------------------------------------------------------------------


def _ascii_bar(value: float, max_val: float, width: int = 30) -> str:
    """Render a simple ASCII bar."""
    if max_val <= 0:
        return ""
    filled = int((value / max_val) * width)
    filled = min(filled, width)
    return "#" * filled + "." * (width - filled)


def _render_trend(
    label: str,
    points: list[tuple[str, float]],
    *,
    fmt: str = ".1f",
    unit: str = "",
    as_percent: bool = False,
) -> list[str]:
    """Render a labeled ASCII trend from (date, value) points."""
    if not points:
        return [f"  {label}: (no data)"]

    lines: list[str] = [f"  {label}:"]

    values = [v for _, v in points]
    max_val = max(values) if values else 1.0
    if as_percent:
        max_val = 1.0

    for date, val in points:
        bar = _ascii_bar(val, max_val, width=25)
        if as_percent:
            val_str = f"{val:.1%}"
        else:
            val_str = f"{val:{fmt}}{unit}"
        lines.append(f"    {date} |{bar}| {val_str}")

    return lines


# ---------------------------------------------------------------------------
# Report sections
# ---------------------------------------------------------------------------


def _section_fuzzing(runs: list[RunSnapshot]) -> list[str]:
    """Fuzzing summary across runs."""
    lines = ["## Fuzzing", ""]

    for run in runs:
        step = run.step_by_name("grammar_fuzz")
        if not step:
            continue
        summary = step.get("summary", {})
        status = "PASS" if step.get("passed") else "FAIL"
        programs = summary.get("programs_tested", "?")
        passed = summary.get("passed", "?")
        mismatches = summary.get("mismatches", 0)
        build_errors = summary.get("build_errors", 0)

        lines.append(f"### {run.date} ({status})")
        lines.append(f"- Programs tested: {programs}")
        lines.append(f"- Passed: {passed}")
        lines.append(f"- Mismatches: {mismatches}")
        lines.append(f"- Build errors: {build_errors}")
        if step.get("error"):
            lines.append(f"- Error: `{step['error'][:150]}`")
        lines.append("")

    # Trend: mismatches over time
    trend_points: list[tuple[str, float]] = []
    for run in reversed(runs):
        step = run.step_by_name("grammar_fuzz")
        if step:
            mismatches = step.get("summary", {}).get("mismatches", 0)
            trend_points.append((run.date, float(mismatches)))

    if trend_points:
        lines.append("### Mismatch Trend (last runs)")
        lines.extend(_render_trend("mismatches", trend_points, fmt=".0f"))
        lines.append("")

    return lines


def _section_mutation(runs: list[RunSnapshot]) -> list[str]:
    """Mutation testing summary."""
    lines = ["## Mutation Testing", ""]

    for run in runs:
        step = run.step_by_name("mutation_testing")
        if not step:
            continue
        summary = step.get("summary", {})
        status = "PASS" if step.get("passed") else "FAIL"
        score = summary.get("mutation_score", None)
        killed = summary.get("killed", "?")
        survived = summary.get("survived", "?")
        total = summary.get("total", "?")

        lines.append(f"### {run.date} ({status})")
        if score is not None:
            lines.append(f"- Mutation score: {score:.1%}")
        lines.append(f"- Killed: {killed}")
        lines.append(f"- Survived: {survived}")
        lines.append(f"- Total: {total}")
        lines.append("")

    # Trend: mutation score
    trend_points: list[tuple[str, float]] = []
    for run in reversed(runs):
        step = run.step_by_name("mutation_testing")
        if step:
            score = step.get("summary", {}).get("mutation_score")
            if score is not None:
                trend_points.append((run.date, score))

    if trend_points:
        lines.append("### Mutation Score Trend")
        lines.extend(_render_trend("score", trend_points, as_percent=True))
        lines.append("")

    return lines


def _section_translation(runs: list[RunSnapshot]) -> list[str]:
    """Translation validation summary."""
    lines = ["## Translation Validation", ""]

    for run in runs:
        step = run.step_by_name("translation_validation")
        if not step:
            continue
        summary = step.get("summary", {})
        status = "PASS" if step.get("passed") else "FAIL"
        files = summary.get("files_tested", "?")
        test_dir = summary.get("test_dir", "?")

        lines.append(f"### {run.date} ({status})")
        lines.append(f"- Test dir: {test_dir}")
        lines.append(f"- Files tested: {files}")
        if "passed" in summary:
            lines.append(f"- Passed: {summary['passed']}")
        if "failed" in summary:
            lines.append(f"- Failed: {summary['failed']}")
        lines.append("")

    return lines


def _section_model_based(runs: list[RunSnapshot]) -> list[str]:
    """Model-based test summary."""
    lines = ["## Model-Based Tests", ""]

    for run in runs:
        step = run.step_by_name("model_based_tests")
        if not step:
            continue
        summary = step.get("summary", {})
        status = "PASS" if step.get("passed") else "FAIL"

        if summary.get("skipped"):
            lines.append(f"### {run.date} — skipped ({summary.get('reason', '')})")
            lines.append("")
            continue

        models = summary.get("models_tested", "?")
        traces = summary.get("total_traces_generated", "?")

        lines.append(f"### {run.date} ({status})")
        lines.append(f"- Models tested: {models}")
        lines.append(f"- Traces generated: {traces}")

        per_model = summary.get("per_model", {})
        if per_model:
            for model_name, info in per_model.items():
                gen = info.get("generated", 0)
                lines.append(f"  - {model_name}: {gen} traces")
        lines.append("")

    return lines


def _section_alerts(runs: list[RunSnapshot]) -> list[str]:
    """Regression alerts across runs."""
    lines = ["## Regression Alerts", ""]

    any_alerts = False
    for run in runs:
        if run.alerts:
            any_alerts = True
            lines.append(f"### {run.date}")
            for alert in run.alerts:
                lines.append(f"- {alert}")
            lines.append("")

    if not any_alerts:
        lines.append("No regression alerts.")
        lines.append("")

    return lines


def _section_overview(runs: list[RunSnapshot]) -> list[str]:
    """High-level overview table."""
    lines = ["## Overview", ""]
    lines.append("| Date | Mode | Status | Steps | Duration |")
    lines.append("|------|------|--------|-------|----------|")

    for run in runs:
        status = "PASS" if run.passed else "FAIL"
        n_steps = len(run.steps)
        total_dur = sum(s.get("duration_s", 0) for s in run.steps)
        dur_str = f"{total_dur:.0f}s" if total_dur else "—"
        lines.append(f"| {run.date} | {run.mode} | {status} | {n_steps} | {dur_str} |")

    lines.append("")
    return lines


# ---------------------------------------------------------------------------
# Compare mode
# ---------------------------------------------------------------------------


def _compare_runs(a: RunSnapshot, b: RunSnapshot) -> list[str]:
    """Generate a diff-style comparison between two runs."""
    lines = [
        f"# Comparison: {a.date} vs {b.date}",
        "",
    ]

    # Step-by-step comparison
    all_step_names = {s.get("name") for s in a.steps} | {s.get("name") for s in b.steps}

    lines.append("| Step | " + a.date + " | " + b.date + " | Delta |")
    lines.append(
        "|------|"
        + "-" * (len(a.date) + 2)
        + "|"
        + "-" * (len(b.date) + 2)
        + "|-------|"
    )

    for step_name in sorted(all_step_names):
        sa = a.step_by_name(step_name or "")
        sb = b.step_by_name(step_name or "")

        status_a = "PASS" if (sa and sa.get("passed")) else ("FAIL" if sa else "—")
        status_b = "PASS" if (sb and sb.get("passed")) else ("FAIL" if sb else "—")

        delta = ""
        if status_a != status_b:
            delta = f"{status_a} -> {status_b}"

        lines.append(f"| {step_name} | {status_a} | {status_b} | {delta} |")

    lines.append("")

    # Mutation score comparison
    mut_a = a.step_by_name("mutation_testing")
    mut_b = b.step_by_name("mutation_testing")
    if mut_a and mut_b:
        score_a = mut_a.get("summary", {}).get("mutation_score")
        score_b = mut_b.get("summary", {}).get("mutation_score")
        if score_a is not None and score_b is not None:
            diff = score_b - score_a
            direction = (
                "improved" if diff > 0 else "regressed" if diff < 0 else "unchanged"
            )
            lines.append(
                f"Mutation score: {score_a:.1%} -> {score_b:.1%} ({direction}, {diff:+.1%})"
            )
            lines.append("")

    return lines


# ---------------------------------------------------------------------------
# Full report assembly
# ---------------------------------------------------------------------------


def generate_full_report(runs: list[RunSnapshot]) -> str:
    """Generate the complete human-readable report."""
    lines = ["# Molt Continuous Testing Report", ""]

    if not runs:
        lines.append("No test results found.")
        return "\n".join(lines)

    lines.extend(_section_overview(runs))
    lines.extend(_section_alerts(runs))
    lines.extend(_section_fuzzing(runs))
    lines.extend(_section_mutation(runs))
    lines.extend(_section_model_based(runs))
    lines.extend(_section_translation(runs))

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate human-readable reports from Molt test runs.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  python tools/test_report.py\n"
            "  python tools/test_report.py --last 5\n"
            "  python tools/test_report.py --compare 2026-03-10 2026-03-12\n"
        ),
    )
    parser.add_argument(
        "--last",
        type=int,
        default=7,
        help="Show last N reports (default: 7)",
    )
    parser.add_argument(
        "--compare",
        nargs=2,
        metavar=("DATE_A", "DATE_B"),
        help="Compare two specific run dates (YYYY-MM-DD)",
    )
    parser.add_argument(
        "--report-dir",
        type=str,
        default=None,
        help="Override report directory",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Output raw JSON data instead of markdown",
    )

    args = parser.parse_args()
    reports_dir = _reports_root(args.report_dir)

    if not reports_dir.exists():
        print(f"No reports directory found at: {reports_dir}", file=sys.stderr)
        print(
            "Run tools/nightly_test_suite.py first to generate results.",
            file=sys.stderr,
        )
        return 1

    if args.compare:
        runs = load_runs(reports_dir, dates=args.compare)
        if len(runs) < 2:
            found = [r.date for r in runs]
            print(
                f"Need 2 runs to compare, found {len(runs)}: {found}",
                file=sys.stderr,
            )
            return 1
        # Sort so a is earlier
        runs.sort(key=lambda r: r.date)
        report = "\n".join(_compare_runs(runs[0], runs[1]))
    else:
        runs = load_runs(reports_dir, last=args.last)
        if not runs:
            print(f"No test results found in: {reports_dir}", file=sys.stderr)
            return 1

        if args.json_output:
            data = [r.data for r in runs]
            print(json.dumps(data, indent=2))
            return 0

        report = generate_full_report(runs)

    print(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())

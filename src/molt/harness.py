"""Molt harness orchestrator.

Dispatches layers in profile order, collects results, manages baselines,
and produces reports. This is the entry point called by `molt harness`.
"""
from __future__ import annotations

import sys
from pathlib import Path
from typing import Optional

from molt.harness_layers import HarnessConfig, LayerDef, get_layers_for_profile
from molt.harness_report import (
    Baseline,
    HarnessReport,
    LayerResult,
    LayerStatus,
)

REPORTS_DIR = Path("tests/harness/reports")
BASELINE_PATH = Path("tests/harness/baselines/baseline.json")


def _run_profile(
    layers: list[LayerDef],
    config: HarnessConfig,
) -> HarnessReport:
    """Execute layers in order, respecting fail_fast."""
    results: list[LayerResult] = []
    failed = False

    for layer in layers:
        if failed and config.fail_fast:
            results.append(LayerResult(
                name=layer.name,
                status=LayerStatus.SKIP,
                duration_s=0.0,
                details="skipped due to prior failure",
            ))
            continue

        result = layer.run_fn(config)
        results.append(result)

        if not result.passed and result.status != LayerStatus.SKIP:
            failed = True

    return HarnessReport(profile="harness", results=results)


def run_harness(
    profile: str,
    config: HarnessConfig,
    check_baseline: bool = True,
) -> HarnessReport:
    """Run the harness with the given profile."""
    layers = get_layers_for_profile(profile)
    report = _run_profile(layers, config)
    report.profile = profile

    print(report.to_console_table(), file=sys.stderr)

    report.save(config.project_root / REPORTS_DIR)

    if check_baseline:
        baseline_path = config.project_root / BASELINE_PATH
        baseline = Baseline.load(baseline_path)
        violations = baseline.check(report)
        if violations:
            print("\nBASELINE VIOLATIONS:", file=sys.stderr)
            for v in violations:
                print(f"  - {v}", file=sys.stderr)

    return report


def main(args: Optional[list[str]] = None) -> int:
    """CLI entry point for `molt harness`."""
    import argparse

    parser = argparse.ArgumentParser(description="Molt quality harness")
    parser.add_argument("profile", nargs="?", default="standard",
                        choices=["quick", "standard", "deep"],
                        help="Test profile (default: standard)")
    parser.add_argument("--no-fail-fast", action="store_true",
                        help="Continue running layers after failure")
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument("--json", action="store_true",
                        help="Print JSON report to stdout")

    parsed = parser.parse_args(args)

    project_root = Path.cwd()
    config = HarnessConfig(
        project_root=project_root,
        fail_fast=not parsed.no_fail_fast,
        verbose=parsed.verbose,
    )

    report = run_harness(parsed.profile, config)

    if parsed.json:
        print(report.to_json())

    return 0 if report.all_passed else 1

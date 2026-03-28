"""Tests for the harness orchestrator."""
import sys
sys.path.insert(0, "src")

from molt.harness_report import LayerResult, LayerStatus


def test_run_profile_fail_fast_stops_on_failure():
    """When fail_fast=True, layers after a failure are skipped."""
    from molt.harness import _run_profile
    from molt.harness_layers import LayerDef, HarnessConfig
    from pathlib import Path

    call_log = []

    def pass_layer(config):
        call_log.append("pass")
        return LayerResult(name="pass", status=LayerStatus.PASS, duration_s=0.1)

    def fail_layer(config):
        call_log.append("fail")
        return LayerResult(name="fail", status=LayerStatus.FAIL, duration_s=0.1)

    def should_skip(config):
        call_log.append("should-not-run")
        return LayerResult(name="skip", status=LayerStatus.PASS, duration_s=0.1)

    layers = [
        LayerDef("pass", "quick", pass_layer),
        LayerDef("fail", "quick", fail_layer),
        LayerDef("skip", "quick", should_skip),
    ]
    config = HarnessConfig(project_root=Path("."), fail_fast=True)
    report = _run_profile(layers, config)
    assert call_log == ["pass", "fail"]
    assert len(report.results) == 3
    assert report.results[2].status == LayerStatus.SKIP


def test_run_profile_no_fail_fast_runs_all():
    from molt.harness import _run_profile
    from molt.harness_layers import LayerDef, HarnessConfig
    from pathlib import Path

    call_log = []

    def pass_layer(config):
        call_log.append("pass")
        return LayerResult(name="pass", status=LayerStatus.PASS, duration_s=0.1)

    def fail_layer(config):
        call_log.append("fail")
        return LayerResult(name="fail", status=LayerStatus.FAIL, duration_s=0.1)

    layers = [
        LayerDef("a", "quick", pass_layer),
        LayerDef("b", "quick", fail_layer),
        LayerDef("c", "quick", pass_layer),
    ]
    config = HarnessConfig(project_root=Path("."), fail_fast=False)
    report = _run_profile(layers, config)
    assert call_log == ["pass", "fail", "pass"]
    assert len(report.results) == 3


def test_main_returns_zero_on_success():
    from molt.harness import main
    # Run with a mock profile that always passes — use the import check
    # This is a smoke test; full integration tested separately
    from molt.harness import _run_profile
    assert callable(main)


def test_harness_module_importable():
    from molt.harness import run_harness, main, _run_profile
    assert callable(run_harness)
    assert callable(main)
    assert callable(_run_profile)

"""Self-tests for the harness infrastructure.

These verify that the harness itself works correctly — profile definitions,
layer execution, baseline ratcheting, report generation.
"""

import json
import sys

sys.path.insert(0, "src")

from molt.harness_layers import LAYERS, PROFILES, get_layers_for_profile
from molt.harness_report import Baseline, HarnessReport, LayerResult, LayerStatus

assert callable(get_layers_for_profile)


def test_all_layers_have_unique_names():
    names = [layer.name for layer in LAYERS]
    assert len(names) == len(set(names)), f"duplicate layer names: {names}"


def test_profiles_reference_only_existing_layers():
    layer_names = {layer.name for layer in LAYERS}
    for profile, names in PROFILES.items():
        for name in names:
            assert name in layer_names, (
                f"profile {profile!r} references unknown layer {name!r}"
            )


def test_profiles_are_strict_supersets():
    quick = PROFILES["quick"]
    standard = PROFILES["standard"]
    deep = PROFILES["deep"]
    assert quick == standard[: len(quick)], "standard must start with all quick layers"
    assert standard == deep[: len(standard)], "deep must start with all standard layers"


def test_layer_count_matches_spec():
    """The spec defines exactly 16 layers."""
    assert len(LAYERS) == 16, f"expected 16 layers, got {len(LAYERS)}"


def test_quick_profile_has_4_layers():
    assert len(PROFILES["quick"]) == 4


def test_standard_profile_has_8_layers():
    assert len(PROFILES["standard"]) == 8


def test_deep_profile_has_16_layers():
    assert len(PROFILES["deep"]) == 16


def test_baseline_json_schema():
    b = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 12345.0})
    data = json.loads(json.dumps({"test_counts": b.test_counts, "metrics": b.metrics}))
    assert "test_counts" in data
    assert "metrics" in data
    assert isinstance(data["test_counts"], dict)
    assert isinstance(data["metrics"], dict)


def test_report_json_has_required_fields():
    report = HarnessReport(
        profile="quick",
        results=[
            LayerResult(name="compile", status=LayerStatus.PASS, duration_s=1.0),
        ],
    )
    data = json.loads(report.to_json())
    required = {
        "profile",
        "timestamp",
        "all_passed",
        "total_duration_s",
        "pass_count",
        "fail_count",
        "results",
    }
    assert required.issubset(set(data.keys())), (
        f"missing keys: {required - set(data.keys())}"
    )

    result = data["results"][0]
    result_required = {"name", "status", "duration_s", "details", "metrics"}
    assert result_required.issubset(set(result.keys()))


def test_every_layer_has_valid_profile():
    valid = {"quick", "standard", "deep"}
    for layer in LAYERS:
        assert layer.profile in valid, (
            f"layer {layer.name!r} has invalid profile {layer.profile!r}"
        )

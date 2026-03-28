"""Tests for individual harness layer implementations."""
import sys
sys.path.insert(0, "src")

from molt.harness_layers import (
    LAYERS,
    PROFILES,
    get_layers_for_profile,
)
from molt.harness_report import LayerStatus


def test_profiles_are_supersets():
    quick = set(l.name for l in get_layers_for_profile("quick"))
    standard = set(l.name for l in get_layers_for_profile("standard"))
    deep = set(l.name for l in get_layers_for_profile("deep"))
    assert quick.issubset(standard)
    assert standard.issubset(deep)


def test_quick_has_four_layers():
    layers = get_layers_for_profile("quick")
    assert [l.name for l in layers] == ["compile", "lint", "unit-rust", "unit-python"]


def test_standard_adds_four_layers():
    layers = get_layers_for_profile("standard")
    names = [l.name for l in layers]
    assert "wasm-compile" in names
    assert "differential" in names
    assert "resource" in names
    assert "audit" in names


def test_deep_adds_remaining_layers():
    layers = get_layers_for_profile("deep")
    names = [l.name for l in layers]
    for expected in ["fuzz", "conformance", "bench", "size", "mutation",
                     "determinism", "miri", "compile-fail"]:
        assert expected in names, f"missing layer: {expected}"


def test_layer_definitions_have_required_fields():
    for layer in LAYERS:
        assert layer.name, "layer must have a name"
        assert layer.profile in ("quick", "standard", "deep"), f"bad profile: {layer.profile}"
        assert callable(layer.run_fn), f"layer {layer.name} must have a callable run_fn"

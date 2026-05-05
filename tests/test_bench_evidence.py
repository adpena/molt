from __future__ import annotations

from tools.bench_evidence import (
    comparator_time,
    metric_is_comparable,
    native_molt_speedup,
    native_molt_time,
    valid_positive_number,
    validated_runtime_samples,
    wasm_molt_time,
)


def test_valid_positive_number_rejects_invalid_values() -> None:
    for value in (None, True, False, "1.0", 0, -1, float("nan"), float("inf")):
        assert valid_positive_number(value) is None

    assert valid_positive_number(1) == 1.0
    assert valid_positive_number(1.25) == 1.25


def test_native_molt_evidence_requires_success_gate() -> None:
    failed = {
        "molt_ok": False,
        "molt_time_s": 0.01,
        "molt_speedup": 100.0,
    }
    ok = {
        "molt_ok": True,
        "molt_time_s": 0.25,
        "molt_speedup": 4.0,
    }

    assert native_molt_time(failed) is None
    assert native_molt_speedup(failed) is None
    assert native_molt_time(ok) == 0.25
    assert native_molt_speedup(ok) == 4.0


def test_wasm_molt_evidence_requires_success_gate() -> None:
    assert wasm_molt_time({"molt_wasm_ok": False, "molt_wasm_time_s": 0.01}) is None
    assert wasm_molt_time({"molt_wasm_ok": True, "molt_wasm_time_s": 0.01}) == 0.01
    assert wasm_molt_time({"molt_wasm_ok": True, "molt_wasm_time_s": 0.0}) is None


def test_metric_comparability_uses_all_required_gates() -> None:
    assert metric_is_comparable({"molt_ok": True}, "molt_time_s")
    assert not metric_is_comparable({"molt_ok": False}, "molt_time_s")
    assert metric_is_comparable(
        {"molt_ok": True, "codon_ok": True},
        "molt_codon_ratio",
    )
    assert not metric_is_comparable(
        {"molt_ok": True, "codon_ok": False},
        "molt_codon_ratio",
    )
    assert metric_is_comparable({}, "molt_build_s")


def test_comparator_time_requires_lane_success_gate() -> None:
    assert comparator_time({"codon_ok": False, "codon_time_s": 0.01}, "codon") is None
    assert comparator_time({"codon_ok": True, "codon_time_s": 0.01}, "codon") == 0.01


def test_validated_runtime_samples_prefers_top_level_samples() -> None:
    entry = {
        "molt_ok": True,
        "molt_samples_s": [0.9, 1.0],
        "super_stats": {"molt": {"samples_s": [9.0]}},
    }

    assert validated_runtime_samples(entry) == [0.9, 1.0]


def test_validated_runtime_samples_accepts_legacy_super_stats() -> None:
    entry = {
        "molt_ok": True,
        "super_stats": {"molt": {"samples_s": [1.1, 1.2]}},
    }

    assert validated_runtime_samples(entry) == [1.1, 1.2]


def test_validated_runtime_samples_requires_success_gate() -> None:
    assert validated_runtime_samples({"molt_ok": False, "molt_samples_s": [0.1]}) is None

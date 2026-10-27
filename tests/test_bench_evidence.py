from __future__ import annotations

import pytest

from tests.tools.perf_scoreboard_fixtures import synthetic_workload
from tools.bench_evidence import (
    comparable_run_metadata_errors,
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


def test_comparable_run_metadata_rejects_cold_warm_mixes() -> None:
    current = {"timing_mode": "warm_throughput", "warmup": 1, "samples": 3}
    baseline = {"timing_mode": "cold_first_run", "warmup": 0, "samples": 3}

    assert comparable_run_metadata_errors(current, baseline) == [
        "timing_mode differs: current='warm_throughput', baseline='cold_first_run'",
        "warmup differs: current=1, baseline=0",
    ]


def test_comparable_run_metadata_rejects_legacy_artifacts_without_mode() -> None:
    assert comparable_run_metadata_errors(
        {"timing_mode": "warm_throughput", "warmup": 1, "samples": 3},
        {"warmup": 1, "samples": 3},
    ) == ["baseline missing timing_mode"]


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
    assert (
        validated_runtime_samples({"molt_ok": False, "molt_samples_s": [0.1]}) is None
    )


def _profile_bound_cell(
    runtime="release-output", *, compiler="release", target="native", guest="release"
):
    return {
        "workload": synthetic_workload("sample.py"),
        "molt_ok": True,
        "molt_cpython_ratio": 1.0,
        "requested_profile": guest,
        "build_observation": {
            "kind": "molt-build-observation-v1",
            "compiled_with_verified": False,
            "selected_profiles": {
                "backend": "native",
                "target": target,
                "guest_profile": guest,
                "runtime_profile": runtime,
                "compiler_profile": compiler,
            },
        },
    }


def _profile_run(cell):
    return {
        "timing_mode": "warm_throughput",
        "warmup": 1,
        "samples": 3,
        "benchmarks": {
            "sample.py": {"workload": synthetic_workload("sample.py"), **cell}
        },
    }


def test_profile_observation_never_resolves_ambient_or_logical_labels(monkeypatch):
    from tools.bench_evidence import (
        observed_build_profiles,
        format_observed_build_profiles,
    )
    from molt.cli.cargo_profiles import _resolve_cargo_profile_name_cached

    # Independent live policy confirms why a logical release label is not a
    # release-fast witness. The observation remains authoritative for selection
    # even when a later process environment requests something else.
    assert _resolve_cargo_profile_name_cached("release", "")[0] == "release-output"
    monkeypatch.setenv("MOLT_RELEASE_CARGO_PROFILE", "release-fast")
    cell = _profile_bound_cell()
    facts = observed_build_profiles(cell["build_observation"])
    assert facts["guest_profile"] == "release"
    assert facts["runtime_profile"] == "release-output"
    assert facts["compiler_profile"] == "release"
    assert "runtime=release-output" in format_observed_build_profiles(cell)
    assert cell["build_observation"]["compiled_with_verified"] is False


def test_profile_observation_requires_complete_producer_shape():
    from tools.bench_evidence import observed_build_profiles

    good = _profile_bound_cell()["build_observation"]
    assert observed_build_profiles(good) is not None
    assert observed_build_profiles(None) is None
    assert (
        observed_build_profiles({"selected_profiles": good["selected_profiles"]})
        is None
    )
    for key in good["selected_profiles"]:
        facts = dict(good["selected_profiles"])
        facts.pop(key)
        assert observed_build_profiles({**good, "selected_profiles": facts}) is None
    assert (
        observed_build_profiles(
            {
                **good,
                "selected_profiles": {
                    **good["selected_profiles"],
                    "invented": "release-fast",
                },
            }
        )
        is None
    )
    for value in (None, False, "", " release-fast "):
        assert (
            observed_build_profiles(
                {
                    **good,
                    "selected_profiles": {
                        **good["selected_profiles"],
                        "runtime_profile": value,
                    },
                }
            )
            is None
        )


def test_profile_comparison_refuses_same_request_with_different_physical_results():
    for changed in (
        _profile_bound_cell("release-fast"),
        _profile_bound_cell(compiler="dev-fast"),
        _profile_bound_cell(target="x86_64-unknown-linux-gnu"),
    ):
        errors = comparable_run_metadata_errors(
            _profile_run(changed), _profile_run(_profile_bound_cell())
        )
        assert len(errors) == 1 and "selected build profiles differ" in errors[0]
    assert (
        comparable_run_metadata_errors(
            _profile_run(_profile_bound_cell()), _profile_run(_profile_bound_cell())
        )
        == []
    )


def test_profile_comparison_refuses_historical_labels_without_observation():
    legacy = {"molt_ok": True, "molt_cpython_ratio": 1.0}
    old_run = {**_profile_run(legacy), "provenance": {"profile": "release-fast"}}
    errors = comparable_run_metadata_errors(
        _profile_run(_profile_bound_cell()), old_run
    )
    assert errors == ["sample.py: baseline selected profiles unbound"]
    assert comparable_run_metadata_errors(old_run, old_run) == [
        "sample.py: current selected profiles unbound",
        "sample.py: baseline selected profiles unbound",
    ]
    contradictory = {**_profile_bound_cell(), "requested_profile": "dev"}
    assert (
        "requested guest profile differs"
        in comparable_run_metadata_errors(
            _profile_run(contradictory), _profile_run(_profile_bound_cell())
        )[0]
    )


def test_profile_policy_binds_identical_observation_to_actual_metric_lane():
    from tools.bench_evidence import comparable_metric_errors

    for metric, target, required in (
        ("molt_wasm_time_s", "native", "wasm"),
        ("molt_time_s", "wasm", "native"),
        ("molt_time_s", "rust", "native"),
        ("molt_wasm_time_s", "luau", "wasm"),
    ):
        cell = {
            "workload": synthetic_workload("sample.py"),
            metric: 1.0,
            "molt_ok": True,
            "molt_wasm_ok": True,
            "build_observation": _profile_bound_cell(target=target)[
                "build_observation"
            ],
        }
        run = {"benchmarks": {"sample.py": cell}}
        errors = comparable_metric_errors(run, run, metric=metric)
        assert errors and all(
            f"requires {required} target" in error for error in errors
        )


def test_profile_policy_preserves_native_triples_and_wasm_intent_variants():
    from tools.bench_evidence import comparable_metric_errors

    for metric, target in (
        ("molt_time_s", "native"),
        ("molt_time_s", "x86_64-unknown-linux-gnu"),
        ("molt_wasm_time_s", "wasm"),
        ("molt_wasm_time_s", "wasm-freestanding"),
        ("molt_wasm_time_s", "wasm32-wasip1"),
        ("molt_wasm_time_s", "wasm32-unknown-unknown"),
    ):
        cell = {
            "workload": synthetic_workload("sample.py"),
            metric: 1.0,
            "build_observation": _profile_bound_cell(target=target)[
                "build_observation"
            ],
        }
        run = {"benchmarks": {"sample.py": cell}}
        assert comparable_metric_errors(run, run, metric=metric) == []


def test_profile_policy_refuses_one_observation_for_mixed_native_wasm_cell():
    from tools.bench_evidence import comparable_metric_errors

    cell = {
        "workload": synthetic_workload("sample.py"),
        **_profile_bound_cell(),
        "molt_time_s": 1.0,
        "molt_wasm_time_s": 2.0,
    }
    run = {"benchmarks": {"sample.py": cell}}
    for metric in ("molt_time_s", "molt_wasm_time_s"):
        errors = comparable_metric_errors(run, run, metric=metric)
        assert errors and all(
            "mixed native/WASM metric cell" in error for error in errors
        )
    assert comparable_metric_errors(run, run, metric="molt_invented_time_s") == [
        "unregistered Molt comparison metric: molt_invented_time_s"
    ]


@pytest.mark.parametrize(
    "metric",
    [
        "cpython_time_s",
        "pypy_time_s",
        "codon_time_s",
        "nuitka_time_s",
        "pyodide_time_s",
    ],
)
@pytest.mark.parametrize("changed", ["missing", "bytes", "sibling", "argv", "entry"])
def test_external_only_comparison_requires_the_same_admitted_workload(metric, changed):
    from copy import deepcopy
    from tools.bench_evidence import comparable_metric_errors

    old = {"workload": synthetic_workload("program.py"), metric: 1.0}
    new = deepcopy(old)
    if changed == "missing":
        new.pop("workload")
    elif changed == "bytes":
        new["workload"]["sources"][0]["sha256"] = "a" * 64
    elif changed == "sibling":
        new["workload"]["sources"].append(
            {"path": "sibling.py", "size": 0, "sha256": "b" * 64}
        )
    elif changed == "argv":
        new["workload"]["run_args"] = ["different"]
    else:
        new["workload"]["entry"] = "other.py"
    current, baseline = (
        {"benchmarks": {"program.py": new}},
        {"benchmarks": {"program.py": old}},
    )
    assert comparable_metric_errors(current, baseline, metric=metric)
    assert comparable_metric_errors(baseline, baseline, metric=metric) == []


@pytest.mark.parametrize("metric", ["cpython_time_s", "molt_time_s"])
@pytest.mark.parametrize("role", ["current", "baseline"])
def test_collection_identity_refusal_precedes_canonical_key_intersection(metric, role):
    from tools.bench_evidence import comparable_metric_errors

    current = {
        "benchmarks": {
            "tests/benchmarks/bench_sum.py": {
                "workload": synthetic_workload("tests/benchmarks/bench_sum.py"),
                metric: 1.0,
            }
        }
    }
    baseline = {"benchmarks": {"bench_sum.py": {metric: 1.0}}}
    if role == "current":
        current, baseline = baseline, current
    errors = comparable_metric_errors(current, baseline, metric=metric)
    assert errors and any(role in error and "workload" in error for error in errors)

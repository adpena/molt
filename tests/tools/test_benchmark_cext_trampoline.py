from __future__ import annotations

import hashlib
import json
import math

import pytest

from tools import benchmark_cext_trampoline as bench


def _sample(baseline: float, admitted: float) -> dict[str, object]:
    return {
        "candidate": "admitted",
        "baseline_ns_per_call": baseline,
        "candidate_ns_per_call": admitted,
    }


def test_parse_sample_requires_exactly_one_valid_record() -> None:
    payload = {
        "candidate": "checked-nested",
        "baseline_ns_per_call": 10.0,
        "candidate_ns_per_call": 9.0,
        "pair_rounds": 2,
        "baseline_rounds_ns_per_call": [10.1, 9.9],
        "candidate_rounds_ns_per_call": [9.1, 8.9],
    }
    assert bench.parse_sample(
        "running 1 test\ntest module::bench ... "
        + bench.SAMPLE_PREFIX
        + json.dumps(payload)
        + "\nok\n"
    ) == payload
    with pytest.raises(ValueError, match="exactly one"):
        bench.parse_sample("noise only")
    with pytest.raises(ValueError, match="invalid benchmark field"):
        bench.parse_sample(
            bench.SAMPLE_PREFIX
            + json.dumps(
                {
                    "candidate": "admitted",
                    "baseline_ns_per_call": math.nan,
                    "candidate_ns_per_call": 1,
                }
            )
        )


def test_one_sided_ratio_ucb_is_paired_log_student_t() -> None:
    samples = [_sample(100.0, value) for value in (98.0, 99.0, 98.5, 99.2, 98.8)]
    result = bench.one_sided_ratio_ucb(samples)
    assert result["student_t_critical"] == pytest.approx(2.132)
    assert result["geometric_mean_delta_pct"] < 0.0
    assert result["one_sided_95_ucb_delta_pct"] < 0.0
    with pytest.raises(ValueError, match="at least five"):
        bench.one_sided_ratio_ucb(samples[:4])


def test_executable_identity_binds_path_size_and_bytes(tmp_path) -> None:
    executable = tmp_path / "runtime-test-bin"
    executable.write_bytes(b"first")
    identity = bench.executable_identity(executable)
    assert identity == {
        "path": str(executable.resolve()),
        "size_bytes": 5,
        "sha256": hashlib.sha256(b"first").hexdigest(),
    }
    executable.write_bytes(b"other")
    assert bench.executable_identity(executable) != identity


def test_sample_process_contract_requires_guarded_exact_child(monkeypatch) -> None:
    sample = {
        "process_execution_contract": {
            "pid": 123,
            "logical_cpu": 7,
            "affinity_mask": 128,
            "priority_class": 0x8000,
            "verified_before_warmup": True,
        }
    }
    isolation = {
        "child_logical_cpu": 7,
        "child_affinity_mask": 128,
        "child_priority_class": 0x8000,
    }
    monkeypatch.setattr(bench.sys, "platform", "win32")
    bench.validate_sample_process_contract(sample, isolation, {"pid": 123})
    sample["process_execution_contract"]["pid"] = 456
    with pytest.raises(RuntimeError, match="guard custody"):
        bench.validate_sample_process_contract(sample, isolation, {"pid": 123})


def test_unsupported_platform_writes_explicit_non_admission(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setattr(bench.sys, "platform", "darwin")
    monkeypatch.delattr(bench.os, "sched_getaffinity", raising=False)
    monkeypatch.delattr(bench.os, "sched_setaffinity", raising=False)
    monkeypatch.setattr(
        bench,
        "_discover_release_test_binary",
        lambda *_args, **_kwargs: pytest.fail("non-admitted platform must not build"),
    )
    output = tmp_path / "non-admission.json"

    payload = bench.benchmark(
        samples=5,
        iterations=120_000,
        threshold_pct=1.0,
        build_timeout=1.0,
        sample_timeout=1.0,
        output=output,
    )

    assert payload["admitted"] is False
    assert payload["status"] == "not-admitted"
    assert payload["platform_admission"]["platform"] == "darwin"
    assert json.loads(output.read_text(encoding="utf-8")) == payload

from __future__ import annotations

import json

from molt.harness_report import Baseline, HarnessReport, LayerResult, LayerStatus


def test_layer_result_pass():
    r = LayerResult(name="syntax", status=LayerStatus.PASS, duration_s=0.5)
    assert r.passed is True
    assert r.status == LayerStatus.PASS


def test_layer_result_fail_with_details():
    r = LayerResult(
        name="wasm_link",
        status=LayerStatus.FAIL,
        duration_s=1.2,
        details="missing export: __main__",
    )
    assert r.passed is False
    assert r.details == "missing export: __main__"


def test_layer_result_skip():
    r = LayerResult(name="native", status=LayerStatus.SKIP, duration_s=0.0)
    assert r.passed is False


def test_harness_report_all_pass():
    results = [
        LayerResult(name="syntax", status=LayerStatus.PASS, duration_s=0.3),
        LayerResult(name="wasm_link", status=LayerStatus.PASS, duration_s=0.7),
    ]
    report = HarnessReport(profile="ci", results=results)
    assert report.all_passed is True
    assert report.total_duration_s == 1.0
    assert report.pass_count == 2
    assert report.fail_count == 0


def test_harness_report_with_failure():
    results = [
        LayerResult(name="syntax", status=LayerStatus.PASS, duration_s=0.3),
        LayerResult(name="wasm_link", status=LayerStatus.FAIL, duration_s=0.7),
    ]
    report = HarnessReport(profile="ci", results=results)
    assert report.all_passed is False
    assert report.fail_count == 1
    assert report.pass_count == 1


def test_harness_report_to_json():
    results = [
        LayerResult(name="syntax", status=LayerStatus.PASS, duration_s=0.3),
        LayerResult(name="wasm_link", status=LayerStatus.FAIL, duration_s=0.7, details="bad"),
    ]
    report = HarnessReport(profile="dev", results=results, timestamp="2026-03-28T00:00:00")
    raw = report.to_json()
    data = json.loads(raw)
    assert data["profile"] == "dev"
    assert data["all_passed"] is False
    assert data["total_duration_s"] == 1.0
    assert data["pass_count"] == 1
    assert data["fail_count"] == 1
    assert len(data["results"]) == 2
    assert data["results"][0]["name"] == "syntax"
    assert data["results"][0]["status"] == "pass"
    assert data["results"][1]["name"] == "wasm_link"
    assert data["results"][1]["status"] == "fail"
    assert data["timestamp"] == "2026-03-28T00:00:00"


def test_harness_report_console_table():
    results = [
        LayerResult(name="syntax", status=LayerStatus.PASS, duration_s=0.3),
        LayerResult(name="wasm_link", status=LayerStatus.FAIL, duration_s=1.234, details="oops"),
    ]
    report = HarnessReport(profile="ci", results=results)
    table = report.to_console_table()
    assert "syntax" in table
    assert "PASS" in table
    assert "wasm_link" in table
    assert "FAIL" in table
    assert "1.23" in table
    assert "oops" in table


def test_harness_report_metrics():
    metrics = {"wasm_size_kb": 1024, "import_count": 42}
    r = LayerResult(
        name="wasm_link",
        status=LayerStatus.PASS,
        duration_s=0.5,
        metrics=metrics,
    )
    results = [r]
    report = HarnessReport(profile="ci", results=results, timestamp="2026-03-28T00:00:00")
    data = json.loads(report.to_json())
    assert data["results"][0]["metrics"]["wasm_size_kb"] == 1024
    assert data["results"][0]["metrics"]["import_count"] == 42


def test_baseline_empty():
    b = Baseline.empty()
    assert b.test_counts == {}
    assert b.metrics == {}


def test_baseline_from_report():
    report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 40}),
        LayerResult(name="bench", status=LayerStatus.PASS, duration_s=60.0,
                    metrics={"fib_30_ns": 12345, "binary_size_bytes": 4096}),
    ])
    b = Baseline.from_report(report)
    assert b.test_counts["unit-rust"] == 40
    assert b.metrics["fib_30_ns"] == 12345


def test_baseline_ratchet_raises_floor():
    old = Baseline(test_counts={"unit-rust": 30}, metrics={"fib_30_ns": 15000})
    new_report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 40}),
        LayerResult(name="bench", status=LayerStatus.PASS, duration_s=60.0,
                    metrics={"fib_30_ns": 12000}),
    ])
    updated = old.ratchet(new_report)
    assert updated.test_counts["unit-rust"] == 40
    assert updated.metrics["fib_30_ns"] == 12000


def test_baseline_ratchet_never_lowers():
    old = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 10000})
    worse_report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 35}),
        LayerResult(name="bench", status=LayerStatus.PASS, duration_s=60.0,
                    metrics={"fib_30_ns": 15000}),
    ])
    updated = old.ratchet(worse_report)
    assert updated.test_counts["unit-rust"] == 40
    assert updated.metrics["fib_30_ns"] == 10000


def test_baseline_check_violations():
    baseline = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 10000})
    report = HarnessReport(profile="deep", results=[
        LayerResult(name="unit-rust", status=LayerStatus.PASS, duration_s=4.0,
                    metrics={"test_count": 38}),
    ])
    violations = baseline.check(report)
    assert len(violations) == 1
    assert "unit-rust" in violations[0]
    assert "40" in violations[0]


def test_baseline_save_load_roundtrip(tmp_path):
    b = Baseline(test_counts={"unit-rust": 40}, metrics={"fib_30_ns": 12345})
    path = tmp_path / "baseline.json"
    b.save(path)
    loaded = Baseline.load(path)
    assert loaded.test_counts == b.test_counts
    assert loaded.metrics == b.metrics

from __future__ import annotations

import datetime as dt
import importlib.util
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOL = REPO_ROOT / "tools" / "release_exit_gate.py"


def _load_gate():
    spec = importlib.util.spec_from_file_location("molt_release_exit_gate", TOOL)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _evidence(tmp_path: Path, name: str) -> dict[str, str]:
    path = tmp_path / f"{name}.json"
    path.write_text(json.dumps({"ok": True}) + "\n", encoding="utf-8")
    return {
        "path": path.name,
        "command": f"prove {name}",
        "summary": f"{name} passed",
    }


def _scoreboard_evidence(
    tmp_path: Path,
    gate,
    name: str = "E2",
    *,
    generated_at: str | None = None,
    gate_fails: bool = False,
    authoritative: bool = True,
) -> dict[str, str]:
    schema = gate.perf_schema
    cell: dict[str, object] = {
        "benchmark": "tests/benchmarks/bench_fib.py",
        "target": "native",
        "backend": "native",
        "profile": "release-fast",
        "build_ok": True,
        "run_blocked": False,
        "molt_ok": True,
        "cpython_ok": True,
        "cold_molt_s": 0.12,
        "cold_cpython_s": 0.24,
        "warm_molt_s": 0.10,
        "warm_cpython_s": 0.20,
        "warm_speedup": 2.0,
        "cold_speedup": 2.0,
        "startup_tax_ms": 5.0,
        "verdict": schema.VERDICT_GREEN,
        "binary_size_kib": 512.0,
        "molt_peak_rss_mib": 18.0,
        "compile_time_s": 0.4,
        "stable": True,
        "pypy_ratio": None,
        "codon_ratio": None,
        "codon_equivalent": None,
        "cpython_peak_rss_mib": 15.0,
        "output_parity": True,
        "log_artifact": "bench/scoreboard/logs/fib.log",
        "classification": schema.CLASS_GREEN,
    }
    path = tmp_path / f"{name}.json"
    stamp = generated_at or dt.datetime.now(dt.timezone.utc).isoformat()
    doc = {
        "schema_version": schema.SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": stamp,
        "git_rev": "a" * 40,
        "provenance": {
            "origin_sha": "a" * 40,
            "local_head_sha": "a" * 40,
            "merge_base_sha": "a" * 40,
            "dirty_tree": False,
            "benchmark_tool_sha": "b" * 40,
            "backend_binary_identity": {"native/release-fast": "sha|1|2"},
            "stdlib_cache_key": "cache",
            "authoritative": authoritative,
        },
        "host": {
            "platform": "test",
            "python_runner": "3.12.13",
            "cpython_baseline": "3.14.3",
        },
        "direction": "speedup = cpython_time / molt_time",
        "red_threshold": 1.0,
        "verdict_legend": {},
        "methodology": {},
        "reserved_columns": {},
        "summary": {
            "cells_fail_engine": 0,
            "cells_fail_cold_budget": 0,
            "cells_warn_cold_floor": 0,
            "cells_fail_stale": 0,
            "verdict_breakdown": {},
            "gate_fails": gate_fails,
        },
        "benchmarks_run": [cell["benchmark"]],
        "benchmarks_deferred": [],
        "scoreboard": {
            cell["benchmark"]: {
                cell["target"]: {cell["backend"]: {cell["profile"]: cell}}
            }
        },
    }
    path.write_text(json.dumps(doc), encoding="utf-8")
    return {
        "path": path.name,
        "command": "tools/perf_scoreboard.py --set core --backend native --backend llvm --profile release-fast --samples 5 --warmup 2 --repeat 5 --classify --require-quiescent",
        "summary": "canonical perf scoreboard passed",
    }


def _manifest(tmp_path: Path, gate, **overrides: object) -> dict[str, object]:
    criteria = {
        key: {"status": "pass", "evidence": [_evidence(tmp_path, key)]}
        for key in ("E1", "E2", "E3", "E4")
    }
    criteria["E2"] = {
        "status": "pass",
        "evidence": [_scoreboard_evidence(tmp_path, gate)],
    }
    criteria.update(overrides)
    return {"schema_version": 1, "criteria": criteria}


def test_release_exit_gate_passes_only_when_all_e1_e4_receipts_pass(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    manifest_path = tmp_path / "release-exit.json"
    manifest_path.write_text(json.dumps(_manifest(tmp_path, gate)), encoding="utf-8")

    report = gate.check_manifest_path(manifest_path)

    assert report.passed is True
    assert {result.criterion for result in report.criteria} == {"E1", "E2", "E3", "E4"}
    assert all(result.passed for result in report.criteria)


def test_release_exit_gate_fails_on_nonpassing_criterion(tmp_path: Path) -> None:
    gate = _load_gate()
    doc = _manifest(
        tmp_path,
        gate,
        E2={"status": "blocked", "evidence": [_evidence(tmp_path, "E2")]},
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E2: status is blocked, expected pass" in p for p in report.problems)


def test_release_exit_gate_fails_when_a_criterion_is_missing(tmp_path: Path) -> None:
    gate = _load_gate()
    doc = _manifest(tmp_path, gate)
    del doc["criteria"]["E4"]  # type: ignore[index]

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E4: missing criterion receipt" in p for p in report.problems)


def test_release_exit_gate_requires_evidence_for_passing_receipts(
    tmp_path: Path,
) -> None:
    gate = _load_gate()
    doc = _manifest(tmp_path, gate, E1={"status": "pass", "evidence": []})

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any(
        "E1: passing criteria require at least one evidence receipt" in p
        for p in report.problems
    )


def test_release_exit_gate_fails_on_missing_evidence_artifact(tmp_path: Path) -> None:
    gate = _load_gate()
    doc = _manifest(
        tmp_path,
        gate,
        E3={
            "status": "pass",
            "evidence": [
                {
                    "path": "missing-e3.json",
                    "command": "prove parity",
                    "summary": "claimed parity",
                }
            ],
        },
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E3: evidence artifact does not exist" in p for p in report.problems)


def test_release_exit_gate_requires_e2_canonical_scoreboard(tmp_path: Path) -> None:
    gate = _load_gate()
    doc = _manifest(
        tmp_path,
        gate,
        E2={"status": "pass", "evidence": [_evidence(tmp_path, "not-scoreboard")]},
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any(
        "E2: passing performance criteria require at least one canonical" in p
        for p in report.problems
    )


def test_release_exit_gate_rejects_stale_or_red_e2_scoreboard(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    old = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=90)).isoformat()
    doc = _manifest(
        tmp_path,
        gate,
        E2={
            "status": "pass",
            "evidence": [
                _scoreboard_evidence(
                    tmp_path,
                    gate,
                    name="stale-red-scoreboard",
                    generated_at=old,
                    gate_fails=True,
                )
            ],
        },
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E2: scoreboard gate_fails is not false" in p for p in report.problems)
    assert any("E2: scoreboard generated_at is 90d old" in p for p in report.problems)


def test_release_exit_gate_json_cli_reports_failures(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    manifest_path = tmp_path / "release-exit.json"
    manifest_path.write_text(
        json.dumps(_manifest(tmp_path, gate, E1={"status": "fail"})),
        encoding="utf-8",
    )

    rc = gate.main([str(manifest_path), "--json"])
    out = json.loads(capsys.readouterr().out)

    assert rc == 1
    assert out["passed"] is False
    assert any(
        item["criterion"] == "E1" and item["status"] == "fail"
        for item in out["criteria"]
    )

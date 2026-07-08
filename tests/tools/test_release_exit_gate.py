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
    # Release-exit unit tests exercise receipt shape, not git ancestry.
    # Dedicated perf-authority tests own the live ancestry probe.
    module.pa.git_rev_is_ancestor_of_origin = lambda _: True
    return module


def _evidence(tmp_path: Path, name: str) -> dict[str, str]:
    path = tmp_path / f"{name}.json"
    path.write_text(json.dumps({"ok": True}) + "\n", encoding="utf-8")
    return {
        "path": path.name,
        "command": f"prove {name}",
        "summary": f"{name} passed",
    }


def _baseline_metrics(gate, rel_path: str) -> dict[str, float]:
    baseline, error = gate._load_metric_baseline(rel_path)
    assert error is None
    assert baseline is not None
    return {str(k): float(v) for k, v in baseline.items()}


def _scoreboard_evidence(
    tmp_path: Path,
    gate,
    name: str = "E2",
    *,
    generated_at: str | None = None,
    gate_fails: bool = False,
    authoritative: bool = True,
    backends: tuple[str, ...] = ("native", "llvm"),
    profile: str = "release-fast",
    command: str | None = None,
    classify_active: bool = True,
) -> dict[str, str]:
    schema = gate.perf_schema
    benchmark = "tests/benchmarks/bench_fib.py"

    def cell_for(backend: str) -> dict[str, object]:
        return {
            "benchmark": benchmark,
            "target": "native",
            "backend": backend,
            "profile": profile,
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
            "log_artifact": f"bench/scoreboard/logs/fib-{backend}.log",
            "classification": schema.CLASS_GREEN,
        }

    cells = {backend: cell_for(backend) for backend in backends}
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
            "backend_binary_identity": {
                f"{backend}/{profile}": f"{backend}-sha|1|2"
                for backend in backends
            },
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
            "cells_total": len(cells),
            "cells_green": len(cells),
            "cells_fail_engine": 0,
            "cells_fail_cold_budget": 0,
            "cells_warn_cold_floor": 0,
            "cells_fail_stale": 0,
            "verdict_breakdown": {},
            "classify_active": classify_active,
            "classification_breakdown": {schema.CLASS_GREEN: []},
            "gate_fails": gate_fails,
        },
        "benchmarks_run": [benchmark],
        "benchmarks_deferred": [],
        "scoreboard": {
            benchmark: {
                "native": {backend: {profile: cell} for backend, cell in cells.items()}
            },
        },
    }
    path.write_text(json.dumps(doc), encoding="utf-8")
    return {
        "path": path.name,
        "command": command or gate.pa.CANONICAL_GATE,
        "summary": "canonical perf scoreboard passed",
    }


def _witness_evidence(
    tmp_path: Path,
    *,
    ok: bool = True,
    write_candidate: bool = True,
) -> dict[str, str]:
    candidate = tmp_path / "candidate_outputs.npz"
    if write_candidate:
        candidate.write_bytes(b"fake-npz-for-release-receipt-test")
    else:
        candidate.unlink(missing_ok=True)
    log = tmp_path / "pact-witness-acceptance.log"
    text = (
        f"candidate_outputs={candidate}\n"
        + ("pact witness acceptance PASS\n" if ok else "pact witness acceptance FAIL\n")
    )
    log.write_text(text, encoding="utf-8")
    return {
        "path": log.name,
        "command": "tools/proof_queue.py pact-witness-acceptance",
        "summary": "pact witness acceptance passed",
    }


def _parity_evidence(tmp_path: Path, *, ok: bool = True) -> dict[str, str]:
    path = tmp_path / "parity-gate.log"
    text = (
        "Parity Gate Summary: 3/3 passed\nPASS: No Tier 1 violations.\n"
        if ok
        else "Parity Gate Summary: 2/3 passed\nFAIL: 1 Tier 1 violation(s)\n"
    )
    path.write_text(text, encoding="utf-8")
    return {
        "path": path.name,
        "command": "tools/parity_gate.py tests/differential/basic/",
        "summary": "parity gate passed",
    }


def _canonicalization_evidence(
    tmp_path: Path,
    gate,
    *,
    metrics: dict[str, float] | None = None,
) -> dict[str, str]:
    path = tmp_path / "canonicalization-contract.json"
    doc = {
        "violations": [],
        "metrics": metrics
        or _baseline_metrics(gate, "tools/canonicalization_contract_baseline.json"),
    }
    path.write_text(json.dumps(doc), encoding="utf-8")
    return {
        "path": path.name,
        "command": "tools/canonicalization_contract.py --json",
        "summary": "canonicalization contract ratchet captured",
    }


def _structural_audit_evidence(
    tmp_path: Path,
    gate,
    *,
    metrics: dict[str, float] | None = None,
) -> dict[str, str]:
    path = tmp_path / "structural-audit.json"
    doc = {
        "findings": [],
        "metrics": metrics
        or _baseline_metrics(gate, "tools/structural_audit_baseline.json"),
    }
    path.write_text(json.dumps(doc), encoding="utf-8")
    return {
        "path": path.name,
        "command": "tools/structural_audit.py --json",
        "summary": "structural audit ratchet captured",
    }


def _degrade_to_slow_evidence(
    tmp_path: Path,
    *,
    ok: bool = True,
    pending: int = 0,
    baseline: int = 0,
) -> dict[str, str]:
    path = tmp_path / "degrade-to-slow.json"
    doc = {
        "ok": ok,
        "errors": [] if ok else ["forced failure"],
        "warnings": [],
        "registry_row_count": 4,
        "metabug_fix_pending_count": pending,
        "metabug_fix_pending_baseline": baseline,
        "discovered_site_count": 4,
    }
    path.write_text(json.dumps(doc), encoding="utf-8")
    return {
        "path": path.name,
        "command": "tools/degrade_to_slow_gate.py --json",
        "summary": "degrade-to-slow gate passed",
    }


def _fail_closed_evidence(tmp_path: Path, *, ok: bool = True) -> dict[str, str]:
    path = tmp_path / "fail-closed.log"
    text = (
        "fail-closed gate: OK (12 registered sites; fail_open_stub=0)\n"
        if ok
        else "fail-closed gate: FAIL\n"
    )
    path.write_text(text, encoding="utf-8")
    return {
        "path": path.name,
        "command": "tools/fail_closed_gate.py",
        "summary": "fail-closed poison gate passed",
    }


def _e4_evidence(tmp_path: Path, gate) -> list[dict[str, str]]:
    return [
        _canonicalization_evidence(tmp_path, gate),
        _structural_audit_evidence(tmp_path, gate),
        _degrade_to_slow_evidence(tmp_path),
        _fail_closed_evidence(tmp_path),
    ]


def _manifest(tmp_path: Path, gate, **overrides: object) -> dict[str, object]:
    criteria = {
        key: {"status": "pass", "evidence": [_evidence(tmp_path, key)]}
        for key in ("E1", "E2", "E3", "E4")
    }
    criteria["E1"] = {"status": "pass", "evidence": [_witness_evidence(tmp_path)]}
    criteria["E2"] = {
        "status": "pass",
        "evidence": [_scoreboard_evidence(tmp_path, gate)],
    }
    criteria["E3"] = {"status": "pass", "evidence": [_parity_evidence(tmp_path)]}
    criteria["E4"] = {"status": "pass", "evidence": _e4_evidence(tmp_path, gate)}
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


def test_release_exit_gate_requires_e2_canonical_scoreboard_command(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(
        tmp_path,
        gate,
        E2={
            "status": "pass",
            "evidence": [
                _scoreboard_evidence(
                    tmp_path,
                    gate,
                    name="noncanonical-command",
                    command=(
                        "tools/perf_scoreboard.py --backend native "
                        "--profile release-fast --classify"
                    ),
                )
            ],
        },
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any(
        "E2: evidence[0] canonical scoreboard command must include --set core" in p
        for p in report.problems
    )
    assert any(
        "E2: evidence[0] canonical scoreboard command missing canonical backends: llvm"
        in p
        for p in report.problems
    )
    assert any(
        "E2: evidence[0] canonical scoreboard command missing --require-quiescent"
        in p
        for p in report.problems
    )


def test_release_exit_gate_rejects_e2_native_only_scoreboard(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(
        tmp_path,
        gate,
        E2={
            "status": "pass",
            "evidence": [
                _scoreboard_evidence(
                    tmp_path,
                    gate,
                    name="native-only-scoreboard",
                    backends=("native",),
                )
            ],
        },
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any(
        "E2: canonical scoreboard missing backend binary identities: llvm/release-fast"
        in p
        for p in report.problems
    )
    assert any(
        "E2: canonical scoreboard must include both native and llvm release-fast cells"
        in p
        for p in report.problems
    )


def test_release_exit_gate_rejects_e2_unclassified_or_wrong_profile_scoreboard(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(
        tmp_path,
        gate,
        E2={
            "status": "pass",
            "evidence": [
                _scoreboard_evidence(
                    tmp_path,
                    gate,
                    name="wrong-profile-scoreboard",
                    profile="dev",
                    classify_active=False,
                )
            ],
        },
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any(
        "E2: canonical scoreboard must be generated with --classify" in p
        for p in report.problems
    )
    assert any(
        "E2: canonical scoreboard may only contain native+llvm release-fast cells"
        in p
        for p in report.problems
    )


def test_release_exit_gate_requires_e1_pact_witness_acceptance_receipt(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(
        tmp_path,
        gate,
        E1={"status": "pass", "evidence": [_evidence(tmp_path, "generic-e1")]},
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E1: witness green requires a pact-witness-acceptance" in p for p in report.problems)


def test_release_exit_gate_rejects_e1_missing_candidate_outputs(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(tmp_path, gate)
    doc["criteria"]["E1"] = {  # type: ignore[index]
        "status": "pass",
        "evidence": [_witness_evidence(tmp_path, write_candidate=False)],
    }

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E1: candidate_outputs artifact does not exist" in p for p in report.problems)


def test_release_exit_gate_requires_e3_parity_receipt(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(
        tmp_path,
        gate,
        E3={"status": "pass", "evidence": [_evidence(tmp_path, "generic-e3")]},
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E3: parity criteria require a tools/parity_gate.py receipt" in p for p in report.problems)


def test_release_exit_gate_rejects_failed_e3_parity_receipt(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(tmp_path, gate)
    doc["criteria"]["E3"] = {  # type: ignore[index]
        "status": "pass",
        "evidence": [_parity_evidence(tmp_path, ok=False)],
    }

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E3: parity receipt lacks PASS verdict" in p for p in report.problems)


def test_release_exit_gate_requires_e4_structural_floor_receipts(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(
        tmp_path,
        gate,
        E4={"status": "pass", "evidence": [_evidence(tmp_path, "generic-e4")]},
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any(
        "E4: structural floor requires canonicalization_contract" in p
        for p in report.problems
    )
    assert any("fail_closed_gate" in p for p in report.problems)


def test_release_exit_gate_rejects_e4_structural_metric_regression(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    metrics = _baseline_metrics(gate, "tools/canonicalization_contract_baseline.json")
    metrics["misplaced_module_lines"] += 1.0
    doc = _manifest(tmp_path, gate)
    doc["criteria"]["E4"] = {  # type: ignore[index]
        "status": "pass",
        "evidence": [
            _canonicalization_evidence(tmp_path, gate, metrics=metrics),
            _structural_audit_evidence(tmp_path, gate),
            _degrade_to_slow_evidence(tmp_path),
            _fail_closed_evidence(tmp_path),
        ],
    }

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any(
        "E4: canonicalization_contract metric 'misplaced_module_lines' regressed"
        in p
        for p in report.problems
    )


def test_release_exit_gate_rejects_failed_e4_gate_receipts(
    tmp_path: Path, monkeypatch
) -> None:
    gate = _load_gate()
    monkeypatch.setattr(gate.pa, "git_rev_is_ancestor_of_origin", lambda _: True)
    doc = _manifest(tmp_path, gate)
    doc["criteria"]["E4"] = {  # type: ignore[index]
        "status": "pass",
        "evidence": [
            _canonicalization_evidence(tmp_path, gate),
            _structural_audit_evidence(tmp_path, gate),
            _degrade_to_slow_evidence(tmp_path, ok=False),
            _fail_closed_evidence(tmp_path, ok=False),
        ],
    }

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E4: degrade_to_slow_gate report is not ok" in p for p in report.problems)
    assert any(
        "E4: fail_closed_gate receipt does not contain OK verdict" in p
        for p in report.problems
    )


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

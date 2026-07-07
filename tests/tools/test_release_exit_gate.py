from __future__ import annotations

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


def _manifest(tmp_path: Path, **overrides: object) -> dict[str, object]:
    criteria = {
        key: {"status": "pass", "evidence": [_evidence(tmp_path, key)]}
        for key in ("E1", "E2", "E3", "E4")
    }
    criteria.update(overrides)
    return {"schema_version": 1, "criteria": criteria}


def test_release_exit_gate_passes_only_when_all_e1_e4_receipts_pass(
    tmp_path: Path,
) -> None:
    gate = _load_gate()
    manifest_path = tmp_path / "release-exit.json"
    manifest_path.write_text(json.dumps(_manifest(tmp_path)), encoding="utf-8")

    report = gate.check_manifest_path(manifest_path)

    assert report.passed is True
    assert {result.criterion for result in report.criteria} == {"E1", "E2", "E3", "E4"}
    assert all(result.passed for result in report.criteria)


def test_release_exit_gate_fails_on_nonpassing_criterion(tmp_path: Path) -> None:
    gate = _load_gate()
    doc = _manifest(
        tmp_path,
        E2={"status": "blocked", "evidence": [_evidence(tmp_path, "E2")]},
    )

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E2: status is blocked, expected pass" in p for p in report.problems)


def test_release_exit_gate_fails_when_a_criterion_is_missing(tmp_path: Path) -> None:
    gate = _load_gate()
    doc = _manifest(tmp_path)
    del doc["criteria"]["E4"]  # type: ignore[index]

    report = gate.validate_manifest(doc, manifest_path=tmp_path / "release-exit.json")

    assert report.passed is False
    assert any("E4: missing criterion receipt" in p for p in report.problems)


def test_release_exit_gate_requires_evidence_for_passing_receipts(
    tmp_path: Path,
) -> None:
    gate = _load_gate()
    doc = _manifest(tmp_path, E1={"status": "pass", "evidence": []})

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


def test_release_exit_gate_json_cli_reports_failures(
    tmp_path: Path,
    capsys,
) -> None:
    gate = _load_gate()
    manifest_path = tmp_path / "release-exit.json"
    manifest_path.write_text(
        json.dumps(_manifest(tmp_path, E1={"status": "fail"})),
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

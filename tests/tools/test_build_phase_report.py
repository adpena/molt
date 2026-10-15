from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools import build_phase_report


def _diagnostics(
    directory: Path, name: str, entry: str, phases: dict[str, float]
) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    payload = {
        "entry": {"module": Path(entry).stem, "path": entry},
        "total_sec": sum(phases.values()),
        "phase_sec": phases,
    }
    (directory / f"{name}.json").write_text(json.dumps(payload), encoding="utf-8")


def test_report_totals_phases_and_ranks_the_slowest_builds(tmp_path: Path) -> None:
    _diagnostics(tmp_path, "1", "a.py", {"backend_prepare": 6.0, "link": 1.0})
    _diagnostics(tmp_path, "2", "b.py", {"backend_prepare": 2.0, "ir_lowering": 2.0})
    _diagnostics(tmp_path, "3", "c.py", {"link": 1.0})

    summary = build_phase_report.summarize(
        build_phase_report.load_records(tmp_path), top=2
    )

    assert summary.builds == 3
    assert summary.total_s == 12.0
    assert [(row.phase, row.total_s) for row in summary.phases] == [
        ("backend_prepare", 8.0),
        ("link", 2.0),
        ("ir_lowering", 2.0),
    ]
    assert summary.phases[0].share == round(8.0 / 12.0, 4)
    assert [(record.entry, record.top_phase) for record in summary.slowest] == [
        ("a.py", "backend_prepare"),
        ("b.py", "backend_prepare"),
    ]


def test_report_cli_emits_json(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    _diagnostics(tmp_path, "1", "a.py", {"runtime_setup": 3.0})

    assert build_phase_report.main([str(tmp_path), "--json"]) == 0

    payload = json.loads(capsys.readouterr().out)
    assert payload["builds"] == 1
    assert payload["slowest"][0] == {
        "entry": "a.py",
        "total_s": 3.0,
        "top_phase": "runtime_setup",
        "top_phase_s": 3.0,
    }


def test_report_refuses_a_file_that_is_not_build_diagnostics(tmp_path: Path) -> None:
    (tmp_path / "other.json").write_text(json.dumps({"phase_sec": 1}), encoding="utf-8")

    with pytest.raises(ValueError, match="not a molt build diagnostics file"):
        build_phase_report.load_records(tmp_path)

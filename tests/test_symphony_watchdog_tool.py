from __future__ import annotations

from pathlib import Path

import tools.symphony_watchdog as symphony_watchdog


def test_collect_paths_respects_patterns(tmp_path: Path) -> None:
    (tmp_path / "WORKFLOW.md").write_text("x", encoding="utf-8")
    package = tmp_path / "src" / "molt" / "symphony"
    package.mkdir(parents=True)
    (package / "http_server.py").write_text("x", encoding="utf-8")

    paths = symphony_watchdog._collect_paths(
        tmp_path, ("WORKFLOW.md", "src/molt/symphony/**/*.py")
    )
    rel = {str(path.relative_to(tmp_path)) for path in paths}
    assert "WORKFLOW.md" in rel
    assert "src/molt/symphony/http_server.py" in rel


def test_fingerprint_changes_when_file_changes(tmp_path: Path) -> None:
    workflow = tmp_path / "WORKFLOW.md"
    workflow.write_text("a", encoding="utf-8")
    first = symphony_watchdog._fingerprint(tmp_path, ("WORKFLOW.md",))
    workflow.write_text("b", encoding="utf-8")
    second = symphony_watchdog._fingerprint(tmp_path, ("WORKFLOW.md",))
    assert first != second


def test_launchd_target_uses_gui_uid() -> None:
    target = symphony_watchdog._launchd_target("com.molt.symphony")
    assert target.startswith("gui/")
    assert target.endswith("/com.molt.symphony")


def test_service_is_busy_from_counts(monkeypatch: object) -> None:
    monkeypatch.setattr(
        symphony_watchdog,
        "_read_state_counts",
        lambda *_args, **_kwargs: {"running": 2, "retrying": 0},
    )
    busy, detail = symphony_watchdog._service_is_busy(
        "http://127.0.0.1:8089/api/v1/state", 0.5
    )
    assert busy is True
    assert "running=2" in detail


def test_service_is_idle_from_counts(monkeypatch: object) -> None:
    monkeypatch.setattr(
        symphony_watchdog,
        "_read_state_counts",
        lambda *_args, **_kwargs: {"running": 0, "retrying": 0},
    )
    busy, detail = symphony_watchdog._service_is_busy(
        "http://127.0.0.1:8089/api/v1/state", 0.5
    )
    assert busy is False
    assert "retrying=0" in detail


def test_service_is_busy_state_unavailable(monkeypatch: object) -> None:
    monkeypatch.setattr(
        symphony_watchdog,
        "_read_state_counts",
        lambda *_args, **_kwargs: None,
    )
    busy, detail = symphony_watchdog._service_is_busy(
        "http://127.0.0.1:8089/api/v1/state", 0.5
    )
    assert busy is None
    assert detail == "state_unavailable"

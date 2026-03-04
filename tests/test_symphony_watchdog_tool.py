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

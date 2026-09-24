"""Teeth for tools/legacy_inventory.py: the registry-driven legacy_count authority."""

from __future__ import annotations

from pathlib import Path

import pytest

from tools import legacy_inventory as li

ROOT = Path(__file__).resolve().parents[2]


def _registry(root: Path, body: str) -> Path:
    path = root / "config" / "legacy_inventory.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f'schema = "{li.SCHEMA}"\n{body}', encoding="utf-8", newline="\n")
    return path


def _row(item_id: str, path: str, pattern: str | None = None) -> str:
    pattern_line = f"pattern = '{pattern}'\n" if pattern else ""
    return (
        "[[item]]\n"
        f'id = "{item_id}"\n'
        f'path = "{path}"\n'
        f"{pattern_line}"
        'superseded_by = "the new authority"\n'
        'removal_release = "1.0.0"\n'
        'reason = "kept only for the test"\n\n'
    )


def test_count_is_the_number_of_present_registered_lanes(tmp_path: Path) -> None:
    (tmp_path / "old_shim.py").write_text(
        "def legacy_entry():\n    pass\n", encoding="utf-8"
    )
    (tmp_path / "gate.toml").write_text("[[target]]\nid = 'x'\n", encoding="utf-8")
    _registry(
        tmp_path,
        _row("shim", "old_shim.py")
        + _row("table", "gate.toml", r"^\[\[target\]\]")
        + _row("gone", "deleted_lane.py")
        + _row("pattern-gone", "old_shim.py", r"^def removed_entry\("),
    )
    report = li.inventory(tmp_path)
    assert report.legacy_count == 2
    assert report.retired == ("gone", "pattern-gone")
    assert [row.id for row in report.items if row.present] == ["shim", "table"]


def test_check_refuses_retired_rows(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    _registry(tmp_path, _row("gone", "deleted_lane.py"))
    assert li.main(["--root", str(tmp_path), "--check"]) == 1
    assert "retired rows must be deleted" in capsys.readouterr().err
    assert li.main(["--root", str(tmp_path)]) == 0


@pytest.mark.parametrize(
    "body, message",
    [
        ('[[item]]\nid = "a"\npath = "x"\n', "keys must be"),
        (_row("a", "../outside.py"), "repository-relative"),
        (_row("a", "x.py") + _row("a", "y.py"), "duplicate legacy item id"),
        (_row("a", "x.py", "["), None),
    ],
)
def test_registry_rows_are_exact(
    tmp_path: Path, body: str, message: str | None
) -> None:
    _registry(tmp_path, body)
    with pytest.raises((ValueError, Exception)) as excinfo:
        li.load_registry(tmp_path / "config" / "legacy_inventory.toml")
    if message is not None:
        assert message in str(excinfo.value)


def test_live_registry_has_no_retired_rows() -> None:
    """Every registered legacy lane must still exist; delete rows with the lane."""
    report = li.inventory(ROOT)
    assert report.retired == (), report.retired

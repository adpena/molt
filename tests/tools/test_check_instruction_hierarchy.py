from __future__ import annotations

from pathlib import Path

from tools import check_instruction_hierarchy as checker


ROOT = Path(__file__).resolve().parents[2]


def _valid_tree(tmp_path: Path) -> Path:
    (tmp_path / "docs" / "agent").mkdir(parents=True)
    (tmp_path / "docs" / "INDEX.md").write_text("# Index\n", encoding="utf-8")
    (tmp_path / "AGENTS.md").write_text(
        "# Agent contract\n\nSee `docs/INDEX.md`.\n", encoding="utf-8"
    )
    (tmp_path / "CLAUDE.md").write_text(checker.CLAUDE_IMPORT, encoding="utf-8")
    for relative in checker.ARCHIVES:
        (tmp_path / relative).write_text(
            checker.ARCHIVE_MARKER + "\n\n# Historical material\n",
            encoding="utf-8",
        )
    return tmp_path


def test_live_repository_instruction_hierarchy_is_valid() -> None:
    assert checker.audit(ROOT).ok


def test_valid_minimal_hierarchy_passes(tmp_path: Path) -> None:
    assert checker.audit(_valid_tree(tmp_path)).ok


def test_root_contract_budget_and_machine_state_fail_closed(tmp_path: Path) -> None:
    root = _valid_tree(tmp_path)
    (root / "AGENTS.md").write_text(
        "# Agent contract\n\n"
        "See `docs/INDEX.md`.\n"
        "cwd C:\\Users\\operator\\OneDrive\\molt; pid=4321\n"
        + "padding\n" * checker.ROOT_AGENT_MAX_LINES,
        encoding="utf-8",
    )
    failures = checker.audit(root).failures
    assert any("line budget" in failure for failure in failures)
    assert any("absolute Windows path" in failure for failure in failures)
    assert any("OneDrive workstation path" in failure for failure in failures)
    assert any("concrete process id" in failure for failure in failures)


def test_root_contract_byte_budget_fails_closed(tmp_path: Path) -> None:
    root = _valid_tree(tmp_path)
    (root / "AGENTS.md").write_text(
        "# Agent contract\n\nSee `docs/INDEX.md`.\n"
        + "x" * checker.ROOT_AGENT_MAX_BYTES,
        encoding="utf-8",
    )
    assert any("byte budget" in failure for failure in checker.audit(root).failures)


def test_claude_contract_is_only_the_canonical_import(tmp_path: Path) -> None:
    root = _valid_tree(tmp_path)
    (root / "CLAUDE.md").write_text("@AGENTS.md\nextra\n", encoding="utf-8")
    assert any("must contain exactly" in failure for failure in checker.audit(root).failures)


def test_missing_pointer_and_unmarked_archive_fail_closed(tmp_path: Path) -> None:
    root = _valid_tree(tmp_path)
    (root / "AGENTS.md").write_text(
        "# Agent contract\n\nSee `docs/MISSING.md`.\n", encoding="utf-8"
    )
    (root / checker.ARCHIVES[0]).write_text("# Normative?\n", encoding="utf-8")
    failures = checker.audit(root).failures
    assert any("pointer does not exist" in failure for failure in failures)
    assert any(checker.ARCHIVES[0] in failure for failure in failures)

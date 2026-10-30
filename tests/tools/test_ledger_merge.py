"""The ledger merge driver: findings rows merge by their ID, not by line.

Each case is a three-way merge whose right answer follows from what each side
did to which row. The last tests run the driver inside real Git rebases and
cherry-picks, the operations the landing queue performs, on the exact shapes
that conflicted on 2026-10-21: two lanes each adding a row at the top of a
section, and a lane editing a row next to one that main moved.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools import ledger_merge

ROOT = Path(__file__).resolve().parents[2]


def _ledger(
    open_rows: list[str], fixed_rows: list[str], prose: str = "Open findings."
) -> str:
    def rows(items: list[str]) -> str:
        return "".join(f"| {item} | x |\n" for item in items)

    return (
        "# Ledger\n\n"
        "## Open\n\n"
        f"{prose}\n\n"
        "| ID | Finding |\n|----|---------|\n"
        f"{rows(open_rows)}\n"
        "## Fixed\n\n"
        "| ID | Fix |\n|----|-----|\n"
        f"{rows(fixed_rows)}"
    )


def _row(text: str, key: str, body: str) -> str:
    return text.replace(f"| {key} | x |", f"| {key} | {body} |")


def _merge(base: str, ours: str, theirs: str) -> ledger_merge.MergeResult:
    return ledger_merge.merge_documents(base, ours, theirs)


def _keys(text: str) -> list[str]:
    return [
        line.split("|")[1].strip()
        for line in text.splitlines()
        if ledger_merge._ROW.match(line)
    ]


def test_rows_both_sides_add_at_the_same_spot_are_both_kept() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _ledger(["HF-10", "HF-1", "HF-2"], ["HF-0"])
    theirs = _ledger(["HF-11", "HF-1", "HF-2"], ["HF-9", "HF-0"])

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert _keys(result.text) == ["HF-11", "HF-10", "HF-1", "HF-2", "HF-9", "HF-0"]


def test_a_moved_and_rewritten_row_follows_its_side() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    # Main fixed HF-2: it leaves Open for the top of Fixed with new text.
    ours = _row(_ledger(["HF-1"], ["HF-2", "HF-0"]), "HF-2", "fixed by main")
    # The lane rewrote HF-1, the row next to it.
    theirs = _row(base, "HF-1", "lane progress")

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert _keys(result.text) == ["HF-1", "HF-2", "HF-0"]
    assert "| HF-1 | lane progress |" in result.text
    assert "| HF-2 | fixed by main |" in result.text
    assert result.text.count("| HF-2 |") == 1


def test_the_other_side_moves_a_row_the_frame_left_alone() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _row(base, "HF-1", "ours")
    theirs = _ledger(["HF-1"], ["HF-2", "HF-0"])

    result = _merge(base, ours, theirs)

    assert _keys(result.text) == ["HF-1", "HF-2", "HF-0"]
    assert "## Fixed" in result.text.split("| HF-2 |")[0]


def test_a_deleted_row_stays_deleted_and_an_edit_elsewhere_survives() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _ledger(["HF-1"], ["HF-0"])
    theirs = _row(base, "HF-0", "theirs")

    result = _merge(base, ours, theirs)

    assert _keys(result.text) == ["HF-1", "HF-0"]
    assert "| HF-0 | theirs |" in result.text


def test_one_side_changes_prose_and_the_other_changes_rows() -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    ours = _ledger(["HF-1"], ["HF-0"], prose="Open findings, all of them.")
    theirs = _row(_ledger(["HF-7", "HF-1"], ["HF-0"]), "HF-1", "theirs")

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert "Open findings, all of them." in result.text
    assert _keys(result.text) == ["HF-7", "HF-1", "HF-0"]
    assert "| HF-1 | theirs |" in result.text


def test_both_sides_rewriting_one_row_is_a_conflict() -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    ours = _row(base, "HF-1", "ours")
    theirs = _row(_row(base, "HF-1", "theirs"), "HF-2", "theirs too")

    result = _merge(base, ours, theirs)

    assert result.conflicts == ("HF-1",)
    block = result.text.split("<<<<<<< ours\n")[1].split(">>>>>>> theirs\n")[0]
    assert block == "| HF-1 | ours |\n=======\n| HF-1 | theirs |\n"
    assert "| HF-2 | theirs too |" in result.text


@pytest.mark.parametrize(
    "ours_prose", ["Ours changed the prose.", "Ours changed it differently."]
)
def test_prose_both_sides_changed_falls_back_to_a_text_merge(ours_prose: str) -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    ours = _ledger(["HF-1"], ["HF-0"], prose=ours_prose)
    theirs = _ledger(["HF-1"], ["HF-0"], prose="Theirs changed the prose.")

    with pytest.raises(ledger_merge._Unmergeable):
        _merge(base, ours, theirs)


def test_a_duplicated_row_key_falls_back_to_a_text_merge() -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    with pytest.raises(ledger_merge._Unmergeable):
        _merge(base, _ledger(["HF-1", "HF-1"], ["HF-0"]), base)


def _git(repo: Path, *args: str):
    return run_guarded_test_process(
        ["git", *args], cwd=str(repo), check=True, capture_output=True, text=True
    )


def _repo_with_driver(tmp_path: Path, base: str) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q", "-b", "main")
    _git(repo, "config", "user.email", "test@example.invalid")
    _git(repo, "config", "user.name", "Test")
    driver = f'"{sys.executable}" "{ROOT / "tools" / "ledger_merge.py"}" %O %A %B'
    _git(repo, "config", "merge.molt-ledger.driver", driver)
    (repo / ".gitattributes").write_text(
        "LEDGER.md merge=molt-ledger\n", encoding="utf-8"
    )
    (repo / "LEDGER.md").write_text(base, encoding="utf-8")
    _git(repo, "add", ".")
    _git(repo, "commit", "-q", "-m", "base")
    return repo


def test_a_cherry_pick_merges_two_rows_added_at_the_same_spot(tmp_path: Path) -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    repo = _repo_with_driver(tmp_path, base)
    _git(repo, "switch", "-q", "-c", "lane")
    (repo / "LEDGER.md").write_text(
        _ledger(["HF-11", "HF-1"], ["HF-0"]), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-am", "lane opens HF-11")
    _git(repo, "switch", "-q", "main")
    (repo / "LEDGER.md").write_text(
        _ledger(["HF-10", "HF-1"], ["HF-0"]), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-am", "main opens HF-10")

    _git(repo, "cherry-pick", "lane")

    assert _keys((repo / "LEDGER.md").read_text(encoding="utf-8")) == [
        "HF-11",
        "HF-10",
        "HF-1",
        "HF-0",
    ]


def test_a_rebase_merges_a_row_move_beside_a_row_edit(tmp_path: Path) -> None:
    base = _ledger(["HF-1", "HF-2"], ["HF-0"])
    repo = _repo_with_driver(tmp_path, base)
    _git(repo, "switch", "-q", "-c", "lane")
    (repo / "LEDGER.md").write_text(
        _row(base, "HF-1", "lane progress"), encoding="utf-8"
    )
    _git(repo, "commit", "-q", "-am", "lane records HF-1")
    _git(repo, "switch", "-q", "main")
    fixed = _row(_ledger(["HF-1"], ["HF-2", "HF-0"]), "HF-2", "fixed by main")
    (repo / "LEDGER.md").write_text(fixed, encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "main fixes HF-2")
    _git(repo, "switch", "-q", "lane")

    _git(repo, "rebase", "main")

    text = (repo / "LEDGER.md").read_text(encoding="utf-8")
    assert _keys(text) == ["HF-1", "HF-2", "HF-0"]
    assert "| HF-1 | lane progress |" in text and "| HF-2 | fixed by main |" in text


def test_a_conflicting_row_stops_the_cherry_pick(tmp_path: Path) -> None:
    base = _ledger(["HF-1"], ["HF-0"])
    repo = _repo_with_driver(tmp_path, base)
    _git(repo, "switch", "-q", "-c", "lane")
    (repo / "LEDGER.md").write_text(_row(base, "HF-1", "lane"), encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "lane")
    _git(repo, "switch", "-q", "main")
    (repo / "LEDGER.md").write_text(_row(base, "HF-1", "main"), encoding="utf-8")
    _git(repo, "commit", "-q", "-am", "main")

    picked = run_guarded_test_process(
        ["git", "cherry-pick", "lane"], cwd=str(repo), capture_output=True, text=True
    )

    assert picked.returncode != 0
    assert "<<<<<<< ours" in (repo / "LEDGER.md").read_text(encoding="utf-8")


def _with_families(text: str, families: dict[str, tuple[str, str]]) -> str:
    rows = "".join(
        f"| {letter}. {name} | cause | {keys} |\n"
        for letter, (name, keys) in families.items()
    )
    return text.replace(
        "## Fixed\n",
        "### Family dependency order\n\n"
        "| Family | Cause | Keys |\n|---|---|---|\n"
        f"{rows}\n"
        "## Fixed\n",
    )


def _family_keys(text: str, letter: str) -> str:
    line = next(line for line in text.splitlines() if line.startswith(f"| {letter}. "))
    return line.split(" | ")[2].removesuffix(" |")


def test_family_keys_merge_as_a_set_when_both_sides_edit_one_family() -> None:
    # The 2026-10-29 refusal: one lane opened a row in family C, main closed
    # another; both edited C's key list, so the old driver merged as text.
    base = _with_families(
        _ledger(["HF-1", "HF-2"], ["HF-0"]), {"C": ("Cells", "HF-1, HF-2")}
    )
    ours = _with_families(
        _ledger(["HF-10", "HF-1", "HF-2"], ["HF-0"]),
        {"C": ("Cells", "HF-1, HF-2, HF-10")},
    )
    theirs = _with_families(
        _ledger(["HF-1"], ["HF-2", "HF-0"]), {"C": ("Cells", "HF-1")}
    )

    result = _merge(base, ours, theirs)

    assert result.conflicts == ()
    assert _family_keys(result.text, "C") == "HF-1, HF-10"
    assert _keys(result.text) == ["HF-10", "HF-1", "HF-2", "HF-0"]


def test_both_sides_renaming_one_family_is_a_conflict() -> None:
    base = _with_families(_ledger(["HF-1"], []), {"C": ("Cells", "HF-1")})
    ours = _with_families(_ledger(["HF-1"], []), {"C": ("Graph cells", "HF-1")})
    theirs = _with_families(_ledger(["HF-1"], []), {"C": ("Proof cells", "HF-1")})

    result = _merge(base, ours, theirs)

    assert result.conflicts == ("family C",)

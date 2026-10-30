"""tools/ledger_edit.py moves a row, its family and the identities together."""

from __future__ import annotations

from pathlib import Path

from tools import finding_status, ledger_edit

_LEDGER = """\
# Ledger

## Open: release blockers

| ID | Finding | Evidence | Next |
|----|---------|----------|------|
| HF-1 | One. | E1. | N1. |
| HF-2 | Two. | E2. | N2. |

### Family dependency order (2026-10-29)

| Family | Cause | Keys |
|---|---|---|
| A. Alpha | cause a | HF-1 |
| B. Beta | cause b | HF-2 |

## Fixed after the handoff

| ID | Defect | Fix |
|----|--------|-----|
| HF-0 | Zero. | Fixed. |
"""

_PRIOR_ART = "# Prior art\n\n## A. Alpha\n\nx\n\n## B. Beta\n\ny\n"


def _root(tmp_path: Path, prior_art: str = _PRIOR_ART) -> Path:
    (tmp_path / "docs/agent").mkdir(parents=True)
    (tmp_path / "docs/design").mkdir(parents=True)
    (tmp_path / "tests/findings").mkdir(parents=True)
    (tmp_path / ledger_edit.LEDGER).write_text(_LEDGER, encoding="utf-8")
    (tmp_path / "docs/design/findings_prior_art.md").write_text(
        prior_art, encoding="utf-8"
    )
    return tmp_path


def _status(root: Path) -> finding_status.FindingStatus:
    return finding_status.project(
        (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    )


def _family_keys(root: Path) -> dict[str, tuple[str, ...]]:
    return {f.letter: f.keys for f in _status(root).dependency_families}


def _identities(root: Path) -> str:
    return (root / "tests/findings/finding_ids.py").read_text(encoding="utf-8")


def test_open_inserts_the_row_joins_its_family_and_projects_it(tmp_path: Path) -> None:
    root = _root(tmp_path)

    status = ledger_edit.main(
        [
            "open",
            "HF-3",
            "--family",
            "B",
            "--section",
            "Open: release blockers",
            "--after",
            "HF-2",
            "--defect",
            "Three.",
            "--evidence",
            "E3.",
            "--next",
            "N3.",
        ],
        root=root,
    )

    assert status == 0
    assert _status(root).open_keys == ("HF-1", "HF-2", "HF-3")
    assert _family_keys(root) == {"A": ("HF-1",), "B": ("HF-2", "HF-3")}
    assert '"HF-3",' in _identities(root).split("FixedFindingId")[0]
    assert (
        ledger_edit.next_key((root / ledger_edit.LEDGER).read_text(encoding="utf-8"))
        == "HF-4"
    )


def test_close_moves_the_row_to_fixed_and_leaves_its_family(tmp_path: Path) -> None:
    root = _root(tmp_path)

    assert ledger_edit.main(["close", "HF-2", "--fix", "Fixed by x."], root=root) == 0

    text = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    assert (
        "| HF-2 | Two. | Fixed by x. |" in text.split("## Fixed after the handoff")[1]
    )
    assert _status(root).open_keys == ("HF-1",)
    assert _family_keys(root) == {"A": ("HF-1",), "B": ()}
    assert '"HF-2",' in _identities(root).split("FixedFindingId")[1]


def test_merge_folds_the_row_into_its_target(tmp_path: Path) -> None:
    root = _root(tmp_path)

    status = ledger_edit.main(
        [
            "merge",
            "HF-2",
            "--into",
            "HF-1",
            "--defect-note",
            "Two too.",
            "--next-note",
            "Also N2.",
            "--date",
            "2026-10-29",
        ],
        root=root,
    )

    assert status == 0
    text = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    assert (
        "| HF-1 | One. Two too. This row absorbed HF-2 on 2026-10-29. | E1. | N1. Also N2. |"
        in text
    )
    assert _status(root).open_keys == ("HF-1",)
    assert _family_keys(root) == {"A": ("HF-1",), "B": ()}


def test_note_appends_to_an_open_row_and_keeps_its_family(tmp_path: Path) -> None:
    root = _root(tmp_path)

    status = ledger_edit.main(
        ["note", "HF-2", "--evidence-note", "Measured.", "--next-note", "Stage 1."],
        root=root,
    )

    assert status == 0
    text = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    assert "| HF-2 | Two. | E2. Measured. | N2. Stage 1. |" in text
    assert _family_keys(root) == {"A": ("HF-1",), "B": ("HF-2",)}


def test_a_note_on_a_fixed_row_or_without_text_writes_nothing(tmp_path: Path) -> None:
    root = _root(tmp_path)

    assert ledger_edit.main(["note", "HF-0", "--defect-note", "x"], root=root) == 2
    assert ledger_edit.main(["note", "HF-1"], root=root) == 2
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == _LEDGER


def test_a_bare_bar_in_a_cell_is_refused(tmp_path: Path) -> None:
    root = _root(tmp_path)

    status = ledger_edit.main(
        [
            "open",
            "HF-3",
            "--family",
            "A",
            "--section",
            "Open: release blockers",
            "--defect",
            "a | b",
            "--evidence",
            "e",
            "--next",
            "n",
        ],
        root=root,
    )

    assert status == 2
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == _LEDGER


def test_an_edit_that_breaks_family_coverage_writes_nothing(tmp_path: Path) -> None:
    # Family B has no prior-art section, so any edit leaves a coverage problem.
    root = _root(tmp_path, prior_art="# Prior art\n\n## A. Alpha\n\nx\n")

    status = ledger_edit.main(["close", "HF-1", "--fix", "Fixed."], root=root)

    assert status == 1
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == _LEDGER
    assert not (root / "tests/findings/finding_ids.py").exists()


def test_family_move_keeps_each_row_in_exactly_one_family(tmp_path: Path) -> None:
    root = _root(tmp_path)

    assert ledger_edit.main(["family", "move", "HF-2", "--to", "A"], root=root) == 0

    assert _family_keys(root) == {"A": ("HF-1", "HF-2"), "B": ()}
    # Moving a row already in the target changes nothing and duplicates nothing.
    assert ledger_edit.main(["family", "move", "HF-2", "--to", "A"], root=root) == 0
    assert _family_keys(root) == {"A": ("HF-1", "HF-2"), "B": ()}


def test_family_move_refuses_a_closed_or_unknown_row(tmp_path: Path) -> None:
    root = _root(tmp_path)
    before = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")

    assert ledger_edit.main(["family", "move", "HF-0", "--to", "A"], root=root) == 2
    assert ledger_edit.main(["family", "move", "HF-2", "--to", "Z"], root=root) == 2

    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == before


def test_family_describe_must_match_its_prior_art(tmp_path: Path) -> None:
    root = _root(tmp_path)
    before = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")

    # The prior-art record names the family; a rename without it is refused.
    assert (
        ledger_edit.main(
            ["family", "describe", "B", "--name", "Gamma", "--cause", "cause g"],
            root=root,
        )
        == 1
    )
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == before

    renamed = _root(tmp_path / "renamed", _PRIOR_ART.replace("B. Beta", "B. Gamma"))
    assert (
        ledger_edit.main(
            ["family", "describe", "B", "--name", "Gamma", "--cause", "cause g"],
            root=renamed,
        )
        == 0
    )
    family = next(f for f in _status(renamed).dependency_families if f.letter == "B")
    assert (family.name, family.keys) == ("Gamma", ("HF-2",))


def test_family_recut_renames_and_moves_atomically(tmp_path: Path) -> None:
    # Each rename alone would leave the other family out of step with its
    # prior art; the re-cut is checked once, after every step.
    root = _root(
        tmp_path,
        _PRIOR_ART.replace("A. Alpha", "A. Ash").replace("B. Beta", "B. Birch"),
    )
    spec = tmp_path / "recut.json"
    spec.write_text(
        '{"describe": ['
        '{"letter": "A", "name": "Ash", "cause": "cause ash"},'
        '{"letter": "B", "name": "Birch", "cause": "cause birch"}],'
        ' "move": [{"keys": ["HF-1"], "to": "B"}]}',
        encoding="utf-8",
    )

    assert ledger_edit.main(["family", "recut", "--spec", str(spec)], root=root) == 0

    families = {f.letter: (f.name, f.keys) for f in _status(root).dependency_families}
    assert families == {"A": ("Ash", ()), "B": ("Birch", ("HF-2", "HF-1"))}


def test_family_recut_refuses_a_malformed_spec(tmp_path: Path) -> None:
    root = _root(tmp_path)
    spec = tmp_path / "recut.json"
    spec.write_text('{"describe": [{"letter": "A"}]}', encoding="utf-8")

    assert ledger_edit.main(["family", "recut", "--spec", str(spec)], root=root) == 2

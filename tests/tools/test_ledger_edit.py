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

| Family | Stratum | HF-459 axes | Cause | Keys |
| --- | --- | --- | --- | --- |
| A. Alpha | engine | loop/per_op | cause a | HF-1 |
| B. Beta | semantics | - | cause b | HF-2 |

| From | To | Reason |
| --- | --- | --- |
| A | B | beta needs alpha |

| Row | V1 class |
| --- | --- |
| HF-1 | release_blocking |
| HF-2 | release_blocking |

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
    (tmp_path / "bench").mkdir()
    (tmp_path / "bench/primitive_ratchet.json").write_text(
        '{"axes": {"loop/per_op": {}, "empty/startup": {}}}', encoding="utf-8"
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
            "--v1-class",
            "release_blocking",
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


def test_class_restates_one_row_and_refuses_a_no_op_or_unclassified_row(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    before = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")

    assert (
        ledger_edit.main(["class", "HF-2", "--v1-class", "apparatus"], root=root) == 0
    )

    status = _status(root)
    assert status.classes["HF-2"] is finding_status.V1Class.APPARATUS
    assert status.classes["HF-1"] is finding_status.V1Class.RELEASE_BLOCKING
    assert _family_keys(root) == {"A": ("HF-1",), "B": ("HF-2",)}
    after = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    assert len(after.split("\n")) == len(before.split("\n"))
    assert (
        ledger_edit.main(["class", "HF-2", "--v1-class", "apparatus"], root=root) == 2
    )
    assert (
        ledger_edit.main(["class", "HF-0", "--v1-class", "apparatus"], root=root) == 2
    )
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == after


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


_REQUIREMENT_LEDGER = (
    _LEDGER.replace(
        "## Open: release blockers",
        "## Open release requirements\n\n"
        "| ID | Open release requirement | Existing authority and acceptance boundary |\n"
        "|----|--------------------------|--------------------------------------------|\n"
        "| V1-1 | Requirement. | Boundary. |\n\n"
        "## Open: release blockers",
    )
    .replace(
        "| A. Alpha | engine | loop/per_op | cause a | HF-1 |",
        "| A. Alpha | engine | loop/per_op | cause a | HF-1, V1-1 |",
    )
    .replace(
        "| HF-1 | release_blocking |",
        "| HF-1 | release_blocking |\n| V1-1 | release_blocking |",
    )
)


def test_note_appends_to_a_release_requirement_and_refuses_a_next_step(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    (root / ledger_edit.LEDGER).write_text(_REQUIREMENT_LEDGER, encoding="utf-8")

    assert ledger_edit.main(["note", "V1-1", "--next-note", "N."], root=root) == 2
    text = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    assert text == _REQUIREMENT_LEDGER

    status = ledger_edit.main(
        ["note", "V1-1", "--defect-note", "Half done.", "--evidence-note", "HF-9."],
        root=root,
    )

    assert status == 0
    text = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    assert "| V1-1 | Requirement. Half done. | Boundary. HF-9. |" in text
    assert "| HF-2 | Two. | E2. | N2. |" in text


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
            "--v1-class",
            "release_blocking",
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


def test_family_describe_restates_axes_and_keeps_every_per_op_axis_owned(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    describe = ["family", "describe", "A", "--name", "Alpha", "--cause", "cause a"]
    ratchet = root / "bench/primitive_ratchet.json"
    ratchet.write_text(
        '{"axes": {"loop/per_op": {}, "call/per_op": {}}}', encoding="utf-8"
    )
    before = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")

    # Dropping the only owner of loop/per_op leaves a measured cost unowned.
    assert ledger_edit.main([*describe, "--axes", "call/per_op"], root=root) == 1
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == before

    assert (
        ledger_edit.main([*describe, "--axes", "loop/per_op, call/per_op"], root=root)
        == 0
    )
    family = next(f for f in _status(root).dependency_families if f.letter == "A")
    assert family.axes == ("loop/per_op", "call/per_op")
    # Without --axes the column stays as it is.
    assert ledger_edit.main(describe, root=root) == 0
    family = next(f for f in _status(root).dependency_families if f.letter == "A")
    assert family.axes == ("loop/per_op", "call/per_op")


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


def test_family_create_requires_its_prior_art(tmp_path: Path) -> None:
    root = _root(tmp_path)
    before = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    argv = [
        "family",
        "create",
        "C",
        "--name",
        "Gamma",
        "--cause",
        "cause g",
        "--stratum",
        "apparatus",
    ]
    # A family without a prior-art section is refused, and nothing is written.
    assert ledger_edit.main(argv, root=root) == 1
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == before

    ready = _root(tmp_path / "ready", _PRIOR_ART + "\n## C. Gamma\n\ntext\n")
    assert ledger_edit.main(argv, root=ready) == 0
    family = next(f for f in _status(ready).dependency_families if f.letter == "C")
    assert (family.name, family.keys) == ("Gamma", ())
    assert ledger_edit.main(argv, root=ready) == 2  # already exists


def test_edges_change_only_through_the_table_and_readiness_follows(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    assert finding_status.dependency_predecessors(_status(root))["HF-2"] == frozenset(
        {"HF-1"}
    )

    # Adding B -> A would order each family after itself; nothing is written.
    before = (root / ledger_edit.LEDGER).read_text(encoding="utf-8")
    assert ledger_edit.main(["edge", "add", "B", "A", "--reason", "x"], root=root) == 1
    assert (root / ledger_edit.LEDGER).read_text(encoding="utf-8") == before
    assert ledger_edit.main(["edge", "add", "A", "B", "--reason", "x"], root=root) == 2

    # An explicitly empty edge table means there are no prerequisites.
    assert ledger_edit.main(["edge", "remove", "A", "B"], root=root) == 0
    assert ledger_edit.main(["edge", "remove", "B", "A"], root=root) == 2

    ready = _root(tmp_path / "ready", _PRIOR_ART + "\n## C. Gamma\n\ntext\n")
    create = [
        "family",
        "create",
        "C",
        "--name",
        "Gamma",
        "--cause",
        "cause g",
        "--stratum",
        "apparatus",
    ]
    assert ledger_edit.main(create, root=ready) == 0
    assert (
        ledger_edit.main(["edge", "add", "B", "C", "--reason", "c needs b"], root=ready)
        == 0
    )
    assert finding_status.dependency_predecessors(_status(ready))["HF-2"] == frozenset(
        {"HF-1"}
    )
    assert ledger_edit.main(["edge", "remove", "A", "B"], root=ready) == 0
    assert finding_status.dependency_predecessors(_status(ready))["HF-2"] == frozenset()


def test_merge_rewrites_row_edges_and_close_satisfies_named_prerequisites(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    assert (
        ledger_edit.main(
            [
                "open",
                "HF-3",
                "--family",
                "B",
                "--v1-class",
                "release_blocking",
                "--section",
                "Open: release blockers",
                "--defect",
                "Three.",
                "--evidence",
                "E3.",
                "--next",
                "N3.",
            ],
            root=root,
        )
        == 0
    )
    assert ledger_edit.main(["edge", "remove", "A", "B"], root=root) == 0
    for source, target in (("HF-1", "HF-2"), ("HF-2", "HF-3")):
        assert (
            ledger_edit.main(
                ["edge", "add", source, target, "--reason", "input producer"], root=root
            )
            == 0
        )
    assert ledger_edit.main(["merge", "HF-2", "--into", "HF-1"], root=root) == 0
    status = _status(root)
    assert [(edge.source, edge.target) for edge in status.dependency_edges] == [
        ("HF-1", "HF-3")
    ]
    assert set(status.classes) == {"HF-1", "HF-3"}
    assert status.open_keys == ("HF-1", "HF-3")
    assert (
        ledger_edit.main(["close", "HF-1", "--fix", "Independent proof."], root=root)
        == 0
    )
    status = _status(root)
    assert set(status.classes) == {"HF-3"}
    assert status.open_keys == ("HF-3",)
    assert finding_status.dependency_predecessors(status)["HF-3"] == frozenset({"HF-1"})
    from tools import dag_dispatch
    from types import SimpleNamespace

    engine = SimpleNamespace(
        active=True, task="engine-owner", payload={"dispatch": {"family": "A"}}
    )
    assert dag_dispatch.plan_dispatch(status, [engine], ["HF-3"]).waits == {"HF-3": ()}


def _ledger(root: Path) -> str:
    return (root / ledger_edit.LEDGER).read_text(encoding="utf-8")


def test_v01_set_creates_the_table_and_restates_a_row(tmp_path: Path) -> None:
    root = _root(tmp_path)
    # No v0.1 table: every open row is unclassified, so every one holds 0.x.
    assert _status(root).v01_classes == {}
    assert _status(root).v01_holding_keys == ("HF-1", "HF-2")

    assert ledger_edit.main(["v01", "set", "HF-2", "tier2"], root=root) == 0
    status = _status(root)
    assert status.problems == ()
    assert status.v01_classes == {"HF-2": finding_status.V01Class.TIER2}
    assert status.v01_holding_keys == ("HF-1",)
    assert "| Row | v0.1 class |" in _ledger(root)

    assert ledger_edit.main(["v01", "set", "HF-2", "v0_1_blocking"], root=root) == 0
    assert ledger_edit.main(["v01", "set", "HF-1", "post_v0_1"], root=root) == 0
    status = _status(root)
    assert status.v01_classes == {
        "HF-1": finding_status.V01Class.POST_V0_1,
        "HF-2": finding_status.V01Class.V0_1_BLOCKING,
    }
    assert status.v01_holding_keys == ("HF-2",)
    # The table stays in canonical finding order after an insertion.
    table = _ledger(root).split("| Row | v0.1 class |")[1]
    assert table.index("| HF-1 |") < table.index("| HF-2 |")


def test_v01_refuses_a_closed_row_and_an_unknown_class(tmp_path: Path) -> None:
    root = _root(tmp_path)
    assert ledger_edit.main(["v01", "set", "HF-0", "tier2"], root=root) == 2
    data = tmp_path / "classes.json"
    data.write_text(
        '{"schema": "molt.v01-classification.v1", "rows": {"HF-1": "maybe"}}',
        encoding="utf-8",
    )
    assert ledger_edit.main(["v01", "apply", str(data)], root=root) == 2
    assert _status(root).v01_classes == {}


def test_v01_apply_writes_every_row_once(tmp_path: Path) -> None:
    root = _root(tmp_path)
    data = tmp_path / "classes.json"
    data.write_text(
        '{"schema": "molt.v01-classification.v1",'
        ' "rows": {"HF-2": "tier2", "HF-1": "v0_1_blocking"}}',
        encoding="utf-8",
    )
    assert ledger_edit.main(["v01", "apply", str(data)], root=root) == 0
    status = _status(root)
    assert status.problems == ()
    assert status.v01_classes == {
        "HF-1": finding_status.V01Class.V0_1_BLOCKING,
        "HF-2": finding_status.V01Class.TIER2,
    }


def test_close_and_merge_clear_the_v01_class_and_merge_keeps_the_stricter(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    assert ledger_edit.main(["v01", "set", "HF-1", "v0_1_blocking"], root=root) == 0
    assert ledger_edit.main(["v01", "set", "HF-2", "tier2"], root=root) == 0
    assert ledger_edit.main(["merge", "HF-1", "--into", "HF-2"], root=root) == 0
    status = _status(root)
    assert status.problems == ()
    # HF-1 blocked v0.1, so the row that absorbed it now blocks too.
    assert status.v01_classes == {"HF-2": finding_status.V01Class.V0_1_BLOCKING}
    assert ledger_edit.main(["close", "HF-2", "--fix", "Fixed."], root=root) == 0
    status = _status(root)
    assert status.problems == ()
    assert status.v01_classes == {}


def test_merge_of_an_unclassified_row_holds_like_blocking(tmp_path: Path) -> None:
    root = _root(tmp_path)
    assert ledger_edit.main(["v01", "set", "HF-2", "post_v0_1"], root=root) == 0
    assert ledger_edit.main(["merge", "HF-1", "--into", "HF-2"], root=root) == 0
    assert _status(root).v01_classes == {"HF-2": finding_status.V01Class.V0_1_BLOCKING}


def test_open_accepts_a_v01_class(tmp_path: Path) -> None:
    root = _root(tmp_path)
    status = ledger_edit.main(
        [
            "open",
            "HF-3",
            "--v1-class",
            "post_1_0",
            "--v01-class",
            "tier2",
            "--family",
            "B",
            "--section",
            "Open: release blockers",
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
    assert _status(root).v01_classes == {"HF-3": finding_status.V01Class.TIER2}


def test_projection_reports_duplicate_unknown_and_closed_v01_classes() -> None:
    table = "\n| Row | v0.1 class |\n| --- | --- |\n"
    duplicate = finding_status.project(
        _LEDGER.replace(
            "| HF-2 | release_blocking |\n",
            "| HF-2 | release_blocking |\n"
            + table
            + "| HF-1 | tier2 |\n| HF-1 | v0_1_blocking |\n",
        )
    )
    assert any("duplicate" in problem for problem in duplicate.problems)
    unknown = finding_status.project(
        _LEDGER.replace(
            "| HF-2 | release_blocking |\n",
            "| HF-2 | release_blocking |\n" + table + "| HF-1 | soon |\n",
        )
    )
    assert any("unknown class 'soon'" in problem for problem in unknown.problems)
    closed = finding_status.project(
        _LEDGER.replace(
            "| HF-2 | release_blocking |\n",
            "| HF-2 | release_blocking |\n" + table + "| HF-0 | tier2 |\n",
        )
    )
    assert closed.problems == ()
    assert any(
        "v0.1 class on closed or unknown row HF-0" in problem
        for problem in finding_status.dependency_problems(closed)
    )

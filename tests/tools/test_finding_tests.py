"""The executable-ledger contract: one typed finding identity, strict owners.

The oracle is the findings ledger itself (`tools/finding_status.py`): a
finding is citable exactly while its row is open, by every record site (pytest
mark, MOLT_META, expectation manifest) and by the coverage projection.
"""

from __future__ import annotations

import ast
import sys
from pathlib import Path

import pytest

from tests import findings
from tests.process_guard_common import run_guarded_test_process
from tests.findings.finding_ids import FIXED_FINDING_IDS, OPEN_FINDING_IDS
from tools import check_suite_honesty, ci_triage, finding_coverage, gen_finding_ids
from tools import cpython_regrtest
from tools.compat import regrtest_expectations, test_policy
from tools.compat.item_outcome import ItemOutcome

OPEN_ID = sorted(OPEN_FINDING_IDS)[0]
OTHER_OPEN_ID = sorted(OPEN_FINDING_IDS)[1]
FIXED_ID = sorted(FIXED_FINDING_IDS)[0]
PIN = "expect_molt_returncode=1 expect_molt_stdout_sha256=" + "0" * 64

LEDGER = """# Ledger

## Open: correctness

| ID | Finding | Evidence |
|---|---|---|
| HF-7 | open one | e |
| V1-3 (shared widths) | open part | e |

## Fixed after the handoff

| ID | Finding | Evidence |
|---|---|---|
| HF-F2 (owner's "latest") | fixed one | done |
| HF-5 (was HF-4) | fixed renamed | done |
"""


def _ledger_root(tmp_path: Path, text: str) -> Path:
    ledger = tmp_path / "docs" / "agent" / "V1_HANDOFF_FINDINGS.md"
    ledger.parent.mkdir(parents=True)
    ledger.write_text(text, encoding="utf-8")
    return tmp_path


def _literal_members(source: str, name: str) -> list[str]:
    for node in ast.parse(source).body:
        if isinstance(node, ast.Assign) and node.targets[0].id == name:  # type: ignore[attr-defined]
            value = node.value
            if isinstance(value, ast.Name):
                return []
            elements = value.slice.elts  # type: ignore[attr-defined]
            return [element.value for element in elements]
    raise AssertionError(f"{name} is not generated")


def test_generated_finding_ids_are_in_sync() -> None:
    for path, text in gen_finding_ids.generated_outputs().items():
        assert path.read_text(encoding="utf-8") == text


def test_generator_projects_open_and_fixed_rows_as_closed_types(
    tmp_path: Path,
) -> None:
    source = gen_finding_ids.render(_ledger_root(tmp_path, LEDGER))

    assert _literal_members(source, "OpenFindingId") == [
        "HF-7",
        "V1-3 (shared widths)",
    ]
    # A qualified key round-trips through its literal, quotes included.
    assert _literal_members(source, "FixedFindingId") == [
        "HF-5",
        'HF-F2 (owner\'s "latest")',
    ]
    namespace: dict[str, object] = {}
    exec(compile(source, "finding_ids.py", "exec"), namespace)
    assert namespace["OPEN_FINDING_IDS"] == {"HF-7", "V1-3 (shared widths)"}


def test_generator_renders_an_empty_open_set_as_never(tmp_path: Path) -> None:
    fixed_only = LEDGER.replace("| HF-7 | open one | e |\n", "").replace(
        "| V1-3 (shared widths) | open part | e |\n", ""
    )
    source = gen_finding_ids.render(_ledger_root(tmp_path, fixed_only))

    assert "OpenFindingId = Never\n" in source
    namespace: dict[str, object] = {}
    exec(compile(source, "finding_ids.py", "exec"), namespace)
    assert namespace["OPEN_FINDING_IDS"] == frozenset()


def test_generator_refuses_a_ledger_with_structural_problems(tmp_path: Path) -> None:
    duplicate = LEDGER.replace(
        "| HF-7 | open one | e |", "| HF-7 | a | e |\n| HF-7 | b | e |"
    )
    with pytest.raises(ValueError, match="names 2 rows"):
        gen_finding_ids.render(_ledger_root(tmp_path, duplicate))


def test_only_an_open_finding_can_own_an_expectation() -> None:
    assert findings.finding_problem(OPEN_ID) is None
    assert "is fixed" in str(findings.finding_problem(FIXED_ID))
    assert "is not a finding" in str(findings.finding_problem("HF-999999"))


def _marks(*finding_ids: str) -> tuple[pytest.Mark, ...]:
    return tuple(findings.finding(finding_id).mark for finding_id in finding_ids)  # type: ignore[arg-type]


def test_a_finding_test_names_one_open_finding_under_tests_findings() -> None:
    inside = findings.FINDINGS_ROOT / "test_example.py"
    outside = findings.FINDINGS_ROOT.parent / "test_example.py"

    assert (
        findings.finding_test_problem(inside, _marks(OPEN_ID), explicit_xfail=False)
        is None
    )
    assert "under tests/findings" in str(
        findings.finding_test_problem(outside, _marks(OPEN_ID), explicit_xfail=False)
    )
    assert "exactly one finding" in str(
        findings.finding_test_problem(
            inside, _marks(OPEN_ID, OTHER_OPEN_ID), explicit_xfail=False
        )
    )
    assert "is fixed" in str(
        findings.finding_test_problem(inside, _marks(FIXED_ID), explicit_xfail=False)
    )
    assert "remove the explicit xfail" in str(
        findings.finding_test_problem(inside, _marks(OPEN_ID), explicit_xfail=True)
    )


def test_the_finding_mark_is_a_strict_expected_failure_on_an_assertion() -> None:
    mark = findings.strict_expectation(OPEN_ID).mark

    assert mark.name == "xfail"
    assert mark.kwargs == {
        "strict": True,
        "raises": AssertionError,
        "reason": f"{OPEN_ID} (open finding)",
    }


def _metadata(tmp_path: Path, payload: str) -> test_policy.TestMetadata:
    program = tmp_path / "program.py"
    program.write_text(f"# MOLT_META: {payload}\nprint(1)\n", encoding="utf-8")
    return test_policy.parse_metadata(program)


def test_a_finding_program_names_its_open_finding_and_pins_its_failure(
    tmp_path: Path,
) -> None:
    metadata = _metadata(
        tmp_path,
        f"expect_fail=molt expect_fail_reason=open_finding finding={OPEN_ID} {PIN}",
    )

    assert metadata.expect_molt_fail
    assert metadata.finding == OPEN_ID
    assert metadata.as_record()["finding"] == OPEN_ID


@pytest.mark.parametrize(
    ("payload", "message"),
    [
        (
            f"expect_fail=molt expect_fail_reason=open_finding finding={FIXED_ID} {PIN}",
            "is fixed",
        ),
        (
            f"expect_fail=molt expect_fail_reason=gap finding={OPEN_ID} {PIN}",
            "declare the two together",
        ),
        (
            f"expect_fail=molt expect_fail_reason=open_finding {PIN}",
            "declare the two together",
        ),
        (
            f"expect_fail=molt expect_fail_reason=open_finding finding={OPEN_ID}",
            "pins how Molt fails",
        ),
    ],
)
def test_a_finding_program_refuses_an_unowned_or_unpinned_expectation(
    tmp_path: Path, payload: str, message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        _metadata(tmp_path, payload)


def _dimension(**fields: str) -> dict[str, object]:
    return {
        "dimensions": {
            "native@3.12": {
                "status": "fail",
                "root_cause": "r",
                "evidence": "e",
                **fields,
            }
        }
    }


def test_manifest_owner_is_one_open_finding_or_a_legacy_tracking_owner() -> None:
    validate = check_suite_honesty._validate_dimensions

    assert validate("t", _dimension(finding=OPEN_ID)) == []
    assert validate("t", _dimension(tracking="#3 legacy")) == []
    assert "is fixed" in validate("t", _dimension(finding=FIXED_ID))[0]
    assert "exactly one owner" in validate("t", _dimension())[0]
    assert (
        "exactly one owner"
        in validate("t", _dimension(finding=OPEN_ID, tracking="#3"))[0]
    )


def test_regrtest_expectations_name_a_finding_for_one_pinned_source() -> None:
    manifest = {
        "tests": {},
        "regrtest": {
            "test.test_importlib.test_api::ImportModuleTests::test_module_import": _dimension(
                finding=OPEN_ID, failure="molt rc=1"
            ),
            "test.test_importlib.test_util": _dimension(finding=OPEN_ID),
            "test.test_importlib.test_lazy": _dimension(tracking="#3"),
            "test_importlib.test_api.Class.method": _dimension(finding=OPEN_ID),
            "test.test_zipimport": {
                "dimensions": {
                    "native": {
                        "status": "fail",
                        "finding": OPEN_ID,
                        "root_cause": "r",
                        "evidence": "e",
                    }
                }
            },
        },
    }
    problems = check_suite_honesty.validate_manifest(manifest, frozenset())

    assert not [p for p in problems if "test_module_import" in p]
    # An owned regrtest failure names the exact reason the run reports.
    assert any(
        "test.test_importlib.test_util" in p and "`failure`" in p for p in problems
    )
    assert any("test_lazy" in p and "never free-text" in p for p in problems)
    assert any(
        "test_importlib.test_api.Class.method" in p
        and "not a CPython regrtest case" in p
        for p in problems
    )
    assert any(
        "test.test_zipimport" in p and "<backend>@<version>" in p for p in problems
    )


def test_coverage_counts_every_record_site_and_enforces_family_members() -> None:
    cited = [
        finding_coverage.Citation(OPEN_ID, "program", "tests/differential/stdlib/a.py"),
        finding_coverage.Citation(
            OPEN_ID, "manifest", "test.test_x::C::m [native@3.12]"
        ),
        finding_coverage.Citation(
            FIXED_ID, "pytest", "tests/findings/test_x.py::test_y"
        ),
    ]
    families = {"pilot": (OPEN_ID, OTHER_OPEN_ID)}
    lanes = {"findings.pilot.native": ("tests/differential/stdlib/other.py",)}

    coverage = finding_coverage.project(cited, families, lanes)

    (family,) = coverage.families
    assert (family.expected_failures, family.covered, family.uncovered) == (
        2,
        (OPEN_ID,),
        (OTHER_OPEN_ID,),
    )
    problems = "\n".join(coverage.problems)
    assert (
        "tests/findings/test_x.py::test_y (pytest)" in problems
        and "is fixed" in problems
    )
    assert f"open finding {OTHER_OPEN_ID} has no finding test" in problems
    assert "named lane findings.pilot.native must run exactly" in problems


def test_the_repository_projection_reads_finding_programs_and_its_lane() -> None:
    cited = finding_coverage.citations()
    programs = [c for c in cited if c.channel == "program"]

    assert programs, "the import-system pilot pins its finding programs"
    for citation in programs:
        metadata = test_policy.parse_metadata(finding_coverage.ROOT / citation.test)
        assert metadata.finding == citation.finding
    assert (
        finding_coverage.project(cited, lanes=finding_coverage.lane_cases()).problems
        == ()
    )


def test_triage_takes_an_expectation_owner_before_text_matching() -> None:
    rows = ci_triage.open_ledger_rows(
        LEDGER.replace("open one", "mentions test_example_case")
    )
    failure = "tests/findings/test_x.py::test_example_case"

    assert rows == [
        ("HF-7", "| HF-7 | mentions test_example_case | e |"),
        ("V1-3 (shared widths)", "| V1-3 (shared widths) | open part | e |"),
    ]
    assert ci_triage.ledger_owners(failure, rows) == ["HF-7"]
    assert ci_triage.ledger_owners(failure, rows, {failure: OPEN_ID}) == [OPEN_ID]


def test_finding_identity_imports_no_test_framework() -> None:
    # The differential and regrtest tools validate identities at their startup.
    child = run_guarded_test_process(
        [
            sys.executable,
            "-B",
            "-c",
            "import sys; import tests.findings; print('pytest' in sys.modules)",
        ],
        cwd=finding_coverage.ROOT,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert child.returncode == 0, child.stderr
    assert child.stdout.strip() == "False"


def _outcome(status: str, reason: str = "") -> ItemOutcome:
    return ItemOutcome(status=status, reason=reason, detail="", duration_s=1.0)  # type: ignore[arg-type]


def test_regrtest_runs_are_strict_in_both_directions() -> None:
    entry = {
        "status": "fail",
        "finding": OPEN_ID,
        "root_cause": "r",
        "evidence": "e",
        "failure": "molt rc=1",
    }
    manifest = {
        "regrtest": {
            "test.test_owned": {"dimensions": {"native@3.12": entry}},
            "test.test_moved": {"dimensions": {"native@3.12": entry}},
            "test.test_fixed": {"dimensions": {"native@3.12": entry}},
            "test.test_skipped": {"dimensions": {"native@3.12": entry}},
            "test.test_absent": {"dimensions": {"native@3.12": entry}},
            "test.test_cases::C::test_m": {"dimensions": {"native@3.12": entry}},
            "test.test_unselected": {"dimensions": {"native@3.12": entry}},
            "test.test_other_dimension": {"dimensions": {"wasm@3.12": entry}},
        }
    }
    outcomes = {
        "test_owned": _outcome("failed", "molt rc=1"),
        # An owned test that now fails another way is a new defect.
        "test_moved": _outcome("failed", "molt rc=143"),
        "test_fixed": _outcome("passed"),
        "test_skipped": _outcome("skipped", "resource denied"),
        "test_new": _outcome("failed", "MOLT_COMPAT_ERROR: x"),
        "test_hung": _outcome("errors", "timeout"),
        "test_cases": _outcome("failed", "molt rc=1"),
        "test_other_dimension": _outcome("failed", "molt rc=1"),
    }
    selected = [*outcomes, "test_absent"]

    report = regrtest_expectations.classify(
        outcomes, manifest, dimension="native@3.12", selected=selected
    )

    assert report.expected == {"test.test_owned": OPEN_ID}
    assert set(report.unexpected_failures) == {
        "test.test_new",
        "test.test_moved",
        "test.test_cases",
        "test.test_other_dimension",
    }
    assert report.unexpected_passes == {"test.test_fixed": OPEN_ID}
    assert set(report.unconfirmed) == {
        "test.test_skipped",
        "test.test_absent",
        "test.test_cases::C::test_m",
    }
    assert report.errors == {"test.test_hung": "timeout"}
    assert report.returncode == 1


def test_regrtest_run_with_only_owned_failures_passes() -> None:
    entry = {
        "status": "fail",
        "finding": OPEN_ID,
        "root_cause": "r",
        "evidence": "e",
        "failure": "molt rc=1",
    }
    manifest = {"regrtest": {"test.test_owned": {"dimensions": {"native@3.12": entry}}}}
    report = regrtest_expectations.classify(
        {"test_owned": _outcome("failed", "molt rc=1"), "test_ok": _outcome("passed")},
        manifest,
        dimension="native@3.12",
        selected=["test_owned", "test_ok"],
    )
    assert (report.expected, report.returncode) == ({"test.test_owned": OPEN_ID}, 0)


def test_an_explicit_regrtest_selection_is_never_inverted_by_the_skip_list(
    tmp_path: Path,
) -> None:
    # libregrtest's -x turns every positional test into an exclusion and runs
    # the rest of the suite.
    config = cpython_regrtest.parse_args(
        ["--cpython-dir", str(tmp_path), "--dry-run", "test_importlib"]
    )
    command = cpython_regrtest.build_regrtest_cmd(
        config, tmp_path / "regrtest.py", ["test_importlib", "test_ftplib"], ["py"]
    )
    assert "-x" not in command
    assert command[-1] == "test_importlib"
    whole_suite = cpython_regrtest.parse_args(
        ["--cpython-dir", str(tmp_path), "--dry-run"]
    )
    command = cpython_regrtest.build_regrtest_cmd(
        whole_suite, tmp_path / "regrtest.py", ["test_ftplib"], ["py"]
    )
    assert command[command.index("-x") + 1] == "test_ftplib"


_DAG_LEDGER = """# Ledger

## Release dependency and landing order

### Family dependency order (2026-10-29)

| Family | Stratum | HF-459 axes | Shared cause or missing authority | Open rows |
| --- | --- | --- | --- | --- |
| A. Typed ABI | engine | loop/per_op | Hand tables | HF-1, HF-2 |
| B. Binding flow | semantics | - | Worst-case rebinding | {b_rows} |

| From | To | Reason |
| --- | --- | --- |
| A | B | binding facts need the typed ABI |

| Row | V1 class |
| --- | --- |
| HF-1 | release_blocking |
| HF-2 | release_blocking |
| HF-3 | release_apparatus |

## Open: correctness and release blockers

| ID | Defect | Evidence | Next |
|----|--------|----------|------|
| HF-1 | d | e | n |
| HF-2 | d | e | n |
| HF-3 | d | e | n |

## Fixed after the handoff

| ID | Defect | Fix |
|----|--------|-----|
| HF-4 | d | fixed by x |
"""
_PRIOR_ART = "# Prior art\n\n## A. Typed ABI\n\ntext\n\n## B. Binding flow\n\ntext\n"


def test_every_per_op_ratchet_axis_has_an_engine_family() -> None:
    """A measured per-op cost with no owning engine family is a coverage gap."""
    from tools import finding_status

    status = finding_status.project(_DAG_LEDGER.format(b_rows="HF-3"))
    owned = {"axes": {"loop/per_op": {}, "empty/startup": {}, "loop/startup": {}}}
    # Startup axes all measure process start; only per-op axes need an owner.
    assert finding_coverage.ratchet_axis_problems(status, owned) == []
    # B is a semantics family: naming an axis there owns no engine cost.
    semantics_only = finding_status.project(
        _DAG_LEDGER.format(b_rows="HF-3").replace(
            "| B. Binding flow | semantics | - |",
            "| B. Binding flow | semantics | call/per_op |",
        )
    )
    assert finding_coverage.ratchet_axis_problems(
        semantics_only, {"axes": {"loop/per_op": {}, "call/per_op": {}}}
    ) == [
        "ratchet axis call/per_op has no engine family; "
        "name it in one family's HF-459 axes"
    ]
    assert finding_coverage.ratchet_axis_problems(status, {}) == [
        "bench/primitive_ratchet.json has no axes table"
    ]


def test_every_open_finding_sits_in_one_dependency_family_with_prior_art() -> None:
    from tools import finding_status

    complete = finding_status.project(_DAG_LEDGER.format(b_rows="HF-3"))
    assert [family.title for family in complete.dependency_families] == [
        "A. Typed ABI",
        "B. Binding flow",
    ]
    assert finding_coverage.dependency_order_problems(complete, _PRIOR_ART) == []

    # A new open row outside the DAG, a closed row listed, and a row placed twice.
    for b_rows, expected in (
        ("", "open finding HF-3 is in no dependency family"),
        ("HF-3, HF-4", "lists HF-4, which is not an open finding"),
        ("HF-3, HF-1", "open finding HF-1 sits in families A, B"),
    ):
        status = finding_status.project(_DAG_LEDGER.format(b_rows=b_rows))
        problems = finding_coverage.dependency_order_problems(status, _PRIOR_ART)
        assert any(expected in problem for problem in problems), problems

    # A family without its prior-art section, and a ledger without the table.
    problems = finding_coverage.dependency_order_problems(
        complete, "# Prior art\n\n## A. Typed ABI\n"
    )
    assert problems == [
        "dependency family B. Binding flow has no section in "
        "docs/design/findings_prior_art.md"
    ]
    bare = finding_status.project("## Open:" + _DAG_LEDGER.split("## Open:", 1)[1])
    assert finding_coverage.dependency_order_problems(bare, _PRIOR_ART) == [
        "the ledger has no family dependency order table"
    ]


def _dag_with(edges: str, extra: str = "") -> str:
    table = "| From | To | Reason |\n| --- | --- | --- |\n" + edges if edges else ""
    return _DAG_LEDGER.format(b_rows="HF-3").replace(
        "| From | To | Reason |\n| --- | --- | --- |\n"
        "| A | B | binding facts need the typed ABI |\n",
        table + extra,
    )


def test_row_prerequisites_derive_from_the_edges_table() -> None:
    from tools import finding_status

    status = finding_status.project(_DAG_LEDGER.format(b_rows="HF-3"))
    assert finding_status.dependency_predecessors(status)["HF-3"] == frozenset(
        {"HF-1", "HF-2"}
    )
    chain = finding_status.project(
        _dag_with("| A | HF-3 | b needs a |\n| HF-3 | HF-1 | a needs b |\n")
    )
    with pytest.raises(
        finding_status.DependencyCycleError, match="row dependency cycle"
    ):
        finding_status.dependency_order(chain)


@pytest.mark.parametrize(
    ("edges", "extra", "expected"),
    [
        ("", "", "has no edges table"),
        ("| A | Z | z needs a |\n", "", "names unknown endpoint Z"),
        ("| A | A | itself |\n", "", "orders an endpoint after itself"),
        ("| A | B |  |\n", "", "gives no reason"),
        ("| A | B | x |\n| A | B | y |\n", "", "repeats the edge"),
        ("| A | B | x |\n| B | A | y |\n", "", "row dependency cycle"),
        (
            "| A | B | x |\n",
            "\nEdges: A -> B (prose copy).\n",
            "states the family order in prose",
        ),
        (
            "| A | B | x |\n",
            "\nLevels: 0 = A; 1 = B.\n",
            "states the family order in prose",
        ),
    ],
)
def test_edges_table_refuses_every_drift(edges: str, extra: str, expected: str) -> None:
    from tools import finding_status

    status = finding_status.project(_dag_with(edges, extra))
    problems = finding_coverage.dependency_order_problems(status, _PRIOR_ART)
    assert any(expected in problem for problem in problems), problems


@pytest.mark.parametrize("run_flaky,expected_status", [(False, 0), (True, 1)])
def test_flaky_collection_skips_presubmit_and_executes_postsubmit(
    tmp_path, run_flaky, expected_status
):
    import os

    project = tmp_path / "flaky-project"
    project.mkdir()
    (project / "pytest.ini").write_text(
        "[pytest]\naddopts =\nmarkers = flaky(finding_id): known random flake\n"
    )
    (project / "conftest.py").write_text(
        "from tests.conftest import pytest_addoption, pytest_collection_modifyitems\n"
    )
    executed = project / "executed"
    (project / "test_flake.py").write_text(
        "from pathlib import Path\nfrom tests.findings import flaky\n"
        f"@flaky({OPEN_ID!r})\n"
        "def test_flake():\n"
        f"    Path({str(executed)!r}).touch()\n"
        "    assert False, 'postsubmit executes the original defect'\n"
    )
    root = Path(__file__).resolve().parents[2]
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-n",
            "0",
            *(["--run-flaky"] if run_flaky else []),
            str(project / "test_flake.py"),
        ],
        cwd=project,
        env=dict(
            os.environ, PYTHONPATH=os.pathsep.join((str(root), str(root / "src")))
        ),
        timeout=180,
    )
    assert result.returncode == expected_status, result.stdout + result.stderr
    assert executed.exists() is run_flaky
    assert ("1 failed" if run_flaky else "1 skipped") in result.stdout


@pytest.mark.parametrize(
    "mark",
    [
        pytest.mark.flaky(FIXED_ID).mark,
        pytest.mark.flaky("HF-999999").mark,
        pytest.mark.flaky().mark,
    ],
)
def test_invalid_flaky_identity_is_a_collection_error(mark):
    from types import SimpleNamespace
    from tests.conftest import pytest_collection_modifyitems

    item = SimpleNamespace(
        nodeid="tests/example.py::test_flake",
        path=Path("tests/example.py"),
        iter_markers=lambda name: iter([mark] if name == "flaky" else []),
        add_marker=lambda value: pytest.fail(
            "invalid flake was marked instead of refused"
        ),
    )
    with pytest.raises(pytest.UsageError, match="exactly one|not a finding|is fixed"):
        pytest_collection_modifyitems(
            SimpleNamespace(getoption=lambda name: False), [item]
        )

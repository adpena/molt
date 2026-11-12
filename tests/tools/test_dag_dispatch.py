"""Ready-row dispatch over the independent integration fixture ledger."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

import pytest

from tests.integration_driver_fixture import FIXTURE_LEDGER
from tools import dag_dispatch, finding_coverage, finding_status
from tools.dag_dispatch import DispatchReason, DispatchRefused


@dataclass
class _Record:
    task: str
    active: bool = True
    payload: dict[str, Any] = field(default_factory=dict)


def _lane(task: str, family: str, *, active: bool = True) -> _Record:
    return _Record(task, active, {"dispatch": {"family": family}})


STATUS = finding_status.project(FIXTURE_LEDGER)
PRIOR_ART = "\n".join(f"## {f.title}" for f in STATUS.dependency_families)


def _closed_one() -> finding_status.FindingStatus:
    # Move the row to the fixture's fixed section, remove only its open
    # membership and family entry. Leave the row edge's family endpoint intact.
    return finding_status.project(
        FIXTURE_LEDGER.replace(
            "| A. First | engine | loop/per_op | a | HF-1, HF-3, HF-4 |",
            "| A. First | engine | loop/per_op | a | HF-3, HF-4 |",
        )
        .replace("| HF-1 | release_blocking |\n", "")
        .replace("| HF-1 | d | e | n |\n", "")
        + "| HF-1 | d | closed by independent proof |\n"
    )


def test_ready_dispatch_keeps_rows_family_and_waits() -> None:
    assert dag_dispatch.plan_dispatch(STATUS, [], ["HF-1"]).record() == {
        "rows": ["HF-1"],
        "family": "A",
        "waits": {"HF-1": []},
        "reason": "dag_ready",
        "text": "",
    }
    # A ready row in a family with a waiting sibling is independently admitted.
    assert (
        dag_dispatch.plan_dispatch(STATUS, [_lane("engine", "A")], ["HF-6"]).family
        == "C"
    )


def test_claiming_predecessor_does_not_close_it() -> None:
    with pytest.raises(DispatchRefused, match="HF-2 waits on HF-1"):
        dag_dispatch.plan_dispatch(STATUS, [_lane("first", "A")], ["HF-2"])
    with pytest.raises(DispatchRefused, match="HF-5 waits on HF-2"):
        dag_dispatch.plan_dispatch(STATUS, [_lane("engine", "A")], ["HF-5", "HF-6"])


def test_family_edge_never_holds_v1_behind_non_v1_siblings() -> None:
    status = _closed_one()
    assert finding_coverage.dependency_order_problems(status, PRIOR_ART) == []
    assert dag_dispatch.plan_dispatch(
        status, [_lane("engine", "A")], ["HF-2"]
    ).waits == {"HF-2": ()}
    assert {"HF-3", "HF-4"} <= set(status.open_keys)


def test_priority_uses_longest_open_v1_chain_then_unique_descendants() -> None:
    # 1 -> 2 -> {5,7}; 6 is independent. Non-v1 rows do not enter ranking.
    plan = dag_dispatch.row_plan(STATUS)
    assert [(r.row, r.critical_path, r.downstream) for r in plan] == [
        ("HF-1", 3, 3),
        ("HF-2", 2, 2),
        ("HF-5", 1, 0),
        ("HF-6", 1, 0),
        ("HF-7", 1, 0),
    ]
    assert [r.row for r in plan if r.ready] == ["HF-1", "HF-6"]
    # Equal longest paths: six's two descendants outrank one's single child,
    # despite its greater numeric key.
    status = finding_status.project(
        FIXTURE_LEDGER.replace(
            "| HF-2 | HF-5 | five consumes two |",
            "| HF-6 | HF-5 | five consumes six |",
        ).replace(
            "| HF-2 | HF-7 | seven consumes two |",
            "| HF-6 | HF-7 | seven consumes six |",
        )
    )
    assert [
        (r.row, r.critical_path, r.downstream)
        for r in dag_dispatch.row_plan(status)
        if r.ready
    ] == [("HF-6", 2, 2), ("HF-1", 2, 1)]
    diamond = finding_status.project(
        FIXTURE_LEDGER.replace(
            "| HF-2 | HF-7 | seven consumes two |",
            "| HF-2 | HF-7 | seven consumes two |\n| HF-1 | HF-6 | six consumes one |\n| HF-6 | HF-5 | shared downstream five |",
        )
    )
    first = dag_dispatch.row_plan(diamond)[0]
    assert (first.row, first.critical_path, first.downstream) == ("HF-1", 3, 4)
    rendered = dag_dispatch.render_plan(
        dag_dispatch.row_plan(STATUS, [_lane("owner", "A")])
    )
    assert "HF-1 family=A path=3 downstream=3 owner" in rendered
    assert "HF-2 family=B path=2 downstream=2 waits=HF-1" in rendered


def test_second_family_lane_requires_recorded_departure() -> None:
    records = [_lane("lane-a", "A")]
    with pytest.raises(DispatchRefused, match="family A already has a lane"):
        dag_dispatch.plan_dispatch(STATUS, records, ["HF-1"])
    assert dag_dispatch.plan_dispatch(
        STATUS, [_lane("old", "A", active=False)], ["HF-1"]
    )
    dispatch = dag_dispatch.plan_dispatch(
        STATUS,
        records,
        ["HF-1"],
        reason=DispatchReason.OWNER_DIRECTED,
        text="owner split",
    )
    assert dispatch.text == "owner split"


@pytest.mark.parametrize(
    ("rows", "reason", "text", "expected"),
    [
        ([], DispatchReason.DAG_READY, "", "name the open rows"),
        (["HF-9"], DispatchReason.DAG_READY, "", "not open findings: HF-9"),
        (["HF-77"], DispatchReason.DAG_READY, "", "not open findings: HF-77"),
        (["HF-1", "HF-2"], DispatchReason.DAG_READY, "", "span families A, B"),
        (["HF-5"], DispatchReason.NEEDS_NOTHING, "  ", "needs --reason TEXT"),
        (["HF-1"], DispatchReason.INTEGRATOR_LEDGER, "ledger", "names no rows"),
    ],
)
def test_malformed_dispatch_is_refused(rows, reason, text, expected) -> None:
    with pytest.raises(DispatchRefused, match=expected):
        dag_dispatch.plan_dispatch(STATUS, [], rows, reason=reason, text=text)


def test_departure_and_ledger_lanes_keep_reason_and_actual_waits() -> None:
    early = dag_dispatch.plan_dispatch(
        STATUS,
        [_lane("engine", "A")],
        ["HF-5"],
        reason=DispatchReason.NEEDS_NOTHING,
        text="co-design",
    )
    assert early.waits == {"HF-5": ("HF-2",)}
    ledger = dag_dispatch.plan_dispatch(
        STATUS, [], [], reason=DispatchReason.INTEGRATOR_LEDGER, text="flush"
    )
    assert (ledger.rows, ledger.family, ledger.waits) == ((), None, {})


@pytest.mark.parametrize(
    ("before", "after", "expected"),
    [
        ("| A | B | b needs a |", "| HF-99 | B | unknown |", "unknown endpoint HF-99"),
        ("| A | B | b needs a |", "| A | ZZ | unknown |", "unknown endpoint ZZ"),
        ("| A | B | b needs a |", "| A | B | |", "gives no reason"),
        ("| A | B | b needs a |", "| A | B |", "needs From, To and Reason"),
        ("| A | B | b needs a |", "| A | B | no closing bar", "needs a closing |"),
        (
            "| A | B | b needs a |",
            "| A | B | b needs a |\n| HF-5 | A | mixed cycle |",
            "row dependency cycle",
        ),
        (
            "| A | B | b needs a |",
            "| HF-5 | HF-2 | row cycle |",
            "row dependency cycle",
        ),
        (
            "| A | B | b needs a |",
            "| A | B | b needs a |\n| B | A | family cycle |",
            "row dependency cycle",
        ),
        (
            "| A | B | b needs a |",
            "| HF-3 | HF-2 | invalid tier |",
            "v1 row HF-2 depends on non-v1 row HF-3",
        ),
        ("| HF-1 | release_blocking |\n", "", "HF-1 has 0 v1 classes"),
        (
            "| HF-1 | release_blocking |",
            "| HF-1 | release_blocking |\n| HF-1 | release_apparatus |",
            "HF-1 has 2 v1 classes",
        ),
        ("| HF-1 | release_blocking |", "| HF-1 | mystery |", "unknown class"),
        (
            "| HF-1 | release_blocking |",
            "| HF-1 | release_blocking |\n| HF-9 | apparatus |",
            "class on closed or unknown row HF-9",
        ),
        (
            "| HF-1 | release_blocking |",
            "| HF-1 | release_blocking |\n| HF-99 | apparatus |",
            "class on closed or unknown row HF-99",
        ),
    ],
)
def test_coverage_refuses_invalid_row_graph_and_membership(
    before, after, expected
) -> None:
    status = finding_status.project(FIXTURE_LEDGER.replace(before, after))
    problems = finding_coverage.dependency_order_problems(status, PRIOR_ART)
    assert any(expected in problem for problem in problems), problems
    # A malformed authority cannot become an empty ready set or silently
    # remove a prerequisite at either dispatch consumer.
    with pytest.raises(DispatchRefused, match="invalid ledger DAG"):
        dag_dispatch.plan_dispatch(status, [_lane("engine", "A")], ["HF-2"])
    with pytest.raises(DispatchRefused, match="invalid ledger DAG"):
        dag_dispatch.row_plan(status)


def test_family_endpoints_expand_all_three_tiers_and_both_directions() -> None:
    text = (
        FIXTURE_LEDGER.replace(
            "| B. Second | semantics | - | b | HF-2 |",
            "| B. Second | semantics | - | b | HF-2, HF-8, HF-10 |",
        )
        .replace(
            "| HF-2 | release_apparatus |",
            "| HF-2 | release_apparatus |\n| HF-8 | post_1_0 |\n| HF-10 | apparatus |",
        )
        .replace(
            "## Fixed after the handoff",
            "| HF-8 | later | e | n |\n| HF-10 | apparatus | e | n |\n\n## Fixed after the handoff",
        )
    )
    status = finding_status.project(text)
    assert finding_status.dependency_problems(status) == []
    predecessors = finding_status.dependency_predecessors(status)
    assert {key: predecessors[key] for key in ("HF-2", "HF-8", "HF-10")} == {
        "HF-2": frozenset({"HF-1"}),
        "HF-8": frozenset({"HF-3"}),
        "HF-10": frozenset({"HF-4"}),
    }
    row_to_family = finding_status.project(
        text.replace("| A | B | b needs a |", "| HF-1 | B | direct prerequisite |")
    )
    assert finding_status.dependency_predecessors(row_to_family)["HF-2"] == frozenset(
        {"HF-1"}
    )
    assert finding_status.dependency_predecessors(row_to_family)["HF-8"] == frozenset()
    family_to_row = finding_status.project(
        text.replace("| A | B | b needs a |", "| A | HF-8 | post-tier prerequisite |")
    )
    assert finding_status.dependency_predecessors(family_to_row)["HF-8"] == frozenset(
        {"HF-3"}
    )


@pytest.mark.parametrize("stratum", ["", "other"])
def test_coverage_and_dispatch_refuse_missing_or_unknown_strata(stratum) -> None:
    text = FIXTURE_LEDGER.replace("| A. First | engine |", f"| A. First | {stratum} |")
    status = finding_status.project(text)
    assert any(
        "stratum" in problem
        for problem in finding_coverage.dependency_order_problems(status, PRIOR_ART)
    )
    with pytest.raises(DispatchRefused, match="stratum"):
        dag_dispatch.plan_dispatch(status, [], ["HF-1"])


@pytest.mark.parametrize(
    "reason",
    [
        DispatchReason.DAG_READY,
        DispatchReason.NEEDS_NOTHING,
        DispatchReason.ROW_ORDER_EDGE,
        DispatchReason.OWNER_DIRECTED,
    ],
)
@pytest.mark.parametrize("family,row", [("B", "HF-2"), ("C", "HF-6"), ("D", "HF-7")])
def test_non_engine_capacity_cannot_be_bypassed_by_departure(
    reason, family, row
) -> None:
    with pytest.raises(
        DispatchRefused, match="engine capacity rule.*active lanes: B: semantic-owner"
    ):
        dag_dispatch.plan_dispatch(
            STATUS,
            [_lane("semantic-owner", "B"), _lane("ended-engine", "A", active=False)],
            [row],
            reason=reason,
            text="recorded departure",
        )


def test_capacity_with_live_engine_and_apparatus_custody() -> None:
    # Remove only the fixture's prerequisite of seven so all three strata have
    # ready representatives; this does not change capacity facts.
    status = finding_status.project(
        FIXTURE_LEDGER.replace("| HF-2 | HF-7 | seven consumes two |\n", "")
    )
    engine = _lane("engine-owner", "A")
    assert dag_dispatch.plan_dispatch(status, [engine], ["HF-6"]).family == "C"
    assert dag_dispatch.plan_dispatch(status, [engine], ["HF-7"]).family == "D"
    assert dag_dispatch.plan_dispatch(_closed_one(), [engine], ["HF-2"]).family == "B"
    with pytest.raises(
        DispatchRefused, match="apparatus capacity rule.*apparatus-owner"
    ):
        dag_dispatch.plan_dispatch(
            status,
            [engine, _lane("apparatus-owner", "D")],
            ["HF-7"],
            reason=DispatchReason.OWNER_DIRECTED,
            text="cannot bypass",
        )
    # Retired claims consume no capacity; active non-engine claims do not stop
    # the first engine lane from starting.
    assert dag_dispatch.plan_dispatch(
        status, [_lane("old", "D", active=False), engine], ["HF-7"]
    )
    assert dag_dispatch.plan_dispatch(status, [_lane("apparatus-owner", "D")], ["HF-1"])
    with pytest.raises(
        DispatchRefused, match="engine capacity rule.*active lanes: none"
    ):
        dag_dispatch.plan_dispatch(status, [], ["HF-7"])


def test_only_landing_repair_and_integrator_ledger_are_capacity_exceptions() -> None:
    records = [_lane("apparatus-owner", "D")]
    repair = dag_dispatch.plan_dispatch(
        STATUS,
        records,
        ["HF-7"],
        reason=DispatchReason.LANDING_REPAIR,
        text="repair failed landing",
    )
    assert repair.family == "D" and repair.waits == {"HF-7": ("HF-2",)}
    assert (
        dag_dispatch.plan_dispatch(
            STATUS,
            records,
            [],
            reason=DispatchReason.INTEGRATOR_LEDGER,
            text="reconcile ledger",
        ).family
        is None
    )


def test_render_plan_names_engine_family_axes() -> None:
    rendered = dag_dispatch.render_plan(dag_dispatch.row_plan(STATUS))
    assert "axes=loop/per_op" in rendered


def test_capacity_is_shared_between_distinct_apparatus_families() -> None:
    text = FIXTURE_LEDGER.replace(
        "| D. Fourth | apparatus | - | d | HF-7 |",
        "| D. Fourth | apparatus | - | d | HF-7 |\n| E. Fifth | apparatus | - | e | HF-11 |",
    )
    text = text.replace(
        "| HF-7 | release_blocking |",
        "| HF-7 | release_blocking |\n| HF-11 | release_apparatus |",
    )
    text = text.replace(
        "| HF-7 | d | e | n |",
        "| HF-7 | d | e | n |\n| HF-11 | independent apparatus | e | n |",
    )
    status = finding_status.project(text)
    assert finding_status.dependency_problems(status) == []
    engine = _lane("engine-owner", "A")
    assert dag_dispatch.plan_dispatch(status, [engine], ["HF-11"]).family == "E"
    with pytest.raises(
        DispatchRefused, match="apparatus capacity rule.*apparatus-owner"
    ):
        dag_dispatch.plan_dispatch(
            status, [engine, _lane("apparatus-owner", "D")], ["HF-11"]
        )
    with pytest.raises(
        DispatchRefused, match="engine capacity rule.*unassigned: ledger-owner"
    ):
        dag_dispatch.plan_dispatch(status, [_Record("ledger-owner")], ["HF-11"])

"""The lane work-order gate (`tools/dag_dispatch.py`) over a fixture ledger."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

import pytest

from tools import dag_dispatch, finding_status
from tools.dag_dispatch import DispatchReason, DispatchRefused

# A (level 0) -> B (level 1) -> C (level 2); D is a second level-0 family.
_LEDGER = """# Ledger

## Release dependency and landing order

### Family dependency order

| Family | Shared cause or missing authority | Open rows |
| --- | --- | --- |
| A. First | a | HF-1, HF-2 |
| B. Second | b | HF-3 |
| C. Third | c | HF-4 |
| D. Fourth | d | HF-5 |

| From | To | Reason |
| --- | --- | --- |
| A | B | b needs a |
| B | C | c needs b |

## Open: correctness and release blockers

| ID | Defect | Evidence | Next |
|----|--------|----------|------|
| HF-1 | d | e | n |
| HF-2 | d | e | n |
| HF-3 | d | e | n |
| HF-4 | d | e | n |
| HF-5 | d | e | n |

## Fixed after the handoff

| ID | Defect | Fix |
|----|--------|-----|
| HF-9 | d | fixed by x |
"""


@dataclass
class _Record:
    task: str
    active: bool = True
    payload: dict[str, Any] = field(default_factory=dict)


def _lane(task: str, family: str, *, active: bool = True) -> _Record:
    return _Record(task, active, {"dispatch": {"family": family}})


STATUS = finding_status.project(_LEDGER)


def test_frontier_lane_is_admitted_and_recorded() -> None:
    dispatch = dag_dispatch.plan_dispatch(STATUS, [], ["HF-1", "HF-2"])
    assert dispatch.record() == {
        "rows": ["HF-1", "HF-2"],
        "family": "A",
        "level": 0,
        "frontier": 0,
        "reason": "dag_frontier",
        "text": "",
    }


@pytest.mark.parametrize(
    ("records", "rows", "admitted"),
    [
        # Level 0 still has unclaimed families (A and D), so B waits.
        ([], ["HF-3"], False),
        ([_lane("lane-a", "A")], ["HF-3"], False),
        # With A and D claimed, level 1 is the frontier.
        ([_lane("lane-a", "A"), _lane("lane-d", "D")], ["HF-3"], True),
        # C is level 2: B must be claimed first.
        ([_lane("lane-a", "A"), _lane("lane-d", "D")], ["HF-4"], False),
        # A closed lane, or a legacy record without dispatch facts, claims nothing.
        ([_lane("lane-a", "A", active=False), _lane("lane-d", "D")], ["HF-3"], False),
        ([_Record("legacy"), _lane("lane-d", "D")], ["HF-3"], False),
    ],
)
def test_lanes_above_the_frontier_are_refused(
    records: list[_Record], rows: list[str], admitted: bool
) -> None:
    if admitted:
        dag_dispatch.plan_dispatch(STATUS, records, rows)
        return
    with pytest.raises(DispatchRefused, match=r"but level \d has families"):
        dag_dispatch.plan_dispatch(STATUS, records, rows)


def test_refusal_names_the_waiting_families_and_counts() -> None:
    with pytest.raises(DispatchRefused) as refused:
        dag_dispatch.plan_dispatch(STATUS, [], ["HF-4"])
    assert "family C is level 2" in str(refused.value)
    assert "A (2 open), D (1 open)" in str(refused.value)


def test_a_claimed_family_takes_no_second_lane_without_a_reason() -> None:
    records = [_lane("lane-a", "A")]
    with pytest.raises(
        DispatchRefused, match=r"family A already has a lane \(lane-a\)"
    ):
        dag_dispatch.plan_dispatch(STATUS, records, ["HF-2"])
    dispatch = dag_dispatch.plan_dispatch(
        STATUS,
        records,
        ["HF-2"],
        reason=DispatchReason.OWNER_DIRECTED,
        text="owner split the family",
    )
    assert (dispatch.reason, dispatch.text) == (
        DispatchReason.OWNER_DIRECTED,
        "owner split the family",
    )


@pytest.mark.parametrize(
    ("rows", "reason", "text", "expected"),
    [
        ([], DispatchReason.DAG_FRONTIER, "", "name the open rows"),
        (["HF-9"], DispatchReason.DAG_FRONTIER, "", "not open findings: HF-9"),
        (["HF-77"], DispatchReason.DAG_FRONTIER, "", "not open findings: HF-77"),
        (["HF-1", "HF-3"], DispatchReason.DAG_FRONTIER, "", "span families A, B"),
        (["HF-4"], DispatchReason.NEEDS_NOTHING, "  ", "needs --reason TEXT"),
        (["HF-1"], DispatchReason.INTEGRATOR_LEDGER, "ledger", "names no rows"),
    ],
)
def test_malformed_dispatch_is_refused(
    rows: list[str], reason: DispatchReason, text: str, expected: str
) -> None:
    with pytest.raises(DispatchRefused, match=expected):
        dag_dispatch.plan_dispatch(STATUS, [], rows, reason=reason, text=text)


def test_out_of_order_and_ledger_lanes_keep_their_reason() -> None:
    early = dag_dispatch.plan_dispatch(
        STATUS,
        [],
        ["HF-4"],
        reason=DispatchReason.NEEDS_NOTHING,
        text="HF-4 depends on no B row",
    )
    assert (early.family, early.level, early.frontier) == ("C", 2, 0)
    ledger = dag_dispatch.plan_dispatch(
        STATUS, [], [], reason=DispatchReason.INTEGRATOR_LEDGER, text="flush notes"
    )
    assert (ledger.rows, ledger.family, ledger.level) == ((), None, None)

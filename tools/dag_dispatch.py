"""Admit a new lane against the ledger's family dependency order.

The ledger's edges table is the work order (``finding_status.dependency_levels``).
A lane names the open rows it fixes; they belong to one family, whose derived
level must not exceed the frontier: the lowest level among families that have
open rows and no active lane. A lane above the frontier needs a typed dispatch
reason and its text, and both stay in the lane's coordination record, so every
departure from the order is visible in the live-lane projection.

The function is pure: callers pass the ledger projection and the coordination
records they read.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import asdict, dataclass
from enum import StrEnum
from typing import Any

from tools import finding_status

RECORD_KEY = "dispatch"


class DispatchReason(StrEnum):
    """Why a lane runs; every value except ``DAG_FRONTIER`` departs from the order."""

    DAG_FRONTIER = "dag_frontier"
    NEEDS_NOTHING = "needs_nothing"
    ROW_ORDER_EDGE = "row_order_edge"
    OWNER_DIRECTED = "owner_directed"
    LANDING_REPAIR = "landing_repair"
    INTEGRATOR_LEDGER = "integrator_ledger"


class DispatchRefused(ValueError):
    """The lane does not fit the work order as declared."""


@dataclass(frozen=True, slots=True)
class Dispatch:
    """What a lane's coordination record keeps about its place in the order."""

    rows: tuple[str, ...]
    family: str | None
    level: int | None
    frontier: int | None
    reason: DispatchReason
    text: str

    def record(self) -> dict[str, Any]:
        payload = asdict(self)
        payload["rows"] = list(self.rows)
        payload["reason"] = self.reason.value
        return payload


def active_families(records: Iterable[Any]) -> dict[str, list[str]]:
    """Each family an active lane record claims, with the claiming tasks."""
    claimed: dict[str, list[str]] = {}
    for record in records:
        if not record.active:
            continue
        dispatch = record.payload.get(RECORD_KEY)
        if isinstance(dispatch, Mapping) and isinstance(dispatch.get("family"), str):
            claimed.setdefault(dispatch["family"], []).append(record.task)
    return claimed


def frontier(
    status: finding_status.FindingStatus, claimed: Mapping[str, object]
) -> tuple[int | None, tuple[tuple[str, int], ...]]:
    """The lowest level with an unclaimed family that has open rows, and those families."""
    levels = finding_status.dependency_levels(status)
    open_keys = set(status.open_keys)
    waiting = [
        (family.letter, sum(key in open_keys for key in family.keys))
        for family in status.dependency_families
        if family.letter not in claimed and any(key in open_keys for key in family.keys)
    ]
    if not waiting:
        return None, ()
    lowest = min(levels[letter] for letter, _ in waiting)
    return lowest, tuple(
        (letter, count) for letter, count in waiting if levels[letter] == lowest
    )


def plan_dispatch(
    status: finding_status.FindingStatus,
    records: Sequence[Any],
    rows: Sequence[str],
    *,
    reason: DispatchReason = DispatchReason.DAG_FRONTIER,
    text: str = "",
) -> Dispatch:
    """Admit or refuse a lane that fixes ``rows``; raise ``DispatchRefused``."""
    text = text.strip()
    if reason is not DispatchReason.DAG_FRONTIER and not text:
        raise DispatchRefused(f"dispatch reason {reason.value} needs --reason TEXT")
    if reason is DispatchReason.INTEGRATOR_LEDGER:
        if rows:
            raise DispatchRefused(
                "an integrator ledger lane edits the ledger itself; it names no rows"
            )
        return Dispatch((), None, None, None, reason, text)
    if not rows:
        raise DispatchRefused(
            "name the open rows this lane fixes (--rows HF-n[,HF-m]); integrator "
            "ledger lanes use --dispatch-reason integrator_ledger"
        )
    open_keys = set(status.open_keys)
    unknown = [row for row in rows if row not in open_keys]
    if unknown:
        raise DispatchRefused(f"not open findings: {', '.join(unknown)}")
    family_of = {
        key: family.letter
        for family in status.dependency_families
        for key in family.keys
    }
    families = sorted({family_of[row] for row in rows})
    if len(families) != 1:
        raise DispatchRefused(
            "a lane fixes one family's cause; these rows span families "
            + ", ".join(families)
        )
    family = families[0]
    levels = finding_status.dependency_levels(status)
    claimed = active_families(records)
    lowest, waiting = frontier(status, claimed)
    level = levels[family]
    if reason is DispatchReason.DAG_FRONTIER and family in claimed:
        raise DispatchRefused(
            f"family {family} already has a lane ({', '.join(claimed[family])}); "
            "one lane owns a family's cause. Extend that lane, or give "
            "--dispatch-reason with --reason TEXT"
        )
    if reason is DispatchReason.DAG_FRONTIER and lowest is not None and level > lowest:
        names = ", ".join(f"{letter} ({count} open)" for letter, count in waiting)
        raise DispatchRefused(
            f"family {family} is level {level}, but level {lowest} has families "
            f"with open rows and no lane: {names}. Dispatch one of those, or give "
            "--dispatch-reason (needs_nothing, row_order_edge, owner_directed, "
            "landing_repair) with --reason TEXT"
        )
    return Dispatch(tuple(rows), family, level, lowest, reason, text)

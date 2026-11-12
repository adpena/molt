"""Ready-row dispatch and critical-path priority from the canonical ledger DAG.

A row is ready only after every predecessor closes. Active lanes retain family
custody, but claiming a prerequisite cannot satisfy it. The pure plan ranks v1
rows by their longest downstream chain, then unique downstream row count.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import asdict, dataclass
from enum import StrEnum
from typing import Any

from tools import finding_status

RECORD_KEY = "dispatch"


class DispatchReason(StrEnum):
    """Why a lane runs; every value except DAG_READY needs recorded text."""

    DAG_READY = "dag_ready"
    NEEDS_NOTHING = "needs_nothing"
    ROW_ORDER_EDGE = "row_order_edge"
    OWNER_DIRECTED = "owner_directed"
    LANDING_REPAIR = "landing_repair"
    INTEGRATOR_LEDGER = "integrator_ledger"


class DispatchRefused(ValueError):
    """The lane does not fit the work order as declared."""


def _validate_graph(status: finding_status.FindingStatus) -> None:
    problems = finding_status.dependency_problems(status)
    if problems:
        raise DispatchRefused("invalid ledger DAG: " + "; ".join(problems))


@dataclass(frozen=True, slots=True)
class Dispatch:
    rows: tuple[str, ...]
    family: str | None
    waits: dict[str, tuple[str, ...]]
    reason: DispatchReason
    text: str

    def record(self) -> dict[str, Any]:
        payload = asdict(self)
        payload["rows"] = list(self.rows)
        payload["waits"] = {key: list(values) for key, values in self.waits.items()}
        payload["reason"] = self.reason.value
        return payload


@dataclass(frozen=True, slots=True)
class PlannedRow:
    row: str
    family: str
    classification: finding_status.V1Class
    waits: tuple[str, ...]
    critical_path: int
    downstream: int
    claimed_by: tuple[str, ...]
    stratum: finding_status.FamilyStratum
    axes: tuple[str, ...]

    @property
    def ready(self) -> bool:
        return not self.waits


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


def row_plan(
    status: finding_status.FindingStatus, records: Iterable[Any] = ()
) -> tuple[PlannedRow, ...]:
    """All open v1 rows, ranked by downstream chain (including self), then reach."""
    _validate_graph(status)
    order = finding_status.dependency_order(status)
    predecessors = finding_status.dependency_predecessors(status)
    open_keys = set(status.open_keys)
    classes = status.classes
    v1 = {key for key, cls in classes.items() if cls.tier == "v1" and key in open_keys}
    successors: dict[str, set[str]] = {key: set() for key in v1}
    for target in v1:
        for source in predecessors[target] & v1:
            successors[source].add(target)
    reach: dict[str, set[str]] = {}
    paths: dict[str, int] = {}
    for key in reversed(order):
        if key not in v1:
            continue
        children = successors[key]
        reach[key] = set(children)
        for child in children:
            reach[key].update(reach[child])
        paths[key] = 1 + max((paths[child] for child in children), default=0)
    family_of = {
        key: family.letter
        for family in status.dependency_families
        for key in family.keys
    }
    claimed = active_families(records)
    families = {family.letter: family for family in status.dependency_families}
    return tuple(
        sorted(
            (
                PlannedRow(
                    key,
                    family_of[key],
                    classes[key],
                    tuple(
                        sorted(
                            predecessors[key] & open_keys,
                            key=finding_status.finding_sort_key,
                        )
                    ),
                    paths[key],
                    len(reach[key]),
                    tuple(sorted(claimed.get(family_of[key], ()))),
                    families[family_of[key]].stratum,
                    families[family_of[key]].axes,
                )
                for key in v1
            ),
            key=lambda row: (
                -row.critical_path,
                -row.downstream,
                finding_status.finding_sort_key(row.row),
            ),
        )
    )


def render_plan(plan: Sequence[PlannedRow]) -> str:
    lines = ["Ready v1 rows (critical path, downstream rows, family custody):"]
    for row in plan:
        if row.ready:
            custody = ", ".join(row.claimed_by) or "unclaimed"
            lines.append(
                f"  {row.row} family={row.family} path={row.critical_path} downstream={row.downstream} {custody}"
                + (
                    f" axes={', '.join(row.axes)}"
                    if row.stratum == finding_status.FamilyStratum.ENGINE
                    else ""
                )
            )
    lines.append("Waiting v1 rows (open predecessors):")
    for row in plan:
        if not row.ready:
            lines.append(
                f"  {row.row} family={row.family} path={row.critical_path} downstream={row.downstream} waits={', '.join(row.waits)}"
            )
    return "\n".join(lines)


def plan_dispatch(
    status: finding_status.FindingStatus,
    records: Sequence[Any],
    rows: Sequence[str],
    *,
    reason: DispatchReason = DispatchReason.DAG_READY,
    text: str = "",
) -> Dispatch:
    """Admit ready rows in one unclaimed family, or an explicit departure."""
    text = text.strip()
    if reason is not DispatchReason.DAG_READY and not text:
        raise DispatchRefused(f"dispatch reason {reason.value} needs --reason TEXT")
    if reason is DispatchReason.INTEGRATOR_LEDGER:
        if rows:
            raise DispatchRefused(
                "an integrator ledger lane edits the ledger itself; it names no rows"
            )
        return Dispatch((), None, {}, reason, text)
    if not rows:
        raise DispatchRefused(
            "name the open rows this lane fixes (--rows HF-n[,HF-m]); integrator ledger lanes use --dispatch-reason integrator_ledger"
        )
    open_keys = set(status.open_keys)
    unknown = [row for row in rows if row not in open_keys]
    if unknown:
        raise DispatchRefused(f"not open findings: {', '.join(unknown)}")
    _validate_graph(status)
    family_of = {
        key: family.letter
        for family in status.dependency_families
        for key in family.keys
    }
    if any(row not in family_of for row in rows):
        raise DispatchRefused("each dispatched row needs a dependency family")
    families = sorted({family_of[row] for row in rows})
    if len(families) != 1:
        raise DispatchRefused(
            "a lane fixes one family's cause; these rows span families "
            + ", ".join(families)
        )
    family = families[0]
    finding_status.dependency_order(status)
    predecessors = finding_status.dependency_predecessors(status)
    waits = {
        row: tuple(
            sorted(predecessors[row] & open_keys, key=finding_status.finding_sort_key)
        )
        for row in rows
    }
    claimed = active_families(records)
    if reason not in (DispatchReason.LANDING_REPAIR, DispatchReason.INTEGRATOR_LEDGER):
        strata = {item.letter: item.stratum for item in status.dependency_families}
        details = [
            f"{key}: {', '.join(tasks)}" for key, tasks in sorted(claimed.items())
        ]
        known = {task for tasks in claimed.values() for task in tasks}
        unassigned = sorted(
            record.task
            for record in records
            if record.active and record.task not in known
        )
        if unassigned:
            details.append("unassigned: " + ", ".join(unassigned))
        active = "; ".join(details) or "none"
        if strata[family] is finding_status.FamilyStratum.APPARATUS and any(
            strata.get(key) is finding_status.FamilyStratum.APPARATUS for key in claimed
        ):
            raise DispatchRefused(
                f"apparatus capacity rule: another apparatus lane is active; active lanes: {active}"
            )
        if strata[family] is not finding_status.FamilyStratum.ENGINE and not any(
            strata.get(key) is finding_status.FamilyStratum.ENGINE for key in claimed
        ):
            raise DispatchRefused(
                f"engine capacity rule: non-engine dispatch requires an active engine lane; active lanes: {active}"
            )
    if reason is DispatchReason.DAG_READY and family in claimed:
        raise DispatchRefused(
            f"family {family} already has a lane ({', '.join(claimed[family])}); one lane owns a family's cause. Extend that lane, or give --dispatch-reason with --reason TEXT"
        )
    if reason is DispatchReason.DAG_READY and any(waits.values()):
        detail = "; ".join(
            f"{row} waits on {', '.join(values)}"
            for row, values in waits.items()
            if values
        )
        raise DispatchRefused(
            f"rows are not ready: {detail}. Give a non-default --dispatch-reason with --reason TEXT to record a departure"
        )
    return Dispatch(tuple(rows), family, waits, reason, text)

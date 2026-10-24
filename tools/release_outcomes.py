"""Pure release-outcome bindings over the existing findings and proof catalog.

The caller owns coordination/ledger source custody and phase observations. A
declared dependency orders outcomes; it cannot attest review, proof or landing readiness.
Finding identity and recorded state come only from ``finding_status``.
"""

from __future__ import annotations

import heapq
import re
from collections.abc import Collection
from dataclasses import dataclass
from pathlib import PurePosixPath, PureWindowsPath
from typing import Literal, TypedDict

from tools.finding_status import (
    STATE_FIXED,
    STATE_OPEN,
    FindingStatus,
    finding_sort_key,
)

_FIELDS = frozenset({"id", "findings", "depends_on", "proof_families", "inputs"})
_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")


class OutcomeProjection(TypedDict):
    id: str
    findings: list[str]
    depends_on: list[str]
    proof_families: list[str]
    inputs: list[str]


class FindingProjection(TypedDict):
    key: str
    recorded_state: str
    outcome_id: str | None
    planning: Literal["UNPLANNED", "PLANNED"] | None


class GraphProjection(TypedDict):
    outcomes: list[OutcomeProjection]
    findings: list[FindingProjection]


@dataclass(frozen=True, slots=True)
class ReleaseOutcome:
    id: str
    findings: tuple[str, ...]
    depends_on: tuple[str, ...]
    proof_families: tuple[str, ...]
    inputs: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class FindingBinding:
    key: str
    recorded_state: str
    outcome_id: str | None

    @property
    def planning(self) -> Literal["UNPLANNED", "PLANNED"] | None:
        # Fixed historical rows have no invented review/proof phase.
        if self.recorded_state == STATE_FIXED:
            return None
        return "UNPLANNED" if self.outcome_id is None else "PLANNED"


@dataclass(frozen=True, slots=True)
class ReleaseOutcomeGraph:
    """Outcomes in dependency order and every finding in canonical ID order."""

    outcomes: tuple[ReleaseOutcome, ...]
    findings: tuple[FindingBinding, ...]

    @property
    def unplanned_findings(self) -> tuple[str, ...]:
        """Discovery candidates only; these have no implementation admission."""
        return tuple(
            finding.key
            for finding in self.findings
            if finding.recorded_state == STATE_OPEN and finding.outcome_id is None
        )

    def project(self) -> GraphProjection:
        """Compact graph facts, with no receipts or inferred phase readiness."""
        return {
            "outcomes": [
                {
                    "id": outcome.id,
                    "findings": list(outcome.findings),
                    "depends_on": list(outcome.depends_on),
                    "proof_families": list(outcome.proof_families),
                    "inputs": list(outcome.inputs),
                }
                for outcome in self.outcomes
            ],
            "findings": [
                {
                    "key": finding.key,
                    "recorded_state": finding.recorded_state,
                    "outcome_id": finding.outcome_id,
                    "planning": finding.planning,
                }
                for finding in self.findings
            ],
        }


def _strings(value: object, *, label: str, nonempty: bool = True) -> tuple[str, ...]:
    if not isinstance(value, list) or (nonempty and not value):
        raise ValueError(f"{label} must be {'a nonempty' if nonempty else 'an'} array")
    seen: set[str] = set()
    for item in value:
        if (
            not isinstance(item, str)
            or not item
            or item != item.strip()
            or any(ord(char) < 32 or ord(char) == 127 for char in item)
        ):
            raise ValueError(f"{label} must contain nonempty, canonical strings")
        if item in seen:
            raise ValueError(f"{label} repeats {item!r}")
        seen.add(item)
    return tuple(sorted(seen))


def _identifier(value: object, *, label: str) -> str:
    if not isinstance(value, str) or _ID.fullmatch(value) is None:
        raise ValueError(f"{label} must be an outcome identifier")
    return value


def _input_glob(value: str, *, label: str) -> None:
    # Review scopes are repository-relative POSIX paths on every host. This
    # validates coordinates only; the model never expands a glob or stats files.
    if (
        PurePosixPath(value).is_absolute()
        or PureWindowsPath(value).drive
        or "\\" in value
        or any(part in {"", ".", ".."} for part in value.split("/"))
    ):
        raise ValueError(f"{label} must contain repository-relative path globs")


def _topological(outcomes: dict[str, ReleaseOutcome]) -> tuple[ReleaseOutcome, ...]:
    # Each vertex/edge is visited once; the heap supplies deterministic ID order
    # among simultaneously available outcomes without transitive-closure storage.
    remaining = {key: len(value.depends_on) for key, value in outcomes.items()}
    dependents: dict[str, list[str]] = {key: [] for key in outcomes}
    for outcome in outcomes.values():
        for dependency in outcome.depends_on:
            if dependency == outcome.id:
                raise ValueError(f"release outcome {outcome.id!r} depends on itself")
            if dependency not in outcomes:
                raise ValueError(
                    f"release outcome {outcome.id!r} has unknown dependency {dependency!r}"
                )
            dependents[dependency].append(outcome.id)
    ready = [key for key, count in remaining.items() if count == 0]
    heapq.heapify(ready)
    ordered: list[ReleaseOutcome] = []
    while ready:
        key = heapq.heappop(ready)
        ordered.append(outcomes[key])
        for dependent in dependents[key]:
            remaining[dependent] -= 1
            if remaining[dependent] == 0:
                heapq.heappush(ready, dependent)
    if len(ordered) != len(outcomes):
        blocked = ", ".join(sorted(key for key, count in remaining.items() if count))
        raise ValueError(f"release outcome dependency cycle blocks: {blocked}")
    return tuple(ordered)


def parse_release_outcomes(
    entries: object,
    *,
    findings: FindingStatus,
    proof_families: Collection[str],
) -> ReleaseOutcomeGraph:
    """Validate explicit coordination ``release_outcomes`` without source I/O.

    The ledger projection must be structurally valid. Every entry requires all
    five fields; only ``depends_on`` may be empty. Dependencies are exclusively
    the declared edges, including when outcomes bind multiple findings.
    """
    if findings.problems:
        raise ValueError("invalid finding authority: " + "; ".join(findings.problems))
    if not isinstance(entries, list):
        raise ValueError("release_outcomes must be an array of objects")
    known_findings = {row.key for row in findings.rows}
    known_families = set(proof_families)
    outcomes: dict[str, ReleaseOutcome] = {}
    bound: dict[str, str] = {}
    for index, entry in enumerate(entries):
        label = f"release_outcomes[{index}]"
        if not isinstance(entry, dict):
            raise ValueError(f"{label} must be an object")
        missing = _FIELDS.difference(entry)
        unknown = entry.keys() - _FIELDS
        if missing or unknown:
            raise ValueError(
                f"{label} fields: missing={sorted(missing)!r}, "
                f"unknown={sorted(unknown, key=repr)!r}"
            )
        key = _identifier(entry["id"], label=f"{label}.id")
        if key in outcomes:
            raise ValueError(f"duplicate release outcome ID {key!r}")
        values = {
            field: _strings(
                entry[field], label=f"{label}.{field}", nonempty=field != "depends_on"
            )
            for field in ("findings", "depends_on", "proof_families", "inputs")
        }
        for finding in values["findings"]:
            if finding not in known_findings:
                raise ValueError(f"{label} names unknown finding {finding!r}")
            if finding in bound:
                raise ValueError(
                    f"finding {finding!r} binds both {bound[finding]!r} and {key!r}"
                )
            bound[finding] = key
        for dependency in values["depends_on"]:
            _identifier(dependency, label=f"{label}.depends_on")
        for family in values["proof_families"]:
            if family not in known_families:
                raise ValueError(f"{label} names unknown proof family {family!r}")
        for pattern in values["inputs"]:
            _input_glob(pattern, label=f"{label}.inputs")
        outcomes[key] = ReleaseOutcome(
            id=key,
            findings=tuple(sorted(values["findings"], key=finding_sort_key)),
            depends_on=values["depends_on"],
            proof_families=values["proof_families"],
            inputs=values["inputs"],
        )
    return ReleaseOutcomeGraph(
        outcomes=_topological(outcomes),
        findings=tuple(
            FindingBinding(row.key, row.state, bound.get(row.key))
            for row in sorted(findings.rows, key=lambda row: finding_sort_key(row.key))
        ),
    )

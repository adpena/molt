"""Project the state of every finding from the findings ledger.

`docs/agent/V1_HANDOFF_FINDINGS.md` is the one live finding authority. This
module is its one reader. The docs checker, the pin-freshness holds and the
release exit gate consume this projection; none of them parses the ledger.

A finding row outside DAG metadata is a Markdown table row whose first cell
starts with a stable ID (`HF-n`, `HF-Fn` or `V1-n`). The `## ` section sets its state:
a section whose title starts with "Fixed" closes its rows, and every other row
is open. Open is the default, so a row cannot leave the release scope by its
position alone.

A parenthetical after the ID qualifies the row. "(was X)" records that this row
is finding X under a new ID, so X must not keep a row of its own. Any other
parenthetical, such as "(was part of X)" or "(shared msghdr widths)", names a
part and closes nothing.

The projection is pure. It reads text and never touches the file system, Git or
the network, so each caller binds the ledger bytes it trusts: the docs checker
reads the checkout, and the release exit gate reads the Git blob at the release
source revision.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path, PurePosixPath

LEDGER_PATH = PurePosixPath("docs/agent/V1_HANDOFF_FINDINGS.md")
STATE_OPEN = "open"
STATE_FIXED = "fixed"
FIXED_SECTION_PREFIX = "Fixed"

_ID_PATTERN = r"(?:HF-F?|V1-)\d+"
_ROW_RE = re.compile(rf"^\| (?P<id>{_ID_PATTERN})(?: \((?P<note>[^)|]*)\))? \|")
_RENAMED_FROM_RE = re.compile(r"was (?P<id>(?:HF|V1)-\d+)")
_SECTION_RE = re.compile(r"^## (?P<title>.*\S)\s*$")
_SUBSECTION_RE = re.compile(r"^### (?P<title>.*\S)\s*$")
_DEPENDENCY_FAMILY_RE = re.compile(r"^(?P<letter>[A-Z])\. (?P<name>\S.*)$")
# Prose that states the order by hand ("Edges: A -> P ...", "Levels: 0 = A ...").
# The edges table alone owns the order.
_PROSE_ORDER_RE = re.compile(r"^(?:Edges|Levels):")
# The subsection whose table maps every open row to one root-cause family.
DEPENDENCY_ORDER_SUBSECTION = "Family dependency order"
_CELL_SEPARATOR_RE = re.compile(r"(?<!\\)\|")
_DELIMITER_ROW_RE = re.compile(r"^\|(?:\s*:?-{3,}:?\s*\|)+\s*$")
_KEY_RE = re.compile(r"(?P<prefix>HF-F?|V1-)(?P<number>\d+)(?P<rest>(?: \([^)|]*\))?)")


@dataclass(frozen=True, slots=True)
class FindingRow:
    """One ledger row and the state its section gives it."""

    key: str
    state: str
    section: str
    line: int
    renamed_from: str | None
    evidence: str


class FamilyStratum(StrEnum):
    ENGINE = "engine"
    SEMANTICS = "semantics"
    DELIVERY = "delivery"
    APPARATUS = "apparatus"


@dataclass(frozen=True, slots=True)
class DependencyFamily:
    """One root-cause family of the ledger's family dependency order."""

    letter: str
    name: str
    keys: tuple[str, ...]
    line: int
    stratum: FamilyStratum
    axes: tuple[str, ...]

    @property
    def title(self) -> str:
        return f"{self.letter}. {self.name}"


@dataclass(frozen=True, slots=True)
class DependencyEdge:
    """``source -> target``: row or family prerequisites, with a reason."""

    source: str
    target: str
    reason: str
    line: int


class DependencyCycleError(ValueError):
    """The expanded row graph orders a row after itself."""


class V1Class(StrEnum):
    RELEASE_BLOCKING = "release_blocking"
    RELEASE_APPARATUS = "release_apparatus"
    POST_1_0 = "post_1_0"
    APPARATUS = "apparatus"

    @property
    def tier(self) -> str:
        return (
            "v1"
            if self in (self.RELEASE_BLOCKING, self.RELEASE_APPARATUS)
            else self.value
        )


@dataclass(frozen=True, slots=True)
class V1Membership:
    key: str
    classification: V1Class
    line: int


class V01Class(StrEnum):
    """A row's place in the v0.1 release (owner decision 2026-11-05).

    A row blocks v0.1 if it fails in a Tier 1 cell (``config/release_targets.toml``),
    is a safety defect, blocks Tier 1 installed delivery or guards a public
    performance claim. ``tier2`` rows fail only in Tier 2 cells; ``post_v0_1``
    rows are other work. An open row with no class holds a 0.x release.
    """

    V0_1_BLOCKING = "v0_1_blocking"
    TIER2 = "tier2"
    POST_V0_1 = "post_v0_1"


# A merge keeps the stricter class; an unclassified side ranks with blocking.
V01_STRICTNESS = (V01Class.V0_1_BLOCKING, V01Class.TIER2, V01Class.POST_V0_1)
V01_MEMBERSHIP_HEADER = ("Row", "v0.1 class")


@dataclass(frozen=True, slots=True)
class V01Membership:
    key: str
    classification: V01Class
    line: int


@dataclass(frozen=True, slots=True)
class FindingStatus:
    """Every finding row of one ledger text, plus its structural defects."""

    rows: tuple[FindingRow, ...]
    problems: tuple[str, ...]
    dependency_families: tuple[DependencyFamily, ...] = ()
    dependency_edges: tuple[DependencyEdge, ...] = ()
    # Lines of the dependency-order subsection that restate work order in
    # prose; the edges table is the order, so the coverage check refuses these.
    prose_order_lines: tuple[int, ...] = ()
    v1_membership: tuple[V1Membership, ...] = ()
    edge_table_lines: tuple[int, ...] = ()
    v01_membership: tuple[V01Membership, ...] = ()

    @property
    def classes(self) -> dict[str, V1Class]:
        return {entry.key: entry.classification for entry in self.v1_membership}

    @property
    def v01_classes(self) -> dict[str, V01Class]:
        """Each classified row's v0.1 class; ``project`` refuses duplicates."""
        return {entry.key: entry.classification for entry in self.v01_membership}

    @property
    def v01_holding_keys(self) -> tuple[str, ...]:
        """The open rows that hold a 0.x release: blocking or unclassified."""
        classes = self.v01_classes
        return tuple(
            key
            for key in self.open_keys
            if classes.get(key, V01Class.V0_1_BLOCKING) is V01Class.V0_1_BLOCKING
        )

    @property
    def open_keys(self) -> tuple[str, ...]:
        """The keys of the open rows, in canonical finding order."""
        return tuple(
            sorted(
                (row.key for row in self.rows if row.state == STATE_OPEN),
                key=finding_sort_key,
            )
        )


def is_finding_key(value: object) -> bool:
    """Return whether *value* is a row key this projection can emit."""
    return isinstance(value, str) and _KEY_RE.fullmatch(value) is not None


def finding_sort_key(key: str) -> tuple[str, int, str]:
    """Order keys by ID family, then by number, then by qualifier."""
    match = _KEY_RE.fullmatch(key)
    if match is None:
        raise ValueError(f"not a finding key: {key!r}")
    return match["prefix"], int(match["number"]), match["rest"]


def _row_key(finding_id: str, note: str | None) -> str:
    # "(was ...)" records the history of this same row, so it is not part of
    # the key. Any other note names a part with its own key.
    if note is None or note.startswith("was "):
        return finding_id
    return f"{finding_id} ({note})"


def cells(line: str) -> list[str]:
    """The stripped cells of one Markdown table row, or [] for any other line."""
    stripped = line.strip()
    if not stripped.endswith("|") or stripped.endswith("\\|"):
        return []
    return [cell.strip() for cell in _CELL_SEPARATOR_RE.split(stripped)[1:-1]]


def has_unescaped_bar(cell: str) -> bool:
    """Whether a cell's text would split its row (a ``|`` not written ``\\|``)."""
    return _CELL_SEPARATOR_RE.search(cell) is not None


def project(text: str) -> FindingStatus:
    """Project every finding row of a ledger text."""
    rows: list[FindingRow] = []
    problems: list[str] = []
    families: list[DependencyFamily] = []
    edges: list[DependencyEdge] = []
    prose_order: list[int] = []
    membership: list[V1Membership] = []
    v01_membership: list[V01Membership] = []
    edge_tables: list[int] = []
    dependency_table = ""
    section = ""
    subsection = ""
    # The column count of the table the current line belongs to, from its
    # header row; a literal | inside a cell silently splits the row otherwise.
    table_width: int | None = None
    previous = ""
    for line_number, line in enumerate(text.splitlines(), start=1):
        header, previous = previous, line
        if _DELIMITER_ROW_RE.match(line.strip()):
            table_width = len(cells(header))
            continue
        if not line.lstrip().startswith("|"):
            table_width = None
        heading = _SECTION_RE.match(line)
        if heading is not None:
            section = heading["title"]
            subsection = ""
            continue
        subheading = _SUBSECTION_RE.match(line)
        if subheading is not None:
            subsection = subheading["title"]
            dependency_table = ""
            continue
        if subsection.startswith(DEPENDENCY_ORDER_SUBSECTION):
            if _PROSE_ORDER_RE.match(line):
                prose_order.append(line_number)
                continue
            row_cells = cells(line)
            if line.lstrip().startswith("|") and not row_cells:
                problems.append(
                    f"dependency table line {line_number} needs a closing |"
                )
                continue
            if row_cells == ["From", "To", "Reason"]:
                dependency_table = "edges"
                edge_tables.append(line_number)
                continue
            if row_cells == ["Row", "V1 class"]:
                dependency_table = "membership"
                continue
            if tuple(row_cells) == V01_MEMBERSHIP_HEADER:
                dependency_table = "v01"
                continue
            family = _DEPENDENCY_FAMILY_RE.match(row_cells[0]) if row_cells else None
            if dependency_table == "v01" and row_cells:
                if len(row_cells) != 2:
                    problems.append(
                        f"v0.1 membership line {line_number} needs Row and v0.1 class"
                    )
                    continue
                try:
                    v01_class = V01Class(row_cells[1])
                except ValueError:
                    problems.append(
                        f"v0.1 membership {row_cells[0]} (line {line_number}) has unknown class {row_cells[1]!r}"
                    )
                    continue
                if any(entry.key == row_cells[0] for entry in v01_membership):
                    problems.append(
                        f"v0.1 membership {row_cells[0]} (line {line_number}) is a duplicate"
                    )
                    continue
                v01_membership.append(
                    V01Membership(row_cells[0], v01_class, line_number)
                )
                continue
            if dependency_table == "membership" and row_cells:
                if len(row_cells) != 2:
                    problems.append(
                        f"v1 membership line {line_number} needs Row and V1 class"
                    )
                    continue
                try:
                    classification = V1Class(row_cells[1])
                except ValueError:
                    problems.append(
                        f"v1 membership {row_cells[0]} (line {line_number}) has unknown class {row_cells[1]!r}"
                    )
                    continue
                membership.append(
                    V1Membership(row_cells[0], classification, line_number)
                )
                continue
            if dependency_table == "edges" and row_cells and family is None:
                if len(row_cells) != 3:
                    problems.append(
                        f"dependency edge line {line_number} needs From, To and Reason"
                    )
                    continue
                edges.append(
                    DependencyEdge(
                        source=row_cells[0],
                        target=row_cells[1],
                        reason=row_cells[2],
                        line=line_number,
                    )
                )
                continue
            if family is not None:
                dependency_table = "families"
                if len(row_cells) != 5:
                    problems.append(
                        f"family {family['letter']} needs Family, Stratum, HF-459 axes, cause and open rows"
                    )
                    continue
                try:
                    stratum = FamilyStratum(row_cells[1])
                except ValueError:
                    problems.append(
                        f"family {family['letter']} has missing or invalid stratum {row_cells[1]!r}"
                    )
                    continue
                families.append(
                    DependencyFamily(
                        letter=family["letter"],
                        name=family["name"],
                        keys=tuple(
                            key.strip()
                            for key in row_cells[4].split(",")
                            if key.strip()
                        ),
                        line=line_number,
                        stratum=stratum,
                        axes=tuple(
                            axis.strip()
                            for axis in row_cells[2].split(",")
                            if axis.strip() and axis.strip() != "-"
                        ),
                    )
                )
            continue
        match = _ROW_RE.match(line)
        if match is None:
            continue
        note = match["note"]
        key = _row_key(match["id"], note)
        renamed = _RENAMED_FROM_RE.fullmatch(note) if note is not None else None
        state = STATE_FIXED if section.startswith(FIXED_SECTION_PREFIX) else STATE_OPEN
        row_cells = cells(line)
        if len(row_cells) < 3:
            problems.append(
                f"row {key} (line {line_number}) needs an ID cell, a finding cell "
                "and an evidence cell"
            )
        elif table_width is not None and len(row_cells) != table_width:
            problems.append(
                f"row {key} (line {line_number}) has {len(row_cells)} cells but its "
                f"table has {table_width}; escape a literal | inside a cell as \\|"
            )
        evidence = row_cells[-1] if len(row_cells) >= 3 else ""
        if state == STATE_FIXED and len(row_cells) >= 3 and not evidence:
            problems.append(
                f"fixed row {key} (line {line_number}) records no fix or "
                "verification evidence"
            )
        rows.append(
            FindingRow(
                key=key,
                state=state,
                section=section,
                line=line_number,
                renamed_from=renamed["id"] if renamed is not None else None,
                evidence=evidence,
            )
        )
    problems.extend(_identity_problems(rows))
    return FindingStatus(
        tuple(rows),
        tuple(problems),
        tuple(families),
        tuple(edges),
        tuple(prose_order),
        tuple(membership),
        tuple(edge_tables),
        tuple(v01_membership),
    )


def dependency_predecessors(status: FindingStatus) -> dict[str, frozenset[str]]:
    """Expand family endpoints into open rows of the same tier.

    The two release classes share the v1 tier. Named closed rows remain nodes
    (and satisfy their dependents); a closed row has no class to filter by.
    dependency_problems validates endpoints and membership for every consumer.
    """
    prerequisites: dict[str, set[str]] = {row.key: set() for row in status.rows}
    families = {family.letter: family.keys for family in status.dependency_families}
    classes = status.classes
    open_keys = set(status.open_keys)

    def expand(endpoint: str) -> tuple[str, ...]:
        if endpoint in families:
            return tuple(key for key in families[endpoint] if key in open_keys)
        return (endpoint,) if endpoint in prerequisites else ()

    for edge in status.dependency_edges:
        for source in expand(edge.source):
            for target in expand(edge.target):
                if edge.source in families or edge.target in families:
                    if (
                        source in classes
                        and target in classes
                        and classes[source].tier != classes[target].tier
                    ):
                        continue
                prerequisites[target].add(source)
    return {key: frozenset(values) for key, values in prerequisites.items()}


def dependency_order(status: FindingStatus) -> tuple[str, ...]:
    """A topological row order; refuse cycles through any mix of endpoints."""
    prerequisites = dependency_predecessors(status)
    ordered: list[str] = []
    visited: set[str] = set()
    visiting: list[str] = []

    def visit(key: str) -> None:
        if key in visited:
            return
        if key in visiting:
            cycle = [*visiting[visiting.index(key) :], key]
            raise DependencyCycleError("row dependency cycle: " + " -> ".join(cycle))
        visiting.append(key)
        for source in sorted(prerequisites[key], key=finding_sort_key):
            visit(source)
        visiting.pop()
        visited.add(key)
        ordered.append(key)

    for key in sorted(prerequisites, key=finding_sort_key):
        visit(key)
    return tuple(ordered)


def dependency_problems(status: FindingStatus) -> list[str]:
    """Validate the row graph and its membership once for every consumer."""
    families = status.dependency_families
    if not families:
        return [*status.problems, "the ledger has no family dependency order table"]
    problems: list[str] = list(status.problems)
    open_keys = set(status.open_keys)
    placed: dict[str, list[str]] = {}
    for family in families:
        if family.stratum is FamilyStratum.ENGINE and not family.axes:
            problems.append(f"engine family {family.title} needs HF-459 axes")
        for key in family.keys:
            placed.setdefault(key, []).append(family.letter)
            if key not in open_keys:
                problems.append(
                    f"dependency family {family.title} lists {key}, which is not "
                    "an open finding"
                )
    problems.extend(
        f"open finding {key} sits in families {', '.join(letters)}; place it in one"
        for key, letters in sorted(placed.items())
        if len(letters) > 1
    )
    problems.extend(
        f"open finding {key} is in no dependency family; place it in the ledger's "
        "family dependency order"
        for key in status.open_keys
        if key not in placed
    )
    classified: dict[str, list[V1Membership]] = {}
    for entry in status.v1_membership:
        classified.setdefault(entry.key, []).append(entry)
        if entry.key not in open_keys:
            problems.append(
                f"v1 class on closed or unknown row {entry.key} (line {entry.line})"
            )
    for key in status.open_keys:
        count = len(classified.get(key, ()))
        if count != 1:
            problems.append(
                f"open finding {key} has {count} v1 classes; needs exactly one"
            )
    problems.extend(
        f"v0.1 class on closed or unknown row {entry.key} (line {entry.line})"
        for entry in status.v01_membership
        if entry.key not in open_keys
    )
    problems.extend(_dependency_edge_problems(status))
    return problems


def _dependency_edge_problems(status: FindingStatus) -> list[str]:
    """The edges table is the one work order: well formed, acyclic, no prose copy."""
    problems = [
        f"line {line} states the family order in prose; the edges table alone owns the order"
        for line in status.prose_order_lines
    ]
    letters = {family.letter for family in status.dependency_families}
    if not status.edge_table_lines:
        problems.append(
            "the family dependency order has no edges table (| From | To | Reason |)"
        )
    endpoints = letters | {row.key for row in status.rows}
    if len(letters) != len(status.dependency_families):
        problems.append("duplicate dependency family letter")
    seen: dict[tuple[str, str], int] = {}
    for edge in status.dependency_edges:
        where = f"edge {edge.source} -> {edge.target} (line {edge.line})"
        for endpoint in (edge.source, edge.target):
            if endpoint not in endpoints:
                problems.append(f"{where} names unknown endpoint {endpoint}")
        if edge.source == edge.target:
            problems.append(f"{where} orders an endpoint after itself")
        if not edge.reason:
            problems.append(f"{where} gives no reason")
        first = seen.setdefault((edge.source, edge.target), edge.line)
        if first != edge.line:
            problems.append(f"{where} repeats the edge on line {first}")
    try:
        dependency_order(status)
    except DependencyCycleError as exc:
        problems.append(str(exc))
    classes = status.classes
    for target, sources in dependency_predecessors(status).items():
        for source in sorted(sources, key=finding_sort_key):
            if (
                target in classes
                and classes[target].tier == "v1"
                and source in classes
                and classes[source].tier != "v1"
            ):
                problems.append(f"v1 row {target} depends on non-v1 row {source}")
    return problems


def _identity_problems(rows: list[FindingRow]) -> list[str]:
    """Each finding has one row, and a renamed finding keeps no old row."""
    problems: list[str] = []
    by_key: dict[str, list[FindingRow]] = {}
    for row in rows:
        by_key.setdefault(row.key, []).append(row)
    # Parallel lanes allocate IDs independently, so a collision silently
    # merges the history of two findings.
    for key, same in sorted(by_key.items(), key=lambda item: finding_sort_key(item[0])):
        if len(same) > 1:
            lines = ", ".join(str(row.line) for row in same)
            problems.append(
                f"finding {key} names {len(same)} rows (lines {lines}); give each "
                "finding one ID"
            )
    # A merge from an older ledger brings back open rows that were fixed. Two
    # rows may name one old ID: a finding can reopen and close again, and each
    # closure keeps its own dated row.
    renamed = sorted(
        {row.renamed_from for row in rows if row.renamed_from is not None},
        key=finding_sort_key,
    )
    for old in renamed:
        for kept in by_key.get(old, ()):
            if kept.state == STATE_OPEN:
                problems.append(
                    f"{old} is open, but a fixed row says it was {old}; remove the "
                    f'resurrected row, or write "(was part of {old})" when the fix '
                    "was partial"
                )
            else:
                problems.append(
                    f"{old} keeps a fixed row (line {kept.line}), but another row "
                    f"says it was {old}; give each finding one row"
                )
    return problems


def read_ledger(root: Path) -> FindingStatus:
    """Project the ledger of the checkout at *root*."""
    path = root.joinpath(*LEDGER_PATH.parts)
    return project(path.read_text(encoding="utf-8"))

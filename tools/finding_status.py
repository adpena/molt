"""Project the state of every finding from the findings ledger.

`docs/agent/V1_HANDOFF_FINDINGS.md` is the one live finding authority. This
module is its one reader. The docs checker, the pin-freshness holds and the
release exit gate consume this projection; none of them parses the ledger.

A finding row is a Markdown table row whose first cell starts with a stable ID
(`HF-n`, `HF-Fn` or `V1-n`). The `## ` section that holds a row sets its state:
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
from pathlib import Path, PurePosixPath

LEDGER_PATH = PurePosixPath("docs/agent/V1_HANDOFF_FINDINGS.md")
STATE_OPEN = "open"
STATE_FIXED = "fixed"
FIXED_SECTION_PREFIX = "Fixed"

_ID_PATTERN = r"(?:HF-F?|V1-)\d+"
_ROW_RE = re.compile(rf"^\| (?P<id>{_ID_PATTERN})(?: \((?P<note>[^)|]*)\))? \|")
_RENAMED_FROM_RE = re.compile(r"was (?P<id>(?:HF|V1)-\d+)")
_SECTION_RE = re.compile(r"^## (?P<title>.*\S)\s*$")
_CELL_SEPARATOR_RE = re.compile(r"(?<!\\)\|")
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


@dataclass(frozen=True, slots=True)
class FindingStatus:
    """Every finding row of one ledger text, plus its structural defects."""

    rows: tuple[FindingRow, ...]
    problems: tuple[str, ...]

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


def _cells(line: str) -> list[str]:
    stripped = line.strip()
    if not stripped.endswith("|") or stripped.endswith("\\|"):
        return []
    return [cell.strip() for cell in _CELL_SEPARATOR_RE.split(stripped)[1:-1]]


def project(text: str) -> FindingStatus:
    """Project every finding row of a ledger text."""
    rows: list[FindingRow] = []
    problems: list[str] = []
    section = ""
    for line_number, line in enumerate(text.splitlines(), start=1):
        heading = _SECTION_RE.match(line)
        if heading is not None:
            section = heading["title"]
            continue
        match = _ROW_RE.match(line)
        if match is None:
            continue
        note = match["note"]
        key = _row_key(match["id"], note)
        renamed = _RENAMED_FROM_RE.fullmatch(note) if note is not None else None
        state = STATE_FIXED if section.startswith(FIXED_SECTION_PREFIX) else STATE_OPEN
        cells = _cells(line)
        if len(cells) < 3:
            problems.append(
                f"row {key} (line {line_number}) needs an ID cell, a finding cell "
                "and an evidence cell"
            )
        evidence = cells[-1] if len(cells) >= 3 else ""
        if state == STATE_FIXED and len(cells) >= 3 and not evidence:
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
    return FindingStatus(tuple(rows), tuple(problems))


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

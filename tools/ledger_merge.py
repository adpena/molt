#!/usr/bin/env python3
"""Git merge driver for the v1.0 ledger: merge findings rows by their ID.

``docs/agent/V1_HANDOFF_FINDINGS.md`` is a set of tables whose rows each begin
with a finding ID. Parallel lanes add, rewrite and move different rows, and a
line merge conflicts whenever two of them touch the same spot, as when two
lanes each add a row at the top of a section. This driver merges the three
versions (base, ours, theirs) row by row, keyed by the full first cell, as a
structured merge driver merges a lock file by package rather than by line:

- a row that one side changed (text or section) takes that side's version;
- a row that one side added goes after the row that precedes it there;
- a row that one side deleted and the other left alone is deleted;
- a row that both sides changed differently is a conflict, except a family
  row of the dependency order (``| C. Name | cause | keys |``, keyed by its
  letter): its name and cause merge cell by cell and its key list merges as a
  set, since every lane that opens or closes a row edits one.

Every other line (headings, prose, table headers) merges as text: when one
side alone changed it, that side's text frames the result; when both changed
it differently, the driver falls back to ``git merge-file`` for the file.

Usage (Git substitutes the paths): ``ledger_merge.py %O %A %B``. The result
is written to the ``%A`` file; the exit status is 0 when clean and 1 when it
wrote conflict markers, as Git requires of a merge driver.
"""

from __future__ import annotations

import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

# A findings row: ``| HF-73 |``, ``| HF-F170 (was HF-112) |``, ``| V1-28 |``.
_ROW = re.compile(r"^\| (?P<key>[A-Z][A-Z0-9]*-[A-Z]?\d+(?: [^|]*?)?) \|")
# A family row of the dependency order: ``| C. Proof cells ... | cause | keys |``.
_FAMILY = re.compile(r"^\| (?P<letter>[A-Z])\. [^|]*\|")
_SECTION = re.compile(r"^#{1,6} ")
_TABLE_RULE = re.compile(r"^\|-")


@dataclass(frozen=True)
class _Row:
    line: str
    section: str


@dataclass(frozen=True)
class _Document:
    lines: tuple[str, ...]
    # Line index -> row key, for the lines that are findings rows.
    keys: dict[int, str]
    rows: dict[str, _Row]

    @property
    def skeleton(self) -> tuple[str, ...]:
        return tuple(
            line for index, line in enumerate(self.lines) if index not in self.keys
        )


class _Unmergeable(Exception):
    """The rows cannot be keyed; merge the file as text."""


def _row_key(line: str) -> str | None:
    """The merge key of a findings row or a family row, else None."""
    match = _ROW.match(line)
    if match is not None:
        return match.group("key").strip()
    family = _FAMILY.match(line)
    if family is not None:
        return f"family {family.group('letter')}"
    return None


def _merged_list(base: list[str], ours: list[str], theirs: list[str]) -> list[str]:
    """Three-way merge of ordered, duplicate-free lists, as sets.

    Each side's removals hold; theirs' additions follow their predecessor in
    theirs, so two lists sorted by one key stay sorted.
    """
    removed = {item for item in base if item not in ours or item not in theirs}
    result = [item for item in ours if item not in removed]
    for index, item in enumerate(theirs):
        if item in base or item in result:
            continue
        position = 0
        for previous in reversed(theirs[:index]):
            if previous in result:
                position = result.index(previous) + 1
                break
        result.insert(position, item)
    return result


def _cells(line: str) -> tuple[list[str], str]:
    body = line.rstrip("\r\n")
    ending = line[len(body) :]
    if not (body.startswith("| ") and body.endswith(" |")):
        raise _Unmergeable("a family row is not a table row")
    return body[2:-2].split(" | "), ending


def _merged_family_row(base: _Row | None, ours: _Row, theirs: _Row) -> _Row | None:
    """A family row both sides changed: cells three-way, keys as a set."""
    if base is None or ours.section != theirs.section:
        return None
    (b, _), (o, ending), (t, _) = (_cells(r.line) for r in (base, ours, theirs))
    if not len(b) == len(o) == len(t) == 3:
        return None
    merged: list[str] = []
    for base_cell, ours_cell, theirs_cell in zip(b[:2], o[:2], t[:2], strict=True):
        if ours_cell == theirs_cell or theirs_cell == base_cell:
            merged.append(ours_cell)
        elif ours_cell == base_cell:
            merged.append(theirs_cell)
        else:
            return None
    keys = _merged_list(*(cell[2].split(", ") for cell in (b, o, t)))
    merged.append(", ".join(keys))
    return _Row("| " + " | ".join(merged) + " |" + ending, ours.section)


def _parse(text: str) -> _Document:
    lines = tuple(text.splitlines(keepends=True))
    keys: dict[int, str] = {}
    rows: dict[str, _Row] = {}
    section = ""
    for index, line in enumerate(lines):
        if _SECTION.match(line):
            section = line.strip()
            continue
        key = _row_key(line)
        if key is None:
            continue
        if key in rows:
            raise _Unmergeable(f"row {key} appears twice")
        keys[index] = key
        rows[key] = _Row(line, section)
    return _Document(lines, keys, rows)


@dataclass(frozen=True)
class MergeResult:
    text: str
    conflicts: tuple[str, ...]


def _merged_rows(
    base: _Document, ours: _Document, theirs: _Document
) -> tuple[dict[str, tuple[_Row | None, str]], list[str]]:
    """Each key's merged row and the side it comes from; conflicted keys."""
    result: dict[str, tuple[_Row | None, str]] = {}
    conflicts: list[str] = []
    for key in {**base.rows, **ours.rows, **theirs.rows}:
        b, o, t = base.rows.get(key), ours.rows.get(key), theirs.rows.get(key)
        if o == t:
            result[key] = (o, "both")
        elif o == b:
            result[key] = (t, "theirs")
        elif t == b:
            result[key] = (o, "ours")
        elif (
            key.startswith("family ")
            and o is not None
            and t is not None
            and (merged := _merged_family_row(b, o, t)) is not None
        ):
            result[key] = (merged, "both")
        else:
            conflicts.append(key)
    return result, conflicts


def merge_documents(base_text: str, ours_text: str, theirs_text: str) -> MergeResult:
    """Merge three ledger versions; raise ``_Unmergeable`` for a text merge."""
    base, ours, theirs = (_parse(t) for t in (base_text, ours_text, theirs_text))
    if ours.skeleton == theirs.skeleton or theirs.skeleton == base.skeleton:
        frame, frame_side, other = ours, "ours", theirs
    elif ours.skeleton == base.skeleton:
        frame, frame_side, other = theirs, "theirs", ours
    else:
        raise _Unmergeable("both sides changed the text around the rows")
    merged, conflicted = _merged_rows(base, ours, theirs)
    output: list[str] = []
    emitted: set[str] = set()
    for index, line in enumerate(frame.lines):
        key = frame.keys.get(index)
        if key is None:
            output.append(line)
            continue
        if key in conflicted:
            output.extend(_conflict_block(ours.rows.get(key), theirs.rows.get(key)))
            emitted.add(key)
            continue
        row, side = merged[key]
        if row is None:
            continue
        here = frame.rows[key].section
        if side in ("both", frame_side) or row.section == here:
            output.append(row.line)
            emitted.add(key)
    # The other side's added or moved rows, and the conflicts the frame lacks,
    # go after the row that precedes them on the other side.
    for index, line in enumerate(other.lines):
        key = other.keys.get(index)
        if key is None or key in emitted:
            continue
        if key in conflicted:
            block = _conflict_block(ours.rows.get(key), theirs.rows.get(key))
        else:
            row, _side = merged[key]
            if row is None:
                continue
            block = [row.line]
        position = _insertion_point(output, other, index)
        output[position:position] = block
        emitted.add(key)
    return MergeResult("".join(output), tuple(sorted(conflicted)))


def _conflict_block(ours: _Row | None, theirs: _Row | None) -> list[str]:
    return [
        "<<<<<<< ours\n",
        ours.line if ours is not None else "",
        "=======\n",
        theirs.line if theirs is not None else "",
        ">>>>>>> theirs\n",
    ]


def _insertion_point(output: list[str], side: _Document, index: int) -> int:
    """Where a row of ``side`` at ``index`` goes in ``output``."""
    for previous in range(index - 1, -1, -1):
        line = side.lines[previous]
        if previous in side.keys:
            # The preceding row's merged line may differ; find it by key.
            key = side.keys[previous]
            for position, out_line in enumerate(output):
                if _row_key(out_line) == key:
                    return position + 1
            continue
        if _TABLE_RULE.match(line) or _SECTION.match(line):
            # No preceding row: the first row after the table rule of the
            # same section, which the frame shares (its text is unchanged).
            section = next(
                (
                    side.lines[i].strip()
                    for i in range(previous, -1, -1)
                    if _SECTION.match(side.lines[i])
                ),
                "",
            )
            return _first_row_position(output, section)
    raise _Unmergeable("a row has no anchor")


def _first_row_position(output: list[str], section: str) -> int:
    in_section = section == ""
    for position, line in enumerate(output):
        if _SECTION.match(line):
            in_section = line.strip() == section
            continue
        if in_section and _TABLE_RULE.match(line):
            return position + 1
    raise _Unmergeable(f"no table in section {section!r}")


def _merge_file(base: Path, ours: Path, theirs: Path) -> int:
    """Git's own line merge, writing ``ours``; its status counts conflicts."""
    # Stdlib only: Git starts this driver inside a merge, and a merge must not
    # depend on the repository's tooling imports or its memory guard.
    completed = subprocess.run(
        [
            "git",
            "merge-file",
            "-L",
            "ours",
            "-L",
            "base",
            "-L",
            "theirs",
            str(ours),
            str(base),
            str(theirs),
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    sys.stderr.write(completed.stderr)
    return 1 if completed.returncode != 0 else 0


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 3:
        print("usage: ledger_merge.py BASE OURS THEIRS", file=sys.stderr)
        return 2
    base, ours, theirs = (Path(arg) for arg in args)
    # newline="" keeps each file's own line endings byte for byte.
    texts = []
    for path in (base, ours, theirs):
        with path.open(encoding="utf-8", newline="") as handle:
            texts.append(handle.read())
    try:
        result = merge_documents(*texts)
    except _Unmergeable as exc:
        print(f"ledger-merge: {exc}; merging as text", file=sys.stderr)
        return _merge_file(base, ours, theirs)
    with ours.open("w", encoding="utf-8", newline="") as handle:
        handle.write(result.text)
    for key in result.conflicts:
        print(f"ledger-merge: both sides changed {key}", file=sys.stderr)
    return 1 if result.conflicts else 0


if __name__ == "__main__":
    raise SystemExit(main())

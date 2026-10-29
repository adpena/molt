#!/usr/bin/env python3
"""Edit the v1.0 findings ledger by row: open, close, merge, next-key.

``docs/agent/V1_HANDOFF_FINDINGS.md`` owns the burndown. Its protocol moves a
row, its family in the dependency order and the generated finding identities
together. Each command edits through ``tools.finding_status``'s projection,
rewrites ``tests/findings/finding_ids.py`` and refuses to write unless the
family-coverage check (``tools/finding_coverage.py``) passes afterwards, so a
row never lands outside the DAG.

    ledger_edit.py next-key
    ledger_edit.py open HF-590 --family F --section "Open: CI, harness and \
release engineering" [--after HF-587] --defect ... --evidence ... --next ...
    ledger_edit.py close HF-506 --fix ... [--defect ...]
    ledger_edit.py merge HF-477 --into HF-442 [--defect-note ...] \
[--evidence-note ...] [--next-note ...]

Cell text is Markdown; a literal ``|`` must be written ``\\|``.
"""

from __future__ import annotations

import argparse
import datetime as dt
import sys
from collections.abc import Callable, Sequence
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools import finding_coverage, finding_status, gen_finding_ids  # noqa: E402

LEDGER = Path("docs/agent/V1_HANDOFF_FINDINGS.md")
FIXED_HEADING = "## Fixed after the handoff"


class LedgerEditError(ValueError):
    """The requested edit does not fit the ledger."""


def _row_index(lines: list[str], key: str) -> int:
    found = [
        index
        for index, line in enumerate(lines)
        if finding_status.cells(line)[:1] == [key]
    ]
    if len(found) != 1:
        raise LedgerEditError(f"{key} is in {len(found)} rows, not one")
    return found[0]


def _row(cells: Sequence[str], *, empty_last: bool = False) -> str:
    """One table row; only a family's key list (``empty_last``) may be empty."""
    for position, cell in enumerate(cells):
        empty_allowed = empty_last and position == len(cells) - 1
        if (not cell and not empty_allowed) or "\n" in cell:
            raise LedgerEditError(f"cell {cell[:60]!r} is empty or multi-line")
        if finding_status.has_unescaped_bar(cell):
            raise LedgerEditError(f"cell {cell[:60]!r} has a bare |; write \\|")
    return "| " + " | ".join(cells) + " |"


def _family_index(
    lines: list[str], status: finding_status.FindingStatus, letter: str
) -> int:
    family = next((f for f in status.dependency_families if f.letter == letter), None)
    if family is None:
        raise LedgerEditError(f"no family {letter} in the dependency order")
    return family.line - 1


def _edit_family(
    lines: list[str], index: int, edit: Callable[[list[str]], list[str]]
) -> None:
    cells = finding_status.cells(lines[index])
    keys = [key.strip() for key in cells[2].split(",") if key.strip()]
    cells[2] = ", ".join(edit(keys))
    lines[index] = _row(cells, empty_last=True)


def _families_holding(status: finding_status.FindingStatus, key: str) -> list[str]:
    return [f.letter for f in status.dependency_families if key in f.keys]


def _table_top(lines: list[str], heading: str) -> int:
    """The index just after the table rule that follows ``heading``."""
    start = lines.index(heading)
    for index in range(start + 1, len(lines)):
        if lines[index].startswith("## "):
            break
        if lines[index].startswith("|-"):
            return index + 1
    raise LedgerEditError(f"no table under {heading!r}")


def open_row(text: str, args: argparse.Namespace) -> str:
    status = finding_status.project(text)
    if any(row.key == args.key for row in status.rows):
        raise LedgerEditError(f"{args.key} already exists")
    lines = text.split("\n")
    row = _row([args.key, args.defect, args.evidence, args.next])
    if args.after is not None:
        position = _row_index(lines, args.after) + 1
    else:
        position = _table_top(lines, f"## {args.section}")
    lines.insert(position, row)
    status = finding_status.project("\n".join(lines))
    _edit_family(
        lines, _family_index(lines, status, args.family), lambda k: [*k, args.key]
    )
    return "\n".join(lines)


def close_row(text: str, args: argparse.Namespace) -> str:
    status = finding_status.project(text)
    row = next((r for r in status.rows if r.key == args.key), None)
    if row is None or row.state != finding_status.STATE_OPEN:
        raise LedgerEditError(f"{args.key} is not an open row")
    lines = text.split("\n")
    index = _row_index(lines, args.key)
    defect = args.defect or finding_status.cells(lines[index])[1]
    for letter in _families_holding(status, args.key):
        _edit_family(
            lines,
            _family_index(lines, status, letter),
            lambda keys: [k for k in keys if k != args.key],
        )
    del lines[index]
    lines.insert(_table_top(lines, FIXED_HEADING), _row([args.key, defect, args.fix]))
    return "\n".join(lines)


def merge_rows(text: str, args: argparse.Namespace) -> str:
    status = finding_status.project(text)
    open_keys = set(status.open_keys)
    for key in (args.key, args.into):
        if key not in open_keys:
            raise LedgerEditError(f"{key} is not an open row")
    lines = text.split("\n")
    into = _row_index(lines, args.into)
    cells = finding_status.cells(lines[into])
    if len(cells) != 4:
        raise LedgerEditError(f"{args.into} does not have four cells")
    today = args.date or dt.date.today().isoformat()
    absorbed = f"This row absorbed {args.key} on {today}."
    cells[1] = " ".join(p for p in (cells[1], args.defect_note, absorbed) if p)
    if args.evidence_note:
        cells[2] = f"{cells[2]} {args.evidence_note}"
    if args.next_note:
        cells[3] = f"{cells[3]} {args.next_note}"
    lines[into] = _row(cells)
    for letter in _families_holding(status, args.key):
        _edit_family(
            lines,
            _family_index(lines, status, letter),
            lambda keys: [k for k in keys if k != args.key],
        )
    del lines[_row_index(lines, args.key)]
    return "\n".join(lines)


def next_key(text: str) -> str:
    numbers = [
        int(key.removeprefix("HF-"))
        for key in (row.key for row in finding_status.project(text).rows)
        if key.startswith("HF-") and key.removeprefix("HF-").isdigit()
    ]
    return f"HF-{max(numbers, default=0) + 1}"


def _checked(root: Path, text: str) -> list[str]:
    status = finding_status.project(text)
    prior_art = (root / finding_coverage.PRIOR_ART_PATH).read_text(encoding="utf-8")
    return [
        *status.problems,
        *finding_coverage.dependency_order_problems(status, prior_art),
    ]


def _references(text: str, key: str) -> list[str]:
    return [
        f"line {number}: {line[:120]}"
        for number, line in enumerate(text.split("\n"), start=1)
        if key in line and finding_status.cells(line)[:1] != [key]
    ]


def main(argv: list[str] | None = None, *, root: Path = _REPO_ROOT) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("next-key")
    opened = sub.add_parser("open")
    opened.add_argument("key")
    opened.add_argument("--family", required=True)
    opened.add_argument("--section", required=True)
    opened.add_argument("--after")
    for name in ("defect", "evidence", "next"):
        opened.add_argument(f"--{name}", required=True)
    closed = sub.add_parser("close")
    closed.add_argument("key")
    closed.add_argument("--fix", required=True)
    closed.add_argument("--defect")
    merged = sub.add_parser("merge")
    merged.add_argument("key")
    merged.add_argument("--into", required=True)
    merged.add_argument("--defect-note")
    merged.add_argument("--evidence-note")
    merged.add_argument("--next-note")
    merged.add_argument(
        "--date", help="ISO date for the absorption note (default today)"
    )
    args = parser.parse_args(argv)

    path = root / LEDGER
    text = path.read_text(encoding="utf-8")
    if args.command == "next-key":
        print(next_key(text))
        return 0
    edit = {"open": open_row, "close": close_row, "merge": merge_rows}[args.command]
    try:
        updated = edit(text, args)
    except LedgerEditError as exc:
        print(f"ledger_edit: {exc}", file=sys.stderr)
        return 2
    problems = _checked(root, updated)
    if problems:
        print(
            "ledger_edit: refused; the edit leaves:",
            *problems,
            sep="\n  ",
            file=sys.stderr,
        )
        return 1
    path.write_text(updated, encoding="utf-8")
    projection = root / gen_finding_ids.OUTPUT
    projection.write_text(gen_finding_ids.render(root), encoding="utf-8")
    print(
        f"ledger_edit: {args.command} {args.key}; wrote {LEDGER} and {gen_finding_ids.OUTPUT}"
    )
    if args.command in ("close", "merge"):
        for reference in _references(updated, args.key):
            print(f"  still names {args.key}: {reference}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Edit the v1.0 findings ledger by row: open, close, merge, note, class, next-key.

``docs/agent/V1_HANDOFF_FINDINGS.md`` owns the burndown. Its protocol moves a
row, its family in the dependency order and the generated finding identities
together. Each command edits through ``tools.finding_status``'s projection,
rewrites ``tests/findings/finding_ids.py`` and refuses to write unless the
family-coverage check (``tools/finding_coverage.py``) passes afterwards, so a
row never lands outside the DAG.

    ledger_edit.py next-key
    ledger_edit.py open HF-590 --family F --v1-class release_apparatus --section "Open: CI, harness and \
release engineering" [--after HF-587] --defect ... --evidence ... --next ...
    ledger_edit.py close HF-506 --fix ... [--defect ...]
    ledger_edit.py merge HF-477 --into HF-442 [--defect-note ...] \
[--evidence-note ...] [--next-note ...]
    ledger_edit.py note HF-185 [--defect-note ...] [--evidence-note ...] \
[--next-note ...]
    ledger_edit.py class HF-605 --v1-class release_apparatus
    ledger_edit.py v01 set HF-605 tier2
    ledger_edit.py v01 apply classification.json
    ledger_edit.py family create W --stratum engine --axes empty/startup --name ... --cause ...
    ledger_edit.py edge add F W --reason ...
    ledger_edit.py edge remove F W

The edges table owns row readiness and critical-path priority. Each open row
also has exactly one v1 class; row edits maintain that membership together. An
open row may carry one v0.1 class (``finding_status.V01Class``); a row without
one holds a 0.x release. ``close`` and ``merge`` remove both classes, and a
merge keeps the stricter v0.1 class of the two rows.

Cell text is Markdown; a literal ``|`` must be written ``\\|``.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
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
# The section whose rows are release requirements, with three cells.
RELEASE_REQUIREMENTS_SECTION = "Open release requirements"


class LedgerEditError(ValueError):
    """The requested edit does not fit the ledger."""


def _row_index(lines: list[str], key: str) -> int:
    found = [
        row.line - 1
        for row in finding_status.project("\n".join(lines)).rows
        if row.key == key
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
    keys = [key.strip() for key in cells[-1].split(",") if key.strip()]
    cells[-1] = ", ".join(edit(keys))
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
    membership = [
        entry.line for entry in finding_status.project("\n".join(lines)).v1_membership
    ]
    if membership:
        position = max(membership)
    else:
        try:
            position = lines.index("| Row | V1 class |") + 2
        except ValueError as exc:
            raise LedgerEditError("the ledger has no v1 membership table") from exc
    lines.insert(position, _row([args.key, args.v1_class]))
    opened = "\n".join(lines)
    if args.v01_class is None:
        return opened
    return _set_v01_classes(opened, {args.key: finding_status.V01Class(args.v01_class)})


def _remove_membership(text: str, key: str) -> str:
    status = finding_status.project(text)
    doomed = sorted(
        (
            entry.line
            for entry in (*status.v1_membership, *status.v01_membership)
            if entry.key == key
        ),
        reverse=True,
    )
    lines = text.split("\n")
    for line in doomed:
        del lines[line - 1]
    return "\n".join(lines)


V01_DATA_SCHEMA = "molt.v01-classification.v1"


def _set_v01_classes(text: str, classes: dict[str, finding_status.V01Class]) -> str:
    """Add or restate the v0.1 class of each open row, in one rewrite."""
    status = finding_status.project(text)
    open_keys = set(status.open_keys)
    for key in classes:
        if key not in open_keys:
            raise LedgerEditError(f"{key} is not an open row")
    current = {entry.key: entry for entry in status.v01_membership}
    lines = text.split("\n")
    for key, value in classes.items():
        entry = current.get(key)
        if entry is not None:
            lines[entry.line - 1] = _row([key, value.value])
    new_rows = sorted(
        (key for key in classes if key not in current),
        key=finding_status.finding_sort_key,
    )
    if not new_rows:
        return "\n".join(lines)
    rendered = [_row([key, classes[key].value]) for key in new_rows]
    if current:
        # Keep the table in canonical finding order: merge, then rewrite it.
        first = min(entry.line for entry in current.values()) - 1
        last = max(entry.line for entry in current.values())
        existing = {finding_status.cells(line)[0]: line for line in lines[first:last]}
        existing.update(zip(new_rows, rendered, strict=True))
        lines[first:last] = [
            existing[key]
            for key in sorted(existing, key=finding_status.finding_sort_key)
        ]
        return "\n".join(lines)
    membership = [entry.line for entry in status.v1_membership]
    if not membership:
        raise LedgerEditError("the ledger has no v1 membership table")
    position = max(membership)
    header = "| " + " | ".join(finding_status.V01_MEMBERSHIP_HEADER) + " |"
    lines[position:position] = ["", header, "|---|---|", *rendered]
    return "\n".join(lines)


def set_v01_class(text: str, args: argparse.Namespace) -> str:
    """Add or restate one open row's v0.1 class."""
    return _set_v01_classes(text, {args.key: finding_status.V01Class(args.v01_class)})


def apply_v01_classes(text: str, args: argparse.Namespace) -> str:
    """Apply a classification data file: {"schema": ..., "rows": {key: class}}."""
    data = json.loads(Path(args.data).read_text(encoding="utf-8"))
    if not isinstance(data, dict) or set(data) != {"schema", "rows"}:
        raise LedgerEditError("classification data needs exactly schema and rows")
    if data["schema"] != V01_DATA_SCHEMA:
        raise LedgerEditError(f"classification data schema must be {V01_DATA_SCHEMA}")
    rows = data["rows"]
    if not isinstance(rows, dict) or not rows:
        raise LedgerEditError("classification data rows must be a non-empty object")
    try:
        classes = {
            str(key): finding_status.V01Class(value) for key, value in rows.items()
        }
    except ValueError as exc:
        raise LedgerEditError(str(exc)) from exc
    return _set_v01_classes(text, classes)


def classify_row(text: str, args: argparse.Namespace) -> str:
    """Restate an open row's v1 class in place."""
    entry = next(
        (e for e in finding_status.project(text).v1_membership if e.key == args.key),
        None,
    )
    if entry is None:
        raise LedgerEditError(f"{args.key} has no v1 class")
    if entry.classification == args.v1_class:
        raise LedgerEditError(f"{args.key} is already {args.v1_class}")
    lines = text.split("\n")
    lines[entry.line - 1] = _row([args.key, args.v1_class])
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
    return _remove_membership("\n".join(lines), args.key)


def _append_notes(
    lines: list[str],
    status: finding_status.FindingStatus,
    key: str,
    args: argparse.Namespace,
    *,
    trailer: str = "",
) -> None:
    """Append the note arguments (and ``trailer``) to open row ``key``'s cells.

    The row's section sets its table: a release requirement has the
    requirement and its acceptance boundary, and a finding has the defect,
    its evidence and the next step.
    """
    index = _row_index(lines, key)
    (row,) = (row for row in status.rows if row.key == key)
    requirement = row.section == RELEASE_REQUIREMENTS_SECTION
    cells = finding_status.cells(lines[index])
    if len(cells) != (3 if requirement else 4):
        raise LedgerEditError(
            f"{key} has {len(cells)} cells; a "
            f"{'release requirement has three' if requirement else 'finding has four'}"
        )
    if requirement and args.next_note:
        raise LedgerEditError(
            f"{key} is a release requirement and has no next-step cell; "
            "note its acceptance boundary with --evidence-note"
        )
    cells[1] = " ".join(p for p in (cells[1], args.defect_note, trailer) if p)
    if args.evidence_note:
        cells[2] = f"{cells[2]} {args.evidence_note}"
    if args.next_note:
        cells[3] = f"{cells[3]} {args.next_note}"
    lines[index] = _row(cells)


def note_row(text: str, args: argparse.Namespace) -> str:
    if not (args.defect_note or args.evidence_note or args.next_note):
        raise LedgerEditError(
            "a note needs --defect-note, --evidence-note or --next-note"
        )
    status = finding_status.project(text)
    if args.key not in status.open_keys:
        raise LedgerEditError(f"{args.key} is not an open row")
    lines = text.split("\n")
    _append_notes(lines, status, args.key, args)
    return "\n".join(lines)


def merge_rows(text: str, args: argparse.Namespace) -> str:
    status = finding_status.project(text)
    open_keys = set(status.open_keys)
    for key in (args.key, args.into):
        if key not in open_keys:
            raise LedgerEditError(f"{key} is not an open row")
    lines = text.split("\n")
    today = args.date or dt.date.today().isoformat()
    _append_notes(
        lines,
        status,
        args.into,
        args,
        trailer=f"This row absorbed {args.key} on {today}.",
    )
    for letter in _families_holding(status, args.key):
        _edit_family(
            lines,
            _family_index(lines, status, letter),
            lambda keys: [k for k in keys if k != args.key],
        )
    del lines[_row_index(lines, args.key)]
    v01 = status.v01_classes
    absorbed, kept = v01.get(args.key), v01.get(args.into)
    merged = _remove_membership("\n".join(lines), args.key)
    if kept is not None and absorbed != kept:
        # The stricter class wins; an unclassified side holds like blocking.
        rank = finding_status.V01_STRICTNESS.index
        stricter = (
            finding_status.V01Class.V0_1_BLOCKING
            if absorbed is None
            else min(absorbed, kept, key=rank)
        )
        if stricter != kept:
            merged = _set_v01_classes(merged, {args.into: stricter})
    lines = merged.split("\n")
    for edge in reversed(finding_status.project(merged).dependency_edges):
        if args.key not in (edge.source, edge.target):
            continue
        source = args.into if edge.source == args.key else edge.source
        target = args.into if edge.target == args.key else edge.target
        if source == target:
            del lines[edge.line - 1]
        else:
            lines[edge.line - 1] = _row([source, target, edge.reason])
    return "\n".join(lines)


def move_to_family(text: str, args: argparse.Namespace) -> str:
    """Move open rows into one family; each row stays in exactly one family."""
    status = finding_status.project(text)
    open_keys = set(status.open_keys)
    for key in args.keys:
        if key not in open_keys:
            raise LedgerEditError(f"{key} is not an open row")
    lines = text.split("\n")
    target = _family_index(lines, status, args.to)
    for key in args.keys:
        for letter in _families_holding(status, key):
            if letter != args.to:
                _edit_family(
                    lines,
                    _family_index(lines, status, letter),
                    lambda keys, key=key: [k for k in keys if k != key],
                )
    held = set(
        next(
            f.keys
            for f in finding_status.project("\n".join(lines)).dependency_families
            if f.letter == args.to
        )
    )
    _edit_family(
        lines, target, lambda keys: [*keys, *(k for k in args.keys if k not in held)]
    )
    return "\n".join(lines)


def describe_family(text: str, args: argparse.Namespace) -> str:
    """Restate one family's name, shared cause and, if given, its HF-459 axes."""
    status = finding_status.project(text)
    lines = text.split("\n")
    index = _family_index(lines, status, args.letter)
    cells = finding_status.cells(lines[index])
    cells[0] = f"{args.letter}. {args.name}"
    cells[-2] = args.cause
    if args.axes is not None:
        cells[2] = args.axes
    lines[index] = _row(cells, empty_last=True)
    return "\n".join(lines)


def create_family(text: str, args: argparse.Namespace) -> str:
    """Add an empty family; the coverage check then requires its prior-art section."""
    status = finding_status.project(text)
    if any(f.letter == args.letter for f in status.dependency_families):
        raise LedgerEditError(f"family {args.letter} already exists")
    if not status.dependency_families:
        raise LedgerEditError("the ledger has no family dependency order table")
    lines = text.split("\n")
    last = max(family.line for family in status.dependency_families) - 1
    lines.insert(
        last + 1,
        _row(
            [f"{args.letter}. {args.name}", args.stratum, args.axes, args.cause, ""],
            empty_last=True,
        ),
    )
    return "\n".join(lines)


def _edge(status: finding_status.FindingStatus, source: str, target: str):
    return next(
        (
            edge
            for edge in status.dependency_edges
            if (edge.source, edge.target) == (source, target)
        ),
        None,
    )


def add_edge(text: str, args: argparse.Namespace) -> str:
    """Order a row or family after another row or family."""
    status = finding_status.project(text)
    if not status.edge_table_lines:
        raise LedgerEditError("the dependency order has no edges table")
    if _edge(status, args.source, args.target) is not None:
        raise LedgerEditError(f"edge {args.source} -> {args.target} already exists")
    lines = text.split("\n")
    position = (
        max(edge.line for edge in status.dependency_edges)
        if status.dependency_edges
        else status.edge_table_lines[0] + 1
    )
    lines.insert(position, _row([args.source, args.target, args.reason]))
    return "\n".join(lines)


def remove_edge(text: str, args: argparse.Namespace) -> str:
    status = finding_status.project(text)
    edge = _edge(status, args.source, args.target)
    if edge is None:
        raise LedgerEditError(f"no edge {args.source} -> {args.target}")
    lines = text.split("\n")
    del lines[edge.line - 1]
    return "\n".join(lines)


def recut_families(text: str, args: argparse.Namespace) -> str:
    """Apply a re-cut (family renames, then row moves) as one checked edit.

    Renaming several families at once passes through states in which the
    ledger and the prior-art record disagree, so the whole re-cut is applied
    before the single coverage check runs.
    """
    try:
        spec = json.loads(Path(args.spec).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise LedgerEditError(f"cannot read re-cut spec {args.spec}: {exc}") from exc
    if not isinstance(spec, dict) or set(spec) - {"describe", "move"}:
        raise LedgerEditError("a re-cut spec holds only 'describe' and 'move' lists")
    for item in spec.get("describe", []):
        if not isinstance(item, dict) or not (
            {"letter", "name", "cause"}
            <= set(item)
            <= {"letter", "name", "cause", "axes"}
        ):
            raise LedgerEditError(
                f"describe entry needs letter, name and cause (axes optional): {item!r}"
            )
        text = describe_family(text, argparse.Namespace(**{"axes": None, **item}))
    for item in spec.get("move", []):
        if not isinstance(item, dict) or set(item) != {"keys", "to"}:
            raise LedgerEditError(f"move entry needs keys and to: {item!r}")
        text = move_to_family(text, argparse.Namespace(**item))
    return text


def next_key(text: str) -> str:
    numbers = [
        int(key.removeprefix("HF-"))
        for key in (row.key for row in finding_status.project(text).rows)
        if key.startswith("HF-") and key.removeprefix("HF-").isdigit()
    ]
    return f"HF-{max(numbers, default=0) + 1}"


def _checked(root: Path, text: str) -> list[str]:
    return finding_coverage.ledger_problems(root, finding_status.project(text))


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
    opened.add_argument(
        "--v1-class",
        required=True,
        choices=[value.value for value in finding_status.V1Class],
    )
    opened.add_argument(
        "--v01-class",
        choices=[value.value for value in finding_status.V01Class],
        help="the row's v0.1 class; without one the row holds a 0.x release",
    )
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
    merged.add_argument(
        "--date", help="ISO date for the absorption note (default today)"
    )
    noted = sub.add_parser("note")
    noted.add_argument("key")
    classified = sub.add_parser("class", help="restate an open row's v1 class")
    classified.add_argument("key")
    classified.add_argument(
        "--v1-class",
        required=True,
        choices=[value.value for value in finding_status.V1Class],
    )
    v01 = sub.add_parser("v01", help="set or apply v0.1 classes of open rows")
    v01_sub = v01.add_subparsers(dest="v01_command", required=True)
    v01_set = v01_sub.add_parser("set")
    v01_set.add_argument("key")
    v01_set.add_argument(
        "v01_class", choices=[value.value for value in finding_status.V01Class]
    )
    v01_apply = v01_sub.add_parser("apply")
    v01_apply.add_argument("data", help=f"JSON file of schema {V01_DATA_SCHEMA}")
    for command in (merged, noted):
        for name in ("defect-note", "evidence-note", "next-note"):
            command.add_argument(f"--{name}")
    family = sub.add_parser("family", help="move rows between families or restate one")
    family_sub = family.add_subparsers(dest="family_command", required=True)
    moved = family_sub.add_parser("move")
    moved.add_argument("keys", nargs="+")
    moved.add_argument("--to", required=True)
    described = family_sub.add_parser("describe")
    described.add_argument("letter")
    described.add_argument("--name", required=True)
    described.add_argument("--cause", required=True)
    described.add_argument(
        "--axes",
        help="comma-separated HF-459 instruction axis keys (tools/perf_instructions.py) (default: unchanged)",
    )
    recut = family_sub.add_parser("recut")
    recut.add_argument("--spec", required=True, help="JSON: describe and move lists")
    created = family_sub.add_parser("create", help="add an empty family")
    created.add_argument("letter")
    created.add_argument("--name", required=True)
    created.add_argument("--cause", required=True)
    created.add_argument(
        "--stratum", required=True, choices=list(finding_status.FamilyStratum)
    )
    created.add_argument(
        "--axes",
        default="-",
        help="comma-separated HF-459 instruction axis keys (tools/perf_instructions.py) (required for engine families)",
    )
    edge = sub.add_parser("edge", help="add or remove a family dependency edge")
    edge_sub = edge.add_subparsers(dest="edge_command", required=True)
    for name in ("add", "remove"):
        command = edge_sub.add_parser(name)
        command.add_argument("source")
        command.add_argument("target")
        if name == "add":
            command.add_argument("--reason", required=True)
    args = parser.parse_args(argv)

    path = root / LEDGER
    text = path.read_text(encoding="utf-8")
    if args.command == "next-key":
        print(next_key(text))
        return 0
    if args.command == "family":
        edit = {
            "move": move_to_family,
            "describe": describe_family,
            "recut": recut_families,
            "create": create_family,
        }[args.family_command]
        subject = {
            "move": lambda: " ".join(args.keys) + f" -> {args.to}",
            "describe": lambda: f"family {args.letter}",
            "recut": lambda: f"re-cut {args.spec}",
            "create": lambda: f"family {args.letter}",
        }[args.family_command]()
    elif args.command == "edge":
        edit = {"add": add_edge, "remove": remove_edge}[args.edge_command]
        subject = f"{args.source} -> {args.target}"
    elif args.command == "v01":
        edit = {"set": set_v01_class, "apply": apply_v01_classes}[args.v01_command]
        subject = args.key if args.v01_command == "set" else f"classes from {args.data}"
    else:
        edit = {
            "open": open_row,
            "close": close_row,
            "merge": merge_rows,
            "note": note_row,
            "class": classify_row,
        }[args.command]
        subject = args.key
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
        f"ledger_edit: {args.command} {subject}; wrote {LEDGER} and {gen_finding_ids.OUTPUT}"
    )
    if args.command in ("close", "merge"):
        for reference in _references(updated, args.key):
            print(f"  still names {args.key}: {reference}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

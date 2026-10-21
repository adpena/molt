#!/usr/bin/env python3
"""Fail-closed RATIO-DIRECTION drift-gate.

The meta-bug this kills (verification-machinery audit, item 2:
ratio-direction canonicalization). A ratio of two wall-clock times is
meaningless without its direction: ``a/b`` and ``b/a`` are both "the ratio" yet
one says molt is 3x FASTER and the other 3x SLOWER. The bench lanes historically
computed raw ``molt_time / cpython_time`` (and four sibling external-runtime
ratios) directly into a JSON record that carried NO direction field, beside the
correctly-directed ``perf_authority.safe_speedup`` - so a downstream/ranking
consumer could read a 3x-slower cell as a 3x win, and a None/0/NaN external time
rendered a finite slowness ratio entirely unguarded. This is the master
meta-bug class PROXY-MEASUREMENT SUBSTITUTION's sibling
AUTHORITY-SINGLE-SOURCED-IN-NAME-NOT-IN-REACH (a correct guard beside an
unguarded twin).

The structural fix routes EVERY wall-clock ratio through the single guarded
implementation authority ``molt.metric_ratios.signed_ratio`` (re-exported for
tools as ``perf_authority.signed_ratio``) with explicit ``RatioDirection``
metadata. This gate makes the unguarded twin UNEXPRESSIBLE: it fails CI on any
``<x>_time`` / ``<x>_time_s`` / ``<x>_ms`` ratio computed in executable code
ANYWHERE except ``src/molt/metric_ratios.py`` (the one source-owned module
permitted to divide one timing by another).

Detection is AST-based, NOT a raw line-grep, so it is strictly more precise than
the regex the audit sketched: it flags a real ``BinOp`` whose operator is ``/``
or ``*`` and whose BOTH sides contain ``_time``-suffixed value references
(``Name`` / ``Attribute`` / ``Subscript``), including normalized deltas such as
``(new_time - old_time) / old_time``. It is structurally incapable of being
fooled by the token appearing inside a string literal, an f-string, a comment,
or a docstring (e.g. the canonical board's own ``"speedup = cpython_time /
molt_time"`` direction LABEL, which a naive grep would falsely flag). A
division hidden in an f-string - which the audit's sketched regex would have
MISSED - is caught here because the AST sees the real BinOp.

Runs as an ``always`` landing gate and the ``repository.ratio-direction``
proof-plan command (tests: tests/tools/test_signed_ratio.py), so an unguarded
``time/time`` re-introduction cannot silently regress.

Usage::

    python3 tools/check_ratio_direction.py            # gate (exit 1 on drift)
    python3 tools/check_ratio_direction.py --json      # machine-readable report
"""

from __future__ import annotations

import argparse
from collections import deque
import ast
import json
import sys
from pathlib import Path
import warnings


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402
from tools.git_identity import repository_source_paths  # noqa: E402
from tools.scan_records import (  # noqa: E402
    ScanRecords,
    decoded_text,
    open_scan_records,
    scan_result,
)

REPO_ROOT = Path(__file__).resolve().parents[1]

_COMMANDS = CommandExecutor.for_file(__file__)

# The ONE module permitted to divide a time by a time: the source authority.
# Every other module must route through molt.metric_ratios directly or through
# perf_authority's re-exported signed_ratio / signed_ratio_value (which carry
# RatioDirection metadata + the None/0/NaN guard). This is the exact
# single-source-in-REACH the audit demands.
EXEMPT_RELATIVE = {
    "src/molt/metric_ratios.py",
}


def _is_time_name(name: str) -> bool:
    return (
        name.endswith("_time")
        or name.endswith("_time_s")
        or name.endswith("_ms")
        or name in {"mean_ms", "median_ms"}
    )


def _is_time_operand(node: ast.expr) -> bool:
    """True if ``node`` is a value reference naming a timing measurement.

    Matches the operand shapes a real wall-clock division uses:

      * ``molt_time``                     -> Name
      * ``entry.molt_time`` / ``r.molt_time_s`` -> Attribute
      * ``result["molt_time_s"]``               -> Subscript with str key
      * ``stats["molt_time"]``            -> Subscript with a constant str key
      * ``cell.warm_time``                -> Attribute

    The trailing identifier (the Name id, the Attribute attr, or a constant
    string subscript key) must look like a timing field. This covers the live
    benchmark naming families (`*_time`, `*_time_s`, `*_ms`, `mean_ms`,
    `median_ms`) while staying AST-based so strings/comments cannot trip it.
    """
    name: str | None = None
    if isinstance(node, ast.Name):
        name = node.id
    elif isinstance(node, ast.Attribute):
        name = node.attr
    elif isinstance(node, ast.Subscript):
        key = node.slice
        if isinstance(key, ast.Constant) and isinstance(key.value, str):
            name = key.value
    if name is None:
        return False
    return _is_time_name(name)


def _time_ratio_hits(tree: ast.AST) -> list[tuple[int, str]]:
    """Compute operand facts once, retaining original preorder diagnostics.

    Traverse every syntax context, including f-string expressions, independently
    of the narrower value-expression edges through which timing facts propagate.
    Reverse parent-before-child order visits operands before their owners without
    Python recursion. Only actual hits need a second walk for preorder output.
    All temporary facts belong to this file; absent set membership means False.
    """
    expressions: list[ast.expr] = []
    candidates: list[ast.BinOp] = []
    timing: set[ast.expr] = set()
    direct_references = 0
    leaf_types = (ast.Name, ast.Constant)
    pending_nodes = deque([tree])
    while pending_nodes:
        node = pending_nodes.popleft()
        if isinstance(node, ast.expr):
            if _is_time_operand(node):
                timing.add(node)
                direct_references += 1
            elif isinstance(
                node, (ast.BinOp, ast.UnaryOp, ast.IfExp, ast.NamedExpr, ast.Call)
            ):
                expressions.append(node)
        if isinstance(node, ast.BinOp) and isinstance(node.op, (ast.Div, ast.Mult)):
            candidates.append(node)
        if type(node) not in leaf_types:
            pending_nodes.extend(ast.iter_child_nodes(node))

    # Each reported ratio needs a direct reference in each disjoint operand
    # subtree. This counts parsed occurrences, including decoded string keys.
    if not candidates or direct_references < 2:
        return []
    for node in reversed(expressions):
        contains = False
        if isinstance(node, ast.BinOp):
            contains = node.left in timing or node.right in timing
        elif isinstance(node, ast.UnaryOp):
            contains = node.operand in timing
        elif isinstance(node, ast.IfExp):
            contains = node.body in timing or node.orelse in timing
        elif isinstance(node, ast.NamedExpr):
            contains = node.value in timing
        elif isinstance(node, ast.Call):
            contains = any(arg in timing for arg in node.args) or any(
                keyword.value in timing for keyword in node.keywords
            )
        if contains:
            timing.add(node)
    hits = {node for node in candidates if node.left in timing and node.right in timing}
    if not hits:
        return []
    ordered: list[tuple[int, str]] = []
    pending = [tree]
    while pending:
        node = pending.pop()
        if isinstance(node, ast.BinOp) and node in hits:
            ordered.append((node.lineno, "/" if isinstance(node.op, ast.Div) else "*"))
        pending.extend(reversed(list(ast.iter_child_nodes(node))))
    return ordered


def _read_source_git(cwd: Path, arguments: list[str]) -> bytes:
    return _COMMANDS.run(
        ["git", *arguments], cwd=cwd, capture_output=True, timeout=30, check=True
    ).stdout


def _iter_py_files() -> list[Path]:
    # Git owns ignored and nested-checkout exclusions. Missing admitted files
    # remain selected so a failed source read cannot silently make the gate pass.
    return [
        path
        for path in repository_source_paths(
            REPO_ROOT, run_git=_read_source_git, include_untracked=True
        )
        if path.suffix == ".py" and not path.is_dir()
    ]


def _rel(path: Path) -> str:
    try:
        return path.resolve().relative_to(REPO_ROOT).as_posix()
    except ValueError:
        return path.name


def source_ratio_hits(data: bytes) -> list[tuple[int, str]]:
    """The timing ratios in one source: a pure function of its bytes."""
    try:
        source = decoded_text(data, encoding="utf-8")
    except UnicodeDecodeError:
        source = data.decode("utf-8", errors="replace")
    # Executable Div/Mult syntax requires these literal operator characters.
    # Timing names cannot be preselected by raw suffix counts: the parser
    # normalizes identifiers and decodes/concatenates constant string keys.
    if "/" not in source and "*" not in source:
        return []
    try:
        # Parser warnings concern the file's own owner, not this gate, and a
        # replayed result must report exactly what a computed one reports.
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", SyntaxWarning)
            tree = ast.parse(source)
    except SyntaxError:
        # A file that does not parse is not our concern here (other gates catch
        # it); it cannot contain a live time/time division we must flag.
        return []
    return _time_ratio_hits(tree)


def scan_file(path: Path, records: ScanRecords | None = None) -> list[dict]:
    """Return a list of violation records for one file (empty == clean)."""
    if (
        any(path == REPO_ROOT / rel for rel in EXEMPT_RELATIVE)
        and _rel(path) in EXEMPT_RELATIVE
    ):
        return []
    try:
        data = path.read_bytes()
    except OSError:
        # Preserve exempt aliases' read-error outcome. Readable aliases may now
        # be parsed, but physical exemption still precedes every diagnostic.
        if _rel(path) in EXEMPT_RELATIVE:
            return []
        raise
    # The hits depend on the bytes alone, so records key them by content and
    # identical files share one entry. Exemption stays a per-path decision.
    hits = scan_result(
        records,
        "",
        data,
        lambda: source_ratio_hits(data),
        encode=lambda found: [[line, op] for line, op in found],
        decode=lambda stored: [(int(line), str(op)) for line, op in stored],
    )
    if not hits:
        return []
    rel = _rel(path)
    if rel in EXEMPT_RELATIVE:
        return []
    return [
        {
            "path": rel,
            "line": lineno,
            "op": op,
            "message": (
                f"{rel}:{lineno}: raw `<x>_time {op} <y>_time` division outside "
                "src/molt/metric_ratios.py -- route it through "
                "molt.metric_ratios.signed_ratio / signed_ratio_value so the ratio "
                "carries explicit RatioDirection metadata and a None/0/NaN time "
                "can never become a finite ratio."
            ),
        }
        for lineno, op in hits
    ]


def run(records: ScanRecords | None = None) -> dict:
    violations: list[dict] = []
    files = _iter_py_files()
    for path in files:
        violations.extend(scan_file(path, records))
    return {
        "kind": "ratio_direction_drift",
        "files_scanned": len(files),
        "exempt": sorted(EXEMPT_RELATIVE),
        "violations": violations,
    }


def main(argv: list[str] | None = None, *, scan_records: bool = False) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--json", action="store_true", help="machine-readable report")
    ns = parser.parse_args(argv)
    with open_scan_records(
        "check_ratio_direction", owner=__file__, enabled=scan_records
    ) as records:
        report = run(records)

    if ns.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    elif report["violations"]:
        print(
            "ratio-direction: FAIL -- raw time/time division(s) outside "
            "src/molt/metric_ratios.py (unguarded, direction-less):"
        )
        for v in report["violations"]:
            print(f"  - {v['message']}")
        print(
            "  (every wall-clock ratio MUST route through "
            "molt.metric_ratios.signed_ratio -- ratio-direction canonicalization.)"
        )
    else:
        print(
            f"ratio-direction: OK -- scanned {report['files_scanned']} file(s); "
            "every wall-clock ratio routes through molt.metric_ratios.signed_ratio."
        )

    return 1 if report["violations"] else 0


if __name__ == "__main__":
    sys.exit(main(scan_records=True))

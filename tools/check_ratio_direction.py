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

Wired into ci_gate tier-1 via tests/tools/test_signed_ratio.py so an unguarded
``time/time`` re-introduction cannot silently regress.

Usage::

    python3 tools/check_ratio_direction.py            # gate (exit 1 on drift)
    python3 tools/check_ratio_direction.py --json      # machine-readable report
"""

from __future__ import annotations

import argparse
import ast
import json
import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The ONE module permitted to divide a time by a time: the source authority.
# Every other module must route through molt.metric_ratios directly or through
# perf_authority's re-exported signed_ratio / signed_ratio_value (which carry
# RatioDirection metadata + the None/0/NaN guard). This is the exact
# single-source-in-REACH the audit demands.
EXEMPT_RELATIVE = {
    "src/molt/metric_ratios.py",
}

# Directories scanned for executable Python. Perf ratios live in the bench
# tooling and harness; we scan the whole tree's *.py so a stray time/time
# division anywhere is caught, but skip vendored / generated trees AND nested
# git worktrees. ``.claude`` holds sibling-agent worktrees (each a FULL,
# independent checkout of this repo under .claude/worktrees/, with its own
# target/ + its own pre-/post-fix copy of the bench lanes) plus session
# scratch; scanning them is both wrong (they are separate checkouts, not THIS
# tree's source, so this gate governs THIS working tree only) and catastrophically
# slow (it would walk every sibling's target/). The gate guards the canonical
# tree; each worktree's own ci_gate run guards itself.
SKIP_DIR_PARTS = {
    ".git",
    ".claude",
    "target",
    "node_modules",
    "repos",
    ".venv",
    "venv",
    "__pycache__",
    ".uv-cache",
    ".molt_cache",
    "tmp",
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


def _contains_time_operand(node: ast.expr) -> bool:
    """True if ``node`` contains a timing value reference.

    Direct-operand scans miss normalized deltas such as
    ``(new_time - old_time) / old_time``. Walk arithmetic/value expressions so
    those ratios are caught while unit conversions like ``timeout_ms / 1000``
    remain outside this gate.
    """
    if _is_time_operand(node):
        return True
    if isinstance(node, ast.BinOp):
        return _contains_time_operand(node.left) or _contains_time_operand(node.right)
    if isinstance(node, ast.UnaryOp):
        return _contains_time_operand(node.operand)
    if isinstance(node, ast.IfExp):
        return _contains_time_operand(node.body) or _contains_time_operand(node.orelse)
    if isinstance(node, ast.NamedExpr):
        return _contains_time_operand(node.value)
    if isinstance(node, ast.Call):
        return any(_contains_time_operand(arg) for arg in node.args) or any(
            _contains_time_operand(keyword.value) for keyword in node.keywords
        )
    return False


class _TimeRatioFinder(ast.NodeVisitor):
    """Collect every timing-expression ``/`` or ``*`` timing-expression BinOp."""

    def __init__(self) -> None:
        self.hits: list[tuple[int, str]] = []

    def visit_BinOp(self, node: ast.BinOp) -> None:  # noqa: N802
        if isinstance(node.op, (ast.Div, ast.Mult)) and (
            _contains_time_operand(node.left) and _contains_time_operand(node.right)
        ):
            op = "/" if isinstance(node.op, ast.Div) else "*"
            self.hits.append((node.lineno, op))
        # Recurse so a nested time/time inside a larger expression is still seen.
        self.generic_visit(node)


def _iter_py_files() -> list[Path]:
    # Walk with os.walk and PRUNE skipped directories in place so the traversal
    # never descends into target/, .git/, .venv/, etc. (Path.rglob has no
    # pruning and would stat every file under those huge trees first - a tier-1
    # gate must stay fast; this is the same prune-during-walk discipline the
    # other repo scanners use.)
    out: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(REPO_ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIR_PARTS]
        for fn in filenames:
            if fn.endswith(".py"):
                out.append(Path(dirpath) / fn)
    return sorted(out)


def _rel(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(REPO_ROOT)).replace("\\", "/")
    except ValueError:
        return path.name


def scan_file(path: Path) -> list[dict]:
    """Return a list of violation records for one file (empty == clean)."""
    rel = _rel(path)
    if rel in EXEMPT_RELATIVE:
        return []
    try:
        source = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        source = path.read_bytes().decode("utf-8", errors="replace")
    # Cheap SOUND pre-filter before the (expensive) full AST parse: a timing
    # BinOp (``<x> (/|*) <y>`` with both operands timing fields) REQUIRES at
    # least two timing-suffix TOKENS in the source AND a ``/`` or ``*``. The
    # token set must cover EVERY suffix _is_time_name accepts (``_time``,
    # ``_time_s``, ``_ms``, ``mean_ms``/``median_ms``); ``_time_s`` and the
    # ``mean_ms``/``median_ms`` forms already contain ``_time`` / ``_ms``, so
    # counting ``_time`` + ``_ms`` occurrences is a sound lower bound. A file
    # with fewer than two timing tokens (or no ``/``/``*``) cannot contain the
    # pattern, so skipping its parse can NEVER miss a real violation - it only
    # avoids parsing the ~99% of files with no time ratio (keeps this tier-1
    # gate fast over thousands of files). If _is_time_name gains a suffix that
    # is not a superstring of one of these tokens, this filter must gain it too.
    if (source.count("_time") + source.count("_ms")) < 2 or (
        "/" not in source and "*" not in source
    ):
        return []
    try:
        tree = ast.parse(source, filename=str(path))
    except SyntaxError:
        # A file that does not parse is not our concern here (other gates catch
        # it); it cannot contain a live time/time division we must flag.
        return []
    finder = _TimeRatioFinder()
    finder.visit(tree)
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
        for lineno, op in finder.hits
    ]


def run() -> dict:
    violations: list[dict] = []
    files = _iter_py_files()
    for path in files:
        violations.extend(scan_file(path))
    return {
        "kind": "ratio_direction_drift",
        "files_scanned": len(files),
        "exempt": sorted(EXEMPT_RELATIVE),
        "violations": violations,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--json", action="store_true", help="machine-readable report")
    ns = parser.parse_args(argv)
    report = run()

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
    sys.exit(main())

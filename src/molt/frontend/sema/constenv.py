"""Const-environment analysis (doc 44 §F2b ``constenv.py``).

Free function over ``ast.Module`` — the ``cfg_analysis.py`` house shape.  Lifts
``SimpleTIRGenerator._collect_module_const_dicts`` verbatim.  This is a true
pre-walk fact: it is computed once in ``visit_Module`` and only ever read
(``self.module_const_dicts[...]`` is read by class/call lowering, never
re-written during the walk).
"""

from __future__ import annotations

import ast
from typing import Any


def collect_module_const_dicts(node: ast.Module) -> dict[str, dict[str, Any]]:
    """Collect module-level assignments of the form NAME = {"key": value, ...}
    where all keys are string literals and all values are constants (bool, int,
    str, None).  Used to resolve compile-time **kwargs spreads like
    @dataclass(**SLOTS) where SLOTS = {"slots": True}.

    Also scans inside top-level if/else blocks (e.g., version-gated
    constants like `if sys.version_info >= (3, 10): SLOTS = {"slots": True}`)."""
    result: dict[str, dict[str, Any]] = {}

    def _scan_assign(stmt: ast.AST) -> None:
        if not isinstance(stmt, ast.Assign):
            return
        if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
            return
        name = stmt.targets[0].id
        if not isinstance(stmt.value, ast.Dict):
            return
        d = stmt.value
        entries: dict[str, Any] = {}
        valid = True
        for k, v in zip(d.keys, d.values):
            if not isinstance(k, ast.Constant) or not isinstance(k.value, str):
                valid = False
                break
            if not isinstance(v, ast.Constant):
                valid = False
                break
            entries[k.value] = v.value
        if valid:
            result[name] = entries

    for stmt in node.body:
        _scan_assign(stmt)
        # Also scan inside top-level if/else blocks for version-gated constants
        if isinstance(stmt, ast.If):
            for sub in stmt.body:
                _scan_assign(sub)
            for sub in stmt.orelse:
                _scan_assign(sub)

    return result

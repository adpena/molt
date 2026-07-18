#!/usr/bin/env python3
"""Mutation testing infrastructure for Molt compiler correctness (MOL-282).

Compiler mutation authority:

**Compiler mutation**:
    Systematically mutates the Molt *compiler* Python source (under ``src/molt/``)
    and runs the differential test suite against each mutant.  Mutations that
    survive (tests still pass) reveal gaps in test coverage of the compiler
    itself.

Mutation score = killed / (killed + survived).

Usage examples:
    # Compiler mutation
    uv run --python 3.12 python3 tools/mutation_test.py \\
        --target src/molt/frontend/__init__.py \\
        --max-mutations 20 \\
        --test-subset tests/differential/basic \\
        --timeout 120

Exit codes:
    0 — all mutations were killed (score = 100%)
    1 — at least one mutation survived
    2 — infrastructure error (build/setup failure)
"""

from __future__ import annotations

import argparse
import ast
import copy
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from tools import harness_memory_guard  # noqa: E402
from tools.proof_counts import fail_closed_proof_exit_code  # noqa: E402
from molt.dx import cargo_target_dir_for_artifact_root  # noqa: E402

DEFAULT_TARGET_DIR = SRC_ROOT / "molt"
DEFAULT_TEST_SUBSET = REPO_ROOT / "tests" / "differential" / "basic"

MutationOperatorName = Literal[
    "arith_op",
    "cmp_op",
    "bool_op",
    "const_replace",
    "stmt_delete",
    "return_mutate",
    "loop_bound",
    "exception_swallow",
    "slice_modify",
    "string_method_swap",
    "container_method_swap",
]

COMPILER_OPERATORS: list[MutationOperatorName] = [
    "arith_op",
    "cmp_op",
    "bool_op",
    "const_replace",
    "stmt_delete",
    "return_mutate",
    "loop_bound",
    "exception_swallow",
    "slice_modify",
    "string_method_swap",
    "container_method_swap",
]

# Mapping for arithmetic operator replacement
_ARITH_SWAPS: dict[type, type] = {
    ast.Add: ast.Sub,
    ast.Sub: ast.Add,
    ast.Mult: ast.FloorDiv,
    ast.FloorDiv: ast.Mult,
    ast.Mod: ast.Add,
    ast.Div: ast.Mult,
    ast.Pow: ast.Mult,
    ast.LShift: ast.RShift,
    ast.RShift: ast.LShift,
    ast.BitOr: ast.BitAnd,
    ast.BitAnd: ast.BitOr,
    ast.BitXor: ast.BitAnd,
}

# Mapping for comparison operator replacement
_CMP_SWAPS: dict[type, type] = {
    ast.Eq: ast.NotEq,
    ast.NotEq: ast.Eq,
    ast.Lt: ast.GtE,
    ast.GtE: ast.Lt,
    ast.Gt: ast.LtE,
    ast.LtE: ast.Gt,
    ast.Is: ast.IsNot,
    ast.IsNot: ast.Is,
    ast.In: ast.NotIn,
    ast.NotIn: ast.In,
}

_STRING_METHOD_SWAPS: dict[str, str] = {
    "strip": "rstrip",
    "rstrip": "strip",
    "upper": "lower",
    "lower": "upper",
    "startswith": "endswith",
    "endswith": "startswith",
    "title": "lower",
}

_CONTAINER_METHOD_SWAPS: dict[str, str] = {
    "add": "discard",
    "discard": "add",
    "extend": "append",
}


# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------


@dataclass
class MutationSite:
    """A location in source where a mutation can be applied."""

    file: str
    lineno: int
    col_offset: int
    operator: MutationOperatorName
    description: str
    node_index: int  # index into the file's AST walk order


@dataclass
class MutationResult:
    """Outcome of running tests against a single mutant."""

    source_file: str
    operator: str
    description: str
    status: str  # "killed", "survived", "build_fail", "timeout", "skip"
    lineno: int = 0
    elapsed_s: float = 0.0
    original_output: str = ""
    mutated_output: str = ""
    error: str | None = None


@dataclass
class MutationReport:
    """Aggregate mutation testing report."""

    results: list[MutationResult] = field(default_factory=list)

    @property
    def killed(self) -> int:
        return sum(1 for r in self.results if r.status == "killed")

    @property
    def survived(self) -> int:
        return sum(1 for r in self.results if r.status == "survived")

    @property
    def build_fail(self) -> int:
        return sum(1 for r in self.results if r.status == "build_fail")

    @property
    def timeout(self) -> int:
        return sum(1 for r in self.results if r.status == "timeout")

    @property
    def skipped(self) -> int:
        return sum(1 for r in self.results if r.status == "skip")

    @property
    def total_scoreable(self) -> int:
        return self.killed + self.survived

    @property
    def score(self) -> float:
        total = self.total_scoreable
        if total == 0:
            return 0.0
        return self.killed / total

    def summary(self) -> str:
        lines = [
            "",
            "=" * 60,
            "  Mutation Testing Report",
            "=" * 60,
            f"  Total mutations applied : {len(self.results)}",
            f"  Killed (detected)       : {self.killed}",
            f"  Survived (undetected)   : {self.survived}",
            f"  Build failures          : {self.build_fail}",
            f"  Timeouts                : {self.timeout}",
            f"  Skipped                 : {self.skipped}",
            "-" * 60,
            f"  Mutation score          : {self.score:.1%}"
            f"  ({self.killed}/{self.total_scoreable})",
            "=" * 60,
            "",
        ]
        return "\n".join(lines)

    def summary_dict(self) -> dict[str, Any]:
        per_file: dict[str, dict[str, int]] = {}
        for r in self.results:
            fname = r.source_file
            bucket = per_file.setdefault(
                fname, {"total": 0, "killed": 0, "survived": 0}
            )
            bucket["total"] += 1
            if r.status == "killed":
                bucket["killed"] += 1
            elif r.status == "survived":
                bucket["survived"] += 1
        survivors = [
            {
                "file": r.source_file,
                "line": r.lineno,
                "operator": r.operator,
                "description": r.description,
            }
            for r in self.results
            if r.status == "survived"
        ]
        return {
            "schema": "molt.mutation-proof.v1",
            "status": (
                "success"
                if self.killed > 0
                and self.survived == 0
                and self.build_fail == 0
                and self.timeout == 0
                and self.skipped == 0
                else "failure"
            ),
            "mutation_score": round(self.score, 4),
            "total": len(self.results),
            "executed": self.killed + self.survived,
            "killed": self.killed,
            "survived": self.survived,
            "build_fail": self.build_fail,
            "timeout": self.timeout,
            "skipped": self.skipped,
            "elapsed_s": round(sum(r.elapsed_s for r in self.results), 2),
            "per_file": per_file,
            "surviving_mutants": survivors,
            "results": [
                {
                    "source_file": r.source_file,
                    "operator": r.operator,
                    "description": r.description,
                    "status": r.status,
                    "lineno": r.lineno,
                    "error": r.error,
                }
                for r in self.results
            ],
        }


# ===========================================================================
# MODE 1: Compiler mutation (new)
# ===========================================================================


# ---------------------------------------------------------------------------
# Mutation discovery (AST-based)
# ---------------------------------------------------------------------------


class MutationFinder(ast.NodeVisitor):
    """Walk an AST and collect all potential mutation sites."""

    def __init__(self, file_path: str, operators: set[MutationOperatorName]) -> None:
        self.file_path = file_path
        self.operators = operators
        self.sites: list[MutationSite] = []
        self._node_counter = 0

    def _next_index(self) -> int:
        idx = self._node_counter
        self._node_counter += 1
        return idx

    # --- Arithmetic ---
    def visit_BinOp(self, node: ast.BinOp) -> None:
        if "arith_op" in self.operators:
            op_type = type(node.op)
            if op_type in _ARITH_SWAPS:
                replacement = _ARITH_SWAPS[op_type]
                self.sites.append(
                    MutationSite(
                        file=self.file_path,
                        lineno=node.lineno,
                        col_offset=node.col_offset,
                        operator="arith_op",
                        description=(f"{op_type.__name__} -> {replacement.__name__}"),
                        node_index=self._next_index(),
                    )
                )
        self.generic_visit(node)

    # --- Comparison ---
    def visit_Compare(self, node: ast.Compare) -> None:
        if "cmp_op" in self.operators:
            for i, op in enumerate(node.ops):
                op_type = type(op)
                if op_type in _CMP_SWAPS:
                    replacement = _CMP_SWAPS[op_type]
                    self.sites.append(
                        MutationSite(
                            file=self.file_path,
                            lineno=node.lineno,
                            col_offset=node.col_offset,
                            operator="cmp_op",
                            description=(
                                f"cmp[{i}] {op_type.__name__} -> {replacement.__name__}"
                            ),
                            node_index=self._next_index(),
                        )
                    )
        self.generic_visit(node)

    # --- Boolean logic ---
    def visit_BoolOp(self, node: ast.BoolOp) -> None:
        if "bool_op" in self.operators:
            op_type = type(node.op)
            replacement = ast.Or if isinstance(node.op, ast.And) else ast.And
            self.sites.append(
                MutationSite(
                    file=self.file_path,
                    lineno=node.lineno,
                    col_offset=node.col_offset,
                    operator="bool_op",
                    description=(f"{op_type.__name__} -> {replacement.__name__}"),
                    node_index=self._next_index(),
                )
            )
        self.generic_visit(node)

    def visit_UnaryOp(self, node: ast.UnaryOp) -> None:
        if "bool_op" in self.operators and isinstance(node.op, ast.Not):
            self.sites.append(
                MutationSite(
                    file=self.file_path,
                    lineno=node.lineno,
                    col_offset=node.col_offset,
                    operator="bool_op",
                    description="Not removal",
                    node_index=self._next_index(),
                )
            )
        self.generic_visit(node)

    # --- Constant replacement ---
    def visit_Constant(self, node: ast.Constant) -> None:
        if "const_replace" in self.operators:
            val = node.value
            desc = None
            if isinstance(val, bool):
                desc = f"{val} -> {not val}"
            elif isinstance(val, int) and not isinstance(val, bool):
                if val == 0:
                    desc = "0 -> 1"
                elif val == 1:
                    desc = "1 -> 0"
            elif isinstance(val, str) and len(val) <= 20:
                desc = f'"{val}" -> ""' if val else '""-> "mutant"'
            if desc:
                self.sites.append(
                    MutationSite(
                        file=self.file_path,
                        lineno=node.lineno,
                        col_offset=node.col_offset,
                        operator="const_replace",
                        description=desc,
                        node_index=self._next_index(),
                    )
                )
        self.generic_visit(node)

    # --- Statement deletion ---
    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._check_stmt_delete(node)
        self.generic_visit(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._check_stmt_delete(node)
        self.generic_visit(node)

    def _check_stmt_delete(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        if "stmt_delete" not in self.operators:
            return
        for i, stmt in enumerate(node.body):
            # Only target assignment and expression statements to avoid
            # trivially broken mutants from removing control flow.
            if isinstance(stmt, (ast.Assign, ast.AugAssign, ast.Expr)):
                self.sites.append(
                    MutationSite(
                        file=self.file_path,
                        lineno=stmt.lineno,
                        col_offset=stmt.col_offset,
                        operator="stmt_delete",
                        description=(f"delete {type(stmt).__name__} at body[{i}]"),
                        node_index=self._next_index(),
                    )
                )

    # --- Return value mutation ---
    def visit_Return(self, node: ast.Return) -> None:
        if "return_mutate" in self.operators and node.value is not None:
            self.sites.append(
                MutationSite(
                    file=self.file_path,
                    lineno=node.lineno,
                    col_offset=node.col_offset,
                    operator="return_mutate",
                    description="return <value> -> return None",
                    node_index=self._next_index(),
                )
            )
        self.generic_visit(node)

    # --- Loop bound mutation ---
    def visit_For(self, node: ast.For) -> None:
        self._check_loop_bound(node)
        self.generic_visit(node)

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        self._check_loop_bound(node)
        self.generic_visit(node)

    def _check_loop_bound(self, node: ast.For | ast.AsyncFor) -> None:
        if "loop_bound" not in self.operators:
            return
        iter_call = node.iter
        if (
            isinstance(iter_call, ast.Call)
            and isinstance(iter_call.func, ast.Name)
            and iter_call.func.id == "range"
            and iter_call.args
        ):
            bound_index = 0 if len(iter_call.args) == 1 else 1
            self.sites.append(
                MutationSite(
                    file=self.file_path,
                    lineno=node.lineno,
                    col_offset=node.col_offset,
                    operator="loop_bound",
                    description=f"range arg[{bound_index}] -> arg[{bound_index}] - 1",
                    node_index=self._next_index(),
                )
            )

    # --- Exception handler mutation ---
    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        if "exception_swallow" in self.operators and node.body:
            self.sites.append(
                MutationSite(
                    file=self.file_path,
                    lineno=node.lineno,
                    col_offset=node.col_offset,
                    operator="exception_swallow",
                    description="replace except body with pass",
                    node_index=self._next_index(),
                )
            )
        self.generic_visit(node)

    # --- Slice mutation ---
    def visit_Subscript(self, node: ast.Subscript) -> None:
        if "slice_modify" in self.operators and isinstance(node.slice, ast.Slice):
            slice_node = node.slice
            desc: str | None = None
            if slice_node.lower is not None:
                desc = "slice lower -> lower + 1"
            elif slice_node.upper is not None:
                desc = "slice upper -> upper - 1"
            if desc is not None:
                self.sites.append(
                    MutationSite(
                        file=self.file_path,
                        lineno=node.lineno,
                        col_offset=node.col_offset,
                        operator="slice_modify",
                        description=desc,
                        node_index=self._next_index(),
                    )
                )
        self.generic_visit(node)

    # --- Method swaps ---
    def visit_Call(self, node: ast.Call) -> None:
        if isinstance(node.func, ast.Attribute):
            if "string_method_swap" in self.operators:
                replacement = _STRING_METHOD_SWAPS.get(node.func.attr)
                if replacement is not None:
                    self.sites.append(
                        MutationSite(
                            file=self.file_path,
                            lineno=node.lineno,
                            col_offset=node.col_offset,
                            operator="string_method_swap",
                            description=f"{node.func.attr} -> {replacement}",
                            node_index=self._next_index(),
                        )
                    )
            if "container_method_swap" in self.operators:
                self._check_container_method_swap(node)
        self.generic_visit(node)

    def _check_container_method_swap(self, node: ast.Call) -> None:
        func = node.func
        assert isinstance(func, ast.Attribute)
        if func.attr == "append" and len(node.args) == 1 and not node.keywords:
            desc = "append(x) -> extend([x])"
        else:
            replacement = _CONTAINER_METHOD_SWAPS.get(func.attr)
            if replacement is None:
                return
            desc = f"{func.attr} -> {replacement}"
        self.sites.append(
            MutationSite(
                file=self.file_path,
                lineno=node.lineno,
                col_offset=node.col_offset,
                operator="container_method_swap",
                description=desc,
                node_index=self._next_index(),
            )
        )


def discover_mutations(
    file_path: str,
    source: str,
    operators: set[MutationOperatorName],
) -> list[MutationSite]:
    """Parse *source* and return all applicable mutation sites."""
    try:
        tree = ast.parse(source, filename=file_path)
    except SyntaxError:
        return []
    finder = MutationFinder(file_path, operators)
    finder.visit(tree)
    return finder.sites


# ---------------------------------------------------------------------------
# Mutation application (AST rewrite — single-site)
# ---------------------------------------------------------------------------


class _MutationApplier(ast.NodeTransformer):
    """Apply exactly one mutation identified by a MutationSite."""

    def __init__(self, site: MutationSite) -> None:
        self.site = site
        self._applied = False

    def _match(self, node: ast.AST) -> bool:
        return (
            not self._applied
            and getattr(node, "lineno", -1) == self.site.lineno
            and getattr(node, "col_offset", -1) == self.site.col_offset
        )

    # --- Arithmetic ---
    def visit_BinOp(self, node: ast.BinOp) -> ast.AST:
        if self.site.operator == "arith_op" and self._match(node):
            op_type = type(node.op)
            if op_type in _ARITH_SWAPS:
                node.op = _ARITH_SWAPS[op_type]()
                self._applied = True
                return node
        return self.generic_visit(node)

    # --- Comparison ---
    def visit_Compare(self, node: ast.Compare) -> ast.AST:
        if self.site.operator == "cmp_op" and self._match(node):
            for i, op in enumerate(node.ops):
                op_type = type(op)
                if op_type in _CMP_SWAPS:
                    expected = (
                        f"cmp[{i}] {op_type.__name__} -> {_CMP_SWAPS[op_type].__name__}"
                    )
                    if expected == self.site.description:
                        node.ops[i] = _CMP_SWAPS[op_type]()
                        self._applied = True
                        return node
        return self.generic_visit(node)

    # --- Boolean ---
    def visit_BoolOp(self, node: ast.BoolOp) -> ast.AST:
        if self.site.operator == "bool_op" and self._match(node):
            if isinstance(node.op, ast.And):
                node.op = ast.Or()
            else:
                node.op = ast.And()
            self._applied = True
            return node
        return self.generic_visit(node)

    def visit_UnaryOp(self, node: ast.UnaryOp) -> ast.AST:
        if (
            self.site.operator == "bool_op"
            and self._match(node)
            and isinstance(node.op, ast.Not)
            and "Not removal" in self.site.description
        ):
            self._applied = True
            return node.operand  # remove the Not
        return self.generic_visit(node)

    # --- Constant ---
    def visit_Constant(self, node: ast.Constant) -> ast.AST:
        if self.site.operator == "const_replace" and self._match(node):
            val = node.value
            if isinstance(val, bool):
                node.value = not val
                self._applied = True
            elif isinstance(val, int) and not isinstance(val, bool):
                if val == 0:
                    node.value = 1
                    self._applied = True
                elif val == 1:
                    node.value = 0
                    self._applied = True
            elif isinstance(val, str):
                node.value = "" if val else "mutant"
                self._applied = True
            return node
        return self.generic_visit(node)

    # --- Statement deletion ---
    def visit_FunctionDef(self, node: ast.FunctionDef) -> ast.AST:
        self._maybe_delete_stmt(node)
        return self.generic_visit(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> ast.AST:
        self._maybe_delete_stmt(node)
        return self.generic_visit(node)

    def _maybe_delete_stmt(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        if self.site.operator != "stmt_delete" or self._applied:
            return
        new_body: list[ast.stmt] = []
        for stmt in node.body:
            if (
                not self._applied
                and stmt.lineno == self.site.lineno
                and stmt.col_offset == self.site.col_offset
                and isinstance(stmt, (ast.Assign, ast.AugAssign, ast.Expr))
            ):
                # Replace with pass to keep syntax valid.
                new_body.append(
                    ast.Pass(
                        lineno=stmt.lineno,
                        col_offset=stmt.col_offset,
                    )
                )
                self._applied = True
            else:
                new_body.append(stmt)
        node.body = new_body

    # --- Return mutation ---
    def visit_Return(self, node: ast.Return) -> ast.AST:
        if (
            self.site.operator == "return_mutate"
            and self._match(node)
            and node.value is not None
        ):
            node.value = ast.Constant(
                value=None,
                lineno=node.lineno,
                col_offset=node.col_offset,
            )
            self._applied = True
            return node
        return self.generic_visit(node)

    # --- Loop bound mutation ---
    def visit_For(self, node: ast.For) -> ast.AST:
        self._maybe_mutate_loop_bound(node)
        return self.generic_visit(node)

    def visit_AsyncFor(self, node: ast.AsyncFor) -> ast.AST:
        self._maybe_mutate_loop_bound(node)
        return self.generic_visit(node)

    def _maybe_mutate_loop_bound(self, node: ast.For | ast.AsyncFor) -> None:
        if self.site.operator != "loop_bound" or not self._match(node):
            return
        iter_call = node.iter
        if (
            not isinstance(iter_call, ast.Call)
            or not isinstance(iter_call.func, ast.Name)
            or iter_call.func.id != "range"
            or not iter_call.args
        ):
            return
        bound_index = 0 if len(iter_call.args) == 1 else 1
        iter_call.args[bound_index] = ast.BinOp(
            left=copy.deepcopy(iter_call.args[bound_index]),
            op=ast.Sub(),
            right=ast.Constant(value=1),
        )
        self._applied = True

    # --- Exception handler mutation ---
    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> ast.AST:
        if self.site.operator == "exception_swallow" and self._match(node):
            node.body = [ast.Pass()]
            self._applied = True
            return node
        return self.generic_visit(node)

    # --- Slice mutation ---
    def visit_Subscript(self, node: ast.Subscript) -> ast.AST:
        if (
            self.site.operator == "slice_modify"
            and self._match(node)
            and isinstance(node.slice, ast.Slice)
        ):
            if node.slice.lower is not None and "lower" in self.site.description:
                node.slice.lower = ast.BinOp(
                    left=copy.deepcopy(node.slice.lower),
                    op=ast.Add(),
                    right=ast.Constant(value=1),
                )
                self._applied = True
                return node
            if node.slice.upper is not None and "upper" in self.site.description:
                node.slice.upper = ast.BinOp(
                    left=copy.deepcopy(node.slice.upper),
                    op=ast.Sub(),
                    right=ast.Constant(value=1),
                )
                self._applied = True
                return node
        return self.generic_visit(node)

    # --- Method swaps ---
    def visit_Call(self, node: ast.Call) -> ast.AST:
        if not isinstance(node.func, ast.Attribute):
            return self.generic_visit(node)
        if self.site.operator == "string_method_swap" and self._match(node):
            replacement = _STRING_METHOD_SWAPS.get(node.func.attr)
            expected = (
                None if replacement is None else f"{node.func.attr} -> {replacement}"
            )
            if expected == self.site.description:
                node.func.attr = replacement
                self._applied = True
                return node
        if self.site.operator == "container_method_swap" and self._match(node):
            if self.site.description == "append(x) -> extend([x])":
                if (
                    len(node.args) == 1
                    and not node.keywords
                    and node.func.attr == "append"
                ):
                    node.func.attr = "extend"
                    node.args = [
                        ast.List(elts=[copy.deepcopy(node.args[0])], ctx=ast.Load())
                    ]
                    self._applied = True
                    return node
            replacement = _CONTAINER_METHOD_SWAPS.get(node.func.attr)
            expected = (
                None if replacement is None else f"{node.func.attr} -> {replacement}"
            )
            if expected == self.site.description:
                node.func.attr = replacement
                self._applied = True
                return node
        return self.generic_visit(node)


def apply_single_mutation(source: str, site: MutationSite) -> str | None:
    """Return mutated source, or None if mutation could not be applied."""
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return None
    applier = _MutationApplier(site)
    new_tree = applier.visit(tree)
    if not applier._applied:
        return None
    ast.fix_missing_locations(new_tree)
    try:
        return ast.unparse(new_tree)
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Temp-dir workspace (never mutate in place!)
# ---------------------------------------------------------------------------


def _temp_root() -> Path:
    ext = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if ext:
        base = Path(ext).expanduser() / "mutation_tmp"
    else:
        base = REPO_ROOT / "tmp" / "mutation_tmp"
    base.mkdir(parents=True, exist_ok=True)
    return base


def _default_cargo_target_dir() -> Path:
    configured = os.environ.get("CARGO_TARGET_DIR", "").strip()
    if configured:
        return Path(configured).expanduser()

    session_id = os.environ.get("MOLT_SESSION_ID") or f"mutation-{os.getpid()}"
    ext = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if ext:
        return cargo_target_dir_for_artifact_root(Path(ext).expanduser(), session_id)

    return cargo_target_dir_for_artifact_root(REPO_ROOT, session_id)


def create_mutant_workspace(
    original_file: Path,
    mutated_source: str,
) -> Path:
    """Copy ``src/molt`` to a temp dir, overwrite the target file, return
    the temp src root (parent of ``molt/``).

    IMPORTANT: The original source tree is NEVER modified.
    """
    workspace = Path(tempfile.mkdtemp(dir=_temp_root(), prefix="mutant_"))
    src_molt = SRC_ROOT / "molt"
    dest_molt = workspace / "molt"
    shutil.copytree(src_molt, dest_molt, symlinks=True)

    # Overwrite the target file inside the copied tree.
    rel = original_file.resolve().relative_to(SRC_ROOT.resolve())
    target = workspace / rel
    target.write_text(mutated_source, encoding="utf-8")
    return workspace


# ---------------------------------------------------------------------------
# Compiler-mutation test runner
# ---------------------------------------------------------------------------


def run_diff_against_mutant(
    workspace: Path,
    test_subset: Path,
    timeout: int,
    build_profile: str = "dev",
) -> tuple[bool, bool, float, str]:
    """Run differential tests with PYTHONPATH pointing at the mutated copy.

    Returns ``(killed, timed_out, elapsed_s, error_snippet)``.
    """
    env = os.environ.copy()
    # Point PYTHONPATH at the mutant workspace so ``import molt`` resolves
    # to the mutated copy.
    env["PYTHONPATH"] = str(workspace)
    # Force single job and no cache to keep mutations isolated.
    env["MOLT_DIFF_FORCE_NO_CACHE"] = "1"
    if not env.get("CARGO_TARGET_DIR", "").strip():
        env["CARGO_TARGET_DIR"] = str(_default_cargo_target_dir())
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)

    cmd = [
        sys.executable,
        "-u",
        str(REPO_ROOT / "tests" / "molt_diff.py"),
        "--build-profile",
        build_profile,
        "--jobs",
        "1",
        str(test_subset),
    ]

    t0 = time.monotonic()
    try:
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(REPO_ROOT),
            env=env,
            limits=limits,
        )
        elapsed = (
            proc.elapsed_s if proc.elapsed_s is not None else time.monotonic() - t0
        )
        # Any non-zero exit means mutation was killed.
        killed = proc.returncode != 0
        snippet = (proc.stderr or proc.stdout or "")[-500:]
        return killed, False, elapsed, snippet
    except subprocess.TimeoutExpired:
        elapsed = time.monotonic() - t0
        return False, True, elapsed, "timeout"


# ---------------------------------------------------------------------------
# Compiler-mutation file collection
# ---------------------------------------------------------------------------


def collect_compiler_files(target: Path) -> list[Path]:
    """Collect Python files to mutate under *target*."""
    if target.is_file():
        return [target]
    files: list[Path] = []
    for p in sorted(target.rglob("*.py")):
        parts = p.parts
        if "__pycache__" in parts:
            continue
        name = p.name
        if name.startswith("_intrinsics") or name.endswith(".generated.py"):
            continue
        files.append(p)
    return files


# ---------------------------------------------------------------------------
# Compiler-mutation campaign driver
# ---------------------------------------------------------------------------


def run_compiler_mutation_campaign(
    targets: list[Path],
    operators: set[MutationOperatorName],
    max_mutations: int,
    test_subset: Path,
    timeout: int,
    build_profile: str = "dev",
    seed: int | None = None,
    verbose: bool = False,
) -> MutationReport:
    """Run the full compiler mutation testing campaign."""
    report = MutationReport()
    # 1. Discover all mutation sites.
    all_sites: list[tuple[MutationSite, str]] = []
    for target_file in targets:
        try:
            source = target_file.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        rel = str(target_file.relative_to(REPO_ROOT))
        sites = discover_mutations(rel, source, operators)
        for site in sites:
            all_sites.append((site, source))

    if not all_sites:
        print("[mutation] No mutation sites found.")
        return report

    # 2. Sample if we exceed max_mutations.
    rng = random.Random(seed)
    if len(all_sites) > max_mutations:
        all_sites = rng.sample(all_sites, max_mutations)

    print(
        f"[mutation] {len(all_sites)} mutations to test across {len(targets)} file(s)"
    )

    # 3. Apply each mutation, run tests.
    for idx, (site, original_source) in enumerate(all_sites, 1):
        label = (
            f"[{idx}/{len(all_sites)}] {site.file}:{site.lineno} "
            f"({site.operator}: {site.description})"
        )
        if verbose:
            print(f"  {label} ...", end=" ", flush=True)

        mutated = apply_single_mutation(original_source, site)
        if mutated is None:
            result = MutationResult(
                source_file=site.file,
                operator=site.operator,
                description=site.description,
                status="skip",
                lineno=site.lineno,
                error="AST mutation failed",
            )
            report.results.append(result)
            if verbose:
                print("SKIP (apply failed)")
            continue

        # Create isolated workspace.
        original_path = REPO_ROOT / site.file
        workspace = create_mutant_workspace(original_path, mutated)
        try:
            killed, timed_out, elapsed, snippet = run_diff_against_mutant(
                workspace, test_subset, timeout, build_profile
            )
        finally:
            shutil.rmtree(workspace, ignore_errors=True)

        if killed:
            status = "killed"
        elif timed_out:
            status = "timeout"
        else:
            status = "survived"

        result = MutationResult(
            source_file=site.file,
            operator=site.operator,
            description=site.description,
            status=status,
            lineno=site.lineno,
            elapsed_s=elapsed,
            error=snippet if not killed else None,
        )
        report.results.append(result)

        if verbose:
            print(f"{status.upper()} ({elapsed:.1f}s)")

    return report


# ===========================================================================
# CLI + main
# ===========================================================================


def print_report(report: MutationReport) -> None:
    """Print a human-readable mutation testing report."""
    print(report.summary())

    summary = report.summary_dict()
    per_file = summary["per_file"]
    if per_file:
        print("  Per-file breakdown:")
        for fname, counts in sorted(per_file.items()):
            total = counts["total"]
            killed = counts["killed"]
            file_score = killed / total * 100 if total else 0
            print(f"    {fname}: {file_score:.0f}% ({killed}/{total})")
        print()

    survivors = summary["surviving_mutants"]
    if survivors:
        print("  SURVIVING MUTANTS (test gaps):")
        for s in survivors:
            line = s.get("line", "?")
            print(f"    {s['file']}:{line} [{s['operator']}] {s['description']}")
        print()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Mutation testing for the Molt compiler.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Mutate compiler source and prove differential detection.",
    )
# --- Compiler-mode flags ---
    parser.add_argument(
        "--target",
        type=Path,
        default=None,
        help="File or directory to mutate (compiler mode, default: src/molt/)",
    )
    parser.add_argument(
        "--operators",
        nargs="+",
        choices=COMPILER_OPERATORS,
        default=None,
        help="Mutation operators (compiler mode)",
    )
    parser.add_argument(
        "--max-mutations",
        type=int,
        default=50,
        help="Max mutations to test (compiler mode, default: 50)",
    )
    parser.add_argument(
        "--test-subset",
        type=Path,
        default=None,
        help="Differential test dir to run (compiler mode, "
        "default: tests/differential/basic)",
    )
    # --- Shared flags ---
    parser.add_argument(
        "--timeout",
        type=int,
        default=120,
        help="Per-mutation timeout in seconds (default: 120)",
    )
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Molt build profile (default: dev)",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed (default: 42)",
    )
    parser.add_argument(
        "--json-out",
        default=None,
        help="Write JSON report (file path, or '-' for stdout)",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Print per-mutation details",
    )

    args = parser.parse_args()
    random.seed(args.seed)

    return _main_compiler(args)

def _main_compiler(args: argparse.Namespace) -> int:
    """Compiler-mutation mode entry point."""
    target = args.target or DEFAULT_TARGET_DIR
    test_subset = args.test_subset or DEFAULT_TEST_SUBSET
    operators = set(args.operators or COMPILER_OPERATORS)

    target_files = collect_compiler_files(target)
    if not target_files:
        print(f"[mutation] No Python files found under {target}")
        return 2

    print(
        f"[mutation] Compiler mutation mode: {len(target_files)} file(s) from {target}"
    )
    print(
        f"[mutation] Test subset: {test_subset}, "
        f"timeout: {args.timeout}s, seed: {args.seed}"
    )
    print(f"[mutation] Operators: {', '.join(sorted(operators))}")
    print()

    report = run_compiler_mutation_campaign(
        targets=target_files,
        operators=operators,
        max_mutations=args.max_mutations,
        test_subset=test_subset,
        timeout=args.timeout,
        build_profile=args.build_profile,
        seed=args.seed,
        verbose=args.verbose or args.json_out is None,
    )

    _emit_output(report, args)

    return fail_closed_proof_exit_code(
        executed=report.killed + report.survived,
        failed=report.survived,
        errors=report.build_fail + report.timeout + report.skipped,
    )


def _emit_output(report: MutationReport, args: argparse.Namespace) -> None:
    """Print or write the report."""
    if args.json_out is not None:
        json_data = report.summary_dict()
        if args.json_out == "-":
            json.dump(json_data, sys.stdout, indent=2)
            print()
        else:
            out_path = Path(args.json_out)
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(json.dumps(json_data, indent=2) + "\n")
            print(f"JSON report written to {args.json_out}")
    else:
        print_report(report)

    # Always print survivors to stderr for visibility.
    survivors = [r for r in report.results if r.status == "survived"]
    if survivors:
        print(
            "\nSurvived mutations (potential gaps):",
            file=sys.stderr,
        )
        for r in survivors:
            line_info = f":{r.lineno}" if r.lineno else ""
            print(
                f"  {r.source_file}{line_info} [{r.operator}]: {r.description}",
                file=sys.stderr,
            )


if __name__ == "__main__":
    sys.exit(main())

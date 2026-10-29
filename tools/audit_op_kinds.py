#!/usr/bin/env python3
"""Audit the Python wire emitter and runtime ABI facts that Rust types cannot prove.

Rust kind membership, mapper/classifier domains and backend routing are closed
OpKind matches checked by rustc. Their former source scrapers are retired.
The frontend producer census remains in tools/op_kinds/census.py.
"""

from __future__ import annotations

import argparse
import ast
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
import tomllib

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]

from tools.op_kinds.paths import TABLE as OP_KINDS_TOML  # noqa: E402

SERIALIZATION_DIR = ROOT / "src/molt/frontend/lowering"
SERIALIZATION_PY = SERIALIZATION_DIR / "serialization.py"
SERIALIZATION_MODULES = (
    SERIALIZATION_PY,
    SERIALIZATION_DIR / "serialization_basic_ops.py",
    SERIALIZATION_DIR / "serialization_collection_ops.py",
    SERIALIZATION_DIR / "serialization_exception_ops.py",
    SERIALIZATION_DIR / "serialization_function_ops.py",
    SERIALIZATION_DIR / "serialization_loop_string_async_ops.py",
    SERIALIZATION_DIR / "serialization_object_attr_ops.py",
    ROOT / "src/molt/cli/backend_ir.py",
)


def _load_op_kinds_toml() -> dict:
    """Read the canonical wire domain used to admit Python serializer output."""
    return tomllib.loads(OP_KINDS_TOML.read_text(encoding="utf-8"))


def _attach_parents(tree: ast.AST) -> None:
    for node in ast.walk(tree):
        for child in ast.iter_child_nodes(node):
            child._parent = node  # type: ignore[attr-defined]


def _kinds_from_test(test: ast.expr) -> set[str] | None:
    """`op.kind == "X"` -> {"X"}; `op.kind in (...)` -> {literals}."""
    if isinstance(test, ast.Compare) and len(test.ops) == 1:
        left = test.left
        if isinstance(left, ast.Attribute) and left.attr == "kind":
            op = test.ops[0]
            comp = test.comparators[0]
            if isinstance(op, ast.Eq) and isinstance(comp, ast.Constant):
                return {comp.value}
            if isinstance(op, ast.In) and isinstance(
                comp, (ast.Tuple, ast.List, ast.Set)
            ):
                return {
                    e.value
                    for e in comp.elts
                    if isinstance(e, ast.Constant) and isinstance(e.value, str)
                }
    return None


def _enclosing_kind_guard(node: ast.AST) -> set[str] | None:
    cur = node
    while getattr(cur, "_parent", None) is not None:
        par = cur._parent  # type: ignore[attr-defined]
        if isinstance(par, ast.If):
            kinds = _kinds_from_test(par.test)
            if kinds is not None:
                return kinds
        cur = par
    return None


def _enclosing_function(node: ast.AST) -> ast.AST | None:
    cur = node
    while getattr(cur, "_parent", None) is not None:
        par = cur._parent  # type: ignore[attr-defined]
        if isinstance(par, (ast.FunctionDef, ast.AsyncFunctionDef)):
            return par
        cur = par
    return None


def _resolve_name_assignment(
    func: ast.AST, name: str, guard_kinds: set[str] | None
) -> set[str] | None:
    """Resolve a local `name = <expr>` inside `func` to the kind string(s) it can
    take, for the two structural transforms used by the emitter:

      * `name = op.kind.lower()`            -> {k.lower() for k in guard_kinds}
      * `name = {"A": "a", ...}[op.kind]`   -> the dict's string values
    """
    for sub in ast.walk(func):
        if isinstance(sub, ast.Assign):
            if any(isinstance(t, ast.Name) and t.id == name for t in sub.targets):
                val = sub.value
                # op.kind.lower()
                if (
                    isinstance(val, ast.Call)
                    and isinstance(val.func, ast.Attribute)
                    and val.func.attr == "lower"
                    and isinstance(val.func.value, ast.Attribute)
                    and val.func.value.attr == "kind"
                ):
                    if guard_kinds is None:
                        return None
                    return {k.lower() for k in guard_kinds}
                # {DICT}[op.kind]
                if isinstance(val, ast.Subscript) and isinstance(val.value, ast.Dict):
                    return {
                        v.value
                        for v in val.value.values
                        if isinstance(v, ast.Constant) and isinstance(v.value, str)
                    }
                return None
    # CLI metadata emission forwards a literal kind through a private helper
    # method. Resolve its class-local calls, refusing even one dynamic or
    # missing argument rather than treating a partial set as exhaustive.
    owner = getattr(func, "_parent", None)
    if (
        isinstance(func, (ast.FunctionDef, ast.AsyncFunctionDef))
        and isinstance(owner, ast.ClassDef)
        and func.name.startswith("_")
    ):
        params = [arg.arg for arg in (*func.args.posonlyargs, *func.args.args)]
        if name not in params[1:]:
            return None
        argument_index = params.index(name) - 1
        kinds: set[str] = set()
        for call in ast.walk(owner):
            if not (
                isinstance(call, ast.Call)
                and isinstance(call.func, ast.Attribute)
                and isinstance(call.func.value, ast.Name)
                and call.func.value.id == params[0]
                and call.func.attr == func.name
            ):
                continue
            argument = (
                call.args[argument_index]
                if len(call.args) > argument_index
                else next((kw.value for kw in call.keywords if kw.arg == name), None)
            )
            resolved = _static_kind_strings(argument) if argument is not None else None
            if resolved is None:
                return None
            kinds.update(resolved)
        return kinds or None
    return None


@dataclass
class FrontendKinds:
    constant: set[str] = field(default_factory=set)
    computed: dict[str, set[str]] = field(default_factory=dict)  # file:line -> kinds
    unresolved: list[tuple[str, int, str]] = field(default_factory=list)

    @property
    def all(self) -> set[str]:
        out = set(self.constant)
        for ks in self.computed.values():
            out |= ks
        return out


def _static_kind_strings(expr: ast.expr) -> set[str] | None:
    """Resolve a fully-static `"kind"` value expression to the set of string
    literals it can evaluate to. Handles the two static emission idioms:
    a bare string constant, and a (possibly nested) conditional expression
    whose branches are themselves static — e.g. the shared binary/inplace
    arms `"div" if op.kind == "DIV" else "inplace_div"`. Returns None for
    anything dynamic (those flow to the guard/assignment resolvers)."""
    if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
        return {expr.value}
    if isinstance(expr, ast.IfExp):
        body = _static_kind_strings(expr.body)
        orelse = _static_kind_strings(expr.orelse)
        if body is not None and orelse is not None:
            return body | orelse
    return None


def extract_frontend_kinds(root: Path = ROOT) -> FrontendKinds:
    fk = FrontendKinds()
    for path in SERIALIZATION_MODULES:
        path = root / path.relative_to(ROOT)
        src = path.read_text(encoding="utf-8")
        tree = ast.parse(src, filename=str(path))
        _attach_parents(tree)
        rel_path = path.relative_to(root).as_posix()
        for node in ast.walk(tree):
            if not isinstance(node, ast.Dict):
                continue
            for k, v in zip(node.keys, node.values):
                if not (isinstance(k, ast.Constant) and k.value == "kind"):
                    continue
                static = _static_kind_strings(v)
                if static is not None:
                    fk.constant.update(static)
                    continue
                ln = getattr(v, "lineno", -1)
                guard = _enclosing_kind_guard(node)
                func = _enclosing_function(node)
                resolved: set[str] | None = None
                if isinstance(v, ast.Attribute) and v.attr == "kind":
                    # bare `op.kind` under a guard with (lowercase) literals
                    resolved = set(guard) if guard else None
                elif (
                    isinstance(v, ast.Call)
                    and not v.args
                    and not v.keywords
                    and isinstance(v.func, ast.Attribute)
                    and v.func.attr == "lower"
                    and isinstance(v.func.value, ast.Attribute)
                    and v.func.value.attr == "kind"
                ):
                    resolved = {kind.lower() for kind in guard} if guard else None
                elif isinstance(v, ast.Name) and func is not None:
                    resolved = _resolve_name_assignment(func, v.id, guard)
                if resolved:
                    fk.computed[f"{rel_path}:{ln}"] = resolved
                else:
                    fk.unresolved.append((rel_path, ln, ast.dump(v)[:60]))
    return fk


def llvm_boxed_runtime_abi_mismatches() -> list[str]:
    from tools.llvm_runtime_abi_audit import boxed_runtime_export_issues

    return [
        f"{issue.problem}:{issue.symbol}/{issue.classified_arity}:"
        f"expected={issue.expected}:actual={issue.actual}"
        for issue in boxed_runtime_export_issues()
    ]


@dataclass
class AuditResult:
    frontend: FrontendKinds
    unregistered_frontend_wire: set[str]
    llvm_boxed_runtime_abi_mismatch: list[str]

    def dangerous(self) -> dict[str, list[str]]:
        return {
            "unregistered_frontend_wire": sorted(self.unregistered_frontend_wire),
            "llvm_boxed_runtime_abi_mismatch": sorted(
                self.llvm_boxed_runtime_abi_mismatch
            ),
        }


def run_audit() -> AuditResult:
    from tools.op_kinds.registration import registered_simpleir_kinds

    frontend = extract_frontend_kinds()
    return AuditResult(
        frontend,
        frontend.all - registered_simpleir_kinds(_load_op_kinds_toml()),
        llvm_boxed_runtime_abi_mismatches(),
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    result = run_audit()
    report = result.dangerous() | {"unresolved_frontend": result.frontend.unresolved}
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(
            "op-kind emitter/ABI audit: " + ("FAILED" if any(report.values()) else "OK")
        )
        for category, findings in report.items():
            for finding in findings:
                print(f"  {category}: {finding}")
    return int(any(report.values()))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

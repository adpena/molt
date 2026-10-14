"""Prove which frontend op kinds production code can build (the producer census).

The registry (``op_kinds.toml``) declares which kinds exist. It cannot see
whether anything still builds them. When a producer is deleted, its kind stays
registered and every string-matched consumer still compiles (HF-197). This
census reads the frontend source and proves the set of ``MoltOp`` kinds that a
production path can build. It also records every identifier-like string literal
in the compiler's Python (``MENTION_ROOTS``): a literal that spells a registered
kind is a consumer of that kind, whether it sits in a comparison, a set, a dict
key or a registry override.

Every ``MoltOp`` kind argument must resolve to a finite set of string literals:

* a literal, or a conditional expression whose branches resolve;
* ``<op>.kind``, a re-emit of an existing operation, which builds nothing new;
* a local or enclosing-scope name, every binding of which resolves;
* a parameter, resolved over every direct call of its function;
* a subscript or ``.get`` of a dict whose values resolve: a literal dict, a
  module or class constant, or a generated ``*_OP_KIND`` table;
* a call to a frontend function whose every ``return`` resolves.

Anything else is unresolved and the census reports the site. There is no
fallback: an unresolved site is a defect in the census or in the code. The
result over-approximates what can run (a resolved branch may be unreachable),
so a kind the census does not produce has no production producer at all.
"""

from __future__ import annotations

import ast
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path

from .paths import ROOT

FRONTEND = Path("src/molt/frontend")
# Compiler Python that may spell frontend op kinds. The stdlib is user-level
# Python compiled by Molt; its strings are not op kinds.
MENTION_ROOTS = (
    Path("src/molt/frontend"),
    Path("src/molt/debug"),
    Path("tools/op_kinds"),
)
MENTION_EXCLUDED = (Path("src/molt/stdlib"),)
GENERATED_MODULE = "molt.frontend.lowering.op_kinds_generated"
_MAX_DEPTH = 24


@dataclass(frozen=True)
class Site:
    path: str
    line: int
    text: str
    reason: str = ""
    function: str = ""  # enclosing function of a producer site

    def __str__(self) -> str:
        suffix = f" ({self.reason})" if self.reason else ""
        return f"{self.path}:{self.line}: {self.text}{suffix}"


@dataclass
class FrontendCensus:
    # kind -> sites that can build it
    produced: dict[str, list[Site]] = field(default_factory=dict)
    # identifier-like string literal -> sites that spell it
    mentioned: dict[str, list[Site]] = field(default_factory=dict)
    unresolved_producers: list[Site] = field(default_factory=list)


class _Unresolved(Exception):
    pass


@dataclass
class _Module:
    rel: str
    tree: ast.Module


class _Index:
    """One parse of the frontend, with the lookups the resolver needs."""

    def __init__(self, root: Path) -> None:
        self.modules: list[_Module] = []
        self.functions: dict[str, list[ast.FunctionDef | ast.AsyncFunctionDef]] = {}
        self.class_attrs: dict[str, list[ast.expr]] = {}
        self.module_constants: dict[str, list[ast.expr]] = {}
        self.calls: dict[str, list[ast.Call]] = {}
        self.non_call_refs: set[str] = set()
        self.generated = _generated_tables()
        for path in sorted((root / FRONTEND).rglob("*.py")):
            rel = path.relative_to(root).as_posix()
            if rel.endswith("_generated.py"):
                continue
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=rel)
            for node in ast.walk(tree):
                for child in ast.iter_child_nodes(node):
                    child._census_parent = node  # type: ignore[attr-defined]
            constants = _single_assignments(tree.body)
            self.modules.append(_Module(rel, tree))
            for name, value in constants.items():
                self.module_constants.setdefault(name, []).append(value)
            for node in ast.walk(tree):
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    if not _is_declaration(node):
                        self.functions.setdefault(node.name, []).append(node)
                elif isinstance(node, ast.ClassDef):
                    for name, value in _single_assignments(node.body).items():
                        self.class_attrs.setdefault(name, []).append(value)
                elif isinstance(node, ast.Call):
                    callee = _callee_name(node.func)
                    if callee is not None:
                        self.calls.setdefault(callee, []).append(node)
                    bound = _partial_application(node)
                    if bound is not None:
                        name = _callee_name(bound.func)
                        assert name is not None
                        self.calls.setdefault(name, []).append(bound)
            for node in ast.walk(tree):
                name = _referenced_name(node)
                if name is None:
                    continue
                parent = getattr(node, "_census_parent", None)
                if isinstance(parent, ast.Call) and (
                    parent.func is node
                    or (
                        _partial_application(parent) is not None
                        and parent.args[0] is node
                    )
                ):
                    continue
                self.non_call_refs.add(name)


def _generated_tables() -> dict[str, frozenset[str]]:
    import importlib

    module = importlib.import_module(GENERATED_MODULE)
    tables: dict[str, frozenset[str]] = {}
    for name, value in vars(module).items():
        if name.endswith("_OP_KIND") and isinstance(value, dict):
            tables[name] = frozenset(v for v in value.values() if isinstance(v, str))
    return tables


def _is_declaration(function: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
    """A Protocol or abstract stub: a docstring and/or ``...`` and nothing else."""
    return all(
        isinstance(stmt, ast.Expr)
        and isinstance(stmt.value, ast.Constant)
        and (stmt.value.value is Ellipsis or isinstance(stmt.value.value, str))
        for stmt in function.body
    )


def _single_assignments(body: list[ast.stmt]) -> dict[str, ast.expr]:
    """Names assigned exactly once in a statement list, with their values."""
    seen: dict[str, ast.expr | None] = {}
    for stmt in body:
        targets: list[ast.expr] = []
        value: ast.expr | None = None
        if isinstance(stmt, ast.Assign):
            targets, value = stmt.targets, stmt.value
        elif isinstance(stmt, ast.AnnAssign) and stmt.value is not None:
            targets, value = [stmt.target], stmt.value
        for target in targets:
            if isinstance(target, ast.Name):
                seen[target.id] = None if target.id in seen else value
    return {name: value for name, value in seen.items() if value is not None}


def _partial_application(call: ast.Call) -> ast.Call | None:
    """``partial(f, *bound)`` as a call of ``f`` with only the bound arguments.

    The later invocation is opaque, so a parameter the partial leaves unbound
    does not resolve (see ``_Resolver._parameter``).
    """
    if _callee_name(call.func) != "partial" or not call.args:
        return None
    target = call.args[0]
    if _callee_name(target) is None or not isinstance(
        target, (ast.Name, ast.Attribute)
    ):
        return None
    bound = ast.Call(func=target, args=call.args[1:], keywords=call.keywords)
    ast.copy_location(bound, call)
    bound._census_partial = True  # type: ignore[attr-defined]
    return bound


def _callee_name(func: ast.expr) -> str | None:
    if isinstance(func, ast.Name):
        return func.id
    if isinstance(func, ast.Attribute):
        return func.attr
    return None


def _referenced_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
        return node.id
    if isinstance(node, ast.Attribute) and isinstance(node.ctx, ast.Load):
        return node.attr
    return None


def _is_molt_op_call(node: ast.AST) -> bool:
    return isinstance(node, ast.Call) and _callee_name(node.func) == "MoltOp"


def _kind_arguments(call: ast.Call) -> Iterator[ast.expr]:
    for keyword in call.keywords:
        if keyword.arg == "kind":
            yield keyword.value
    if call.args:
        yield call.args[0]


def _enclosing_functions(
    node: ast.AST,
) -> Iterator[ast.FunctionDef | ast.AsyncFunctionDef]:
    current = getattr(node, "_census_parent", None)
    while current is not None:
        if isinstance(current, (ast.FunctionDef, ast.AsyncFunctionDef)):
            yield current
        current = getattr(current, "_census_parent", None)


def _is_method(function: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
    parent = getattr(function, "_census_parent", None)
    params = function.args.posonlyargs + function.args.args
    return (
        isinstance(parent, ast.ClassDef)
        and bool(params)
        and params[0].arg in {"self", "cls"}
    )


class _Resolver:
    def __init__(self, index: _Index) -> None:
        self.index = index
        self.active: set[tuple[int, str]] = set()

    def strings(self, expr: ast.expr, depth: int = 0) -> frozenset[str]:
        """The literal strings ``expr`` can evaluate to, or raise _Unresolved."""
        if depth > _MAX_DEPTH:
            raise _Unresolved("resolution too deep")
        if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
            return frozenset({expr.value})
        if isinstance(expr, ast.IfExp):
            return self.strings(expr.body, depth + 1) | self.strings(
                expr.orelse, depth + 1
            )
        if isinstance(expr, ast.Attribute) and expr.attr == "kind":
            return frozenset()  # re-emit of an existing operation
        if isinstance(expr, ast.Name):
            return self._name(expr, depth + 1)
        if isinstance(expr, ast.Subscript):
            return self._table_values(expr.value, depth + 1)
        if isinstance(expr, ast.Call):
            func = expr.func
            if isinstance(func, ast.Attribute) and func.attr == "get":
                values = self._table_values(func.value, depth + 1)
                if len(expr.args) > 1:
                    values |= self.strings(expr.args[1], depth + 1)
                return values
            return self._returns(expr, depth + 1)
        raise _Unresolved(f"cannot prove a {type(expr).__name__}")

    def _table_values(self, expr: ast.expr, depth: int) -> frozenset[str]:
        if isinstance(expr, ast.IfExp):
            return self._table_values(expr.body, depth + 1) | self._table_values(
                expr.orelse, depth + 1
            )
        if isinstance(expr, ast.DictComp):
            return self.strings(expr.value, depth + 1)
        if isinstance(expr, ast.Dict):
            out: frozenset[str] = frozenset()
            for value in expr.values:
                out |= self.strings(value, depth + 1)
            return out
        name = _referenced_name(expr)
        if name is None:
            raise _Unresolved(f"cannot prove a {type(expr).__name__} table")
        if name in self.index.generated:
            return self.index.generated[name]
        if isinstance(expr, ast.Name):
            local = self._local_bindings(expr, name)
            if local:
                out = frozenset()
                for value in local:
                    out |= self._table_values(value, depth + 1)
                return out
            candidates = self.index.module_constants.get(name, [])
        else:
            candidates = self.index.class_attrs.get(name, [])
        if not candidates:
            raise _Unresolved(f"no table named {name}")
        out = frozenset()
        for value in candidates:
            out |= self._table_values(value, depth + 1)
        return out

    def _local_bindings(self, node: ast.Name, name: str) -> list[ast.expr]:
        for function in _enclosing_functions(node):
            values = [
                value
                for stmt in ast.walk(function)
                for value in _binding_values(stmt, name)
                if _owned_by(stmt, function)
            ]
            if values:
                return values
        return []

    def _name(self, node: ast.Name, depth: int) -> frozenset[str]:
        name = node.id
        for function in _enclosing_functions(node):
            bindings = [
                stmt
                for stmt in ast.walk(function)
                if _binds(stmt, name) and _owned_by(stmt, function)
            ]
            params = [
                arg.arg
                for arg in (
                    *function.args.posonlyargs,
                    *function.args.args,
                    *function.args.kwonlyargs,
                )
            ]
            if not bindings and name not in params:
                continue
            out: frozenset[str] = frozenset()
            for stmt in bindings:
                for value in _binding_values(stmt, name, strict=True):
                    out |= self.strings(value, depth + 1)
            if name in params:
                out |= self._parameter(function, name, depth + 1)
            return out
        if name in self.index.module_constants:
            out = frozenset()
            for value in self.index.module_constants[name]:
                out |= self.strings(value, depth + 1)
            return out
        raise _Unresolved(f"unbound name {name}")

    def _parameter(
        self,
        function: ast.FunctionDef | ast.AsyncFunctionDef,
        name: str,
        depth: int,
    ) -> frozenset[str]:
        key = (id(function), name)
        if key in self.active:
            return frozenset()  # recursion adds no new literals
        if function.name in self.index.non_call_refs:
            raise _Unresolved(f"{function.name} is referenced as a value")
        calls = self.index.calls.get(function.name, [])
        if not calls:
            raise _Unresolved(f"{function.name} has no direct caller")
        positional = [
            arg.arg for arg in (*function.args.posonlyargs, *function.args.args)
        ]
        method = _is_method(function)
        self.active.add(key)
        try:
            out: frozenset[str] = frozenset()
            default = _default_for(function, name)
            if default is not None:
                out |= self.strings(default, depth + 1)
            for call in calls:
                if any(isinstance(arg, ast.Starred) for arg in call.args) or any(
                    keyword.arg is None for keyword in call.keywords
                ):
                    raise _Unresolved(f"{function.name} called with unpacking")
                argument = next(
                    (kw.value for kw in call.keywords if kw.arg == name), None
                )
                if argument is None and name in positional:
                    index = positional.index(name)
                    if method and isinstance(call.func, ast.Attribute):
                        index -= 1
                    if 0 <= index < len(call.args):
                        argument = call.args[index]
                if argument is not None:
                    out |= self.strings(argument, depth + 1)
                elif getattr(call, "_census_partial", False):
                    raise _Unresolved(
                        f"a partial of {function.name} leaves {name} unbound"
                    )
            return out
        finally:
            self.active.discard(key)

    def _returns(self, call: ast.Call, depth: int) -> frozenset[str]:
        name = _callee_name(call.func)
        functions = self.index.functions.get(name or "", [])
        if not functions:
            raise _Unresolved(ast.unparse(call))
        out: frozenset[str] = frozenset()
        for function in functions:
            returns = [
                node.value
                for node in ast.walk(function)
                if isinstance(node, ast.Return) and _owned_by(node, function)
            ]
            if not returns or any(value is None for value in returns):
                raise _Unresolved(f"{name} does not return a kind")
            for value in returns:
                assert value is not None
                out |= self.strings(value, depth + 1)
        return out


def _default_for(
    function: ast.FunctionDef | ast.AsyncFunctionDef, name: str
) -> ast.expr | None:
    positional = [*function.args.posonlyargs, *function.args.args]
    defaults = function.args.defaults
    for arg, default in zip(positional[len(positional) - len(defaults) :], defaults):
        if arg.arg == name:
            return default
    for arg, default in zip(function.args.kwonlyargs, function.args.kw_defaults):
        if arg.arg == name:
            return default
    return None


def _owned_by(node: ast.AST, function: ast.AST) -> bool:
    """Whether ``node`` belongs to ``function`` itself, not a nested function."""
    return next(_enclosing_functions(node), None) is function


def _binds(stmt: ast.AST, name: str) -> bool:
    if isinstance(stmt, (ast.Assign, ast.AnnAssign, ast.AugAssign, ast.NamedExpr)):
        targets = stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]
        return any(_target_binds(target, name) for target in targets)
    if isinstance(stmt, (ast.For, ast.AsyncFor, ast.comprehension)):
        return _target_binds(stmt.target, name)
    if isinstance(stmt, ast.withitem):
        return stmt.optional_vars is not None and _target_binds(
            stmt.optional_vars, name
        )
    return False


def _target_binds(target: ast.expr, name: str) -> bool:
    if isinstance(target, ast.Name):
        return target.id == name
    if isinstance(target, (ast.Tuple, ast.List)):
        return any(_target_binds(elt, name) for elt in target.elts)
    return False


def _binding_values(
    stmt: ast.AST, name: str, *, strict: bool = False
) -> list[ast.expr]:
    """The values a binding statement gives ``name``; strict mode rejects shapes
    whose value is not a single expression."""
    if not _binds(stmt, name):
        return []
    if isinstance(stmt, (ast.Assign, ast.AnnAssign, ast.NamedExpr)):
        targets = stmt.targets if isinstance(stmt, ast.Assign) else [stmt.target]
        if all(isinstance(target, ast.Name) for target in targets) and stmt.value:
            return [stmt.value]
    if isinstance(stmt, (ast.For, ast.AsyncFor)) and isinstance(stmt.target, ast.Name):
        if isinstance(stmt.iter, (ast.Tuple, ast.List, ast.Set)):
            return list(stmt.iter.elts)
    if strict:
        raise _Unresolved(f"{name} is bound by {type(stmt).__name__}")
    return []


def _mention_files(root: Path) -> Iterator[Path]:
    excluded = [root / path for path in MENTION_EXCLUDED]
    for base in MENTION_ROOTS:
        for path in sorted((root / base).rglob("*.py")):
            if path.name.endswith("_generated.py") or any(
                path.is_relative_to(prefix) for prefix in excluded
            ):
                continue
            yield path


def frontend_census(root: Path = ROOT) -> FrontendCensus:
    index = _Index(root)
    resolver = _Resolver(index)
    census = FrontendCensus()
    for module in index.modules:
        for node in ast.walk(module.tree):
            if not _is_molt_op_call(node):
                continue
            assert isinstance(node, ast.Call)
            enclosing = next(_enclosing_functions(node), None)
            function = enclosing.name if enclosing is not None else ""
            for argument in _kind_arguments(node):
                site = Site(
                    module.rel, node.lineno, ast.unparse(argument), "", function
                )
                try:
                    kinds = resolver.strings(argument)
                except _Unresolved as error:
                    census.unresolved_producers.append(
                        Site(site.path, site.line, site.text, str(error), function)
                    )
                    continue
                for kind in kinds:
                    census.produced.setdefault(kind, []).append(site)
    for path in _mention_files(root):
        rel = path.relative_to(root).as_posix()
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=rel)
        for node in ast.walk(tree):
            if (
                isinstance(node, ast.Constant)
                and isinstance(node.value, str)
                and node.value.isidentifier()
            ):
                census.mentioned.setdefault(node.value, []).append(
                    Site(rel, node.lineno, repr(node.value))
                )
    return census

"""Top-level function-metadata analysis (doc 44 §F2b: "function metadata —
param counts / defaults shapes / generator-vs-async classification").

Free functions over ``ast`` nodes — the ``cfg_analysis.py`` house shape.  Lifts
``SimpleTIRGenerator._collect_module_func_kinds`` /
``_collect_module_class_names`` / ``_collect_module_func_defaults`` and their
pure dependencies (``_function_contains_yield``, ``_function_param_names``,
``_split_function_args``, ``_default_specs_from_args``,
``_default_spec_for_expr``) verbatim.  These are pure functions of the AST today;
``self`` was used only to call other pure helpers.

The defaults table here is the AST-derived value.  The walk prefers an
externally-supplied ``known_func_defaults`` override when present; that override
is applied by the populate-shim (it is a runtime input, not an AST fact), so it
deliberately does **not** live in this module.
"""

from __future__ import annotations

import ast
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from enum import StrEnum
from typing import Any


class FunctionKind(StrEnum):
    SYNC = "sync"
    ASYNC = "async"
    GENERATOR = "gen"
    ASYNC_GENERATOR = "asyncgen"


FUNCTION_KIND_VALUES = frozenset(kind.value for kind in FunctionKind)
STATEFUL_FUNCTION_KINDS = frozenset(
    {FunctionKind.ASYNC, FunctionKind.GENERATOR, FunctionKind.ASYNC_GENERATOR}
)


def normalize_function_kind(kind: object) -> FunctionKind | None:
    if isinstance(kind, FunctionKind):
        return kind
    if isinstance(kind, str) and kind in FUNCTION_KIND_VALUES:
        return FunctionKind(kind)
    return None


def _require_stateful_function_kind(kind: FunctionKind) -> FunctionKind:
    if kind not in STATEFUL_FUNCTION_KINDS:
        raise ValueError(f"{kind!r} is not a stateful function kind")
    return kind


def stateful_function_task_kind(kind: FunctionKind) -> str:
    kind = _require_stateful_function_kind(kind)
    if kind == FunctionKind.ASYNC:
        return "coroutine"
    return "generator"


def stateful_function_result_type_hint(kind: FunctionKind) -> str:
    kind = _require_stateful_function_kind(kind)
    if kind == FunctionKind.ASYNC:
        return "Future"
    if kind == FunctionKind.ASYNC_GENERATOR:
        return "async_generator"
    return "generator"


def stateful_function_tag(kind: FunctionKind, *, has_closure: bool) -> str:
    kind = _require_stateful_function_kind(kind)
    if kind == FunctionKind.ASYNC:
        return "AsyncClosureFunc" if has_closure else "AsyncFunc"
    if kind == FunctionKind.ASYNC_GENERATOR:
        return "AsyncGenClosureFunc" if has_closure else "AsyncGenFunc"
    return "GenClosureFunc" if has_closure else "GenFunc"


STATEFUL_FUNCTION_TAGS: dict[str, tuple[FunctionKind, bool]] = {
    stateful_function_tag(kind, has_closure=has_closure): (kind, has_closure)
    for kind in (
        FunctionKind.ASYNC,
        FunctionKind.GENERATOR,
        FunctionKind.ASYNC_GENERATOR,
    )
    for has_closure in (False, True)
}


@dataclass(frozen=True)
class StatefulFunctionTypeHint:
    kind: FunctionKind
    has_closure: bool
    poll_symbol: str
    closure_size: int

    def __post_init__(self) -> None:
        _require_stateful_function_kind(self.kind)
        if self.closure_size < 0:
            raise ValueError("stateful function closure_size must be non-negative")

    @property
    def result_type_hint(self) -> str:
        return stateful_function_result_type_hint(self.kind)

    @property
    def task_kind(self) -> str:
        return stateful_function_task_kind(self.kind)

    @property
    def function_tag(self) -> str:
        return stateful_function_tag(self.kind, has_closure=self.has_closure)

    def frame_plan(
        self,
        *,
        param_count: int,
        gen_control_size: int,
    ) -> StatefulFunctionFramePlan:
        return stateful_function_frame_plan(
            kind=self.kind,
            poll_symbol=self.poll_symbol,
            param_count=param_count,
            has_closure=self.has_closure,
            gen_control_size=gen_control_size,
        )


def parse_stateful_function_type_hint(
    type_hint: object,
) -> StatefulFunctionTypeHint | None:
    if not isinstance(type_hint, str):
        return None
    parts = type_hint.split(":")
    if len(parts) != 3:
        return None
    tag, poll_symbol, raw_closure_size = parts
    tag_entry = STATEFUL_FUNCTION_TAGS.get(tag)
    if tag_entry is None or not poll_symbol:
        return None
    try:
        closure_size = int(raw_closure_size)
    except ValueError:
        return None
    kind, has_closure = tag_entry
    return StatefulFunctionTypeHint(
        kind=kind,
        has_closure=has_closure,
        poll_symbol=poll_symbol,
        closure_size=closure_size,
    )


@dataclass(frozen=True)
class StatefulFunctionFramePlan:
    kind: FunctionKind
    poll_symbol: str
    param_count: int
    has_closure: bool
    gen_control_size: int

    def __post_init__(self) -> None:
        _require_stateful_function_kind(self.kind)
        if self.param_count < 0:
            raise ValueError("stateful function param_count must be non-negative")

    @property
    def include_gen_control(self) -> bool:
        return self.kind != FunctionKind.ASYNC

    @property
    def payload_slots(self) -> int:
        return self.param_count + (1 if self.has_closure else 0)

    @property
    def task_kind(self) -> str:
        return stateful_function_task_kind(self.kind)

    def callable_task_metadata(self, closure_size: int) -> dict[str, str | int]:
        """Publish callable trampoline kind and the finalized frame byte size."""
        callable_kind = {
            FunctionKind.GENERATOR: "generator",
            FunctionKind.ASYNC: "coroutine",
            FunctionKind.ASYNC_GENERATOR: "async_generator",
        }[self.kind]
        return {"task_kind": callable_kind, "task_closure_size": closure_size}

    @property
    def result_type_hint(self) -> str:
        return stateful_function_result_type_hint(self.kind)

    @property
    def function_tag(self) -> str:
        return stateful_function_tag(self.kind, has_closure=self.has_closure)

    @property
    def async_closure_offset(self) -> int | None:
        if not self.has_closure:
            return None
        if self.include_gen_control:
            return self.gen_control_size
        return 0

    @property
    def async_locals_base(self) -> int:
        if self.has_closure:
            return (self.gen_control_size if self.include_gen_control else 0) + 8
        if self.include_gen_control:
            return self.gen_control_size
        return 0

    def function_type_hint(self, closure_size: int) -> str:
        return f"{self.function_tag}:{self.poll_symbol}:{closure_size}"

    def public_locals_layout(
        self,
        *,
        public_slots: Iterable[tuple[str, int]],
        parameter_names: Sequence[str],
        cell_names: Sequence[str],
        free_vars: Sequence[str],
    ) -> StatefulLocalsLayout:
        """Project the finished typed frame onto its Python-visible bindings.

        Parameters are the constructor-bound payload prefix that callable
        trampolines store before the first poll; every other public slot is a
        body local. ``cell_names`` are public slots whose compiled prologue
        publishes a closure cell, and ``free_vars`` is co_freevars order, which
        is also the closure-tuple order.
        """
        if len(parameter_names) != self.param_count:
            raise ValueError("stateful locals parameters must match the frame plan")
        cell_ordinals = {name: index for index, name in enumerate(cell_names)}
        ordered = sorted(public_slots, key=lambda entry: entry[1])
        if len(ordered) < self.param_count:
            raise ValueError("stateful parameters must own public frame slots")
        slots: list[StatefulLocalSlot] = []
        for index, (name, offset) in enumerate(ordered):
            parameter = index < self.param_count
            if parameter and (
                name != parameter_names[index]
                or offset != self.async_locals_base + index * 8
            ):
                raise ValueError(
                    "stateful parameter slots must match the constructor payload"
                )
            slots.append(
                StatefulLocalSlot(
                    name=name,
                    offset=offset,
                    parameter=parameter,
                    cell=cell_ordinals.get(name, -1),
                )
            )
        if free_vars and not self.has_closure:
            raise ValueError("stateful free variables require a closure slot")
        return StatefulLocalsLayout(
            slots=tuple(slots),
            free_vars=tuple(free_vars),
            closure_offset=self.async_closure_offset if free_vars else None,
        )


def stateful_function_frame_plan(
    *,
    kind: FunctionKind,
    poll_symbol: str,
    param_count: int,
    has_closure: bool,
    gen_control_size: int,
) -> StatefulFunctionFramePlan:
    return StatefulFunctionFramePlan(
        kind=kind,
        poll_symbol=poll_symbol,
        param_count=param_count,
        has_closure=has_closure,
        gen_control_size=gen_control_size,
    )


@dataclass(frozen=True)
class StatefulLocalSlot:
    """One Python-visible binding stored in a typed stateful frame slot."""

    name: str
    offset: int
    parameter: bool
    cell: int

    def __post_init__(self) -> None:
        if not self.name:
            raise ValueError("stateful local slots require a name")
        if self.offset < 0 or self.offset % 8 != 0:
            raise ValueError(
                "stateful local slot offset must be nonnegative and aligned"
            )


@dataclass(frozen=True)
class StatefulLocalsLayout:
    """The single public-locals authority of one stateful activation.

    The runtime ABI (``stateful_locals_register``) carries two tuples:
    ``wire_names()`` lists slot bindings in slot order followed by co_freevars
    in closure-tuple order, and ``wire_layout()`` is
    ``(parameter_count, slot_offsets, slot_cells, closure_offset)``. The
    runtime validates exactly this schema; no other table describes it.
    """

    slots: tuple[StatefulLocalSlot, ...]
    free_vars: tuple[str, ...]
    closure_offset: int | None

    def __post_init__(self) -> None:
        names = [slot.name for slot in self.slots] + list(self.free_vars)
        if len(set(names)) != len(names):
            raise ValueError("stateful locals names must be unique")
        offsets = [slot.offset for slot in self.slots]
        if offsets != sorted(set(offsets)):
            raise ValueError("stateful local slots must have increasing offsets")
        cells = sorted(slot.cell for slot in self.slots if slot.cell >= 0)
        if cells != list(range(len(cells))):
            raise ValueError("stateful cell ordinals must be unique and contiguous")
        body_seen = False
        for slot in self.slots:
            if slot.parameter and body_seen:
                raise ValueError("stateful parameters must be the slot prefix")
            body_seen = body_seen or not slot.parameter
        if (self.closure_offset is None) != (not self.free_vars):
            raise ValueError("stateful free variables and closure slot must agree")
        if self.closure_offset is not None and (
            self.closure_offset < 0
            or self.closure_offset % 8 != 0
            or self.closure_offset in offsets
        ):
            raise ValueError("stateful closure slot must be aligned and distinct")

    @property
    def parameter_count(self) -> int:
        return sum(1 for slot in self.slots if slot.parameter)

    def wire_names(self) -> tuple[str, ...]:
        return tuple(slot.name for slot in self.slots) + self.free_vars

    def wire_layout(
        self,
    ) -> tuple[int, tuple[int, ...], tuple[int, ...], int | None]:
        return (
            self.parameter_count,
            tuple(slot.offset for slot in self.slots),
            tuple(slot.cell for slot in self.slots),
            self.closure_offset,
        )


def _push_arg_annotations(stack: list[ast.AST], args: ast.arguments) -> None:
    for arg in (
        args.posonlyargs
        + args.args
        + args.kwonlyargs
        + ([] if args.vararg is None else [args.vararg])
        + ([] if args.kwarg is None else [args.kwarg])
    ):
        if arg.annotation is not None:
            stack.append(arg.annotation)


def expression_contains_yield(node: ast.AST) -> bool:
    class YieldVisitor(ast.NodeVisitor):
        def __init__(self) -> None:
            self.found = False

        def visit_Yield(self, node: ast.Yield) -> None:
            self.found = True

        def visit_YieldFrom(self, node: ast.YieldFrom) -> None:
            self.found = True

        def visit_Lambda(self, node: ast.Lambda) -> None:
            return

        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            return

        def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
            return

        def visit_ClassDef(self, node: ast.ClassDef) -> None:
            return

    visitor = YieldVisitor()
    visitor.visit(node)
    return visitor.found


def function_contains_yield(
    node: ast.FunctionDef | ast.AsyncFunctionDef,
) -> bool:
    stack: list[ast.AST] = list(node.body)
    while stack:
        current = stack.pop()
        if isinstance(current, (ast.Yield, ast.YieldFrom)):
            return True
        if isinstance(current, (ast.FunctionDef, ast.AsyncFunctionDef)):
            stack.extend(current.decorator_list)
            stack.extend(current.args.defaults)
            stack.extend(
                default for default in current.args.kw_defaults if default is not None
            )
            _push_arg_annotations(stack, current.args)
            if current.returns is not None:
                stack.append(current.returns)
            continue
        if isinstance(current, ast.ClassDef):
            stack.extend(current.decorator_list)
            stack.extend(current.bases)
            stack.extend(keyword.value for keyword in current.keywords)
            continue
        if isinstance(current, ast.Lambda):
            continue
        stack.extend(ast.iter_child_nodes(current))
    return False


def async_generator_contains_yield_from(node: ast.AsyncFunctionDef) -> bool:
    stack: list[ast.AST] = list(node.body)
    while stack:
        current = stack.pop()
        if isinstance(current, ast.YieldFrom):
            return True
        if isinstance(
            current,
            (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
        ):
            continue
        stack.extend(ast.iter_child_nodes(current))
    return False


def async_generator_contains_return_value(node: ast.AsyncFunctionDef) -> bool:
    stack: list[ast.AST] = list(node.body)
    while stack:
        current = stack.pop()
        if isinstance(current, ast.Return) and current.value is not None:
            return True
        if isinstance(
            current,
            (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
        ):
            continue
        stack.extend(ast.iter_child_nodes(current))
    return False


def signature_contains_yield(
    *,
    decorators: list[ast.expr],
    args: ast.arguments,
    returns: ast.expr | None,
) -> bool:
    exprs: list[ast.expr] = list(decorators)
    exprs.extend(args.defaults)
    exprs.extend(expr for expr in args.kw_defaults if expr is not None)
    for arg in (
        args.posonlyargs
        + args.args
        + args.kwonlyargs
        + ([] if args.vararg is None else [args.vararg])
        + ([] if args.kwarg is None else [args.kwarg])
    ):
        if arg.annotation is not None:
            exprs.append(arg.annotation)
    if returns is not None:
        exprs.append(returns)
    return any(expression_contains_yield(expr) for expr in exprs)


def _split_function_args(
    args: ast.arguments,
) -> tuple[list[ast.arg], list[ast.arg], list[ast.arg], str | None, str | None]:
    posonly = list(args.posonlyargs)
    pos_or_kw = list(args.args)
    kwonly = list(args.kwonlyargs)
    vararg = args.vararg.arg if args.vararg else None
    varkw = args.kwarg.arg if args.kwarg else None
    return posonly, pos_or_kw, kwonly, vararg, varkw


def _function_param_names(args: ast.arguments) -> list[str]:
    posonly, pos_or_kw, kwonly, vararg, varkw = _split_function_args(args)
    names = [arg.arg for arg in posonly + pos_or_kw]
    if vararg is not None:
        names.append(vararg)
    names.extend(arg.arg for arg in kwonly)
    if varkw is not None:
        names.append(varkw)
    return names


def _default_spec_for_expr(expr: ast.expr) -> dict[str, Any]:
    if isinstance(expr, ast.Constant):
        return {"const": True, "value": expr.value}
    return {"const": False}


def _default_specs_from_args(args: ast.arguments) -> list[dict[str, Any]]:
    default_specs = [_default_spec_for_expr(expr) for expr in args.defaults]
    if not args.kwonlyargs or not args.kw_defaults:
        return default_specs
    kwonly_names = [arg.arg for arg in args.kwonlyargs]
    kwonly_pairs = list(zip(kwonly_names, args.kw_defaults))
    suffix: list[tuple[str, ast.expr]] = []
    for name, expr in reversed(kwonly_pairs):
        if expr is None:
            break
        suffix.append((name, expr))
    for name, expr in reversed(suffix):
        spec = _default_spec_for_expr(expr)
        spec["kwonly"] = True
        spec["name"] = name
        default_specs.append(spec)
    return default_specs


def collect_module_func_kinds(node: ast.Module) -> dict[str, FunctionKind]:
    kinds: dict[str, FunctionKind] = {}
    for stmt in node.body:
        if isinstance(stmt, ast.AsyncFunctionDef):
            kinds[stmt.name] = (
                FunctionKind.ASYNC_GENERATOR
                if function_contains_yield(stmt)
                else FunctionKind.ASYNC
            )
        elif isinstance(stmt, ast.FunctionDef):
            if function_contains_yield(stmt):
                kinds[stmt.name] = FunctionKind.GENERATOR
            else:
                kinds[stmt.name] = FunctionKind.SYNC
    return kinds


def collect_module_class_names(node: ast.Module) -> set[str]:
    return {stmt.name for stmt in node.body if isinstance(stmt, ast.ClassDef)}


def collect_module_func_defaults(node: ast.Module) -> dict[str, dict[str, Any]]:
    defaults: dict[str, dict[str, Any]] = {}
    for stmt in node.body:
        if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if isinstance(stmt, ast.AsyncFunctionDef):
            kind = (
                FunctionKind.ASYNC_GENERATOR
                if function_contains_yield(stmt)
                else FunctionKind.ASYNC
            )
        else:
            kind = (
                FunctionKind.GENERATOR
                if function_contains_yield(stmt)
                else FunctionKind.SYNC
            )
        has_decorators = bool(stmt.decorator_list)
        if stmt.args.vararg or stmt.args.kwarg:
            defaults[stmt.name] = {
                "has_vararg": True,
                "kind": kind,
                "has_decorators": has_decorators,
            }
            continue
        params = _function_param_names(stmt.args)
        default_specs = _default_specs_from_args(stmt.args)
        defaults[stmt.name] = {
            "params": len(params),
            "defaults": default_specs,
            "posonly": len(stmt.args.posonlyargs),
            "kwonly": len(stmt.args.kwonlyargs),
            "kind": kind,
            "has_decorators": has_decorators,
        }
    return defaults

"""Replay emitted argument assembly against CPython's observable schedule.

This is frontend proof only; the differential capsule exercises target runtimes.
"""

from __future__ import annotations

import ast
from typing import Any

import pytest

from molt.frontend import MoltOp, MoltValue
from molt.frontend._types import AsyncFrameSlot, AsyncFrameSlotRole
from molt.frontend.lowering.local_bindings import LocalBindingMixin
from molt.frontend.visitors.call_runtime_helpers import CallRuntimeHelperMixin


class _ArgumentEmitter(CallRuntimeHelperMixin):
    def __init__(self) -> None:
        self.ops: list[MoltOp] = []
        self.serial = 0

    def next_var(self) -> str:
        self.serial += 1
        return f"v{self.serial}"

    def emit(self, op: MoltOp) -> None:
        self.ops.append(op)

    def is_async(self) -> bool:
        return False

    def _emit_builtin_type_value(self, name: str) -> MoltValue:
        value = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="BUILTIN_TYPE", args=[name], result=value))
        return value

    def visit(self, node: ast.expr) -> MoltValue:
        value = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="EVALUATE", args=[ast.unparse(node)], result=value))
        return value


def _observe(source: str, *, emitted: bool) -> tuple[list[str], str]:
    events: list[str] = []

    class Star:
        def __init__(self, label: str, fail: bool):
            self.label, self.fail = label, fail

        def __iter__(self):
            events.append(f"iter:{self.label}")
            if self.fail:
                raise ValueError(self.label)
            return iter((1, 2))

    class Mapping:
        def __init__(self, label: str, key: str):
            self.label, self.key = label, key

        def keys(self):
            events.append(f"keys:{self.label}")
            return [self.key]

        def __getitem__(self, key):
            events.append(f"get:{self.label}:{key}")
            return 3

    def star(label, fail=False):
        events.append(f"eval:{label}")
        return Star(label, fail)

    def mapping(label, key="x"):
        events.append(f"eval:{label}")
        return Mapping(label, key)

    def value(label):
        events.append(f"eval:{label}")
        return 4

    def f(*args, **kwargs):
        events.append(f"call:{args}:{kwargs}")

    namespace = dict(star=star, mapping=mapping, value=value, f=f)
    try:
        if not emitted:
            eval(source, namespace)
        else:
            call = ast.parse(source, mode="eval").body
            assert isinstance(call, ast.Call)
            emitter = _ArgumentEmitter()
            result = emitter._emit_call_args_builder(call)
            values: dict[str, Any] = {}
            for op in emitter.ops:
                args = [
                    values[arg.name] if isinstance(arg, MoltValue) else arg
                    for arg in op.args
                ]
                if op.kind == "EVALUATE":
                    values[op.result.name] = eval(args[0], namespace)
                elif op.kind == "CALLARGS_NEW":
                    values[op.result.name] = ([], {})
                elif op.kind == "CONST_STR":
                    values[op.result.name] = args[0]
                elif op.kind == "BUILTIN_TYPE":
                    assert args == ["tuple"]
                    values[op.result.name] = tuple
                elif op.kind == "CALL_FUNC":
                    values[op.result.name] = args[0](*args[1:])
                elif op.kind == "CALLARGS_PUSH_POS":
                    args[0][0].append(args[1])
                elif op.kind == "CALLARGS_EXPAND_STAR":
                    args[0][0].extend(args[1])
                elif op.kind == "CALLARGS_PUSH_KW":
                    if args[1] in args[0][1]:
                        raise TypeError("duplicate")
                    args[0][1][args[1]] = args[2]
                elif op.kind == "CALLARGS_EXPAND_KWSTAR":
                    for key in args[1].keys():
                        if key in args[0][1]:
                            raise TypeError("duplicate")
                        args[0][1][key] = args[1][key]
                else:
                    pytest.fail(f"unmodeled argument instruction: {op.kind}")
            args, kwargs = values[result.name]
            f(*args, **kwargs)
    except (TypeError, ValueError) as error:
        return events, type(error).__name__
    return events, "ok"


@pytest.mark.parametrize(
    "source",
    [
        "f(*star('s'), k=value('k'))",
        "f(*star('s', True), **mapping('m'))",
        "f(0, *star('s'), k=value('k'))",
        "f(0, *star('s', True), k=value('k'))",
        "f(*star('s'), *star('t'), k=value('k'))",
        "f(*star('s'), value('p'), k=value('k'))",
        "f(**mapping('m'), x=value('x'), y=value('y'), **mapping('n'))",
        "f(x=value('x'), **mapping('m'), y=value('y'))",
        "f(**mapping('m'), **mapping('n'), y=value('y'))",
        "f(k=value('k'), *star('s'))",
    ],
)
def test_emitted_argument_schedule_matches_cpython(source: str) -> None:
    assert _observe(source, emitted=True) == _observe(source, emitted=False)


class _SuspendingArgumentEmitter(_ArgumentEmitter, LocalBindingMixin):
    def is_async(self) -> bool:
        return True

    def _expr_may_yield(self, node: ast.expr) -> bool:
        return isinstance(node, ast.Await)

    def _allocate_async_frame_slot(self, role: AsyncFrameSlotRole) -> AsyncFrameSlot:
        self.serial += 1
        return AsyncFrameSlot(self.serial * 8, role)

    def visit(self, node: ast.expr) -> MoltValue:
        if isinstance(node, ast.Await):
            self.emit(MoltOp(kind="SUSPEND", args=[], result=MoltValue("none")))
            node = node.value
        return super().visit(node)


@pytest.mark.parametrize(
    "source",
    ["f(*a, x=await b, y=await c)", "f(0, *a, x=await b, y=await c)"],
)
def test_argument_storage_survives_suspension_and_releases_frame_slots(
    source: str,
) -> None:
    call = ast.parse(source, mode="eval").body
    assert isinstance(call, ast.Call)
    emitter = _SuspendingArgumentEmitter()
    result = emitter._emit_call_args_builder(call)
    values: dict[str, Any] = {}
    frame: dict[int, Any] = {}
    namespace = {"a": (1, 2), "b": 3, "c": 4}
    for op in emitter.ops:
        args = [
            values[arg.name] if isinstance(arg, MoltValue) else arg for arg in op.args
        ]
        if op.kind == "SUSPEND":
            values.clear()  # No SSA temporary survives the frame boundary.
        elif op.kind == "EVALUATE":
            values[op.result.name] = eval(args[0], namespace)
        elif op.kind == "CONST_NONE":
            values[op.result.name] = None
        elif op.kind == "CONST_STR":
            values[op.result.name] = args[0]
        elif op.kind == "BUILTIN_TYPE":
            assert args == ["tuple"]
            values[op.result.name] = tuple
        elif op.kind == "CALL_FUNC":
            values[op.result.name] = args[0](*args[1:])
        elif op.kind == "STORE_CLOSURE":
            frame[args[1]] = args[2]
        elif op.kind == "LOAD_CLOSURE":
            values[op.result.name] = frame[args[1]]
        elif op.kind == "CALLARGS_NEW":
            values[op.result.name] = ([], {})
        elif op.kind == "CALLARGS_PUSH_POS":
            args[0][0].append(args[1])
        elif op.kind == "CALLARGS_EXPAND_STAR":
            args[0][0].extend(args[1])
        elif op.kind == "CALLARGS_PUSH_KW":
            args[0][1][args[1]] = args[2]
        else:
            pytest.fail(f"unmodeled suspension operation: {op.kind}")
    expected = [0, 1, 2] if "f(0" in source else [1, 2]
    assert values[result.name] == (expected, {"x": 3, "y": 4})
    assert frame and all(value is None for value in frame.values())

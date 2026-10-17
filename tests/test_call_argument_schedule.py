"""Replay emitted argument assembly against CPython's observable schedule.

This is frontend proof only; the differential capsule exercises target runtimes.
"""

from __future__ import annotations

import ast
import dis
import sys
from typing import Any

import pytest

from molt.compiler_analysis.python_call_arguments import (
    call_form,
    collect_module_import_names,
)
from molt.frontend import MoltOp, MoltValue
from molt.frontend._types import AsyncFrameSlot, AsyncFrameSlotRole
from molt.frontend.lowering.emission_core import EmissionCoreMixin
from molt.frontend.lowering.exception_lowering import ExceptionLoweringMixin
from molt.frontend.lowering.local_bindings import LocalBindingMixin
from molt.frontend.sema import analyze_module
from molt.frontend.visitors.call_runtime_helpers import CallRuntimeHelperMixin


class _ArgumentEmitter(CallRuntimeHelperMixin):
    def __init__(self) -> None:
        self.ops: list[MoltOp] = []
        self.serial = 0
        # Call lowering reads module facts from sema; these sources import nothing.
        self._sema = analyze_module(ast.Module(body=[], type_ignores=[]))

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


class _SuspendingArgumentEmitter(
    _ArgumentEmitter, LocalBindingMixin, EmissionCoreMixin, ExceptionLoweringMixin
):
    def __init__(self) -> None:
        super().__init__()
        self.try_suppress_depth = None
        self.try_end_labels: list[int] = [-1]
        self.try_scopes = []

    def next_label(self) -> int:
        self.serial += 1
        return self.serial

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
    labels = {
        op.args[0]: index for index, op in enumerate(emitter.ops) if op.kind == "LABEL"
    }
    position = 0
    while position < len(emitter.ops):
        op = emitter.ops[position]
        position += 1
        if op.kind == "JUMP":
            position = labels[op.args[0]]
            continue
        if op.kind in {"TRY_START", "TRY_END", "LABEL", "CHECK_EXCEPTION"}:
            continue
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


def test_suspension_before_the_first_push_parks_no_builder() -> None:
    call = ast.parse("f(*a, x=await b)", mode="eval").body
    assert isinstance(call, ast.Call)
    emitter = _SuspendingArgumentEmitter()
    builder = emitter._emit_call_args_builder(call)
    kinds = [op.kind for op in emitter.ops]
    assert kinds.index("SUSPEND") < kinds.index("CALLARGS_NEW")
    parked = [op.args[2] for op in emitter.ops if op.kind == "STORE_CLOSURE"]
    assert parked, "the pending *a operand is parked across the suspension"
    assert all(value is not builder for value in parked)


def test_builder_is_created_at_its_first_push() -> None:
    """No operand is evaluated between the builder's creation and first push.

    The builder models CPython's value stack for the call. Created before
    evaluation, it would be older than a pending operand that CPython keeps
    beneath the kwargs mapping (a deferred sole ``*x``).
    """
    for source in (
        "f(0, *star('s'), k=value('k'))",
        "f(*star('s'), k=value('k'))",
        "f(**mapping('m'), x=value('x'))",
        "f(*star('s'))",
    ):
        call = ast.parse(source, mode="eval").body
        assert isinstance(call, ast.Call)
        emitter = _ArgumentEmitter()
        builder = emitter._emit_call_args_builder(call)
        kinds = [op.kind for op in emitter.ops]
        assert kinds.count("CALLARGS_NEW") == 1, source
        created = kinds.index("CALLARGS_NEW")
        assert emitter.ops[created].result is builder
        first_push = next(
            index
            for index, kind in enumerate(kinds)
            if kind.startswith("CALLARGS_") and kind != "CALLARGS_NEW"
        )
        assert created < first_push, source
        assert "EVALUATE" not in kinds[created:first_push], source
    empty = ast.parse("f()", mode="eval").body
    assert isinstance(empty, ast.Call)
    emitter = _ArgumentEmitter()
    builder = emitter._emit_call_args_builder(empty)
    assert [op.kind for op in emitter.ops] == ["CALLARGS_NEW"]
    assert emitter.ops[0].result is builder


def _cpython_call_forms(source: str) -> list[str]:
    """The call instructions the host CPython compiler emits for `source`."""
    forms = []
    for instruction in dis.get_instructions(compile(source, "<call-form>", "exec")):
        if instruction.opname == "CALL_FUNCTION_EX":
            forms.append("expanded")
        elif instruction.opname in {"CALL", "CALL_KW"}:
            forms.append("stack")
    return forms


def _arguments(template: str, count: int) -> str:
    return ", ".join(template.format(index) for index in range(count))


@pytest.mark.parametrize(
    "source",
    [
        "f(a, b)",
        "f(*a)",
        "f(a, *b, k=c)",
        "f(**k)",
        f"f({_arguments('a{}', 30)})",
        f"f({_arguments('a{}', 31)})",
        f"f({_arguments('k{}=v', 15)})",
        f"f({_arguments('k{}=v', 16)})",
        f"obj.m({_arguments('k{}=v', 16)})",
        f"import mod\nmod.m({_arguments('k{}=v', 16)})",
        f"obj.m({_arguments('a{}', 30)})",
        f"obj.m({_arguments('a{}', 31)})",
        f"if flag:\n    import mod\nmod.m({_arguments('k{}=v', 16)})",
        f"def g():\n    import mod\nmod.m({_arguments('k{}=v', 16)})",
    ],
)
def test_call_form_matches_the_cpython_compiler(source: str) -> None:
    """The host CPython compiler is the oracle for CALL vs CALL_FUNCTION_EX,
    including its stack-use threshold and the module-import method-call rule."""
    tree = ast.parse(source)
    imports = collect_module_import_names(tree)
    calls = [node for node in ast.walk(tree) if isinstance(node, ast.Call)]
    assert [
        call_form(call, module_imports=imports) for call in calls
    ] == _cpython_call_forms(source)


@pytest.mark.parametrize(
    ("source", "form"),
    [("f(a, k=b)", "stack"), ("f(*a)", "expanded"), ("f(**k)", "expanded")],
)
def test_builder_records_the_call_form(source: str, form: str) -> None:
    call = ast.parse(source, mode="eval").body
    assert isinstance(call, ast.Call)
    emitter = _ArgumentEmitter()
    builder = emitter._emit_call_args_builder(call)
    created = next(op for op in emitter.ops if op.kind == "CALLARGS_NEW")
    assert created.result is builder
    assert created.metadata == {"call_form": form}


def test_rejected_argument_lowering_restores_exception_routing() -> None:
    emitter = _SuspendingArgumentEmitter()
    emitter.try_suppress_depth = 0
    original_labels = emitter.try_end_labels.copy()
    with pytest.raises(RuntimeError, match="rejected argument"):
        with emitter._scratch_cleanup_on_error([]):
            assert emitter.try_end_labels != original_labels
            assert emitter.try_suppress_depth is None
            raise RuntimeError("rejected argument")
    assert emitter.try_end_labels == original_labels
    assert emitter.try_suppress_depth == 0
    assert not emitter.ops, "a rejected expression must not emit half a cleanup CFG"


def test_each_pending_argument_has_one_frame_owner_until_its_scheduled_push() -> None:
    """A keyword group retains earlier values; another await must not re-park them.

    Each value needs one scratch owner, and the mutable argument builder reuses
    its one slot. This fails the former quadratic park/reload assembly without
    pinning incidental IR counts or execution timings.
    """
    emitter = _SuspendingArgumentEmitter()
    initial = emitter._new_call_args_builder("stack")
    call = ast.parse(
        "f(a=await one, b=await two, c=await three, d=await four)", mode="eval"
    ).body
    assert isinstance(call, ast.Call)
    emitter._emit_call_args_builder(call, initial=initial)
    evaluated = {
        op.result.name: op.args[0] for op in emitter.ops if op.kind == "EVALUATE"
    }
    stored = [op for op in emitter.ops if op.kind == "STORE_CLOSURE"]
    parked_arguments = [
        evaluated[op.args[2].name] for op in stored if op.args[2].name in evaluated
    ]
    assert parked_arguments == ["one", "two", "three"]
    builder_slots = {op.args[1] for op in stored if op.args[2].type_hint == "callargs"}
    assert len(builder_slots) == 1


def test_pending_argument_parking_work_scales_linearly() -> None:
    """Count actual compiler loop work, independent of emitted slot counts.

    Wall time is noisy; opcode tracing exposes a quadratic scan even when it
    skips already parked entries and emits linear IR. The trace is local to
    this compiler routine and is restored on every exit.
    """
    code = CallRuntimeHelperMixin._emit_call_args_builder.__code__

    def work(width: int) -> int:
        count = 0

        def trace(frame, event, argument):
            nonlocal count
            if frame.f_code is not code:
                return None
            frame.f_trace_opcodes = True
            if (
                event == "opcode"
                and code.co_code[frame.f_lasti] == dis.opmap["FOR_ITER"]
            ):
                count += 1
            return trace

        source = "f(" + ", ".join(f"k{i}=await v{i}" for i in range(width)) + ")"
        call = ast.parse(source, mode="eval").body
        assert isinstance(call, ast.Call)
        previous = sys.gettrace()
        frame = sys._getframe()
        previous_opcodes = frame.f_trace_opcodes
        # CPython 3.12 requires this before settrace, including its first call:
        # https://docs.python.org/3.12/library/sys.html#sys.settrace
        frame.f_trace_opcodes = True
        try:
            sys.settrace(trace)
            _SuspendingArgumentEmitter()._emit_call_args_builder(call)
        finally:
            sys.settrace(previous)
            frame.f_trace_opcodes = previous_opcodes
        assert count > 0, "opcode instrumentation did not observe compiler work"
        return count

    small, large = work(8), work(24)
    assert small > 0
    assert large <= 3 * small, (small, large)

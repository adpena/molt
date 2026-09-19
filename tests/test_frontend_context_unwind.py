from __future__ import annotations

import ast
import runpy
from pathlib import Path
from textwrap import dedent

import pytest

from molt.frontend import MoltOp, SimpleTIRGenerator, compile_to_tir
from molt.frontend._types import (
    AsyncContextExit,
    AsyncFrameSlotRole,
    MoltValue,
    ScratchCell,
    SyncContextExit,
    TryScope,
)


def _function_ops(source: str, suffix: str = "__f_poll") -> list[MoltOp]:
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(dedent(source)))
    return next(
        function["ops"]
        for name, function in generator.funcs_map.items()
        if name.endswith(suffix)
    )


def _context_body_start(ops: list[MoltOp]) -> int:
    # Entry has its own failure guard. The next protected region after the
    # synchronous enter or asynchronous entry suspension owns the source body.
    entered = next(
        i
        for i, op in enumerate(ops)
        if op.kind in {"CONTEXT_ENTER", "STATE_TRANSITION"}
    )
    return next(i for i in range(entered + 1, len(ops)) if ops[i].kind == "TRY_START")


@pytest.mark.parametrize("asynchronous", [False, True])
def test_suspended_context_reopens_one_operand_named_region(asynchronous: bool) -> None:
    prefix = "async " if asynchronous else ""
    ops = _function_ops(
        f"{prefix}def f(cm):\n    {prefix}with cm:\n        yield 1\n        yield 2\n"
    )
    body = ops[_context_body_start(ops)]
    assert len(body.args) == 1
    labels = {op.args[0] for op in ops if op.kind in {"LABEL", "STATE_LABEL"}}
    markers = [op for op in ops if op.kind in {"TRY_START", "TRY_END"}]
    assert all(len(op.args) == 1 and op.args[0] in labels for op in markers)
    assert sum(op.kind == "TRY_START" and op.args == body.args for op in markers) >= 3
    serialized = [
        op
        for op in SimpleTIRGenerator().map_ops_to_json(ops)
        if op["kind"] in {"try_start", "try_end"}
    ]
    assert [(op["kind"], op["value"]) for op in serialized] == [
        (op.kind.lower(), op.args[0]) for op in markers
    ]


@pytest.mark.parametrize("asynchronous", [False, True])
@pytest.mark.parametrize(
    "transfer, layout",
    [
        ("return value", "plain"),
        ("return value", "outer_manager"),
        ("return value", "inner_manager"),
        ("break", "inner_manager"),
        ("continue", "inner_manager"),
    ],
)
def test_serialized_nonlocal_context_close_preserves_enclosing_if_and_loop(
    asynchronous: bool, transfer: str, layout: str
) -> None:
    prefix = "async " if asynchronous else ""
    if layout == "plain":
        body = f"    {prefix}with cm:\n        if flag:\n            {transfer}\n"
    elif layout == "outer_manager":
        body = (
            f"    {prefix}with cm:\n"
            "        while value:\n"
            "            if flag:\n"
            f"                {transfer}\n"
        )
    else:
        body = (
            "    while value:\n"
            f"        {prefix}with cm:\n"
            "            if flag:\n"
            f"                {transfer}\n"
        )
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(f"{prefix}def f(cm, flag, value):\n{body}    return 2\n"))
    suffix = "__f_poll" if asynchronous else "__f"
    name, function = next(
        (name, function)
        for name, function in generator.funcs_map.items()
        if name.endswith(suffix)
    )
    region = function["ops"][_context_body_start(function["ops"])].args[0]
    serialized = next(
        item["ops"] for item in generator.to_json()["functions"] if item["name"] == name
    )
    stack: list[str] = []
    closes: list[tuple[str, ...]] = []
    for op in serialized:
        kind = op["kind"]
        if kind in {"if", "loop_start"}:
            stack.append(kind)
        elif kind in {"end_if", "loop_end"}:
            assert stack.pop() == ("if" if kind == "end_if" else "loop_start")
        elif kind == "try_end" and op.get("value") == region:
            closes.append(tuple(stack))
    assert not stack
    assert len(closes) >= 2
    assert "if" in closes[0]
    assert ("loop_start" in closes[0]) is (layout != "plain")
    assert any(op["kind"] == "label" and op.get("value") == region for op in serialized)


@pytest.mark.parametrize("exit_statement", ["return value", "break", "continue"])
def test_async_context_nonlocal_exit_uses_captured_exit_before_transfer(
    exit_statement: str,
) -> None:
    ops = _function_ops(
        f"async def f(cm, value):\n"
        f"    while value:\n"
        f"        async with cm:\n"
        f"            {exit_statement}\n"
    )
    body_start = _context_body_start(ops)
    body_label = ops[body_start].args[0]
    handler = next(
        i for i, op in enumerate(ops) if op.kind == "LABEL" and op.args == [body_label]
    )
    exit_ops = ops[body_start + 1 : handler]
    close = next(i for i, op in enumerate(exit_ops) if op.kind == "TRY_END")
    pop = next(i for i, op in enumerate(exit_ops) if op.kind == "EXCEPTION_POP")
    await_exit = next(
        i for i, op in enumerate(exit_ops) if op.kind == "STATE_TRANSITION"
    )
    assert close < pop < await_exit
    assert not any(
        op.kind == "CHECK_EXCEPTION" and op.args == [body_label]
        for op in exit_ops[close + 1 :]
    )
    # Every nonlocal source transfer has an explicit lexical destination.
    assert any(op.kind == "JUMP" for op in exit_ops[await_exit + 1 :])


def test_async_context_captures_special_exit_before_entering() -> None:
    ops = _function_ops("async def f(cm):\n    async with cm:\n        pass\n")
    special = [(i, op) for i, op in enumerate(ops) if op.kind == "GETATTR_SPECIAL_OBJ"]
    assert [op.args[1] for _, op in special] == ["__aenter__", "__aexit__"]
    first_call = next(i for i, op in enumerate(ops) if op.kind == "CALL_FUNC")
    assert special[-1][0] < first_call
    captured = special[-1][1].result
    assert any(
        op.kind == "STORE_CLOSURE" and op.args[-1] == captured
        for op in ops[special[-1][0] + 1 : first_call]
    )


@pytest.mark.parametrize("asynchronous", [False, True])
def test_context_assignment_is_inside_registered_body_scope(
    monkeypatch: pytest.MonkeyPatch, asynchronous: bool
) -> None:
    original = SimpleTIRGenerator._emit_assign_target
    observed: list[bool] = []

    def assign(self: SimpleTIRGenerator, target: ast.expr, *args: object) -> None:
        if isinstance(target, ast.Tuple):
            observed.append(
                bool(self.try_scopes)
                and self.try_scopes[-1].handler_label == self.try_end_labels[-1]
            )
        original(self, target, *args)

    monkeypatch.setattr(SimpleTIRGenerator, "_emit_assign_target", assign)
    prefix = "async " if asynchronous else ""
    compile_to_tir(
        f"{prefix}def f(cm):\n    {prefix}with cm as (x, y):\n        pass\n"
    )
    assert observed and all(observed)


def test_exceptional_async_exit_retains_context_until_await_and_truth_finish() -> None:
    ops = _function_ops(
        "async def f(cm):\n    async with cm:\n        raise ValueError()\n"
    )
    context = next(i for i, op in enumerate(ops) if op.kind == "EXCEPTION_CONTEXT_SET")
    tail = ops[context:]
    await_exit = next(i for i, op in enumerate(tail) if op.kind == "STATE_TRANSITION")
    truth = next(i for i, op in enumerate(tail) if i > await_exit and op.kind == "NOT")
    pop = next(i for i, op in enumerate(tail) if op.kind == "EXCEPTION_POP")
    assert await_exit < truth < pop
    assert any(
        op.kind == "GETATTR_GENERIC_OBJ" and op.args[1] == "__traceback__"
        for op in tail
    )
    cleanup_label = next(
        op.args[0] for op in reversed(ops[:context]) if op.kind == "TRY_START"
    )
    assert any(
        op.kind == "CHECK_EXCEPTION" and op.args == [cleanup_label]
        for op in tail[await_exit:pop]
    )


@pytest.mark.parametrize("exit_statement", ["return value", "break", "continue"])
def test_nested_async_exit_keeps_enclosing_handler_active(
    monkeypatch: pytest.MonkeyPatch, exit_statement: str
) -> None:
    original = SimpleTIRGenerator._emit_context_exit
    calls: list[tuple[int, list[str | None]]] = []

    def emit_exit(
        self: SimpleTIRGenerator,
        action: SyncContextExit | AsyncContextExit,
        *,
        exception: MoltValue | ScratchCell | None = None,
        abandon_on_error: ScratchCell | None = None,
    ) -> None:
        if exception is None:
            calls.append(
                (
                    len(self.try_end_labels),
                    [e.handler_name for e in self.active_exceptions],
                )
            )
            # No eager handler cleanup is allowed before nested __aexit__.
            last_context = next(
                (
                    op
                    for op in reversed(self.current_ops)
                    if op.kind == "EXCEPTION_CONTEXT_SET"
                ),
                None,
            )
            assert last_context is not None
            assert isinstance(last_context.args[0], MoltValue)
            assert last_context.args[0].type_hint != "None"
        original(self, action, exception=exception, abandon_on_error=abandon_on_error)

    monkeypatch.setattr(SimpleTIRGenerator, "_emit_context_exit", emit_exit)
    compile_to_tir(
        "async def f(cm, value):\n"
        "    try:\n        raise ValueError()\n"
        "    except ValueError as error:\n"
        "        while value:\n"
        "            async with cm:\n"
        f"                {exit_statement}\n"
    )
    assert calls and calls[0][1] == ["error"]


def test_sync_context_saved_error_has_independent_post_pop_owner() -> None:
    ops = _function_ops(
        "def f(cm):\n    with cm:\n        raise ValueError('body')\n", "__f"
    )
    observed = next(
        i for i, op in enumerate(ops) if op.kind == "EXCEPTION_LAST_PENDING"
    )
    raw = ops[observed].result
    captured = next(
        i for i, op in enumerate(ops[observed:], observed) if op.kind == "BINDING_ALIAS"
    )
    assert ops[captured].args == [raw]
    saved = ops[captured].result
    pop = next(
        i for i, op in enumerate(ops[captured:], captured) if op.kind == "EXCEPTION_POP"
    )
    assert captured < pop
    assert not any(raw in op.args for op in ops[pop + 1 :])
    assert any(
        op.kind == "CONTEXT_EXIT" and op.args[-1] == saved for op in ops[pop + 1 :]
    )
    assert any(op.kind == "RAISE" and op.args == [saved] for op in ops[pop + 1 :])


def test_async_context_capture_is_cleared_on_failed_entry_and_both_exit_paths() -> None:
    ops = _function_ops("async def f(cm):\n    async with cm:\n        pass\n")
    callback = next(
        op.result
        for op in ops
        if op.kind == "GETATTR_SPECIAL_OBJ" and op.args[1] == "__aexit__"
    )
    slot = next(
        op.args[1]
        for op in ops
        if op.kind == "STORE_CLOSURE" and op.args[-1] == callback
    )
    clears = [
        i
        for i, op in enumerate(ops)
        if op.kind == "STORE_CLOSURE"
        and op.args[1] == slot
        and op.args[-1].type_hint == "None"
    ]
    assert len(clears) == 3
    # Failed entry clears capture without calling __aexit__, and cannot branch
    # on the already-pending exception before consuming its entry frame.
    after_entry_clear = ops[clears[0] + 1 :]
    assert after_entry_clear[0].kind == "EXCEPTION_POP"
    dispatch = next(
        i for i, op in enumerate(after_entry_clear) if op.kind == "CHECK_EXCEPTION"
    )
    done = next(i for i, op in enumerate(after_entry_clear) if op.kind == "LABEL")
    assert 0 < dispatch < done
    assert not any(
        op.kind in {"CALL_FUNC", "CONTEXT_EXIT"} for op in after_entry_clear[:dispatch]
    )


@pytest.mark.parametrize("body", ["await value", "async with value:\n        pass"])
def test_await_result_carriers_are_consumed_before_pending_dispatch(body: str) -> None:
    ops = _function_ops(f"async def f(value):\n    {body}\n")
    constants = {op.result.name: op.args[0] for op in ops if op.kind == "CONST"}
    for index, op in enumerate(ops):
        if op.kind != "STATE_TRANSITION":
            continue
        slot = constants[op.args[1].name]
        tail = ops[index + 1 :]
        load = next(
            i
            for i, item in enumerate(tail)
            if item.kind == "LOAD_CLOSURE" and item.args == ["self", slot]
        )
        clear = next(
            i
            for i, item in enumerate(tail)
            if item.kind == "STORE_CLOSURE" and item.args[1] == slot
        )
        assert load < clear
        assert tail[clear].args[-1].type_hint == "None"
        assert not any(item.kind == "CHECK_EXCEPTION" for item in tail[:clear])


def test_async_context_lowering_is_repeatable() -> None:
    source = "async def f(a, b):\n    async with a, b:\n        return object()\n"
    assert compile_to_tir(source) == compile_to_tir(source)


def test_guarded_body_is_one_region_not_a_recursive_statement_chain() -> None:
    generator = SimpleTIRGenerator()
    before = len(generator.current_ops)
    generator._emit_guarded_body([ast.Pass() for _ in range(1500)])
    ops = generator.current_ops[before:]
    assert sum(op.kind == "TRY_START" for op in ops) == 1
    assert sum(op.kind == "EXCEPTION_PUSH" for op in ops) == 1
    assert sum(op.kind == "EXCEPTION_POP" for op in ops) == 1
    assert not generator.try_scopes and not generator.try_end_labels


@pytest.mark.parametrize("asynchronous", [False, True])
def test_context_exit_truth_test_retains_handled_context(asynchronous: bool) -> None:
    prefix = "async " if asynchronous else ""
    ops = _function_ops(
        f"{prefix}def f(cm):\n    {prefix}with cm:\n        raise ValueError()\n",
        "__f_poll" if asynchronous else "__f",
    )
    context = next(i for i, op in enumerate(ops) if op.kind == "EXCEPTION_CONTEXT_SET")
    tail = ops[context:]
    truth = next(i for i, op in enumerate(tail) if op.kind == "NOT")
    pop = next(i for i, op in enumerate(tail) if op.kind == "EXCEPTION_POP")
    assert truth < pop
    cleanup_label = next(
        op.args[0] for op in reversed(ops[:context]) if op.kind == "TRY_START"
    )
    assert any(
        op.kind == "CHECK_EXCEPTION" and op.args == [cleanup_label]
        for op in tail[truth:pop]
    )


@pytest.mark.parametrize("asynchronous", [False, True])
def test_nonlocal_finally_uses_remaining_scope_prefix(asynchronous: bool) -> None:
    prefix = "async " if asynchronous else ""
    compile_to_tir(
        f"{prefix}def f(a, b):\n"
        f"    {prefix}with a:\n"
        "        try:\n            return 1\n"
        "        finally:\n"
        f"            {prefix}with b:\n                return 2\n"
    )


@pytest.mark.parametrize("asynchronous", [False, True])
def test_nonlocal_finally_retains_only_live_exception_owners(
    monkeypatch: pytest.MonkeyPatch, asynchronous: bool
) -> None:
    original = SimpleTIRGenerator._emit_finalbody
    observed: list[list[str | None]] = []

    def emit_finalbody(
        self: SimpleTIRGenerator,
        scope: TryScope,
        *,
        pending_return: ScratchCell | None = None,
    ) -> None:
        if self.return_unwind_depth == 0:
            for entry in self.active_exceptions:
                assert any(entry.scope is scope for scope in self.try_scopes)
            observed.append([entry.handler_name for entry in self.active_exceptions])
        original(self, scope, pending_return=pending_return)

    monkeypatch.setattr(SimpleTIRGenerator, "_emit_finalbody", emit_finalbody)
    prefix = "async " if asynchronous else ""
    compile_to_tir(
        f"{prefix}def f():\n"
        "    try:\n        raise ValueError('saved')\n"
        "    except ValueError as error:\n"
        "        try:\n            return None\n"
        "        finally:\n            return lambda: error\n"
    )
    assert ["error"] in observed


def test_bare_raise_reads_runtime_handled_state_not_saved_pending_slot() -> None:
    ops = _function_ops(
        "def f():\n"
        "    try:\n        raise ValueError('outer')\n"
        "    except ValueError:\n"
        "        try:\n            pass\n"
        "        finally:\n            raise\n",
        "__f",
    )
    assert any(op.kind == "CALL" and op.args == ["molt_exception_active"] for op in ops)


def test_exit_failure_inside_handler_has_live_cleanup_continuation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    original = SimpleTIRGenerator._emit_context_exit
    continuation_labels: list[int] = []

    def emit_exit(
        self: SimpleTIRGenerator,
        action: SyncContextExit | AsyncContextExit,
        *,
        exception: MoltValue | ScratchCell | None = None,
        abandon_on_error: ScratchCell | None = None,
    ) -> None:
        if exception is None:
            assert self.try_end_labels
            guard = self.try_scopes[-1]
            assert guard.handler_label == self.try_end_labels[-1]
            assert guard.context_exit is None and guard.finalbody is None
            continuation_labels.append(guard.handler_label)
        original(self, action, exception=exception, abandon_on_error=abandon_on_error)

    monkeypatch.setattr(SimpleTIRGenerator, "_emit_context_exit", emit_exit)
    compile_to_tir(
        "async def f(cm, record):\n"
        "    try:\n        raise ValueError()\n"
        "    except ValueError:\n"
        "        async with cm:\n            return 1\n"
        "    finally:\n        record()\n"
    )
    assert continuation_labels


def test_async_context_capsule_cpython_reference(
    capsys: pytest.CaptureFixture[str],
) -> None:
    capsule = Path(__file__).parent / "differential/basic/async_with_scope_unwind.py"
    runpy.run_path(str(capsule))
    lines = capsys.readouterr().out.splitlines()
    assert lines[:4] == [
        "('outer', 'enter')",
        "('inner', 'enter')",
        "('inner', 'exit', None)",
        "('outer', 'exit', None)",
    ]
    assert "('assignment', 'exit', 'ValueError')" in lines
    assert "('failed', 'ValueError')" in lines
    assert lines.index("('handler-failed', 'finally')") < lines.index(
        "('handler-failed', 'caught')"
    )
    assert "('finally-outer', 'exit', 'RuntimeError')" in lines
    assert lines.index("('override-inner', 'exit', None)") < lines.index(
        "('override-outer', 'exit', None)"
    )
    assert "('handler-return', 'outer-handler')" in lines
    assert "('finally-capture', 'deleted')" in lines
    assert "('finally-reraise', 'ValueError')" in lines
    assert "('escaped-finally', 'RuntimeError')" in lines
    assert "('live-finally', 'ValueError')" in lines
    for mode in ("sync", "async"):
        assert f"('{mode}-entry', 'entry')" in lines
        assert f"('{mode}-truth', 'ValueError')" in lines
        assert f"('{mode}-truth-error', 'ValueError')" in lines
        assert f"('{mode}-finally-entry', 'entry')" in lines


def test_finally_loop_capsule_cpython_reference(
    capsys: pytest.CaptureFixture[str],
) -> None:
    capsule = Path(__file__).parent / "differential/basic/finally_loop_targets.py"
    runpy.run_path(str(capsule))
    lines = capsys.readouterr().out.splitlines()
    assert len(lines) == 12
    assert "continue-break [0, 1, 2, 'else']" in lines
    assert "dynamic-range -2 [0, -2, -4]" in lines


@pytest.mark.parametrize("asynchronous", [False, True])
def test_failed_entry_during_return_uses_live_finally_guard(
    monkeypatch: pytest.MonkeyPatch, asynchronous: bool
) -> None:
    original = SimpleTIRGenerator._emit_context_entry
    observed: list[int] = []

    def enter(
        self: SimpleTIRGenerator,
        action: SyncContextExit | AsyncContextExit,
        callback: MoltValue,
    ) -> MoltValue:
        start = len(self.current_ops)
        destination = self.try_end_labels[-1]
        suppression = self.try_suppress_depth
        result = original(self, action, callback)
        assert self.try_suppress_depth == suppression
        if self.return_unwind_depth:
            ops = self.current_ops[start:]
            failed = next(op.args[0] for op in ops if op.kind == "TRY_START")
            failed_pos = next(
                i
                for i, op in enumerate(ops)
                if op.kind == "LABEL" and op.args == [failed]
            )
            forwarded = [op.args for op in ops[failed_pos:] if op.kind == "JUMP"]
            assert forwarded == [[destination]]
            observed.append(destination)
        return result

    monkeypatch.setattr(SimpleTIRGenerator, "_emit_context_entry", enter)
    prefix = "async " if asynchronous else ""
    _function_ops(
        f"{prefix}def f(cm):\n"
        "    try:\n"
        "        try:\n            raise KeyError()\n"
        "        except KeyError:\n            return 1\n"
        "    finally:\n"
        f"        {prefix}with cm:\n            pass\n",
        "__f_poll" if asynchronous else "__f",
    )
    assert observed


@pytest.mark.parametrize("transfer", ["break", "continue"])
@pytest.mark.parametrize(
    "asynchronous, outer",
    [
        (asynchronous, outer)
        for asynchronous in (False, True)
        for outer in (
            "for outer in items",
            "for outer in range(3)",
            "for outer in (1, 2)",
            "while items",
        )
    ]
    + [(True, "async for outer in items")],
)
def test_inlined_finally_transfers_to_lexical_loop(
    monkeypatch: pytest.MonkeyPatch, asynchronous: bool, transfer: str, outer: str
) -> None:
    original = getattr(SimpleTIRGenerator, "visit_" + transfer.title())
    observed: list[tuple[list[MoltOp], int, int]] = []

    def visit(self: SimpleTIRGenerator, node: ast.Break | ast.Continue) -> None:
        # Both the inline and normal finally copies belong to the outer loop.
        assert len(self.loop_scopes) == 1
        loop = self.loop_scopes[-1]
        target = loop.break_label if transfer == "break" else loop.continue_label
        original(self, node)
        assert self.current_ops[-1].kind == "JUMP"
        assert self.current_ops[-1].args == [target]
        observed.append((self.current_ops, len(self.current_ops) - 1, target))

    monkeypatch.setattr(SimpleTIRGenerator, "visit_" + transfer.title(), visit)
    prefix = "async " if asynchronous else ""
    _function_ops(
        f"{prefix}def f(items):\n"
        f"    {outer}:\n"
        "        try:\n"
        "            for inner in [0]:\n                return 1\n"
        "        finally:\n"
        f"            {transfer}\n"
        "    return 2\n",
        "__f_poll" if asynchronous else "__f",
    )
    inline_count = 0
    for ops, position, target in observed:
        opened: list[int] = []
        ends: dict[int, int] = {}
        at_transfer: list[int] = []
        for i, op in enumerate(ops):
            if op.kind == "LOOP_START":
                opened.append(i)
            elif op.kind == "LOOP_END":
                ends[opened.pop()] = i
            if i == position:
                at_transfer = list(opened)
        assert not opened
        if len(at_transfer) != 2:
            continue
        inline_count += 1
        destination = next(
            i for i, op in enumerate(ops) if op.kind == "LABEL" and op.args == [target]
        )
        assert destination > ends[at_transfer[-1]]
        if transfer == "break":
            assert destination > ends[at_transfer[0]]
        else:
            assert destination < ends[at_transfer[0]]
    assert inline_count


@pytest.mark.parametrize("coroutine", [False, True])
def test_suspending_return_cleanup_uses_consumed_scratch_payload(
    monkeypatch: pytest.MonkeyPatch, coroutine: bool
) -> None:
    original_finalbody = SimpleTIRGenerator._emit_finalbody
    original_consume = SimpleTIRGenerator._consume_scratch_cell
    carriers: list[tuple[ScratchCell, list[MoltOp]]] = []
    consumed: list[ScratchCell] = []

    def finalbody(
        self: SimpleTIRGenerator,
        scope: TryScope,
        *,
        pending_return: ScratchCell | None = None,
    ) -> None:
        if pending_return is not None:
            carriers.append((pending_return, self.current_ops))
        original_finalbody(self, scope, pending_return=pending_return)

    def consume(self: SimpleTIRGenerator, cell: ScratchCell) -> MoltValue:
        consumed.append(cell)
        return original_consume(self, cell)

    monkeypatch.setattr(SimpleTIRGenerator, "_emit_finalbody", finalbody)
    monkeypatch.setattr(SimpleTIRGenerator, "_consume_scratch_cell", consume)
    prefix = "async " if coroutine else ""
    suspend = "await pause()" if coroutine else "yield 'cleanup'"
    _function_ops(
        f"{prefix}def f(make, pause):\n"
        "    try:\n        return make()\n"
        f"    finally:\n        {suspend}\n"
    )
    assert carriers
    for carrier, ops in carriers:
        assert carrier.async_slot is not None
        assert carrier.async_slot.role is AsyncFrameSlotRole.SCRATCH
        assert any(cell is carrier for cell in consumed)
        slot = carrier.async_slot.offset
        loads = [
            i
            for i, op in enumerate(ops)
            if op.kind == "LOAD_CLOSURE" and op.args == ["self", slot]
        ]
        clears = [
            i
            for i, op in enumerate(ops)
            if op.kind == "STORE_CLOSURE"
            and op.args[1] == slot
            and op.args[-1].type_hint == "None"
        ]
        assert loads and clears
        assert any(load < clear for load in loads for clear in clears)


@pytest.mark.parametrize("inner_transfer", ["break", "continue"])
@pytest.mark.parametrize("outer_transfer", ["break", "continue"])
def test_finally_loop_transfer_retires_only_crossed_return_owner(
    monkeypatch: pytest.MonkeyPatch, inner_transfer: str, outer_transfer: str
) -> None:
    observations: list[tuple[bool, bool]] = []
    for transfer in sorted({inner_transfer, outer_transfer}):
        method_name = "visit_" + transfer.title()
        original = getattr(SimpleTIRGenerator, method_name)

        def visit(
            self: SimpleTIRGenerator,
            node: ast.Break | ast.Continue,
            original=original,
        ) -> None:
            owners = [
                scope
                for scope in self.try_scopes
                if scope.abandon_on_unwind is not None
            ]
            if owners:
                (owner,) = owners
                carrier = owner.abandon_on_unwind
                assert carrier is not None and carrier.async_slot is not None
                inner = len(self.loop_scopes) == 2
                escaped = self.try_scopes[self.loop_scopes[-1].try_depth :]
                assert any(scope is owner for scope in escaped) is not inner
                start = len(self.current_ops)
                original(self, node)
                cleared = any(
                    op.kind == "STORE_CLOSURE"
                    and op.args[1] == carrier.async_slot.offset
                    and op.args[-1].type_hint == "None"
                    for op in self.current_ops[start:]
                )
                observations.append((inner, cleared))
                assert cleared is not inner
            else:
                original(self, node)

        monkeypatch.setattr(SimpleTIRGenerator, method_name, visit)
    _function_ops(
        "def f(make):\n"
        "    for outer in (0,):\n"
        "        try:\n            return make()\n"
        "        finally:\n"
        "            for inner in (0,):\n"
        f"                {inner_transfer}\n"
        "            yield 'cleanup'\n"
        f"            {outer_transfer}\n"
        "    yield 'after'\n"
    )
    assert (True, False) in observations
    assert (False, True) in observations


@pytest.mark.parametrize(
    "function_prefix, manager_prefix, suspension",
    [
        ("", "", "yield 'body'"),
        ("async ", "", "await pause()"),
        ("async ", "async ", "await pause()"),
    ],
)
def test_return_payload_reaches_manager_failure_cleanup(
    monkeypatch: pytest.MonkeyPatch,
    function_prefix: str,
    manager_prefix: str,
    suspension: str,
) -> None:
    original = SimpleTIRGenerator._emit_context_exit
    observed: list[ScratchCell] = []

    def emit_exit(
        self: SimpleTIRGenerator,
        action: SyncContextExit | AsyncContextExit,
        *,
        exception: MoltValue | ScratchCell | None = None,
        abandon_on_error: ScratchCell | None = None,
    ) -> None:
        start = len(self.current_ops)
        original(self, action, exception=exception, abandon_on_error=abandon_on_error)
        if abandon_on_error is None:
            return
        assert exception is None
        observed.append(abandon_on_error)
        assert abandon_on_error.async_slot is not None
        slot = abandon_on_error.async_slot.offset
        ops = self.current_ops[start:]
        clears = [
            i
            for i, op in enumerate(ops)
            if op.kind == "STORE_CLOSURE"
            and op.args[1] == slot
            and op.args[-1].type_hint == "None"
        ]
        assert clears
        handlers = {op.args[0] for op in ops if op.kind == "TRY_START"}
        assert handlers
        assert any(
            op.kind == "CHECK_EXCEPTION" and op.args[0] in handlers for op in ops
        )
        assert any(op.kind == "EXCEPTION_POP" for op in ops[clears[-1] + 1 :])

    monkeypatch.setattr(SimpleTIRGenerator, "_emit_context_exit", emit_exit)
    _function_ops(
        f"{function_prefix}def f(cm, make, pause):\n"
        f"    {manager_prefix}with cm:\n"
        f"        {suspension}\n"
        "        return make()\n"
    )
    assert observed


def test_generator_finally_return_lifetime_cpython_reference(
    capsys: pytest.CaptureFixture[str],
) -> None:
    capsule = (
        Path(__file__).parent
        / "differential/basic/generator_finally_return_lifetime.py"
    )
    runpy.run_path(str(capsule))
    assert capsys.readouterr().out.splitlines() == [
        "generator transfers passed",
        "generator retained return passed",
        "generator manager ordering passed",
        "coroutine transfers passed",
        "coroutine manager ordering passed",
    ]

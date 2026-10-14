"""Guarded builtin specialization in functions (HF-79).

A builtin read inside a function is a live lookup, so the frontend may only
specialize it behind ``molt_guard_builtin_bindings``. These tests pin where the
guard sits, what one guard covers, and that the fallback arm is the dynamic
lookup and call. ``tests/differential/basic/builtin_binding_guards.py`` and
the ``builtin_binding_guards_matrix_*.py`` rotations prove the runtime
behavior under every rebinding path against CPython.
"""

from __future__ import annotations

import ast
from pathlib import Path

from molt.compiler_analysis.builtin_binding_guards_generated import (
    BUILTIN_BINDING_GUARD_BITS,
)
from molt.compiler_analysis.python_binding_flow import analyze_python_source_bindings
from molt.frontend import compile_to_tir

GUARD = "molt_guard_builtin_bindings"
MATRIX_SUPPORT = (
    Path(__file__).parent / "differential" / "basic" / "builtin_binding_guard_support"
)


def _ops(source: str, suffix: str) -> list[dict]:
    ir = compile_to_tir(source)
    (function,) = [f for f in ir["functions"] if f["name"].endswith(suffix)]
    return function["ops"]


def _guards(ops: list[dict]) -> list[dict]:
    return [op for op in ops if op["kind"] == "call" and op.get("s_value") == GUARD]


def _guard_mask(ops: list[dict], guard: dict) -> int:
    (mask_op,) = [op for op in ops if op.get("out") == guard["args"][0]]
    return mask_op["value"]


def _arms(ops: list[dict], guard: dict) -> tuple[list[dict], list[dict]]:
    start = next(
        index
        for index, op in enumerate(ops)
        if op["kind"] == "if" and op.get("args") == [guard["out"]]
    )
    arms: list[list[dict]] = [[], []]
    depth = arm = 0
    for op in ops[start + 1 :]:
        if op["kind"] == "if":
            depth += 1
        elif op["kind"] == "else" and depth == 0:
            arm = 1
            continue
        elif op["kind"] == "end_if":
            if depth == 0:
                break
            depth -= 1
        arms[arm].append(op)
    return arms[0], arms[1]


def _kinds(ops: list[dict]) -> list[str]:
    return [op["kind"] for op in ops]


def _bit(*names: str) -> int:
    mask = 0
    for name in names:
        mask |= BUILTIN_BINDING_GUARD_BITS[name]
    return mask


def test_binding_facts_mark_only_unbound_builtin_reads_in_activations() -> None:
    source = """
def work(xs):
    return len(xs), max(xs), [ord(c) for c in xs], (lambda: abs(1))()

max = 3
top = len([1])
"""
    index = analyze_python_source_bindings(source)
    bits: dict[tuple[int, str], int] = {}
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
            fact = index.expression_fact(node)
            if fact is not None:
                bits[(node.lineno, node.id)] = fact.builtin_guard_bit
    assert bits[(3, "len")] == _bit("len")
    assert bits[(3, "ord")] == _bit("ord")
    assert bits[(3, "abs")] == _bit("abs")
    # The module binds `max`: a guard would fail on every call.
    assert bits[(3, "max")] == 0
    # Module code keeps its own source-order proof and needs no guard.
    assert bits[(6, "len")] == 0


def test_builtin_call_is_specialized_only_in_the_guarded_arm() -> None:
    ops = _ops("def size(xs: list[int]) -> int:\n    return len(xs)\n", "size")
    (guard,) = _guards(ops)
    assert _guard_mask(ops, guard) == _bit("len")
    fast, slow = _arms(ops, guard)
    assert "len" in _kinds(fast)
    assert "module_get_global" not in _kinds(fast)
    assert {"module_get_global", "call_func"} <= set(_kinds(slow))
    assert "len" not in _kinds(slow)


def test_guard_precedes_argument_evaluation() -> None:
    ops = _ops(
        "def show(callback):\n    print('x', callback())\n",
        "show",
    )
    (guard,) = _guards(ops)
    guard_index = ops.index(guard)
    calls = [
        index
        for index, op in enumerate(ops)
        if op["kind"] in {"call_func", "call_bind"} and index > guard_index
    ]
    # Python loads `print` before it runs `callback()`: the guard comes first,
    # and each arm evaluates the arguments after it.
    assert calls and min(calls) > guard_index
    assert not [
        op for op in ops[:guard_index] if op["kind"] in {"call_func", "call_bind"}
    ]


def test_one_guard_covers_reads_that_precede_any_python_code() -> None:
    ops = _ops(
        "def nested(xs):\n    total = 0\n    for i in range(len(xs)):\n"
        "        total += i\n    return total\n",
        "nested",
    )
    (guard,) = _guards(ops)
    assert _guard_mask(ops, guard) == _bit("range", "len")
    fast, slow = _arms(ops, guard)
    assert {"loop_index_start", "len"} <= set(_kinds(fast))
    assert "loop_index_start" not in _kinds(slow)
    assert {"iter", "module_get_global"} <= set(_kinds(slow))
    assert not _guards(fast) and not _guards(slow)


def test_a_python_callback_ends_coverage() -> None:
    ops = _ops(
        "def check(probe):\n    return isinstance(probe.value, int)\n",
        "check",
    )
    (guard,) = _guards(ops)
    # The attribute load may run a property that rebinds `int`.
    assert _guard_mask(ops, guard) == _bit("isinstance")
    covered = _ops(
        "def check(value):\n    return isinstance(value, int)\n",
        "check",
    )
    (covered_guard,) = _guards(covered)
    assert _guard_mask(covered, covered_guard) == _bit("isinstance", "int")


def test_shadowed_local_and_module_names_get_no_guard() -> None:
    assert not _guards(_ops("def size(xs, len=len):\n    return len(xs)\n", "size"))
    assert not _guards(
        _ops("def size(xs):\n    return len(xs)\n\nlen = print\n", "size")
    )


def test_regions_with_nested_scopes_stay_dynamic() -> None:
    ops = _ops(
        "def ordered(xs):\n    return sorted(xs, key=lambda x: -x)\n",
        "ordered",
    )
    assert not _guards(ops)
    loop = _ops(
        "def table(n):\n    rows = []\n    for i in range(n):\n"
        "        rows.append([j for j in range(i)])\n    return rows\n",
        "table",
    )
    # The loop body holds a comprehension scope: the loop is not duplicated.
    fast_loops = [
        guard
        for guard in _guards(loop)
        if "loop_index_start" in _kinds(_arms(loop, guard)[0])
    ]
    assert not fast_loops


def test_shadowing_matrix_probes_compile_behind_guards() -> None:
    """Each probe the shadowing matrix rebinds starts on the guarded arm.

    The matrix rotations compare with CPython, which has no guard, so they
    cannot see which arm ran. This pins that every probe they shadow is a
    guarded call: a shadow then switches it from the specialized arm to the
    generic one, rather than exercising only the dynamic lowering.
    """
    expected = {
        "size": _bit("len"),
        "to_int": _bit("int"),
        "is_int": _bit("isinstance", "int"),
    }
    for module, prefix in (("guarded.py", ""), ("matrix.py", "builtins_")):
        source = (MATRIX_SUPPORT / module).read_text(encoding="utf-8")
        for name, mask in expected.items():
            ops = _ops(source, f"__{prefix}{name}")
            (guard,) = _guards(ops)
            assert _guard_mask(ops, guard) == mask, (module, name)

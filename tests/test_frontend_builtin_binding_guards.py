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
import pytest

from molt.compiler_analysis.python_binding_flow import analyze_python_source_bindings
from molt.compiler_analysis.python_identity_catalog_generated import PythonMember
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.frontend import SimpleTIRGenerator, compile_to_tir

GUARD = "molt_guard_builtin_bindings"
MATRIX_SUPPORT = (
    Path(__file__).parent / "differential" / "basic" / "builtin_binding_guard_support"
)


def _ops(source: str, suffix: str) -> list[dict]:
    ir = compile_to_tir(source)
    (function,) = [f for f in ir["functions"] if f["name"].endswith(suffix)]
    return function["ops"]


# The premise a closed graph supplies when no module writes this one, no code
# writes a canonical member and no source body escapes into another activation.
CLOSED = PythonNamespacePremise((), 0, 0, (), ())


def _premise_ops(
    source: str, suffix: str, premise: PythonNamespacePremise = CLOSED
) -> list[dict]:
    generator = SimpleTIRGenerator(module_name="mod", namespace_premise=premise)
    generator.visit(ast.parse(source))
    (function,) = [
        f for f in generator.to_json()["functions"] if f["name"].endswith(suffix)
    ]
    return function["ops"]


def _global_reads(ops: list[dict]) -> list[str]:
    names = {op["out"]: op.get("s_value") for op in ops if op["kind"] == "const_str"}
    return [
        names.get(op["args"][1])
        for op in ops
        if op["kind"] == "module_get_global"
    ]


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


COUNT_STRINGS = """
def run(objs, n):
    count = 0
    for i in range(n):
        if isinstance(objs[i & 63], str):
            count += 1
    return count
"""


def test_closed_premise_proves_builtins_without_guard_or_lookup() -> None:
    """HF-226: the flow's exact identity replaces the runtime guard."""
    ops = _premise_ops(COUNT_STRINGS, "run")
    assert not _guards(ops)
    assert _global_reads(ops) == []
    kinds = _kinds(ops)
    # One counted loop, not a guarded fast arm plus a dynamic slow arm.
    assert kinds.count("loop_start") == 1 and "loop_index_start" in kinds
    assert {"builtin_type", "isinstance"} <= set(kinds)
    # Standalone compilation has no graph premise and keeps both guards.
    standalone = _ops(COUNT_STRINGS, "run")
    assert len(_guards(standalone)) == 2
    assert "str" in _global_reads(standalone)


@pytest.mark.parametrize(
    "prefix,premise",
    [
        ("def rebind():\n    global str\n    str = int\n", CLOSED),
        ("__builtins__ = {}\n", CLOSED),
        ("", PythonNamespacePremise(("str",), 0, 0, (), ())),
        (
            "",
            PythonNamespacePremise(
                (), int(PythonMember.BUILTINS_STR), int(PythonMember.BUILTINS_STR), (), ()
            ),
        ),
    ],
    ids=["global-writer", "builtins-mapping-writer", "foreign-writer", "member-writer"],
)
def test_any_writer_keeps_the_live_read(
    prefix: str, premise: PythonNamespacePremise
) -> None:
    ops = _premise_ops(prefix + COUNT_STRINGS, "run", premise)
    assert "str" in _global_reads(ops)
    assert "builtin_type" not in _kinds(ops)


def test_mapping_writer_withdraws_every_builtin_proof() -> None:
    ops = _premise_ops("__builtins__ = {}\n" + COUNT_STRINGS, "run")
    # range keeps its guard; the loop is the guarded fast/slow pair again.
    assert _guard_mask(ops, _guards(ops)[0]) & _bit("range")


def test_builtin_call_shape_is_proved_separately_from_the_callee() -> None:
    """`len()` reaches the builtin, which raises; it is no LEN operation.

    Main crashed the frontend here: the guard's fast arm trusted the binding
    as a proof of the call's shape.
    """
    source = "def f():\n    return len()\n\nlen()\n"
    for ops in (_premise_ops(source, "f"), _ops(source, "f")):
        assert "len" not in _kinds(ops)
        assert "call_func" in _kinds(ops)
    module = _premise_ops(source, "molt_main")
    assert "len" not in _kinds(module)


def test_frame_reflective_builtins_keep_their_lowering() -> None:
    ops = _premise_ops("def f():\n    return globals\n", "f")
    assert "globals" in _global_reads(ops)
    assert "builtin_func" not in _kinds(ops)


def test_unproved_catalogued_builtin_calls_its_live_binding() -> None:
    """A guard's slow arm calls whatever the frame's namespaces select.

    Main lowered `isinstance` to its operation in both arms, so a member or
    `__builtins__` writer could not replace it (CPython calls the writer's
    object). The typed identity now requires a proof for the named lowering.
    """
    source = "__builtins__ = {}\ndef check(value):\n    return isinstance(value, int)\n"
    for ops in (_premise_ops(source, "check"), _ops(source, "check")):
        (guard,) = _guards(ops)
        fast, slow = _arms(ops, guard)
        assert "isinstance" in _kinds(fast)
        assert "isinstance" not in _kinds(slow)
        assert "isinstance" in _global_reads(slow)
        assert "call_func" in _kinds(slow)

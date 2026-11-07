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
from molt.compiler_analysis.python_identity_catalog_generated import (
    BUILTIN_IDENTITIES,
    PythonMember,
)
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


def test_implicit_class_builder_read_keeps_the_proved_inline_arm():
    ops = _premise_ops("class C: pass\n", "molt_main")
    assert not _guards(ops)
    assert not any(op.get("s_value") == "molt_class_builder_check" for op in ops)
    assert any(op["kind"] == "class_def" for op in ops)


def test_implicit_class_builder_guard_precedes_base_evaluation():
    ops = _premise_ops(
        "import builtins\n"
        "builtins.__build_class__ = None\n"
        "def base(): return object\n"
        "class C(base()): pass\n",
        "molt_main",
    )
    guard = next(
        guard
        for guard in _guards(ops)
        if _guard_mask(ops, guard) == BUILTIN_BINDING_GUARD_BITS["__build_class__"]
    )
    checked = next(op for op in ops if op.get("s_value") == "molt_class_builder_check")
    base_call = next(
        op
        for op in ops
        if op["kind"].startswith("call")
        and op.get("source_line") == 4
        and op.get("col_offset") == 8
    )
    assert ops.index(guard) < ops.index(checked) < ops.index(base_call)


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
    return [names.get(op["args"][1]) for op in ops if op["kind"] == "module_get_global"]


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
                (),
                int(PythonMember.BUILTINS_STR),
                int(PythonMember.BUILTINS_STR),
                (),
                (),
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


def test_builtin_function_values_keep_their_live_read() -> None:
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


def test_star_import_source_origins_withdraw_the_builtin_proof() -> None:
    """A star import may export `len`; its read has a source origin.

    The flow publishes star exports as source origins of later global reads,
    not as identity bits, so the identity alone still reads BUILTIN_LEN.
    `__import__` carries its own read as an origin too (import provenance);
    the star lookup, not that origin, withdraws its proof (HF-776).
    """
    source = (
        "def measure(xs):\n    return len(xs), __import__(xs)\n\n"
        "measure([1])\nfrom support import *\nmeasure([1])\n"
    )
    ops = _premise_ops(source, "measure")
    assert {"len", "__import__"} <= set(_global_reads(ops))
    assert not {"len", "builtin_func"} & set(_kinds(ops))


# HF-776: one call or value per family of named builtin lowering, with the
# operation its proved read lowers to.
NAMED_LOWERINGS = [
    ("type", "type(x)", "type_of"),
    ("ord", "ord(x)", "ord"),
    ("abs", "abs(x)", "abs"),
    ("sum", "sum(x)", "builtin_func"),
    ("sorted", "sorted(x)", "builtin_func"),
    ("print", "print(x)", "builtin_func"),
    ("getattr", "getattr(x, 'a')", "get_attr_name_default"),
    ("dir", "dir(x)", "builtin_func"),
    ("ValueError", "ValueError(x)", "exception_new_builtin_one"),
    ("KeyError", "KeyError", "exception_class"),
    ("NotImplemented", "NotImplemented", "const_not_implemented"),
    ("Ellipsis", "Ellipsis", "const_ellipsis"),
    # Its read is also its own import provenance origin.
    ("__import__", "__import__(x)", "builtin_func"),
]


def _member_writer(name: str) -> PythonNamespacePremise:
    member = int(BUILTIN_IDENTITIES[name][1])
    return PythonNamespacePremise((), member, member, (), ())


@pytest.mark.parametrize(
    "name,expression,operation", NAMED_LOWERINGS, ids=[c[0] for c in NAMED_LOWERINGS]
)
def test_every_named_builtin_lowering_requires_its_proof(
    name: str, expression: str, operation: str
) -> None:
    """HF-776: the named lowering keys on the proved identity, not the spelling.

    Main lowered every uncatalogued builtin by name in both guard arms, so a
    `builtins` member writer could not replace `type` or `ord` (CPython calls
    the writer's object).
    """
    source = f"def f(x):\n    return {expression}\n"
    proved = _premise_ops(source, "f")
    assert operation in _kinds(proved)
    assert not _guards(proved) and name not in _global_reads(proved)
    ops = _premise_ops(source, "f", _member_writer(name))
    guards = _guards(ops)
    if name in BUILTIN_BINDING_GUARD_BITS:
        # The guard's fast arm proves the read; its slow arm has no proof.
        (guard,) = guards
        fast, slow = _arms(ops, guard)
        assert operation in _kinds(fast)
        assert operation not in _kinds(slow)
        assert name in _global_reads(slow)
    else:
        assert not guards
        assert operation not in _kinds(ops)
        assert name in _global_reads(ops)


def test_closed_premise_proves_every_family_without_guard_or_lookup() -> None:
    names = [name for name, _, _ in NAMED_LOWERINGS]
    body = ", ".join(expression for _, expression, _ in NAMED_LOWERINGS)
    ops = _premise_ops(f"def f(x):\n    return {body}\n", "f")
    assert not _guards(ops)
    assert not set(names) & set(_global_reads(ops))


def test_module_scope_member_writer_keeps_the_live_call() -> None:
    """The base repro: a module-level `builtins.type` write before the call."""
    source = (
        "import builtins\nbuiltins.type = lambda value: 'replaced'\nresult = type(1)\n"
    )
    ops = _premise_ops(source, "molt_main")
    assert "type_of" not in _kinds(ops)
    assert "type" in _global_reads(ops)


CLASS_BODY = """
class Body:
    error = ValueError
    kind = type
    marker = NotImplemented
    names = sorted(locals())
    later = type(2)
"""


def test_class_body_reads_are_proved_until_a_call() -> None:
    """HF-776: a class-body read that misses the plain class mapping is global.

    The flow proves it until the first call, which may reflect the class
    namespace; the later `type` read keeps the live lookup, as on main.
    """
    ops = _premise_ops(CLASS_BODY, "molt_main")
    assert _global_reads(ops) == ["type"]
    assert {"exception_class", "builtin_type", "const_not_implemented"} <= set(
        _kinds(ops)
    )
    writer = _premise_ops(CLASS_BODY, "molt_main", _member_writer("ValueError"))
    assert "ValueError" in _global_reads(writer)
    assert "exception_class" not in _kinds(writer)

from __future__ import annotations

import ast
from functools import partial

import pytest

from molt.compiler_analysis.python_binding_flow import analyze_python_bindings
from molt.compiler_analysis.python_source_keys import python_ast_digest
from molt.compiler_analysis.python_value_identity import UNBOUND_IDENTITY
from molt.frontend import SimpleTIRGenerator
from molt.frontend._types import MoltOp, MoltValue


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("inside_try", [False, True])
@pytest.mark.parametrize(
    "kind", ["FRAME_HOME_STORE", "FRAME_HOME_CELL", "FRAME_HOME_PRIVATE_CELL"]
)
def test_home_store_effect_selects_the_current_exception_edge(target, inside_try, kind):
    gen = SimpleTIRGenerator(module_name="home_edges", target_python=target)
    gen.function_exception_label = 100
    gen.try_end_labels = [200] if inside_try else []
    gen._expr_col = (4, 19)
    value = MoltValue("input", type_hint="int" if kind == "FRAME_HOME_STORE" else "Any")
    before = len(gen.current_ops)
    view = gen._emit_frame_home_store("value", value, kind=kind, slot=0)
    gen.emit(MoltOp(kind="TRY_END", args=[200], result=MoltValue("none")))
    emitted = gen.current_ops[before:]
    assert emitted[0].result is view and view.borrows_binding
    assert gen._op_instance_cannot_raise(emitted[0], {}) is (kind != "FRAME_HOME_STORE")
    if kind == "FRAME_HOME_STORE":
        assert gen._op_may_raise_for_sccp(kind) is True
        assert [op.kind for op in emitted] == [kind, "CHECK_EXCEPTION", "TRY_END"]
        assert emitted[1].args == [200 if inside_try else 100]
        assert (emitted[0].col_offset, emitted[0].end_col_offset) == (4, 19)
    else:
        # Existing cell objects are transferred; there is no raw view to box.
        assert [op.kind for op in emitted] == [kind, "TRY_END"]


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
def test_parameter_and_body_stores_have_authored_checks(target):
    gen = SimpleTIRGenerator(module_name="home_edges", target_python=target)
    gen.visit(
        ast.parse(
            "def probe(parameter):\n    try:\n        value = 33554432 * 33554432\n    except MemoryError:\n        return parameter\n    return value\n"
        )
    )
    ops = gen.funcs_map["home_edges__probe"]["ops"]
    stores = [index for index, op in enumerate(ops) if op.kind == "FRAME_HOME_STORE"]
    assert len(stores) >= 2, (
        "parameter prologue and local assignment both publish homes"
    )
    for index in stores:
        assert ops[index + 1].kind == "CHECK_EXCEPTION"
    targets = {ops[index + 1].args[0] for index in stores}
    assert len(targets) >= 2, "prologue and try body must use their own handlers"


def test_nested_cleanup_store_checks_but_boxed_restore_preserves_pending_exception():
    gen = SimpleTIRGenerator(module_name="home_edges")
    gen.function_exception_label = 100
    gen.frame_home_slots = {"value": 0}
    saved = gen._emit_frame_home_take("value")
    assert saved.type_hint == "Any" and not saved.borrows_binding
    scope = gen._enter_frame_restore_scope(
        partial(gen._emit_frame_home_store, "value", saved)
    )
    before = len(gen.current_ops)
    gen._emit_frame_home_store("value", MoltValue("raw", type_hint="int"))
    gen._exit_frame_restore_scope(scope)
    emitted = gen.current_ops[before:]
    assert emitted[0].kind == "FRAME_HOME_STORE"
    assert emitted[1].kind == "CHECK_EXCEPTION"
    assert emitted[1].args == [scope.cleanup_label]
    restores = [
        index
        for index, op in enumerate(emitted)
        if op.kind == "FRAME_HOME_STORE" and op.args == [saved]
    ]
    assert len(restores) == 2, (
        "normal and exceptional PEP 709 exits restore the same owned boxed take"
    )
    for index in restores:
        assert emitted[index + 1].kind == "JUMP"
    assert emitted[restores[1] + 1].args == [100]


def test_new_handler_inside_suppression_still_observes_fallible_store():
    gen = SimpleTIRGenerator(module_name="home_edges")
    gen.function_exception_label = 100
    with gen._suppress_check_exception(emit_on_exit=False):
        gen.try_end_labels.append(200)
        before = len(gen.current_ops)
        gen._emit_frame_home_store("value", MoltValue("raw", type_hint="int"), slot=0)
        gen.try_end_labels.pop()
    assert [op.kind for op in gen.current_ops[before:]] == [
        "FRAME_HOME_STORE",
        "CHECK_EXCEPTION",
    ]
    assert gen.current_ops[-1].args == [200]


# A frame-home store can raise, and a failed store keeps the earlier binding.
# Each source below resumes after such a store: a suppressing __exit__ (the
# `with` target binds first in its protected region) or a handler (the store is
# the try body's only fallible step). CPython's rule for an unbound local then
# applies: the later read raises UnboundLocalError instead of reading garbage.
_RECOVERED_STORE_SOURCES = {
    "with_target": (
        "def probe(manager):\n"
        "    with manager as bound:\n"
        "        pass\n"
        "    return bound\n"
    ),
    "with_target_in_loop": (
        "def probe(managers):\n"
        "    for manager in managers:\n"
        "        with manager as bound:\n"
        "            pass\n"
        "        print(bound)\n"
    ),
    "inner_with_target": (
        "def probe(outer, inner):\n"
        "    with outer, inner as bound:\n"
        "        pass\n"
        "    return bound\n"
    ),
    "try_body_local_copy": (
        "def probe(other):\n"
        "    try:\n"
        "        bound = other\n"
        "    except MemoryError:\n"
        "        pass\n"
        "    return bound\n"
    ),
}

_UNBOUND_BOUND_MESSAGE = (
    "cannot access local variable 'bound' where it is not associated with a value"
)


def _bound_read_facts(source: str):
    tree = ast.parse(source)
    index = analyze_python_bindings(tree, source_digest=python_ast_digest(tree))
    reads = [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Name)
        and node.id == "bound"
        and isinstance(node.ctx, ast.Load)
    ]
    assert reads
    return [index.expression_fact(node) for node in reads]


def _probe_ops(source: str, target: tuple[int, int]) -> list[MoltOp]:
    gen = SimpleTIRGenerator(module_name="home_edges", target_python=target)
    gen.visit(ast.parse(source))
    return gen.funcs_map["home_edges__probe"]["ops"]


@pytest.mark.parametrize(
    "source", _RECOVERED_STORE_SOURCES.values(), ids=_RECOVERED_STORE_SOURCES.keys()
)
def test_read_after_a_recovered_store_may_see_the_name_unbound(source):
    for fact in _bound_read_facts(source):
        assert fact is not None and fact.name_lookup == "lexical"
        assert fact.identities & UNBOUND_IDENTITY


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "source", _RECOVERED_STORE_SOURCES.values(), ids=_RECOVERED_STORE_SOURCES.keys()
)
def test_read_after_a_recovered_store_checks_the_home(source, target):
    ops = _probe_ops(source, target)
    assert any(
        op.kind == "CONST_STR" and op.args == [_UNBOUND_BOUND_MESSAGE] for op in ops
    )
    assert not any(
        op.kind == "LOAD_VAR" and (op.metadata or {}).get("var") == "bound"
        for op in ops
    ), "an SSA read would use a definition the failed store skipped"


def test_failed_with_target_store_keeps_an_earlier_binding():
    source = (
        "def probe(manager):\n"
        "    bound = None\n"
        "    with manager as bound:\n"
        "        pass\n"
        "    return bound\n"
    )
    (fact,) = _bound_read_facts(source)
    assert fact is not None and not fact.identities & UNBOUND_IDENTITY
    ops = _probe_ops(source, (3, 12))
    assert not any(
        op.kind == "CONST_STR" and op.args == [_UNBOUND_BOUND_MESSAGE] for op in ops
    )


def test_failed_store_raises_from_the_state_before_it():
    # Boxing fails before the home publishes the new binding, so the handler
    # sees only the state before the store: `bound` is definitely unbound.
    source = (
        "def probe(other):\n"
        "    try:\n"
        "        bound = 1\n"
        "    except MemoryError:\n"
        "        return bound\n"
        "    return other\n"
    )
    (fact,) = _bound_read_facts(source)
    assert fact is not None and fact.identities == UNBOUND_IDENTITY

from __future__ import annotations

import ast
from functools import partial

import pytest

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

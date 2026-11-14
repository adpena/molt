"""Static module cells (call_anatomy.md 4.5a).

The oracle is the eligibility list C1-C5 in ``molt.cli.static_module_cells``:
each case breaks one fact of an otherwise eligible module and must keep its
dictionary namespace with a reason naming that fact. An eligible module's
stamps carry ``cell`` and its init binds the block right after creating its
module object; an ineligible one keeps its slot stamps unchanged. The
``InitOnce`` cases check the F1/F2 rule against hand-written bodies.
"""

from __future__ import annotations

from typing import Any

import pytest

from molt.cli.closed_world_bindings import ModuleNamespaceFacts
from molt.cli.frontend_integration import module_global_layout, stamp_global_slots
from molt.cli.static_module_cells import (
    InitOnce,
    StaticCellPlan,
    plan_static_module_cells,
)
from molt.compiler_analysis.python_identity_catalog_generated import PythonMember
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise


def _premise(
    foreign: tuple[str, ...] | None = (),
    members: int = 0,
    reflected: bool = False,
) -> PythonNamespacePremise:
    return PythonNamespacePremise(
        foreign, members, 0, (), namespace_reflected=reflected
    )


_UNSET = object()


def _facts(
    *,
    own_names: frozenset[str] | None | object = _UNSET,
    premise: PythonNamespacePremise | None | object = _UNSET,
    own_namespace_line: int | None | object = _UNSET,
) -> ModuleNamespaceFacts:
    return ModuleNamespaceFacts(
        frozenset() if own_names is _UNSET else own_names,  # ty: ignore[invalid-argument-type]
        _premise() if premise is _UNSET else premise,  # ty: ignore[invalid-argument-type]
        0 if own_namespace_line is _UNSET else own_namespace_line,  # ty: ignore[invalid-argument-type]
    )


def _init(*, star: bool = False, module_new: bool = True) -> dict[str, Any]:
    """``mod``'s init: create the module object, def f, store x twice in a
    loop body, read both, and ``del`` y."""
    ops: list[dict[str, Any]] = [{"kind": "const_str", "out": "nm", "s_value": "mod"}]
    if module_new:
        ops += [
            {"kind": "module_new", "args": ["nm"], "out": "m"},
            {"kind": "check_exception", "value": 1},
        ]
    ops += [
        {"kind": "func_new", "s_value": "mod__f", "out": "fn"},
        {"kind": "const_str", "out": "kf", "s_value": "f"},
        {"kind": "module_set_attr", "args": ["m", "kf", "fn"], "out": "none"},
        {"kind": "loop_start"},
        {"kind": "const_str", "out": "kx", "s_value": "x"},
        {"kind": "module_set_attr", "args": ["m", "kx", "v"], "out": "none"},
        {"kind": "module_get_global", "args": ["m", "kx"], "out": "rx"},
        {"kind": "loop_end"},
        {"kind": "const_str", "out": "ky", "s_value": "y"},
        {"kind": "module_del_global", "args": ["m", "ky"], "out": "none"},
        {"kind": "const_str", "out": "kn", "s_value": "__name__"},
        {"kind": "module_get_global", "args": ["m", "kn"], "out": "rn"},
    ]
    if star:
        ops.append({"kind": "module_import_star", "args": ["src", "m"]})
    return {"name": "molt_init_mod", "params": [], "ops": ops}


def _plan(
    funcs: list[dict[str, Any]],
    facts: ModuleNamespaceFacts,
    extension_modules: frozenset[str] = frozenset(),
) -> StaticCellPlan:
    layouts = {"mod": module_global_layout(funcs)}
    return plan_static_module_cells(
        {"mod": funcs}, {"mod": facts}, layouts, {"mod"}, extension_modules
    )


def test_layout_names_every_non_dunder_global_the_body_names() -> None:
    assert module_global_layout([_init()]) == ("f", "x", "y")


def test_an_eligible_module_has_cells_and_init_once_facts() -> None:
    plan = _plan([_init()], _facts())
    assert plan.modules == {"mod"}
    assert plan.ineligible == {}
    # f is stored once, unconditionally, by a function definition; x is
    # stored in a loop; y is deleted.
    assert plan.init_once == {"mod": {"f": InitOnce("function")}}


@pytest.mark.parametrize(
    ("change", "reason"),
    [
        ({"own_names": None}, "reflects its namespace dynamically"),
        ({"own_namespace_line": None}, "no namespace reach"),
        ({"own_namespace_line": 7}, "line 7 reaches its namespace mapping"),
        ({"premise": None}, "no namespace premise"),
        ({"premise": _premise(reflected=True)}, "by identity"),
        ({"premise": _premise(None)}, "computed name"),
        (
            {"premise": _premise(members=int(PythonMember.SYS_MODULES))},
            "sys.modules",
        ),
    ],
)
def test_each_broken_fact_keeps_the_dictionary(
    change: dict[str, object], reason: str
) -> None:
    plan = _plan([_init()], _facts(**change))  # ty: ignore[invalid-argument-type]
    assert plan.modules == frozenset()
    assert reason in plan.ineligible["mod"]


def test_a_star_import_or_a_missing_module_object_keeps_the_dictionary() -> None:
    assert "star-imports" in _plan([_init(star=True)], _facts()).ineligible["mod"]
    plan = _plan([_init(module_new=False)], _facts())
    assert "module object" in plan.ineligible["mod"]


def test_program_blockers_withdraw_every_module() -> None:
    plan = _plan([_init()], _facts(), extension_modules=frozenset({"_ext"}))
    assert plan.modules == frozenset()
    assert "extension module _ext" in plan.ineligible["mod"]
    spawner = _init()
    spawner["ops"].append(
        {"kind": "call", "s_value": "molt_thread_spawn", "args": ["p"], "out": "t"}
    )
    plan = _plan([spawner], _facts())
    assert "isolate thread" in plan.ineligible["mod"]


def test_a_foreign_or_deferred_writer_removes_init_once() -> None:
    plan = _plan([_init()], _facts(own_names=frozenset({"f"})))
    assert plan.modules == {"mod"}
    assert plan.init_once == {}
    plan = _plan([_init()], _facts(premise=_premise(("f",))))
    assert plan.init_once == {}


def _stamped(cells: bool) -> list[dict[str, Any]]:
    init = _init()
    functions = [init]
    layouts = {"mod": module_global_layout([init])}
    stamp_global_slots(
        functions, {"mod": [init]}, layouts, {"mod"}, {"mod"} if cells else ()
    )
    return functions[0]["ops"]


def test_cell_stamps_and_the_bind_follow_module_creation() -> None:
    ops = _stamped(cells=True)
    stamps = [op["global_slot"] for op in ops if "global_slot" in op]
    assert stamps == [
        {"module": "mod", "index": 0, "cell": True},
        {"module": "mod", "index": 1, "cell": True},
        {"module": "mod", "index": 1, "cell": True},
    ]
    kinds = [op["kind"] for op in ops[:7]]
    assert kinds == [
        "const_str",
        "module_new",
        "check_exception",
        "static_cell_base",
        "const",
        "call",
        "func_new",
    ]
    # No layout name is a guarded builtin here: the bind records no bit.
    assert ops[4]["value"] == 0
    assert ops[3]["s_value"] == "mod"
    assert ops[5]["s_value"] == "molt_static_cells_bind"
    assert ops[5]["args"] == ["m", ops[3]["out"], ops[4]["out"]]
    # The dunder read and the delete keep their by-name ops.
    assert all("global_slot" not in op for op in ops if op.get("args") == ["m", "kn"])


def test_an_ineligible_module_keeps_slot_stamps_and_binds_nothing() -> None:
    ops = _stamped(cells=False)
    assert all("cell" not in op.get("global_slot", {}) for op in ops)
    assert not any(op["kind"] == "static_cell_base" for op in ops)


def test_the_bind_records_guard_bits_of_written_builtin_names() -> None:
    from molt.compiler_analysis.builtin_binding_guards_generated import (
        BUILTIN_BINDING_GUARD_BITS,
    )

    init = _init()
    init["ops"] += [
        {"kind": "const_str", "out": "kl", "s_value": "len"},
        {"kind": "module_set_attr", "args": ["m", "kl", "v"], "out": "none"},
        {"kind": "const_str", "out": "ka", "s_value": "abs"},
        {"kind": "module_get_global", "args": ["m", "ka"], "out": "ra"},
    ]
    plan = _plan([init], _facts(premise=_premise(("sorted",))))
    # len is stored here; abs is only read; sorted is written by another
    # module but is not a global this module names.
    assert plan.guard_masks == {"mod": BUILTIN_BINDING_GUARD_BITS["len"]}


def test_an_entry_module_imported_by_name_keeps_its_dictionary() -> None:
    init = _init()
    layouts = {"mod": module_global_layout([init])}
    facts = {"mod": _facts()}
    plan = plan_static_module_cells(
        {"mod": [init]},
        facts,
        layouts,
        {"mod"},
        entry_module="mod",
        dep_closures={"mod": {"mod", "lib"}, "lib": {"lib"}},
    )
    assert plan.modules == {"mod"}
    plan = plan_static_module_cells(
        {"mod": [init]},
        facts,
        layouts,
        {"mod"},
        entry_module="mod",
        dep_closures={"mod": {"mod", "lib"}, "lib": {"lib", "mod"}},
    )
    assert "lib imports the entry module" in plan.ineligible["mod"]

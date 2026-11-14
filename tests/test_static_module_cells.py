"""Static module cells (call_anatomy.md 4.5a).

The oracle is the eligibility list C1-C5 in ``molt.cli.static_module_cells``:
each case breaks one fact of an otherwise eligible module and must keep its
dictionary namespace with a reason naming that fact. An eligible module's
stamps carry ``cell`` and its init binds the block right after creating its
module object; an ineligible one keeps its slot stamps unchanged. The
``InitOnce`` cases check the F1/F2 rule against hand-written bodies, and the
dominance cases check the HF-846 rule: a read runs after the store when it
follows it in the init chain, or when every reference to its function does.
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
    assert plan.init_once == {"mod": {"f": InitOnce("function", "molt_init_mod", 5)}}


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


def _const(out: str, name: str) -> dict[str, Any]:
    return {"kind": "const_str", "out": out, "s_value": name}


def _store(name: str, value: str, line: int) -> list[dict[str, Any]]:
    return [
        _const(f"k_{name}_{line}", name),
        {
            "kind": "module_set_attr",
            "args": ["m", f"k_{name}_{line}", value],
            "out": "none",
            "source_line": line,
        },
    ]


def _read(name: str, out: str, line: int, module: str = "m") -> list[dict[str, Any]]:
    return [
        _const(f"r_{name}_{line}", name),
        {
            "kind": "module_get_global",
            "args": [module, f"r_{name}_{line}"],
            "out": out,
            "source_line": line,
        },
    ]


def _function(name: str, *ops: dict[str, Any]) -> dict[str, Any]:
    return {"name": name, "params": [], "ops": list(ops)}


def _func_new(symbol: str, out: str, line: int) -> dict[str, Any]:
    return {"kind": "func_new", "s_value": symbol, "out": out, "source_line": line}


def _dominance_program() -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """``mod``: a body read before and after LIMIT's store, a function
    defined before the store, one after it, a recursive one, one with no
    reference, one another module names, and a conditional store."""
    init = _function(
        "molt_init_mod",
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
        *_read("LIMIT", "early_read", 2),
        _func_new("mod__early", "f_early", 3),
        *_store("early", "f_early", 3),
        {"kind": "const", "value": 10, "out": "ten"},
        *_store("LIMIT", "ten", 5),
        _func_new("mod__late", "f_late", 6),
        *_store("late", "f_late", 6),
        _func_new("mod__fact", "f_fact", 7),
        *_store("fact", "f_fact", 7),
        _func_new("mod__shared", "f_shared", 8),
        *_store("shared", "f_shared", 8),
        *_read("LIMIT", "late_read", 9),
        {"kind": "if", "args": ["late_read"]},
        *_store("COND", "ten", 11),
        {"kind": "end_if"},
        *_read("COND", "cond_read", 12),
    )
    funcs = [
        init,
        _function("mod__early", *_read("LIMIT", "v", 20, "none")),
        _function("mod__late", *_read("LIMIT", "v", 30, "none")),
        _function(
            "mod__fact",
            *_read("LIMIT", "v", 40, "none"),
            {"kind": "call_internal", "s_value": "mod__fact", "args": ["v"]},
        ),
        _function("mod__orphan", *_read("LIMIT", "v", 50, "none")),
        _function("mod__shared", *_read("LIMIT", "v", 60, "none")),
    ]
    other = _function(
        "molt_init_other",
        {"kind": "call_internal", "s_value": "mod__shared", "args": []},
    )
    return funcs, [other]


def _dominance_plan(
    funcs: list[dict[str, Any]], functions: list[dict[str, Any]]
) -> StaticCellPlan:
    return plan_static_module_cells(
        {"mod": funcs},
        {"mod": _facts()},
        {"mod": module_global_layout(funcs)},
        {"mod"},
        program_functions=functions,
    )


def _lines(funcs: list[dict[str, Any]], sites: frozenset[tuple[str, int]]) -> set:
    by_name = {func["name"]: func for func in funcs}
    return {by_name[name]["ops"][index].get("source_line") for name, index in sites}


def test_init_dominance_admits_reads_that_run_after_the_store() -> None:
    funcs, others = _dominance_program()
    plan = _dominance_plan(funcs, funcs + others)
    assert plan.modules == {"mod"}
    assert set(plan.init_once["mod"]) == {"LIMIT", "early", "late", "fact", "shared"}
    # The body read after the store, the function defined after it, and the
    # recursive function whose only other reference follows the store.
    assert _lines(funcs, plan.init_dominated["mod"]) == {9, 30, 40}
    checked = "\n".join(plan.checked_init_once_reads)
    assert (
        "mod:2 LIMIT: checked read: the store at line 5 does not run before it"
        in checked
    )
    assert "mod:20 LIMIT: checked read: mod__early may run before the store" in checked
    assert "it may start at molt_init_mod line 3" in checked
    assert "mod:50 LIMIT: checked read: mod__orphan may run before the store" in checked
    assert "mod__orphan has no static reference" in checked
    # Another module's init names mod__shared: mod's init order does not
    # bound when it runs, as under an import cycle.
    assert "mod:60 LIMIT: checked read: mod__shared may run before the store" in checked
    # A conditional store has no InitOnce fact, so its read is not reported.
    assert "COND" not in checked


def test_dominated_reads_carry_init_once_stamps() -> None:
    funcs, others = _dominance_program()
    functions = funcs + others
    plan = _dominance_plan(funcs, functions)
    stamp_global_slots(
        functions,
        {"mod": funcs},
        {"mod": module_global_layout(funcs)},
        {"mod"},
        plan.modules,
        plan.guard_masks,
        plan.init_dominated,
    )
    stamps = {
        op.get("source_line"): op["global_slot"]
        for func in functions
        for op in func["ops"]
        if "global_slot" in op and op["kind"] == "module_get_global"
    }
    assert {line for line, stamp in stamps.items() if stamp.get("init_once")} == {
        9,
        30,
        40,
    }
    assert all(stamp["cell"] for stamp in stamps.values())


def test_a_chunk_called_twice_orders_nothing() -> None:
    from molt.frontend._types import _MOLT_MODULE_CHUNK_PARAM

    chunk = _function("mod__chunk", *_read("LIMIT", "v", 70))
    chunk["params"] = [_MOLT_MODULE_CHUNK_PARAM]
    init = _function(
        "molt_init_mod",
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
        {"kind": "const", "value": 1, "out": "one"},
        *_store("LIMIT", "one", 3),
        {"kind": "call_internal", "s_value": "mod__chunk", "args": ["m"]},
        {"kind": "call_internal", "s_value": "mod__chunk", "args": ["m"]},
    )
    funcs = [init, chunk]
    plan = _dominance_plan(funcs, funcs)
    assert "LIMIT" in plan.init_once["mod"]
    assert "mod" not in plan.init_dominated
    assert any(
        "mod__chunk is a body chunk outside the run-once chain" in line
        for line in plan.checked_init_once_reads
    )


def test_a_store_after_a_class_body_diamond_is_unconditional() -> None:
    """A class statement lowers to a forward jump around its handler and
    exception edges to the init's unwind landing. A store after that diamond
    still runs once on every normal completion, and dominates later reads;
    a store inside a loop runs more than once and has no InitOnce fact."""
    init = _function(
        "molt_init_mod",
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
        {"kind": "check_exception", "value": 3},
        {"kind": "call", "s_value": "molt_class_frame_exit", "args": [], "out": "e1"},
        {"kind": "jump", "value": 9},
        {"kind": "label", "value": 8},
        {"kind": "call", "s_value": "molt_class_frame_exit", "args": [], "out": "e2"},
        {"kind": "jump", "value": 3},
        {"kind": "label", "value": 9},
        {"kind": "const", "value": 1, "out": "one"},
        *_store("AFTER", "one", 10),
        {"kind": "check_exception", "value": 3},
        {"kind": "loop_start"},
        *_store("LOOPED", "one", 12),
        {"kind": "loop_end"},
        *_read("AFTER", "after_read", 14),
        *_read("LOOPED", "looped_read", 15),
        {"kind": "ret_void"},
        {"kind": "label", "value": 3, "exception_source": "unwind"},
        {"kind": "ret_void"},
    )
    funcs = [init]
    plan = _dominance_plan(funcs, funcs)
    assert set(plan.init_once["mod"]) == {"AFTER"}
    assert _lines(funcs, plan.init_dominated["mod"]) == {14}


def test_a_call_in_a_loop_after_the_store_is_one_dominated_start() -> None:
    init = _function(
        "molt_init_mod",
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
        {"kind": "const", "value": 1, "out": "one"},
        *_store("LIMIT", "one", 3),
        _func_new("mod__step", "f_step", 4),
        *_store("step", "f_step", 4),
        {"kind": "loop_start"},
        {"kind": "call_guarded", "s_value": "mod__step", "args": ["f_step"]},
        {"kind": "loop_end"},
        {"kind": "ret_void"},
    )
    step = _function("mod__step", *_read("LIMIT", "v", 30, "none"))
    funcs = [init, step]
    plan = _dominance_plan(funcs, funcs)
    assert _lines(funcs, plan.init_dominated["mod"]) == {30}

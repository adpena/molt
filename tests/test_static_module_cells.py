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
    FunctionCells,
    InitOnce,
    StaticCellPlan,
    apply_function_cells,
    plan_function_cells,
    plan_static_module_cells,
)
from molt.compiler_analysis.python_import_redirects import (
    NO_IMPORT_REDIRECTS,
    ImportRedirects,
    ImportWriter,
    PythonImportKeyBound,
    import_redirects,
)
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise


def _premise(
    foreign: tuple[str, ...] | None = (),
    members: int = 0,
    reflected: bool = False,
    code_rebinding: bool = False,
) -> PythonNamespacePremise:
    return PythonNamespacePremise(
        foreign,
        members,
        0,
        (),
        namespace_reflected=reflected,
        code_rebinding=code_rebinding,
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
    return {"name": "pyinit.mod", "params": [], "ops": ops}


def _plan(
    funcs: list[dict[str, Any]],
    facts: ModuleNamespaceFacts,
    extension_modules: frozenset[str] = frozenset(),
    redirects: ImportRedirects = NO_IMPORT_REDIRECTS,
    dep_closures: dict[str, set[str]] | None = None,
) -> StaticCellPlan:
    layouts = {"mod": module_global_layout(funcs)}
    return plan_static_module_cells(
        {"mod": funcs},
        {"mod": facts},
        layouts,
        {"mod"},
        redirects,
        extension_modules,
        dep_closures=dep_closures,
    )


def test_layout_names_every_non_dunder_global_the_body_names() -> None:
    assert module_global_layout([_init()]) == ("f", "x", "y")


def test_an_eligible_module_has_cells_and_init_once_facts() -> None:
    plan = _plan([_init()], _facts())
    assert plan.modules == {"mod"}
    assert plan.ineligible == {}
    # f is stored once, unconditionally, by a function definition; x is
    # stored in a loop; y is deleted.
    assert plan.init_once == {
        "mod": {"f": InitOnce("function", "pyinit.mod", 5, value="fn")}
    }


@pytest.mark.parametrize(
    ("change", "reason"),
    [
        ({"own_names": None}, "reflects its namespace dynamically"),
        ({"own_namespace_line": None}, "no namespace reach"),
        ({"own_namespace_line": 7}, "line 7 reaches its namespace mapping"),
        ({"premise": None}, "no namespace premise"),
        ({"premise": _premise(reflected=True)}, "by identity"),
        ({"premise": _premise(code_rebinding=True)}, "another globals mapping"),
        ({"premise": _premise(None)}, "computed name"),
    ],
)
def test_each_broken_fact_keeps_the_dictionary(
    change: dict[str, object], reason: str
) -> None:
    plan = _plan([_init()], _facts(**change))  # ty: ignore[invalid-argument-type]
    assert plan.modules == frozenset()
    assert reason in plan.ineligible["mod"]


_WRITER = ImportWriter("app:3", "sys.modules write")


@pytest.mark.parametrize(
    ("redirects", "eligible"),
    [
        (import_redirects(key_writers=(("mod", _WRITER),)), False),
        # A write of an ancestor key redirects the row; a descendant does not.
        (import_redirects(key_writers=(("mod.sub", _WRITER),)), True),
        (import_redirects(key_writers=(("other", _WRITER),)), True),
        (
            import_redirects(
                bound_writers=((PythonImportKeyBound("m", "d", 3), _WRITER),)
            ),
            False,
        ),
        (
            import_redirects(
                bound_writers=((PythonImportKeyBound("", ".path", 5), _WRITER),)
            ),
            True,
        ),
        (import_redirects((_WRITER,)), False),
    ],
)
def test_a_redirect_of_the_row_keeps_the_dictionary(
    redirects: ImportRedirects, eligible: bool
) -> None:
    plan = _plan([_init()], _facts(), redirects=redirects)
    assert (plan.modules == {"mod"}) is eligible
    if not eligible:
        assert "import mod may be redirected" in plan.ineligible["mod"]
        assert "sys.modules write at app:3" in plan.ineligible["mod"]


@pytest.mark.parametrize(
    ("guarded", "eligible"),
    [
        ("mod", False),
        # Importing a submodule runs its parent's body first.
        ("mod.sub", False),
        # An importer's failure carries the body failure it ran.
        ("app", False),
        ("unrelated", True),
        (PythonImportKeyBound("mo", "", 2), False),
        (None, False),
    ],
)
def test_a_handler_that_can_swallow_the_body_failure_keeps_the_dictionary(
    guarded: str | PythonImportKeyBound | None, eligible: bool
) -> None:
    """M11/HF-877: a swallowed body failure re-runs the body with the first
    run's cell values, so the module keeps its dictionary."""
    redirects = import_redirects(
        guarded=((guarded, ImportWriter("app:5", "guarded"), ()),)
    )
    plan = _plan(
        [_init()],
        _facts(),
        redirects=redirects,
        dep_closures={"app": {"app", "mod"}, "mod": {"mod"}},
    )
    assert (plan.modules == {"mod"}) is eligible
    if not eligible:
        assert "swallow a failure of its body" in plan.ineligible["mod"]


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
    kinds = [op["kind"] for op in ops[:8]]
    assert kinds == [
        "const_str",
        "module_new",
        "check_exception",
        "static_cell_base",
        "const",
        "const",
        "call",
        "func_new",
    ]
    # No layout name is a guarded builtin here: the bind records no bit, and
    # no proven method load names a function cell.
    assert ops[4]["value"] == 0
    assert ops[5]["value"] == 0
    assert ops[3]["s_value"] == "mod"
    assert ops[6]["s_value"] == "molt_static_cells_bind"
    assert ops[6]["args"] == ["m", ops[3]["out"], ops[4]["out"], ops[5]["out"]]
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
        NO_IMPORT_REDIRECTS,
        entry_module="mod",
        dep_closures={"mod": {"mod", "lib"}, "lib": {"lib"}},
    )
    assert plan.modules == {"mod"}
    plan = plan_static_module_cells(
        {"mod": [init]},
        facts,
        layouts,
        {"mod"},
        NO_IMPORT_REDIRECTS,
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
        "pyinit.mod",
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
        "pyinit.other",
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
        NO_IMPORT_REDIRECTS,
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
    assert "it may start at pyinit.mod line 3" in checked
    assert "mod:50 LIMIT: checked read: mod__orphan may run before the store" in checked
    assert "mod__orphan has no static reference" in checked
    # Another module's init names mod__shared: mod's init order does not
    # bound when it runs, as under an import cycle.
    assert "mod:60 LIMIT: checked read: mod__shared may run before the store" in checked
    # A conditional store has no InitOnce fact, so its read is not reported.
    assert "COND" not in checked


def test_a_store_into_the_new_function_does_not_start_it() -> None:
    """An annotated def stores ``__annotations__`` into its function object
    before the module stores the function: the store cannot run the
    function, so the function's read of itself stays dominated. Storing the
    function into another object can, so that function keeps the checked
    read."""
    init = _function(
        "pyinit.mod",
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
        _func_new("mod__rec", "f_rec", 2),
        {"kind": "dict_new", "args": [], "out": "ann"},
        {
            "kind": "set_attr_generic_obj",
            "args": ["f_rec", "ann"],
            "s_value": "__annotations__",
            "out": "none",
            "source_line": 2,
        },
        *_store("rec", "f_rec", 2),
        _func_new("mod__escaped", "f_escaped", 4),
        {
            "kind": "set_attr_generic_obj",
            "args": ["ann", "f_escaped"],
            "s_value": "hook",
            "out": "none",
            "source_line": 4,
        },
        *_store("escaped", "f_escaped", 4),
    )
    funcs = [
        init,
        _function(
            "mod__rec",
            *_read("rec", "v", 10, "none"),
            {"kind": "call_internal", "s_value": "mod__rec", "args": ["v"]},
        ),
        _function("mod__escaped", *_read("escaped", "v", 20, "none")),
    ]
    plan = _dominance_plan(funcs, funcs)
    assert _lines(funcs, plan.init_dominated["mod"]) == {10}
    checked = "\n".join(plan.checked_init_once_reads)
    assert "mod:20 escaped: checked read: mod__escaped may run before the store" in (
        checked
    )
    assert "mod:10" not in checked


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
        "pyinit.mod",
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
        "pyinit.mod",
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
        "pyinit.mod",
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


def _call_program(
    *,
    shape: bool = True,
    arity: int = 1,
    supplied: int = 1,
    kind: str = "func_new",
    entry_custody: tuple[str, ...] = ("transferred",),
    call_custody: bool = True,
    caller_before_store: bool = False,
    caller_use_before_store: dict[str, Any] | None = None,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """``mod`` defines ``helper`` (symbol ``mod__helper``) and a ``caller``
    whose guarded call reads ``helper`` from its cell. With
    ``caller_use_before_store``, the caller's object is created and used by
    that op before ``helper``'s store and published after it."""
    func_new: dict[str, Any] = {
        "kind": kind,
        "s_value": "mod__helper",
        "value": arity,
        "out": "f_helper",
        "source_line": 3,
    }
    if shape:
        func_new["direct_shape"] = True
    caller_new = _func_new("mod__caller", "f_caller", 2 if caller_before_store else 5)
    init_ops: list[dict[str, Any]] = [
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
    ]
    if caller_before_store:
        init_ops += [caller_new, *_store("caller", "f_caller", 2)]
    if caller_use_before_store is not None:
        init_ops += [caller_new, caller_use_before_store]
    init_ops += [func_new, *_store("helper", "f_helper", 3)]
    if caller_use_before_store is not None:
        init_ops += _store("caller", "f_caller", 5)
    elif not caller_before_store:
        init_ops += [caller_new, *_store("caller", "f_caller", 5)]
    init_ops.append({"kind": "ret_void"})
    call: dict[str, Any] = {
        "kind": "call_guarded",
        "s_value": "mod__helper",
        "args": ["callee", *[f"a{i}" for i in range(supplied)]],
        "out": "r",
        "source_line": 20,
    }
    if call_custody:
        call["argument_custody"] = ["transferred"] * (supplied + 1)
    caller = _function("mod__caller", *_read("helper", "callee", 20, "none"), call)
    helper = _function("mod__helper")
    helper["parameter_custody"] = list(entry_custody)
    funcs = [_function("pyinit.mod", *init_ops), caller, helper]
    return funcs, call


def _prove(funcs: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], list[str]]:
    from molt.cli.static_module_cells import prove_init_once_calls

    from molt.cli.frontend_integration import stamp_global_slots

    plan = _dominance_plan(funcs, funcs)
    # As in integration, both views start with the same function objects; the
    # global slot stamps replace bodies copy-on-write in ``functions`` only,
    # and the proof reads their init_once stamps and follows them by name.
    functions = list(funcs)
    module_functions = {"mod": list(funcs)}
    stamp_global_slots(
        functions,
        module_functions,
        {"mod": module_global_layout(funcs)},
        {"mod"},
        plan.modules,
        plan.guard_masks,
        plan.init_dominated,
    )
    diagnostics = prove_init_once_calls(functions, module_functions, plan)
    assert functions == module_functions["mod"], "both views see the rewrite"
    return functions, diagnostics


def _guarded(functions: list[dict[str, Any]]) -> dict[str, Any]:
    return next(
        op for func in functions for op in func["ops"] if op["kind"] == "call_guarded"
    )


def test_a_call_of_a_proven_init_once_function_is_marked_proven() -> None:
    funcs, _call = _call_program()
    snapshot = [dict(func, ops=[dict(op) for op in func["ops"]]) for func in funcs]
    functions, diagnostics = _prove(funcs)
    assert diagnostics == []
    assert _guarded(functions)["proven_target"] is True
    # Copy-on-write: the admitted bodies keep their ops.
    assert funcs == snapshot


@pytest.mark.parametrize(
    ("change", "reason"),
    [
        ({"shape": False}, "parameter shape that needs binding"),
        ({"supplied": 2, "arity": 1}, "passes 2 of 1 parameters"),
        ({"kind": "func_new_closure"}, "not a func_new without a closure"),
        ({"entry_custody": ()}, "entry custody differs"),
        ({"call_custody": False}, "entry custody differs"),
        ({"supplied": 0, "arity": 0}, "entry custody differs"),
    ],
)
def test_a_guarded_call_keeps_its_guards_when_a_fact_fails(
    change: dict[str, Any], reason: str
) -> None:
    funcs, _call = _call_program(**change)
    functions, diagnostics = _prove(funcs)
    assert "proven_target" not in _guarded(functions)
    assert len(diagnostics) == 1 and reason in diagnostics[0], diagnostics


def test_a_zero_argument_call_of_a_borrowing_entry_is_proven() -> None:
    funcs, _call = _call_program(supplied=0, arity=0, entry_custody=())
    functions, diagnostics = _prove(funcs)
    assert diagnostics == []
    assert _guarded(functions)["proven_target"] is True


def test_a_caller_that_may_run_before_the_store_is_not_proven() -> None:
    funcs, _call = _call_program(caller_before_store=True)
    functions, diagnostics = _prove(funcs)
    assert "proven_target" not in _guarded(functions)
    # The read is not dominated, so no InitOnce call fact applies.
    assert diagnostics == []


def _generator_call_program(
    *, generator: bool = True, **changes: Any
) -> list[dict[str, Any]]:
    """``_call_program`` with ``helper`` a generator function (its ``func_new``
    names the poll function and carries the frame size) and the caller's call
    the generic ``call_func`` the frontend emits for it."""
    funcs, call = _call_program(**changes)
    for func in funcs:
        for op in func["ops"]:
            if generator and op.get("out") == "f_helper":
                op.update(task_kind="generator", task_closure_size=64)
    call["kind"] = "call_func"
    del call["s_value"]
    return funcs


def _generic_call(functions: list[dict[str, Any]]) -> dict[str, Any]:
    return next(op for func in functions for op in func["ops"] if op.get("out") == "r")


def test_a_proven_call_of_a_generator_function_is_its_frame_allocation() -> None:
    funcs = _generator_call_program()
    snapshot = [dict(func, ops=[dict(op) for op in func["ops"]]) for func in funcs]
    functions, diagnostics = _prove(funcs)
    assert diagnostics == []
    # The poll function, the frame's bytes and the arguments: no callable, and
    # no argument custody (alloc_task borrows; the frame retains).
    assert _generic_call(functions) == {
        "kind": "alloc_task",
        "s_value": "mod__helper",
        "value": 64,
        "task_kind": "generator",
        "args": ["callee", "a0"],
        "out": "r",
        "source_line": 20,
    }
    assert funcs == snapshot


@pytest.mark.parametrize(
    ("change", "reason"),
    [
        ({"shape": False}, "parameter shape that needs binding"),
        ({"supplied": 2, "arity": 1}, "passes 2 of 1 parameters"),
    ],
)
def test_a_generator_call_stays_generic_when_a_fact_fails(
    change: dict[str, Any], reason: str
) -> None:
    functions, diagnostics = _prove(_generator_call_program(**change))
    assert _generic_call(functions)["kind"] == "call_func"
    assert len(diagnostics) == 1 and reason in diagnostics[0], diagnostics


@pytest.mark.parametrize(
    "change",
    [
        # Not a generator function: a generic call of a plain def stays.
        {"generator": False},
        # A closure's cells are part of the frame; this proof does not cover it.
        {"kind": "func_new_closure"},
        # The caller may run before the store: the cell may be empty.
        {"caller_before_store": True},
    ],
)
def test_a_generic_call_without_a_generator_fact_is_untouched(
    change: dict[str, Any],
) -> None:
    functions, diagnostics = _prove(_generator_call_program(**change))
    assert _generic_call(functions)["kind"] == "call_func"
    assert diagnostics == []


def _set_attr(owner: str, name: str, value: str) -> dict[str, Any]:
    return {
        "kind": "set_attr_generic_obj",
        "args": [owner, value],
        "s_value": name,
        "out": "none",
    }


@pytest.mark.parametrize(
    "name", ["__annotations__", "__annotate__", "__type_params__", "tag"]
)
def test_a_store_into_the_new_object_does_not_start_the_function(name: str) -> None:
    # `def caller(x: int)` stores its annotations on the new object before
    # the def publishes it. Any attribute store into a plain function runs
    # no code (no user __setattr__) and a new object has no old value to
    # release, so the caller still starts only after its store, which
    # follows helper's store.
    funcs, _call = _call_program(
        caller_use_before_store=_set_attr("f_caller", name, "meta")
    )
    functions, diagnostics = _prove(funcs)
    assert diagnostics == []
    assert _guarded(functions)["proven_target"] is True


@pytest.mark.parametrize(
    "use",
    [
        # The object stored into itself, or into another object, stays a use.
        _set_attr("f_caller", "__annotations__", "f_caller"),
        _set_attr("meta", "hook", "f_caller"),
        # Passing the object to a call may run it at once.
        {"kind": "call_func", "args": ["f_caller"], "out": "r0"},
    ],
)
def test_another_use_of_the_new_object_may_start_the_function(
    use: dict[str, Any],
) -> None:
    funcs, _call = _call_program(caller_use_before_store=use)
    functions, _diagnostics = _prove(funcs)
    assert "proven_target" not in _guarded(functions)


def _foreign_caller(name: str) -> dict[str, Any]:
    """``user`` calls ``mod.<name>(a0)``, its receiver proven to be ``mod``
    (the binder's ``foreign_global`` record)."""
    return _function(
        "user__caller",
        {
            "kind": "get_attr_generic_obj",
            "args": ["recv"],
            "s_value": name,
            "out": "callee",
            "foreign_global": "mod",
        },
        {
            "kind": "call_guarded",
            "s_value": "mod__helper",
            "args": ["callee", "a0"],
            "argument_custody": ["transferred", "transferred"],
            "out": "r",
            "source_line": 40,
        },
    )


def _prove_foreign(name: str) -> tuple[dict[str, Any], list[str]]:
    from molt.cli.frontend_integration import (
        stamp_foreign_globals,
        stamp_global_slots,
    )
    from molt.cli.static_module_cells import prove_init_once_calls

    funcs, _call = _call_program()
    plan = _dominance_plan(funcs, funcs)
    caller = _foreign_caller(name)
    functions = [*funcs, caller]
    module_functions = {"mod": list(funcs), "user": [caller]}
    layouts = {"mod": module_global_layout(funcs)}
    stamp_global_slots(
        functions,
        module_functions,
        layouts,
        {"mod"},
        plan.modules,
        plan.guard_masks,
        plan.init_dominated,
    )
    diagnostics = stamp_foreign_globals(
        functions, layouts, plan.modules, plan.init_once, plan.ineligible
    )
    diagnostics += prove_init_once_calls(functions, module_functions, plan)
    return functions[-1], diagnostics


def test_a_proven_foreign_call_reads_the_cell_and_enters_its_target() -> None:
    caller, diagnostics = _prove_foreign("helper")
    ops = caller["ops"]
    (read,) = [op for op in ops if op.get("out") == "callee"]
    assert read["kind"] == "module_get_global"
    assert read["global_slot"] == {
        "module": "mod",
        "index": module_global_layout(_call_program()[0]).index("helper"),
        "cell": True,
        "init_once": True,
    }
    assert "foreign_global" not in read
    (call,) = [op for op in ops if op["kind"] == "call_guarded"]
    assert call["proven_target"] is True
    assert not [line for line in diagnostics if ":40 " in line]


def test_a_foreign_read_without_an_init_once_cell_keeps_its_by_name_read() -> None:
    caller, diagnostics = _prove_foreign("missing")
    assert any(
        "mod.missing: by-name read: mod has no cell for missing" in line
        for line in diagnostics
    )
    ops = caller["ops"]
    (read,) = [op for op in ops if op.get("out") == "callee"]
    assert read["kind"] == "get_attr_generic_obj"
    assert "foreign_global" not in read
    (call,) = [op for op in ops if op["kind"] == "call_guarded"]
    assert "proven_target" not in call


def _constant_program(
    producer: dict[str, Any],
    *,
    second_store: bool = False,
    reader_before_store: bool = False,
) -> list[dict[str, Any]]:
    """``mod`` binds ``SCALE`` to ``producer``'s value once and defines a
    ``reader`` that reads it. With ``second_store`` the body writes ``SCALE``
    again; with ``reader_before_store`` the reader may run before the store."""
    reader_new = _func_new("mod__reader", "f_reader", 2 if reader_before_store else 6)
    init_ops: list[dict[str, Any]] = [
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
    ]
    if reader_before_store:
        init_ops += [reader_new, *_store("reader", "f_reader", 2)]
    init_ops += [{**producer, "out": "v_scale"}, *_store("SCALE", "v_scale", 3)]
    if second_store:
        init_ops += [
            {"kind": "const", "value": 4, "out": "v_scale_2"},
            *_store("SCALE", "v_scale_2", 4),
        ]
    if not reader_before_store:
        init_ops += [reader_new, *_store("reader", "f_reader", 6)]
    init_ops.append({"kind": "ret_void"})
    reader = _function(
        "mod__reader",
        *_read("SCALE", "scale", 20, "none"),
        {"kind": "add", "args": ["scale", "scale"], "out": "twice", "source_line": 20},
    )
    return [_function("pyinit.mod", *init_ops), reader]


def _fold(funcs: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], int]:
    from molt.cli.frontend_integration import stamp_global_slots
    from molt.cli.static_module_cells import fold_init_once_constants

    plan = _dominance_plan(funcs, funcs)
    functions = list(funcs)
    module_functions = {"mod": list(funcs)}
    stamp_global_slots(
        functions,
        module_functions,
        {"mod": module_global_layout(funcs)},
        {"mod"},
        plan.modules,
        plan.guard_masks,
        plan.init_dominated,
    )
    folded = fold_init_once_constants(functions, module_functions, plan)
    assert functions == module_functions["mod"], "both views see the rewrite"
    return functions, folded


def _scale_read(functions: list[dict[str, Any]]) -> dict[str, Any]:
    reader = next(func for func in functions if func["name"] == "mod__reader")
    return next(op for op in reader["ops"] if op.get("out") == "scale")


@pytest.mark.parametrize(
    ("producer", "constant"),
    [
        ({"kind": "const", "value": 3}, {"kind": "const", "value": 3}),
        (
            {"kind": "const", "value": -(1 << 46)},
            {"kind": "const", "value": -(1 << 46)},
        ),
        ({"kind": "const_bool", "value": 1}, {"kind": "const_bool", "value": 1}),
        ({"kind": "const_none"}, {"kind": "const_none"}),
        (
            {"kind": "const_float", "f_value": 0.5},
            {"kind": "const_float", "f_value": 0.5},
        ),
        (
            {"kind": "const_float", "f_value": "NaN"},
            {"kind": "const_float", "f_value": "NaN"},
        ),
    ],
)
def test_a_read_of_an_init_once_immediate_constant_is_the_constant(
    producer: dict[str, Any], constant: dict[str, Any]
) -> None:
    funcs = _constant_program(producer)
    snapshot = [dict(func, ops=[dict(op) for op in func["ops"]]) for func in funcs]
    functions, folded = _fold(funcs)
    assert folded == 1
    assert _scale_read(functions) == {**constant, "source_line": 20, "out": "scale"}
    # Copy-on-write: the input bodies keep their ops.
    assert funcs == snapshot


@pytest.mark.parametrize(
    ("producer", "change"),
    [
        # A heap object: a copy would be a different object (`is` changes).
        ({"kind": "const_bigint", "bytes": [0, 0, 0, 0, 0, 0, 0, 0, 1]}, {}),
        ({"kind": "const_str", "s_value": "x"}, {}),
        # Not InitOnce: a second store is a writer.
        ({"kind": "const", "value": 3}, {"second_store": True}),
        # The read may run before the store: the cell may be empty.
        ({"kind": "const", "value": 3}, {"reader_before_store": True}),
    ],
)
def test_a_read_that_is_not_a_proven_immediate_constant_keeps_the_cell(
    producer: dict[str, Any], change: dict[str, Any]
) -> None:
    functions, folded = _fold(_constant_program(producer, **change))
    assert folded == 0
    assert _scale_read(functions)["kind"] == "module_get_global"


def _intrinsic_call_program(supplied: int) -> list[dict[str, Any]]:
    """``mod`` binds ``_INTERN = require_intrinsic("molt_sys_intern")`` (a
    ``builtin_func`` of arity 1) once, and ``caller`` calls it generically."""
    init = _function(
        "pyinit.mod",
        _const("nm", "mod"),
        {"kind": "module_new", "args": ["nm"], "out": "m"},
        _const("bn", "molt_sys_intern"),
        {
            "kind": "builtin_func",
            "s_value": "molt_sys_intern",
            "value": 1,
            "args": ["bn"],
            "out": "f_intern",
            "source_line": 3,
        },
        *_store("_INTERN", "f_intern", 3),
        _func_new("mod__caller", "f_caller", 5),
        *_store("caller", "f_caller", 5),
        {"kind": "ret_void"},
    )
    caller = _function(
        "mod__caller",
        *_read("_INTERN", "callee", 20, "none"),
        {
            "kind": "call_func",
            "args": ["callee", *[f"a{i}" for i in range(supplied)]],
            "argument_custody": ["transferred"] * (supplied + 1),
            "out": "r",
            "type_hint": "str",
            "source_line": 20,
        },
    )
    return [init, caller]


def test_a_call_of_an_init_once_intrinsic_cell_is_the_direct_intrinsic_call() -> None:
    """The cell holds one builtin_func for the whole program (closed world), so
    an exact positional call is the frontend's direct intrinsic call: no
    callable to find, bind or admit at run time."""
    functions, diagnostics = _prove(_intrinsic_call_program(1))
    assert diagnostics == []
    (call,) = [op for f in functions for op in f["ops"] if op.get("out") == "r"]
    assert call == {
        "kind": "call",
        "s_value": "molt_sys_intern",
        "args": ["a0"],
        "out": "r",
        "type_hint": "str",
        "source_line": 20,
    }


def test_an_intrinsic_cell_call_with_another_arity_stays_generic() -> None:
    functions, diagnostics = _prove(_intrinsic_call_program(2))
    (call,) = [op for f in functions for op in f["ops"] if op.get("out") == "r"]
    assert call["kind"] == "call_func"
    assert any(
        "generic intrinsic call: the call passes 2 of 1 parameters" in line
        for line in diagnostics
    )


def test_a_foreign_call_through_the_object_starts_no_earlier_than_its_publication() -> (
    None
):
    """Another module that calls ``shared`` through its object (a guarded call
    whose callee word is the function) can only do so after ``mod`` published
    that object, which follows LIMIT's store; a call by symbol alone (the
    ``call_internal`` of ``_dominance_program``) keeps the read checked."""
    funcs, _others = _dominance_program()
    other = _function(
        "pyinit.other",
        _const("k_shared", "shared"),
        {"kind": "module_get_global", "args": ["mod_value", "k_shared"], "out": "f"},
        {"kind": "call_guarded", "s_value": "mod__shared", "args": ["f"], "out": "r"},
    )
    plan = _dominance_plan(funcs, funcs + [other])
    assert _lines(funcs, plan.init_dominated["mod"]) == {9, 30, 40, 60}


# Function cells (design 78, 5.9).


def _method_program(
    *,
    makers: int = 1,
    maker_in_loop: bool = False,
    run_first: bool = False,
    pair_read: bool = False,
    maker_kind: str = "func_new",
) -> list[dict[str, Any]]:
    """``mod``'s init makes method ``mod__C_m`` (``makers`` times, optionally
    in a loop) and ``mod__run``, whose body loads ``m`` on an exact receiver
    and calls it through a proven call."""
    maker = {"kind": maker_kind, "s_value": "mod__C_m", "out": "method"}
    run_maker = {"kind": "func_new", "s_value": "mod__run", "out": "runner"}
    ops: list[dict[str, Any]] = [
        {"kind": "const_str", "out": "nm", "s_value": "mod"},
        {"kind": "module_new", "args": ["nm"], "out": "m"},
        {"kind": "check_exception", "value": 1},
    ]
    store_run = [
        {"kind": "const_str", "out": "kr", "s_value": "run"},
        {"kind": "module_set_attr", "args": ["m", "kr", "runner"], "out": "none"},
    ]
    if run_first:
        # `run` escapes into the namespace, so it may start, before the
        # method's maker runs.
        ops += [run_maker, *store_run]
    if maker_in_loop:
        ops += [{"kind": "loop_start"}, maker, {"kind": "loop_end"}]
    else:
        ops += [dict(maker, out=f"method{n}") for n in range(makers)]
    if not run_first:
        ops += [run_maker, *store_run]
    body = [
        {"kind": "load_var", "var": "obj", "out": "receiver"},
        {
            "kind": "load_method",
            "args": ["receiver"],
            "s_value": "m",
            "var": "callable",
            "out": "pair",
            "proven_target": True,
        },
        {"kind": "check_exception", "value": 1},
        {
            "kind": "call_guarded",
            "s_value": "mod__C_m",
            "args": ["callable", "receiver"],
            "out": "result",
            "proven_target": True,
        },
    ]
    if pair_read:
        body.append({"kind": "load_attr", "args": ["pair"], "out": "leak"})
    return [
        {"name": "pyinit.mod", "params": [], "ops": ops},
        {"name": "mod__run", "params": [], "ops": body},
    ]


def _function_cells(
    funcs: list[dict[str, Any]],
) -> tuple[FunctionCells, dict[str, tuple[str, ...]]]:
    plan = _plan(funcs, _facts())
    assert plan.modules == frozenset({"mod"})
    layouts = {"mod": module_global_layout(funcs)}
    return plan_function_cells(plan, {"mod": funcs}, layouts, funcs), layouts


@pytest.mark.parametrize("maker_kind", ["func_new", "func_new_closure"])
def test_a_proven_load_after_its_callees_one_maker_reads_a_function_cell(
    maker_kind: str,
) -> None:
    # A closure method's object (a `__class__` cell, design 78 5.10) is one
    # per process too: the proven call reads its environment from it.
    funcs = _method_program(maker_kind=maker_kind)
    cells, layouts = _function_cells(funcs)
    index = len(layouts["mod"])
    assert cells.cells == {"mod": {"mod__C_m": index}}
    assert cells.counts() == {"mod": 1}
    apply_function_cells(funcs, cells)
    init_ops, run_ops = funcs[0]["ops"], funcs[1]["ops"]
    store = next(op for op in init_ops if op["kind"] == "static_cell_set")
    assert store["args"][1] == "method0"
    assert store["global_slot"] == {"module": "mod", "index": index, "cell": True}
    # The store follows the maker; the load becomes one cell read.
    assert [op["kind"] for op in init_ops[3:6]] == [
        maker_kind,
        "static_cell_base",
        "static_cell_set",
    ]
    assert "load_method" not in [op["kind"] for op in run_ops]
    read = next(op for op in run_ops if op["kind"] == "static_cell_load")
    assert read["out"] == "callable"
    assert read["global_slot"] == {
        "module": "mod",
        "index": index,
        "cell": True,
        "init_once": True,
    }


@pytest.mark.parametrize(
    ("program", "reason"),
    [
        ({"makers": 2}, "2 function makers make mod__C_m"),
        ({"maker_in_loop": True}, "does not run once"),
        ({"run_first": True}, "may run before mod__C_m's func_new"),
        ({"pair_read": True}, "its bound pair is read"),
    ],
    ids=["two-makers", "maker-in-loop", "run-made-first", "pair-read"],
)
def test_a_load_whose_callee_object_is_not_proven_keeps_load_method(
    program: dict[str, bool | int], reason: str
) -> None:
    funcs = _method_program(**program)  # ty: ignore[invalid-argument-type]
    cells, _layouts = _function_cells(funcs)
    assert cells.cells == {}
    (diagnostic,) = cells.diagnostics
    assert reason in diagnostic
    apply_function_cells(funcs, cells)
    assert "load_method" in [op["kind"] for op in funcs[1]["ops"]]

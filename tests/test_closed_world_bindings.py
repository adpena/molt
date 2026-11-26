"""Closed-world bindings of module attribute calls (call_anatomy.md 4.5).

The oracle is the proof obligation list F1-F4 in
``molt.cli.closed_world_bindings``: each case breaks one obligation of an
otherwise proven ``math.sqrt(x)`` site and must keep the generic call with a
diagnostic naming that obligation; the intact program must become one direct
intrinsic call with the attribute read deleted.
"""

from __future__ import annotations

import copy
from typing import Any

import pytest

from molt.cli.closed_world_bindings import (
    summarize_module_initializer,
    ModuleNamespaceFacts,
    bind_closed_world_calls,
)
from molt.cli.static_module_cells import plan_module_cells
from molt.compiler_analysis.python_import_redirects import (
    NO_IMPORT_REDIRECTS,
    ImportRedirects,
    ImportWriter,
    import_redirects,
)
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.frontend._types import _MOLT_MODULE_CHUNK_PARAM

M = _MOLT_MODULE_CHUNK_PARAM


def _premise(
    foreign: tuple[str, ...] | None = (), members: int = 0
) -> PythonNamespacePremise:
    return PythonNamespacePremise(foreign, members, 0, ())


def _math_body(*, conditional: bool = False, twice: bool = False) -> dict[str, Any]:
    ops: list[dict[str, Any]] = [
        {"kind": "const_str", "out": "n", "s_value": "molt_math_sqrt"},
        {
            "kind": "builtin_func",
            "args": ["n"],
            "s_value": "molt_math_sqrt",
            "out": "f",
        },
        {"kind": "const_str", "out": "k", "s_value": "sqrt"},
    ]
    store = {"kind": "module_set_attr", "args": [M, "k", "f"], "out": "none"}
    if conditional:
        ops += [{"kind": "if", "args": ["c"]}, store, {"kind": "end_if"}]
    else:
        ops.append(store)
    if twice:
        ops.append(dict(store))
    return {"name": "math__molt_module_chunk_1", "params": [M], "ops": ops}


def _user(
    *,
    arity: int = 1,
    import_name: str = "math",
    level: int = 0,
    fromlist: tuple[str, ...] = (),
) -> dict[str, Any]:
    return {
        "name": "user__molt_module_chunk_1",
        "params": [M],
        "ops": [
            {"kind": "const_str", "out": "mn", "s_value": import_name},
            {"kind": "const", "out": "lvl", "value": level},
            {
                "kind": "import_name",
                "s_value": import_name,
                "value": level,
                "fromlist": list(fromlist),
                "import_form": "dunder",
                "args": ["mn", "g", "none", "tup", "lvl"],
                "out": "mod",
            },
            {"kind": "const_str", "out": "b", "s_value": "math"},
            {"kind": "module_set_attr", "args": [M, "b", "mod"], "out": "none"},
            {"kind": "const_str", "out": "b2", "s_value": "math"},
            {"kind": "module_get_global", "args": [M, "b2"], "out": "recv"},
            {
                "kind": "get_attr_generic_obj",
                "args": ["recv"],
                "s_value": "sqrt",
                "out": "callee",
                "module_attribute": "math",
            },
            {
                "kind": "call_func",
                "args": ["callee", *[f"a{i}" for i in range(arity)]],
                "argument_custody": ["transferred"] * (arity + 1),
                "out": "r",
                "source_line": 9,
            },
        ],
    }


def _init(module: str, chunk: dict[str, Any]) -> dict[str, Any]:
    """The module's init, calling its one body chunk unconditionally."""
    return {
        "name": f"pyinit.{module}",
        "params": [],
        "ops": [{"kind": "call_internal", "s_value": chunk["name"], "args": []}],
    }


def _run(
    *,
    math: dict[str, Any] | None = None,
    user: dict[str, Any] | None = None,
    facts: dict[str, ModuleNamespaceFacts] | None = None,
    closures: dict[str, frozenset[str]] | None = None,
    redirects: ImportRedirects = NO_IMPORT_REDIRECTS,
) -> tuple[list[dict[str, Any]], list[str]]:
    math = math or _math_body()
    user = user or _user()
    math_init, user_init = _init("math", math), _init("user", user)
    functions = [math_init, math, user_init, user]
    module_functions = {"math": [math_init, math], "user": [user_init, user]}
    facts = facts or {
        "math": ModuleNamespaceFacts(frozenset(), _premise()),
        "user": ModuleNamespaceFacts(frozenset(), _premise()),
    }
    closures = closures or {
        "math": frozenset({"math"}),
        "user": frozenset({"user", "math"}),
    }
    snapshot = copy.deepcopy(functions)
    admitted = list(functions)
    diagnostics, _ = bind_closed_world_calls(
        functions,
        module_functions,
        facts,
        closures,
        redirects,
    )
    # Copy-on-write: the admitted bodies never change.
    assert admitted == snapshot
    return [functions[1], functions[3]], diagnostics


def test_a_proven_site_becomes_one_direct_intrinsic_call() -> None:
    functions, diagnostics = _run()
    assert diagnostics == []
    ops = functions[1]["ops"]
    assert all(op["kind"] != "get_attr_generic_obj" for op in ops)
    (call,) = [
        op for op in ops if op["kind"] in {"call_func", "call"} and op.get("out") == "r"
    ]
    assert call == {
        "kind": "call",
        "s_value": "molt_math_sqrt",
        "args": ["a0"],
        "out": "r",
        "source_line": 9,
    }
    # The receiver read stays: a read before the import still raises NameError.
    assert any(op["kind"] == "module_get_global" for op in ops)
    assert all("module_attribute" not in op for op in ops)


@pytest.mark.parametrize(
    ("target", "literal", "arity", "expected"),
    [
        (
            "py._intrinsics.require_intrinsic",
            "molt_buffer2d_get",
            3,
            "molt_buffer2d_get",
        ),
        ("py._intrinsics.require_intrinsic", "molt_buffer2d_get", 2, None),
        ("py._intrinsics.require_intrinsic", "unknown_symbol", 3, None),
        ("py.replacement.require_intrinsic", "molt_buffer2d_get", 3, None),
    ],
)
def test_conditional_intrinsic_lookup_suggests_a_target_on_the_live_call(
    target: str, literal: str, arity: int, expected: str | None
) -> None:
    producer: list[dict[str, Any]] = [
        {"kind": "const_str", "out": "name", "s_value": literal},
        {"kind": "const_str", "out": "binding", "s_value": "access"},
        {"kind": "if", "args": ["active"]},
        {
            "kind": "call_guarded",
            "args": ["live_resolver", "name"],
            "s_value": target,
            "out": "resolved",
        },
        {"kind": "module_set_attr", "args": [M, "binding", "resolved"]},
        {"kind": "end_if"},
    ]
    chunk = {"name": "py.user.chunk", "params": [M], "ops": producer}
    call = {
        "kind": "call_func",
        "args": ["loaded", *[f"a{i}" for i in range(arity)]],
        "argument_custody": ["transferred"] * (arity + 1),
        "out": "r",
    }
    consumer = {
        "name": "py.user.read",
        "ops": [
            {"kind": "const_str", "out": "name", "s_value": "access"},
            {"kind": "module_get_global", "args": ["module", "name"], "out": "loaded"},
            call,
        ],
    }
    funcs = [_init("user", chunk), chunk, consumer]
    modules = {"user": funcs.copy()}
    initializer = summarize_module_initializer("user", modules["user"])
    assert not initializer.unconditional(initializer.stores["access"][0])
    bind_closed_world_calls(funcs, modules, {}, {}, NO_IMPORT_REDIRECTS)
    assert funcs[1]["ops"] == producer
    assert funcs[2]["ops"][:-1] == consumer["ops"][:-1]
    assert funcs[2]["ops"][-1] == (
        {**call, "runtime_candidate": expected} if expected is not None else call
    )


@pytest.mark.parametrize(
    "user",
    [
        _user(import_name="math.sub"),
        _user(import_name="math", fromlist=("sqrt",)),
    ],
    ids=["dotted-binds-root", "from-import-binds-module"],
)
def test_the_typed_import_names_the_module_its_result_is(user: dict[str, Any]) -> None:
    """M3: the recognizer reads ``import_name``'s typed facts. ``import
    math.sub`` binds the root ``math`` (the transaction spelling matcher took
    the dotted name and refused this site), and a from-import binds the
    named module."""
    functions, diagnostics = _run(user=user)
    assert diagnostics == []
    assert sum(op["kind"] == "call" for op in functions[1]["ops"]) == 1


@pytest.mark.parametrize(
    ("change", "reason"),
    [
        (
            {"facts_math": ModuleNamespaceFacts(frozenset({"sqrt"}), _premise())},
            "deferred code",
        ),
        (
            {"facts_math": ModuleNamespaceFacts(frozenset(), _premise(("sqrt",)))},
            "another module writes it",
        ),
        (
            {"facts_math": ModuleNamespaceFacts(frozenset(), _premise(None))},
            "computed name",
        ),
        (
            {"facts_math": ModuleNamespaceFacts(None, _premise())},
            "reflects its own namespace",
        ),
        (
            {"facts_user": ModuleNamespaceFacts(frozenset({"math"}), _premise())},
            "user.math may be rebound",
        ),
        (
            {
                "redirects": import_redirects(
                    key_writers=(("math", ImportWriter("user:3", "sys.modules write")),)
                )
            },
            "import math may be redirected: sys.modules write at user:3",
        ),
        (
            {"redirects": import_redirects((ImportWriter("user:4", "hook write"),))},
            "import math may be redirected: hook write at user:4",
        ),
        (
            {
                "facts_user": ModuleNamespaceFacts(
                    frozenset(),
                    PythonNamespacePremise((), 0, 0, (), code_rebinding=True),
                )
            },
            "another globals mapping",
        ),
        ({"math": _math_body(conditional=True)}, "conditional"),
        ({"math": _math_body(twice=True)}, "2 times"),
        ({"user": _user(arity=2)}, "arity"),
        ({"user": _user(import_name="cmath")}, "other than import math"),
        # `from math import x` binds math itself only when it names math.
        (
            {"user": _user(import_name="cmath", fromlist=("x",))},
            "other than import math",
        ),
        # A relative import's target depends on the executing frame.
        ({"user": _user(level=1)}, "other than import math"),
        (
            {
                "closures": {
                    "math": frozenset({"math", "user"}),
                    "user": frozenset({"user", "math"}),
                }
            },
            "partly initialized",
        ),
    ],
)
def test_each_broken_obligation_keeps_the_generic_call_and_names_it(
    change: dict[str, Any], reason: str
) -> None:
    facts = {
        "math": change.get("facts_math", ModuleNamespaceFacts(frozenset(), _premise())),
        "user": change.get("facts_user", ModuleNamespaceFacts(frozenset(), _premise())),
    }
    functions, diagnostics = _run(
        math=change.get("math"),
        user=change.get("user"),
        facts=facts,
        closures=change.get("closures"),
        redirects=change.get("redirects", NO_IMPORT_REDIRECTS),
    )
    ops = functions[1]["ops"]
    assert any(op["kind"] == "call_func" for op in ops)
    assert any(op["kind"] == "get_attr_generic_obj" for op in ops)
    assert all("module_attribute" not in op for op in ops)
    (line,) = diagnostics
    assert line.startswith("user:9 math.sqrt: generic call:"), line
    assert reason in line, line


def test_global_slot_stamps_see_the_rebound_bodies() -> None:
    """Integration finds each module's bodies by identity: the slot stamps
    that run after binding must stamp the rewritten body, not skip it."""
    from molt.cli.frontend_integration import stamp_global_slots

    math, user = _math_body(), _user()
    functions = [_init("math", math), math, _init("user", user), user]
    module_functions = {"math": functions[:2], "user": functions[2:]}
    facts = {
        "math": ModuleNamespaceFacts(frozenset(), _premise()),
        "user": ModuleNamespaceFacts(frozenset(), _premise()),
    }
    closures = {"math": frozenset({"math"}), "user": frozenset({"user", "math"})}
    assert (
        bind_closed_world_calls(
            functions,
            module_functions,
            facts,
            closures,
            NO_IMPORT_REDIRECTS,
        )[0]
        == []
    )
    assert module_functions["user"][1] is functions[3]
    stamp_global_slots(
        functions,
        module_functions,
        {"user": ("math",), "math": ("sqrt",)},
        {"user", "math"},
    )
    read = next(op for op in functions[3]["ops"] if op["kind"] == "module_get_global")
    assert read["global_slot"] == {"module": "user", "index": 0}
    assert any(
        op["kind"] == "call" and op.get("s_value") == "molt_math_sqrt"
        for op in functions[3]["ops"]
    )


def test_a_foreign_writer_is_named_by_location() -> None:
    user = _user()
    user["ops"].append(
        {
            "kind": "set_attr_generic_obj",
            "args": ["recv", "x"],
            "s_value": "sqrt",
            "source_line": 30,
        }
    )
    facts = {
        "math": ModuleNamespaceFacts(frozenset(), _premise(("sqrt",))),
        "user": ModuleNamespaceFacts(frozenset(), _premise()),
    }
    _functions, (line,) = _run(user=user, facts=facts)
    assert "another module writes it by name" in line, line


def _chunked(calls: list[dict[str, Any]]) -> tuple[dict[str, Any], dict[str, Any]]:
    """math's publication in a chunk, called from math's init by `calls`."""
    chunk = _math_body()
    init = {"name": "pyinit.math", "params": [], "ops": calls}
    return init, chunk


@pytest.mark.parametrize(
    ("calls", "reason"),
    [
        # 5. A chunk whose call the scan never sees proves nothing.
        ([], "conditional"),
        # 6. A chunk called twice may bind twice, or conditionally once.
        (
            [
                {
                    "kind": "call_internal",
                    "s_value": "math__molt_module_chunk_1",
                    "args": [],
                },
                {"kind": "if", "args": ["c"]},
                {
                    "kind": "call_internal",
                    "s_value": "math__molt_module_chunk_1",
                    "args": [],
                },
                {"kind": "end_if"},
            ],
            "conditional",
        ),
        (
            [
                {
                    "kind": "call_internal",
                    "s_value": "math__molt_module_chunk_1",
                    "args": [],
                },
                {
                    "kind": "call_internal",
                    "s_value": "math__molt_module_chunk_1",
                    "args": [],
                },
            ],
            "conditional",
        ),
    ],
)
def test_a_publication_chunk_must_run_exactly_once(
    calls: list[dict[str, Any]], reason: str
) -> None:
    init, chunk = _chunked(calls)
    user = _user()
    user_init = _init("user", user)
    functions = [init, chunk, user_init, user]
    module_functions = {"math": [init, chunk], "user": [user_init, user]}
    facts = {
        "math": ModuleNamespaceFacts(frozenset(), _premise()),
        "user": ModuleNamespaceFacts(frozenset(), _premise()),
    }
    closures = {"math": frozenset({"math"}), "user": frozenset({"user", "math"})}
    (line,), _ = bind_closed_world_calls(
        functions,
        module_functions,
        facts,
        closures,
        NO_IMPORT_REDIRECTS,
    )
    assert reason in line, line


def test_a_publication_chunk_called_once_unconditionally_binds() -> None:
    init, chunk = _chunked(
        [{"kind": "call_internal", "s_value": "math__molt_module_chunk_1", "args": []}]
    )
    user = _user()
    user_init = _init("user", user)
    functions = [init, chunk, user_init, user]
    module_functions = {"math": [init, chunk], "user": [user_init, user]}
    facts = {
        "math": ModuleNamespaceFacts(frozenset(), _premise()),
        "user": ModuleNamespaceFacts(frozenset(), _premise()),
    }
    closures = {"math": frozenset({"math"}), "user": frozenset({"user", "math"})}
    assert (
        bind_closed_world_calls(
            functions,
            module_functions,
            facts,
            closures,
            NO_IMPORT_REDIRECTS,
        )[0]
        == []
    )


def _user_reading(name: str) -> dict[str, Any]:
    """The user module calling ``math.<name>(x)``: a name with no intrinsic
    publication, so the site stays a generic call."""
    user = _user()
    for op in user["ops"]:
        if op["kind"] == "get_attr_generic_obj":
            op["s_value"] = name
    return user


def _foreign_record(functions: list[dict[str, Any]]) -> str | None:
    (read,) = [op for op in functions[1]["ops"] if op.get("out") == "callee"]
    assert "module_attribute" not in read
    module = read.get("foreign_global")
    assert module is None or isinstance(module, str)
    return module


def test_a_proven_foreign_read_records_the_module_it_reads() -> None:
    # F1 (math never rebinds `helper`) and F3 (the receiver is math) hold:
    # the read keeps its by-name form and records math for the cell stamps.
    functions, diagnostics = _run(user=_user_reading("helper"))
    assert diagnostics == []
    assert _foreign_record(functions) == "math"
    assert any(op["kind"] == "call_func" for op in functions[1]["ops"])


@pytest.mark.parametrize(
    ("facts", "reason"),
    [
        # F1: math's deferred code rebinds helper.
        (
            {
                "math": ModuleNamespaceFacts(frozenset({"helper"}), _premise()),
                "user": ModuleNamespaceFacts(frozenset(), _premise()),
            },
            "math.helper may be rebound",
        ),
        # F1: another module writes math.helper.
        (
            {
                "math": ModuleNamespaceFacts(frozenset(), _premise(("helper",))),
                "user": ModuleNamespaceFacts(frozenset(), _premise()),
            },
            "math.helper may be rebound",
        ),
        # F3: user rebinds its `math` global.
        (
            {
                "math": ModuleNamespaceFacts(frozenset(), _premise()),
                "user": ModuleNamespaceFacts(frozenset({"math"}), _premise()),
            },
            "user.math may be rebound",
        ),
    ],
)
def test_an_unproven_foreign_read_records_nothing_and_names_its_reason(
    facts: dict[str, ModuleNamespaceFacts], reason: str
) -> None:
    # The read stays a by-name runtime lookup: the build diagnostic must say
    # which obligation failed, as every refused closed-world fact does.
    functions, diagnostics = _run(user=_user_reading("helper"), facts=facts)
    assert _foreign_record(functions) is None
    (line,) = diagnostics
    assert line.startswith("user:"), line
    assert " math.helper: by-name read: " in line, line
    assert reason in line, line


def test_conditional_maker_cannot_supply_an_unconditional_method_cell() -> None:
    math, user = _math_body(), _user()
    read, call = user["ops"][-2:]
    user["ops"] += [
        {**read, "out": "second_callee"},
        {**call, "args": ["second_callee", "a0"], "out": "second_result"},
        {"kind": "if", "args": ["condition"]},
        {"kind": "func_new", "s_value": "user__method", "out": "method"},
        {"kind": "end_if"},
        {
            "kind": "load_method",
            "args": ["recv"],
            "s_value": "method",
            "var": "callable",
            "out": "pair",
            "proven_target": True,
        },
        {
            "kind": "call_guarded",
            "s_value": "user__method",
            "args": ["callable", "recv"],
            "out": "result",
            "proven_target": True,
        },
    ]
    user_init = _init("user", user)
    user_init["ops"].insert(0, {"kind": "module_new", "out": "module"})
    functions = [_init("math", math), math, user_init, user]
    modules = {"math": functions[:2], "user": functions[2:]}
    facts = {
        module: ModuleNamespaceFacts(frozenset(), _premise(), own_namespace_line=0)
        for module in modules
    }
    diagnostics, initializers = bind_closed_world_calls(
        functions,
        modules,
        facts,
        {"math": {"math"}, "user": {"user", "math"}},
        NO_IMPORT_REDIRECTS,
    )
    assert diagnostics == []
    assert len(functions[-1]["ops"]) == len(user["ops"]) - 2
    plan, cells = plan_module_cells(
        modules,
        facts,
        {"user": ("math",)},
        {"user"},
        NO_IMPORT_REDIRECTS,
        program_functions=functions,
        initializers=initializers,
    )
    assert plan.modules == {"user"}
    assert cells.counts() == {}
    assert len(cells.diagnostics) == 1
    assert "may run before user__method's func_new" in cells.diagnostics[0]

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
    ModuleNamespaceFacts,
    bind_closed_world_calls,
)
from molt.compiler_analysis.python_identity_catalog_generated import PythonMember
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


def _user(*, arity: int = 1, import_name: str = "math") -> dict[str, Any]:
    return {
        "name": "user__molt_module_chunk_1",
        "params": [M],
        "ops": [
            {"kind": "const_str", "out": "mn", "s_value": import_name},
            {"kind": "const", "out": "lvl", "value": 0},
            {
                "kind": "call",
                "s_value": "molt_importlib_import_transaction",
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
        "name": f"molt_init_{module}",
        "params": [],
        "ops": [{"kind": "call_internal", "s_value": chunk["name"], "args": []}],
    }


def _run(
    *,
    math: dict[str, Any] | None = None,
    user: dict[str, Any] | None = None,
    facts: dict[str, ModuleNamespaceFacts] | None = None,
    closures: dict[str, frozenset[str]] | None = None,
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
    diagnostics = bind_closed_world_calls(functions, module_functions, facts, closures)
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
                "facts_user": ModuleNamespaceFacts(
                    frozenset(), _premise((), int(PythonMember.SYS_MODULES))
                )
            },
            "sys.modules",
        ),
        ({"math": _math_body(conditional=True)}, "conditional"),
        ({"math": _math_body(twice=True)}, "2 times"),
        ({"user": _user(arity=2)}, "arity"),
        ({"user": _user(import_name="cmath")}, "other than import math"),
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
    assert bind_closed_world_calls(functions, module_functions, facts, closures) == []
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
    assert "another module writes it by name (attribute writes: user:30)" in line, line


def _chunked(calls: list[dict[str, Any]]) -> tuple[dict[str, Any], dict[str, Any]]:
    """math's publication in a chunk, called from math's init by `calls`."""
    chunk = _math_body()
    init = {"name": "molt_init_math", "params": [], "ops": calls}
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
    (line,) = bind_closed_world_calls(functions, module_functions, facts, closures)
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
    assert bind_closed_world_calls(functions, module_functions, facts, closures) == []

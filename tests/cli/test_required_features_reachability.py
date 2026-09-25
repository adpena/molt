"""Reachability-driven runtime-feature requirement authority (Option b).

Pins the ``molt.cli.required_features`` authority that replaces whole-file
``module_required_intrinsic_names`` presence as the build's feature-requirement /
refusal driver (docs/design/foundation/feature_reachability_tree_shaking.md).

The fact under test: a runtime intrinsic becomes a hard link dependency exactly
when a REACHED SimpleIR ``builtin_func``/``const_str`` op references its symbol,
where reachability is the call-graph closure the native/WASM backends dead-strip
with (``molt-tir`` ``eliminate_dead_functions``). These tests use synthetic
merged-IR function lists so the reachability rules are pinned directly, plus a
generated op-kind agreement and runtime-root checks shared with Rust dead-function
elimination.
"""

from __future__ import annotations

import re

import pytest

import molt.cli as cli
from molt.cli import function_references as FR
from molt.cli import required_features as RF
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS,
)


def _const_str(out: str, value: str) -> dict[str, object]:
    return {"kind": "const_str", "s_value": value, "out": out}


def _builtin_func(out: str, symbol: str, arity: int = 1) -> dict[str, object]:
    return {"kind": "builtin_func", "s_value": symbol, "value": arity, "out": out}


def _call(symbol: str) -> dict[str, object]:
    return {"kind": "call", "s_value": symbol, "args": [], "out": "v0"}


def _func_new(symbol: str) -> dict[str, object]:
    return {"kind": "func_new", "s_value": symbol, "value": 0, "out": "v0"}


# ---------------------------------------------------------------------------
# Reachability closure (mirror of eliminate_dead_functions)
# ---------------------------------------------------------------------------


def test_entry_is_always_reachable() -> None:
    functions = [{"name": "molt_main", "params": [], "ops": []}]
    assert RF.reachable_function_names(functions) == frozenset({"molt_main"})


def test_call_edge_makes_callee_reachable() -> None:
    functions = [
        {"name": "molt_main", "params": [], "ops": [_call("helper")]},
        {"name": "helper", "params": [], "ops": []},
        {"name": "orphan", "params": [], "ops": []},
    ]
    reachable = RF.reachable_function_names(functions)
    assert "helper" in reachable
    assert "orphan" not in reachable


def test_func_new_edge_keeps_defined_function_reachable() -> None:
    # ``func_new`` (a function-object *definition*) is a reachability edge in
    # eliminate_dead_functions - defining a function keeps its body. This is the
    # exact rule that makes an imported stdlib module's eagerly-bound methods
    # reachable, so the requirement scan must honor it or it would under-count.
    functions = [
        {"name": "molt_main", "params": [], "ops": [_func_new("method")]},
        {
            "name": "method",
            "params": [],
            "ops": [_builtin_func("regex", "molt_re_compile")],
        },
    ]
    assert "method" in RF.reachable_function_names(functions)
    assert RF.required_link_features(functions) == frozenset({"stdlib_regex"})


def test_unreferenced_function_is_unreachable_and_contributes_no_requirement() -> None:
    # A function that is never referenced from any reachable function contributes
    # no intrinsic requirement even if it directly references a gated intrinsic.
    functions = [
        {"name": "molt_main", "params": [], "ops": []},
        {
            "name": "dead_regex_user",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        },
    ]
    assert "dead_regex_user" not in RF.reachable_function_names(functions)
    assert RF.required_link_features(functions) == frozenset()


def test_relocation_table_root_contributes_its_runtime_feature() -> None:
    functions = [
        {"name": "molt_main", "params": [], "ops": []},
        {
            "name": "molt_init_re",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        },
    ]
    assert RF.required_link_features(functions) == frozenset()
    assert RF.required_link_features(
        functions, extra_roots={"molt_init_re"}
    ) == frozenset({"stdlib_regex"})


def test_protected_isolate_entrypoint_is_a_root() -> None:
    # ``molt_isolate_*`` is a protected runtime entrypoint (the isolate import
    # dispatcher), so a module-init it references is reachable even with no edge
    # from ``molt_main`` - mirroring runtime_roots.is_protected_runtime_entrypoint.
    functions = [
        {"name": "molt_main", "params": [], "ops": []},
        {
            "name": "molt_isolate_import",
            "params": ["p0"],
            "ops": [_call("molt_init_re")],
        },
        {
            "name": "molt_init_re",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        },
    ]
    reachable = RF.reachable_function_names(functions)
    assert "molt_init_re" in reachable
    assert RF.required_link_features(functions) == frozenset({"stdlib_regex"})


@pytest.mark.parametrize("kind", ["alloc_task", "call_async"])
def test_task_creation_does_not_invent_poll_companion(kind: str) -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [{"kind": kind, "s_value": "worker", "out": "v0"}],
        },
        {"name": "worker", "params": [], "ops": []},
        {
            "name": "worker_poll",
            "params": ["p0"],
            "ops": [_builtin_func("v1", "molt_re_finditer_collect")],
        },
    ]
    reachable = RF.reachable_function_names(functions)
    assert reachable == frozenset({"molt_main", "worker"})
    assert RF.required_link_features(functions) == frozenset()


@pytest.mark.parametrize("kind", ["alloc_task", "call_async"])
def test_explicit_poll_target_retains_its_feature(kind: str) -> None:
    functions = [
        {
            "name": "molt_main",
            "ops": [{"kind": kind, "s_value": "worker_poll"}],
        },
        {"name": "worker", "ops": []},
        {
            "name": "worker_poll",
            "ops": [_builtin_func("v1", "molt_re_finditer_collect")],
        },
        {"name": "worker_poll_poll", "ops": []},
    ]
    assert RF.reachable_function_names(functions) == frozenset(
        {"molt_main", "worker_poll"}
    )
    assert RF.required_link_features(functions) == frozenset({"stdlib_regex"})


@pytest.mark.parametrize(
    "kind", sorted(SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS)
)
def test_generated_reference_kinds_retain_only_exact_defined_target(kind: str) -> None:
    functions = [
        {"name": "molt_main", "ops": [{"kind": kind, "s_value": "target"}]},
        {"name": "target", "ops": []},
        {"name": "target_poll", "ops": []},
    ]
    assert RF.reachable_function_names(functions) == frozenset({"molt_main", "target"})
    assert (
        FR.function_references(
            {"ops": [{"kind": kind, "s_value": "undefined"}]},
            frozenset({"target"}),
        )
        == frozenset()
    )


@pytest.mark.parametrize("kind", ["generator_create", "coro_create", "const_str"])
def test_non_reference_kinds_do_not_retain_defined_bodies(kind: str) -> None:
    functions = [
        {"name": "molt_main", "ops": [{"kind": kind, "s_value": "worker"}]},
        {"name": "worker", "ops": []},
        {"name": "worker_poll", "ops": []},
    ]
    assert RF.reachable_function_names(functions) == frozenset({"molt_main"})


@pytest.mark.parametrize("kind", ["alloc_task", "call_async"])
def test_local_reference_validation_requires_only_explicit_target(kind: str) -> None:
    functions = [
        {"name": "molt_init_pkg", "ops": [{"kind": kind, "s_value": "pkg__worker"}]},
        {"name": "pkg__worker", "ops": []},
    ]
    assert FR.missing_local_function_references("pkg", functions) == ()
    functions[0]["ops"] = [{"kind": kind, "s_value": "pkg__worker_poll"}]
    assert FR.missing_local_function_references("pkg", functions) == (
        FR.FunctionReferenceEdge("molt_init_pkg", 0, kind, "pkg__worker_poll"),
    )


# ---------------------------------------------------------------------------
# Intrinsic-symbol scanning + feature mapping
# ---------------------------------------------------------------------------


def test_builtin_func_direct_symbol_is_a_requirement() -> None:
    # The direct link reference (builtin_func -> func_addr / Linkage::Import).
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        }
    ]
    by_feature = RF.reached_intrinsic_symbols_by_feature(functions)
    assert by_feature == {"stdlib_regex": {"molt_re_compile"}}


def test_const_str_resolver_name_is_a_requirement() -> None:
    # The name-based resolver candidate shape (const_str intrinsic name).
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [_const_str("v1", "molt_ast_parse")],
        }
    ]
    assert RF.required_link_features(functions) == frozenset({"stdlib_ast"})


def test_core_and_resolver_only_symbols_are_not_requirements() -> None:
    # ``molt_stdlib_probe`` is a core ungated intrinsic; resolver-only features
    # (e.g. importlib_resources) are not link-affecting. Neither is a requirement.
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                _builtin_func("v1", "molt_stdlib_probe"),
                _const_str("v2", "molt_importlib_resources_reader_contents_from_roots"),
                _const_str("v3", "just a normal string"),
                _builtin_func("v4", "molt_list_append"),
            ],
        }
    ]
    assert RF.required_link_features(functions) == frozenset()


def test_reached_intrinsic_symbols_include_core_and_gated_manifest_symbols() -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                _builtin_func("v1", "molt_os_name"),
                _const_str("v2", "molt_re_compile"),
                _builtin_func("v3", "molt_list_append"),
            ],
        },
        {
            "name": "dead_codec_user",
            "params": [],
            "ops": [_builtin_func("v4", "molt_codecs_decode")],
        },
    ]

    assert RF.reached_intrinsic_symbols(functions) == frozenset(
        {"molt_os_name", "molt_re_compile"}
    )


def test_multiple_features_grouped_by_reached_symbols() -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                _builtin_func("v1", "molt_re_compile"),
                _builtin_func("v2", "molt_re_escape"),
                _builtin_func("v3", "molt_hash_new"),
            ],
        }
    ]
    by_feature = RF.reached_intrinsic_symbols_by_feature(functions)
    assert by_feature == {
        "stdlib_regex": {"molt_re_compile", "molt_re_escape"},
        "stdlib_crypto": {"molt_hash_new"},
    }


# ---------------------------------------------------------------------------
# Reachability ceiling refusal (truthful, reached-intrinsic message)
# ---------------------------------------------------------------------------


def test_refusal_fires_when_reached_feature_excluded() -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        }
    ]
    message = RF.reachability_profile_feature_refusal(
        functions,
        profile_name="micro",
        profile_features=frozenset({"stdlib_asyncio"}),
    )
    assert message is not None
    assert "stdlib_regex" in message
    assert "molt_re_compile" in message
    assert "'micro'" in message
    assert "--stdlib-profile full" in message
    assert "MOLT_STDLIB_PROFILE=full" in message
    assert "statically retained" in message
    assert "executed code paths" not in message


def test_refusal_silent_when_feature_within_ceiling() -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        }
    ]
    message = RF.reachability_profile_feature_refusal(
        functions,
        profile_name="full",
        profile_features=frozenset({"stdlib_regex"}),
    )
    assert message is None


def test_refusal_silent_when_reaching_only_core_intrinsics() -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [_builtin_func("v1", "molt_stdlib_probe")],
        }
    ]
    assert (
        RF.reachability_profile_feature_refusal(
            functions, profile_name="micro", profile_features=frozenset()
        )
        is None
    )


def test_refusal_ignores_unreached_intrinsic_user() -> None:
    # The headline distinction the design buys: a function that references a gated
    # intrinsic but is never reached does NOT trigger the refusal.
    functions = [
        {"name": "molt_main", "params": [], "ops": []},
        {
            "name": "never_called",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        },
    ]
    assert (
        RF.reachability_profile_feature_refusal(
            functions, profile_name="micro", profile_features=frozenset()
        )
        is None
    )


# ---------------------------------------------------------------------------
# Source-level agreement with the Rust dead-function authority
# ---------------------------------------------------------------------------


def _rust_source(relative: str) -> str:
    root = cli._compiler_root()
    return (root / relative).read_text(encoding="utf-8")


def test_defined_function_consumers_use_generated_authority() -> None:
    source = _rust_source("runtime/molt-tir/src/passes/dead_functions.rs")
    assert "simpleir_kind_references_defined_function(&op.kind)" in source
    assert "match op.kind.as_str()" not in source
    assert FR.SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS is (
        SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS
    )
    assert not hasattr(FR, "FUNCTION_REFERENCE_OP_KINDS")
    assert not hasattr(FR, "POLL_COMPANION_OP_KINDS")
    assert "_DEAD_FUNCTION_ELIM_REFERENCE_KINDS" not in dir(cli)


def test_protected_entrypoints_match_runtime_roots_rs() -> None:
    source = _rust_source("runtime/molt-tir/src/passes/runtime_roots.rs")
    names = set(re.findall(r'"([A-Za-z_]+)"', source))
    exact = {n for n in names if not n.endswith("_")}
    prefixes = {n for n in names if n.endswith("_")}
    assert RF._PROTECTED_RUNTIME_ENTRYPOINTS == frozenset(exact), (
        "protected runtime entrypoints diverged from runtime_roots.rs: "
        f"py={sorted(RF._PROTECTED_RUNTIME_ENTRYPOINTS)} rust={sorted(exact)}"
    )
    assert set(RF._PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES) == prefixes, (
        "protected runtime entrypoint prefixes diverged from runtime_roots.rs: "
        f"py={sorted(RF._PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES)} rust={sorted(prefixes)}"
    )


def test_real_re_module_intrinsics_are_link_affecting_regex() -> None:
    # Sanity: every molt_re_* symbol the real re module reaches maps to
    # stdlib_regex (the feature-gate authority), so the requirement is truthful.
    from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol

    for symbol in (
        "molt_re_compile",
        "molt_re_execute",
        "molt_re_escape",
        "molt_re_match_group",
    ):
        assert link_affecting_feature_gate_for_symbol(symbol) == "stdlib_regex"

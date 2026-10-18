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

from collections.abc import Mapping, Sequence
import re

import pytest

import molt.cli as cli
from molt.cli import function_references as FR
from molt.cli import required_features as RF
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS,
)
from molt.cli import compiler_metadata as cli_compiler_metadata


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
    assert RF.required_link_features(
        RF.reached_intrinsic_symbols(functions)
    ) == frozenset({"stdlib_regex"})


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
    assert (
        RF.required_link_features(RF.reached_intrinsic_symbols(functions))
        == frozenset()
    )


def test_relocation_table_root_contributes_its_runtime_feature() -> None:
    functions = [
        {"name": "molt_main", "params": [], "ops": []},
        {
            "name": "molt_init_re",
            "params": [],
            "ops": [_builtin_func("v1", "molt_re_compile")],
        },
    ]
    assert (
        RF.required_link_features(RF.reached_intrinsic_symbols(functions))
        == frozenset()
    )
    assert RF.required_link_features(
        RF.reached_intrinsic_symbols(functions, extra_roots={"molt_init_re"})
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
    assert RF.required_link_features(
        RF.reached_intrinsic_symbols(functions)
    ) == frozenset({"stdlib_regex"})


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
            "ops": [_builtin_func("v1", "molt_re_finditer_next")],
        },
    ]
    reachable = RF.reachable_function_names(functions)
    assert reachable == frozenset({"molt_main", "worker"})
    assert (
        RF.required_link_features(RF.reached_intrinsic_symbols(functions))
        == frozenset()
    )


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
            "ops": [_builtin_func("v1", "molt_re_finditer_next")],
        },
        {"name": "worker_poll_poll", "ops": []},
    ]
    assert RF.reachable_function_names(functions) == frozenset(
        {"molt_main", "worker_poll"}
    )
    assert RF.required_link_features(
        RF.reached_intrinsic_symbols(functions)
    ) == frozenset({"stdlib_regex"})


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
    by_feature = RF.reached_intrinsic_symbols_by_feature(
        RF.reached_intrinsic_symbols(functions)
    )
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
    assert RF.required_link_features(
        RF.reached_intrinsic_symbols(functions)
    ) == frozenset({"stdlib_ast"})


def test_core_and_resolver_only_symbols_are_not_requirements() -> None:
    # ``molt_capabilities_has`` is a core ungated intrinsic; resolver-only features
    # (e.g. importlib_resources) are not link-affecting. Neither is a requirement.
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                _builtin_func("v1", "molt_capabilities_has"),
                _const_str("v2", "molt_importlib_resources_reader_contents_from_roots"),
                _const_str("v3", "just a normal string"),
                _builtin_func("v4", "molt_list_append"),
            ],
        }
    ]
    assert (
        RF.required_link_features(RF.reached_intrinsic_symbols(functions))
        == frozenset()
    )


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
    by_feature = RF.reached_intrinsic_symbols_by_feature(
        RF.reached_intrinsic_symbols(functions)
    )
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
        RF.reached_intrinsic_symbols(functions),
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
        RF.reached_intrinsic_symbols(functions),
        profile_name="full",
        profile_features=frozenset({"stdlib_regex"}),
    )
    assert message is None


def test_refusal_silent_when_reaching_only_core_intrinsics() -> None:
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [_builtin_func("v1", "molt_capabilities_has")],
        }
    ]
    assert (
        RF.reachability_profile_feature_refusal(
            RF.reached_intrinsic_symbols(functions),
            profile_name="micro",
            profile_features=frozenset(),
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
            RF.reached_intrinsic_symbols(functions),
            profile_name="micro",
            profile_features=frozenset(),
        )
        is None
    )


@pytest.mark.parametrize("target_triple", ["wasm32-wasip1", "x86_64-unknown-linux-gnu"])
@pytest.mark.parametrize("exclude_features", [True, False])
def test_refusal_reports_complete_independent_target_and_profile_requirements(
    target_triple: str, exclude_features: bool
) -> None:
    # Explicit manifest-backed requirements exceed both former display limits:
    # eight target symbols and six symbols per excluded feature. Expectations
    # come from these retained references, not from the requirement scanner.
    sqlite_symbols = (
        "molt_sqlite3_close",
        "molt_sqlite3_commit",
        "molt_sqlite3_connect",
        "molt_sqlite3_cursor",
        "molt_sqlite3_cursor_close",
        "molt_sqlite3_cursor_drop",
        "molt_sqlite3_execute",
        "molt_sqlite3_executemany",
        "molt_sqlite3_executescript",
        "molt_sqlite3_fetchall",
    )
    feature_symbols = {
        "sqlite": sqlite_symbols,
        "stdlib_ast": ("molt_ast_parse",),
        "stdlib_regex": (
            "molt_re_compile",
            "molt_re_escape",
            "molt_re_execute",
            "molt_re_expand_replacement",
            "molt_re_finditer_next",
            "molt_re_group_values",
            "molt_re_scanner_new",
        ),
    }
    symbols = (*sqlite_symbols, *feature_symbols["stdlib_regex"])
    functions = [
        {"name": "molt_main", "ops": [_call("retained")]},
        {
            "name": "retained",
            "ops": [
                _builtin_func(f"v{index}", symbol)
                for index, symbol in enumerate(reversed(symbols))
            ]
            + [_const_str("duplicate", sqlite_symbols[0])],
        },
        {"name": "registry_root", "ops": [_const_str("ast", "molt_ast_parse")]},
        {"name": "dead", "ops": [_builtin_func("zlib", "molt_zlib_compress")]},
    ]
    profile_features = frozenset() if exclude_features else frozenset(feature_symbols)

    def refusal(ordered_functions: Sequence[Mapping[str, object]]) -> str | None:
        return RF.reachability_profile_feature_refusal(
            RF.reached_intrinsic_symbols(
                ordered_functions, extra_roots=iter(("registry_root",))
            ),
            profile_name="selected",
            profile_features=profile_features,
            target_triple=target_triple,
        )

    message = refusal(functions)
    assert (
        refusal(
            [
                {**function, "ops": list(reversed(function["ops"]))}
                for function in (functions[0], *reversed(functions[1:]))
            ]
        )
        == message
    )
    unavailable_target = target_triple == "wasm32-wasip1"
    if not (unavailable_target or exclude_features):
        assert message is None
        return

    assert message is not None
    assert "..." not in message
    assert "molt_zlib_compress" not in message
    assert ("has no runtime provider" in message) == unavailable_target
    if unavailable_target:
        target_section = message.split("\n\n", 1)[0]
        assert re.findall(r"\bmolt_[a-z0-9_]+\b", target_section) == list(
            sqlite_symbols
        )
        assert "choose a supported target or remove the reached operation" in message

    assert ("Profile 'selected'" in message) == exclude_features
    if exclude_features:
        assert [line for line in message.splitlines() if ": reached " in line] == [
            f"  {feature}: reached "
            + ("intrinsic " if len(required) == 1 else "intrinsics ")
            + ", ".join(required)
            for feature, required in feature_symbols.items()
        ]
        assert "--stdlib-profile full" in message
        assert "MOLT_STDLIB_PROFILE=full" in message
        if unavailable_target:
            assert (
                "does not provide symbols unavailable on the selected target" in message
            )


# ---------------------------------------------------------------------------
# Source-level agreement with the Rust dead-function authority
# ---------------------------------------------------------------------------


def _rust_source(relative: str) -> str:
    root = cli_compiler_metadata._compiler_root()
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
        "molt_re_scanner_new",
        "molt_re_finditer_next",
        "molt_re_escape",
    ):
        assert link_affecting_feature_gate_for_symbol(symbol) == "stdlib_regex"


@pytest.mark.parametrize(
    "name",
    [
        "asyncio__run",
        "asyncio___debug__flag",
        "asyncio__debug__flag",
        "molt_init_asyncio",
        "molt_init_asyncio__debug",
        "molt_init_",
        "json__decoder__scan",
        "jsonx__dump",
        "__private",
        "plain",
        "a___b",
    ],
)
def test_any_module_symbol_match_equals_one_test_per_symbol(name: str) -> None:
    # The per-symbol prefix test is the oracle for the O(len(name)) form.
    for symbols in (
        frozenset({"asyncio", "asyncio__debug"}),
        frozenset({"json", "json__decoder"}),
        frozenset({"a_", "a"}),
        frozenset({""}),
        frozenset(),
    ):
        expected = any(
            FR.emitted_name_matches_module_symbol(name, symbol) for symbol in symbols
        )
        assert FR.emitted_name_matches_any_module_symbol(name, symbols) is expected


def test_a_published_gpu_descriptor_body_is_a_reference() -> None:
    import json

    body = {"name": "kernel_body", "ops": [{"kind": "ret_void"}]}
    descriptor = json.dumps({"python_bodies": {"kernel": {"symbol": "kernel_body"}}})
    constant = {"kind": "const_str", "out": "descriptor", "s_value": descriptor}
    publication = {
        "kind": "call",
        "s_value": "molt_gpu_kernel_descriptor_set",
        "args": ["kernel", "descriptor"],
    }
    defined = frozenset({"kernel_body"})
    assert FR.function_references(
        {"name": "kernel", "ops": [constant, publication]}, defined
    ) == {"kernel_body"}
    # The same constant without its publication call names no body.
    assert FR.function_references({"name": "kernel", "ops": [constant]}, defined) == (
        frozenset()
    )
    assert body["name"] in defined

"""Reachability-driven refusal for feature-gated runtime callables.

The build must refuse a selected stdlib profile only when the finalized,
reachable backend IR links a runtime callable whose defining feature is absent
from that profile's target-specific Cargo feature ceiling. Importing or staging
a source module is not link authority.
"""

from __future__ import annotations

import tomllib

import molt.cli as cli
import pytest
from molt.cli import backend_ir as BACKEND_IR
from molt.cli import required_features as RF
from molt.cli import runtime_features as RUNTIME_FEATURES
from molt._runtime_feature_gates import (
    LINK_AFFECTING_FEATURES,
    feature_gate_for_symbol,
    link_affecting_feature_gate_for_symbol,
)


MOLT_ROOT = cli._compiler_root()


def _micro_features() -> frozenset[str]:
    return frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            "micro", target_triple=None
        )
    )


def _full_features() -> frozenset[str]:
    return frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            "full", target_triple=None
        )
    )


def _builtin_func(out: str, symbol: str, arity: int = 1) -> dict[str, object]:
    return {"kind": "builtin_func", "s_value": symbol, "value": arity, "out": out}


def _functions_reaching(*symbols: str) -> list[dict[str, object]]:
    return [
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                _builtin_func(f"v{index}", symbol)
                for index, symbol in enumerate(symbols)
            ],
        }
    ]


def _refusal_for_reached_symbols(
    *symbols: str,
    profile: str,
    target_triple: str | None,
) -> str | None:
    profile_features = frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            profile, target_triple=target_triple
        )
    )
    return RF.reachability_profile_feature_refusal(
        _functions_reaching(*symbols),
        profile_name=profile,
        profile_features=profile_features,
        target_triple=target_triple,
    )


# --- Cargo ladder/profile ceiling -----------------------------------------


def test_micro_profile_excludes_stdlib_ast() -> None:
    assert "stdlib_ast" not in _micro_features()


def test_full_profile_includes_stdlib_ast() -> None:
    assert "stdlib_ast" in _full_features()


def test_full_profile_includes_sqlite() -> None:
    assert "sqlite" in _full_features()


def test_full_profile_links_gpu_primitives_claimed_by_tinygrad_profile() -> None:
    cargo = tomllib.loads((MOLT_ROOT / "runtime/molt-runtime/Cargo.toml").read_text())

    assert "molt_gpu_primitives" in _full_features()
    assert "molt_gpu_primitives" in cargo["features"]["stdlib_full"]


def test_full_profile_includes_stdlib_stringprep() -> None:
    assert "stdlib_stringprep" in _full_features()


def test_full_profile_includes_stdlib_text() -> None:
    assert "stdlib_text" in _full_features()


def test_full_profile_includes_stdlib_zoneinfo() -> None:
    assert "stdlib_zoneinfo" in _full_features()


# --- Generated symbol -> link-affecting feature gates ----------------------


def test_representative_symbols_map_to_link_affecting_features() -> None:
    cases = {
        "molt_ast_parse": "stdlib_ast",
        "molt_hash_new": "stdlib_crypto",
        "molt_sqlite3_connect": "sqlite",
        "molt_stringprep_in_table": "stdlib_stringprep",
        "molt_html_escape": "stdlib_text",
        "molt_unicodedata_name": "stdlib_text",
        "molt_zoneinfo_new": "stdlib_zoneinfo",
        "molt_gpu_prim_device": "molt_gpu_primitives",
        "molt_base64_b64encode": "stdlib_serial",
        "molt_email_message_new": "stdlib_email",
        "molt_colorsys_rgb_to_hls": "stdlib_math",
        "molt_difflib_sequence_matcher_new": "stdlib_difflib",
        "molt_xml_element_new": "stdlib_xml",
        "molt_ipaddress_ip_address": "stdlib_ipaddress",
    }
    for symbol, feature in cases.items():
        assert link_affecting_feature_gate_for_symbol(symbol) == feature


def test_ungated_ssl_abi_is_never_link_refused() -> None:
    # ssl keeps a deliberately always-linkable ABI; even if asyncio imports it
    # eagerly, no ssl symbol maps to a link-affecting feature.
    assert link_affecting_feature_gate_for_symbol("molt_ssl_context_new") is None


def test_importlib_extra_is_resolver_only_not_link_affecting() -> None:
    # importlib.machinery uses molt_importlib_resources_* resolver arms, but
    # those symbols are compiled unconditionally. The raw resolver-arm gate may
    # name stdlib_importlib_extra; the link-affecting predicate must not.
    assert "stdlib_importlib_extra" not in LINK_AFFECTING_FEATURES
    sym = "molt_importlib_resources_reader_contents_from_roots"
    assert feature_gate_for_symbol(sym) == "stdlib_importlib_extra"
    assert link_affecting_feature_gate_for_symbol(sym) is None


def test_empty_cargo_group_features_are_resolver_only() -> None:
    resolver_only = {
        "stdlib_logging",
        "stdlib_concurrent",
        "stdlib_dbm",
        "stdlib_importlib_extra",
        "stdlib_signal",
        "stdlib_select",
    }
    assert resolver_only.isdisjoint(LINK_AFFECTING_FEATURES)


# --- Reachability-owned refusal -------------------------------------------


def test_micro_profile_refuses_reached_ast_callable_loudly() -> None:
    message = _refusal_for_reached_symbols(
        "molt_ast_parse", profile="micro", target_triple=None
    )
    assert message is not None
    assert "stdlib_ast" in message
    assert "--stdlib-profile full" in message
    assert (
        _refusal_for_reached_symbols(
            "molt_ast_parse", profile="full", target_triple=None
        )
        is None
    )
    assert message is not None
    assert "stdlib_ast" in message
    assert "molt_ast_parse" in message
    assert "'micro'" in message
    assert "--stdlib-profile full" in message
    assert "MOLT_STDLIB_PROFILE=full" in message


def test_full_profile_allows_reached_ast_callable() -> None:
    assert (
        _refusal_for_reached_symbols(
            "molt_ast_parse", profile="full", target_triple=None
        )
        is None
    )


def test_refusal_groups_reached_symbols_by_feature_not_module() -> None:
    message = _refusal_for_reached_symbols(
        "molt_ast_parse",
        "molt_hash_new",
        profile="micro",
        target_triple=None,
    )
    assert message is not None
    assert "stdlib_ast" in message
    assert "molt_ast_parse" in message
    assert "stdlib_crypto" in message
    assert "molt_hash_new" in message
    assert "import graph requires" not in message
    assert "required by module" not in message


def test_core_intrinsic_reachability_is_unaffected() -> None:
    assert (
        _refusal_for_reached_symbols(
            "molt_stdlib_probe", profile="micro", target_triple=None
        )
        is None
    )


def test_wasm_micro_uses_wasm_feature_surface_and_refuses_ast() -> None:
    message = _refusal_for_reached_symbols(
        "molt_ast_parse", profile="micro", target_triple="wasm32-wasip1"
    )
    assert message is not None
    assert "stdlib_ast" in message


def test_wasm_full_excludes_sqlite_and_refuses_reached_sqlite() -> None:
    wasm_full = frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            "full", target_triple="wasm32-wasip1"
        )
    )
    assert "sqlite" not in wasm_full
    message = _refusal_for_reached_symbols(
        "molt_sqlite3_connect", profile="full", target_triple="wasm32-wasip1"
    )
    assert message is not None
    assert "has no runtime provider" in message
    assert "wasm32" in message
    assert "molt_sqlite3_connect" in message


def test_wasm_micro_excludes_crypto_so_reached_hashlib_refuses() -> None:
    wasm_micro = frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            "micro", target_triple="wasm32-wasip1"
        )
    )
    assert "stdlib_crypto" not in wasm_micro
    assert "stdlib_compression" not in wasm_micro
    assert "stdlib_archive" not in wasm_micro
    message = _refusal_for_reached_symbols(
        "molt_hash_new", profile="micro", target_triple="wasm32-wasip1"
    )
    assert message is not None
    assert "stdlib_crypto" in message
    assert "molt_hash_new" in message


# --- Phase 0: profile feature sets read the Cargo ladder, not a Python mirror -
#
# ``runtime_features.profile_link_features`` resolves "what link-affecting +
# builtin features does profile P provide" by transitively expanding the Cargo
# ``[features]`` chain (micro -> stdlib_micro ... full -> stdlib_full), replacing
# the hand-maintained ``_ALL_DOMAIN_FEATURES`` flat list that drifted from the
# Cargo chain. These guards turn that class into a CI failure.

_PREVIOUSLY_DRIFTED_FULL_FEATURES = frozenset(
    {
        "stdlib_regex",
        "stdlib_itertools",
        "stdlib_path",
        "stdlib_difflib",
        "stdlib_xml",
        "stdlib_ipaddress",
    }
)


def _cargo_feature_graph() -> dict[str, list[str]]:
    cargo = tomllib.loads(
        (MOLT_ROOT / "runtime" / "molt-runtime" / "Cargo.toml").read_text()
    )
    return {
        name: list(entries)
        for name, entries in cargo["features"].items()
        if isinstance(entries, list)
    }


def _independent_cargo_expansion(seed: str) -> frozenset[str]:
    graph = _cargo_feature_graph()
    reached: set[str] = set()
    stack = [seed]
    while stack:
        current = stack.pop()
        if current in reached:
            continue
        reached.add(current)
        for entry in graph.get(current, []):
            if entry.startswith("dep:") or "/" in entry:
                continue
            stack.append(entry)
    reached.discard(seed)
    return frozenset(reached)


_LADDER_PROFILE_TO_CARGO = {
    "micro": "stdlib_micro",
    "edge": "stdlib_edge",
    "standard": "stdlib_standard",
    "server": "stdlib_server",
    "full": "stdlib_full",
}


def test_profile_link_features_full_includes_dep_backed_leaf_features() -> None:
    full = RUNTIME_FEATURES.profile_link_features("full", target_triple=None)
    assert _PREVIOUSLY_DRIFTED_FULL_FEATURES <= full, (
        "profile_link_features('full') is missing Cargo-linked features: "
        f"{sorted(_PREVIOUSLY_DRIFTED_FULL_FEATURES - full)}"
    )


def test_profile_link_features_matches_cargo_chain_for_every_ladder_tier() -> None:
    for profile, cargo_feature in _LADDER_PROFILE_TO_CARGO.items():
        derived = RUNTIME_FEATURES.profile_link_features(profile, target_triple=None)
        expected = _independent_cargo_expansion(cargo_feature)
        assert derived == expected, (
            f"profile_link_features({profile!r}) diverged from Cargo "
            f"{cargo_feature!r} chain: "
            f"only-in-derived={sorted(derived - expected)} "
            f"only-in-cargo={sorted(expected - derived)}"
        )


def test_auto_runtime_profile_selects_smallest_sufficient_ladder_tier() -> None:
    assert (
        RUNTIME_FEATURES.runtime_stdlib_profile_for_required_features(
            "auto",
            frozenset(),
            target_triple=None,
        )
        == "micro"
    )
    assert (
        RUNTIME_FEATURES.runtime_stdlib_profile_for_required_features(
            "auto",
            frozenset({"stdlib_regex"}),
            target_triple=None,
        )
        == "edge"
    )
    assert (
        RUNTIME_FEATURES.runtime_stdlib_profile_for_required_features(
            "micro",
            frozenset({"stdlib_regex"}),
            target_triple=None,
        )
        == "micro"
    )


def test_wasm_runtime_feature_plan_requires_concrete_runtime_tier() -> None:
    import pytest

    with pytest.raises(ValueError, match="must be concrete"):
        RUNTIME_FEATURES._wasm_runtime_feature_plan(
            stdlib_profile="auto",
            runtime_features=(),
            builtin_features=(),
            resolved_modules=frozenset(),
            required_link_features=frozenset(),
        )


def test_profile_link_features_rejects_unknown_profile() -> None:
    import pytest

    with pytest.raises(ValueError):
        RUNTIME_FEATURES.profile_link_features("nonsense", target_triple=None)


def test_full_features_superset_includes_previously_drifted_features() -> None:
    assert _PREVIOUSLY_DRIFTED_FULL_FEATURES <= _full_features()


def test_native_feature_sets_unchanged_after_cargo_migration() -> None:
    builtin = {
        "builtin_set",
        "builtin_memoryview",
        "builtin_complex",
        "builtin_contextvars",
        "builtin_fcntl",
    }
    micro_base = {
        "stdlib_asyncio",
        "stdlib_collections",
        "stdlib_fs_extra",
        "stdlib_logging",
        "stdlib_logging_ext",
    }
    old_domain = {
        "stdlib_tk",
        "stdlib_net",
        "stdlib_asyncio",
        "stdlib_email",
        "stdlib_decimal",
        "stdlib_logging",
        "stdlib_logging_ext",
        "stdlib_concurrent",
        "stdlib_dbm",
        "stdlib_importlib_extra",
        "stdlib_csv",
        "stdlib_signal",
        "stdlib_select",
        "stdlib_text",
        "stdlib_zoneinfo",
        "stdlib_crypto",
        "stdlib_compression",
        "stdlib_math",
        "stdlib_serialization",
        "stdlib_serial",
        "stdlib_archive",
        "stdlib_ast",
        "stdlib_unicode_names",
        "stdlib_stringprep",
        "stdlib_fs_extra",
        "sqlite",
        "molt_gpu_primitives",
    }
    assert _micro_features() == builtin | micro_base
    old_full = builtin | old_domain | micro_base
    assert old_full <= _full_features()


def test_wasm_feature_surface_matches_cargo_ladder_after_migration() -> None:
    def wasm(profile: str) -> frozenset[str]:
        return frozenset(
            RUNTIME_FEATURES._runtime_builtin_features_for_profile(
                profile, target_triple="wasm32-wasip1"
            )
        )

    builtin = set(RUNTIME_FEATURES._ALL_BUILTIN_FEATURES)
    micro_base = set(RUNTIME_FEATURES._MICRO_BASE_RUNTIME_FEATURES)
    assert wasm("micro") == builtin | micro_base
    assert "stdlib_crypto" not in wasm("micro")
    assert "stdlib_compression" not in wasm("micro")
    assert "stdlib_archive" not in wasm("micro")
    assert wasm("full") == builtin | RUNTIME_FEATURES.profile_link_features(
        "full",
        target_triple="wasm32-wasip1",
    )


def test_full_remedy_for_reached_ast_refusal_is_truthful() -> None:
    assert "stdlib_ast" in _full_features()
    message = _refusal_for_reached_symbols(
        "molt_ast_parse", profile="micro", target_triple=None
    )


@pytest.mark.parametrize(
    "target_triple",
    ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"),
)
def test_backend_reachability_uses_resolved_cross_native_target_triple(
    monkeypatch: pytest.MonkeyPatch,
    target_triple: str,
) -> None:
    observed: list[tuple[str, str | None]] = []

    def profile_features(
        _profile: str, *, target_triple: str | None
    ) -> tuple[str, ...]:
        observed.append(("profile", target_triple))
        return ()

    def refusal(
        _functions: object,
        *,
        profile_name: str,
        profile_features: frozenset[str],
        target_triple: str | None,
    ) -> None:
        del profile_name, profile_features
        observed.append(("refusal", target_triple))

    monkeypatch.setattr(
        BACKEND_IR._runtime_features,
        "_runtime_builtin_features_for_profile",
        profile_features,
    )
    monkeypatch.setattr(
        BACKEND_IR._required_features,
        "reachability_profile_feature_refusal",
        refusal,
    )

    assert (
        BACKEND_IR._reachability_feature_refusal(
            {"functions": []},
            stdlib_profile="full",
            target="native",
            target_triple=target_triple,
        )
        is None
    )
    assert observed == [("profile", target_triple), ("refusal", target_triple)]

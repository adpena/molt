"""Reachability-driven refusal for feature-gated runtime callables.

The build must refuse a selected stdlib profile only when the finalized,
reachable backend IR links a runtime callable whose defining feature is absent
from that profile's target-specific Cargo feature ceiling. Importing or staging
a source module is not link authority.
"""

from __future__ import annotations

from molt.compilation_target import admit_target

import tomllib

import pytest
from molt.cli import backend_ir as BACKEND_IR
from molt.cli import required_features as RF
from molt.cli import runtime_features as RUNTIME_FEATURES
from molt._runtime_feature_gates import RUNTIME_FEATURES as DECLARED_FEATURES
from molt._runtime_feature_gates import feature_for_symbol
from molt.cli import compiler_metadata as cli_compiler_metadata


MOLT_ROOT = cli_compiler_metadata._compiler_root()


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
        RF.reached_intrinsic_symbols(_functions_reaching(*symbols)),
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
    cargo = tomllib.loads(
        (MOLT_ROOT / "runtime/molt-runtime/Cargo.toml").read_text(encoding="utf-8")
    )

    assert "molt_gpu_primitives" in _full_features()
    assert "molt_gpu_primitives" in cargo["features"]["stdlib_full"]


def test_full_profile_includes_stdlib_stringprep() -> None:
    assert "stdlib_stringprep" in _full_features()


def test_full_profile_includes_stdlib_text() -> None:
    assert "stdlib_text" in _full_features()


def test_full_profile_includes_stdlib_zoneinfo() -> None:
    assert "stdlib_zoneinfo" in _full_features()


# --- Generated symbol -> declared feature ---------------------------------


def test_representative_symbols_map_to_their_declared_features() -> None:
    cases = {
        "molt_ast_parse": "stdlib_ast",
        "molt_hash_new": "stdlib_crypto",
        "molt_sqlite3_connect": "sqlite",
        "molt_stringprep_in_table": "stdlib_stringprep",
        "molt_html_escape": "stdlib_text",
        "molt_unicodedata_name": "stdlib_unicode_names",
        "molt_unicodedata_lookup": "stdlib_unicode_names",
        "molt_zoneinfo_new": "stdlib_zoneinfo",
        "molt_gpu_prim_device": "molt_gpu_primitives",
        "molt_base64_b64encode": "stdlib_serial",
        "molt_email_message_new": "stdlib_email",
        "molt_math_sqrt": "stdlib_math",
        "molt_xml_fromstring": "stdlib_xml",
        "molt_ipaddress_v4_int": "stdlib_ipaddress",
        "molt_socketpair": "stdlib_net",
        "molt_select_select": "stdlib_select",
    }
    for symbol, feature in cases.items():
        assert feature_for_symbol(symbol) == feature


@pytest.mark.parametrize("target_triple", [None, "wasm32-wasip1"])
@pytest.mark.parametrize("symbol", ["molt_unicodedata_name", "molt_unicodedata_lookup"])
def test_unicode_names_demand_owns_auto_profile_and_explicit_ceiling(
    target_triple: str | None, symbol: str
) -> None:
    reached = RF.reached_intrinsic_symbols(_functions_reaching(symbol))
    required = RF.required_link_features(reached)
    assert required == frozenset({"stdlib_unicode_names"})
    assert (
        RUNTIME_FEATURES.runtime_stdlib_profile_for_required_features(
            "auto", required, target_triple=target_triple
        )
        == "full"
    )
    refusal = _refusal_for_reached_symbols(
        symbol, profile="standard", target_triple=target_triple
    )
    assert (
        refusal is not None and symbol in refusal and "stdlib_unicode_names" in refusal
    )
    assert (
        _refusal_for_reached_symbols(
            symbol, profile="full", target_triple=target_triple
        )
        is None
    )
    # Property-only and empty closures do not acquire the public names feature.
    property_required = RF.required_link_features({"molt_unicodedata_category"})
    assert property_required == frozenset({"stdlib_text"})
    assert (
        RUNTIME_FEATURES.runtime_stdlib_profile_for_required_features(
            "auto", property_required, target_triple=target_triple
        )
        == "standard"
    )
    assert (
        RUNTIME_FEATURES.runtime_stdlib_profile_for_required_features(
            "auto", (), target_triple=target_triple
        )
        == "micro"
    )


def test_compile_is_a_core_builtin() -> None:
    # The parser is linked into every runtime for traceback ranges, and the
    # import system (zipimport) retains compile(); gating it moved every such
    # program to the full tier. Only the ast module needs stdlib_ast.
    assert feature_for_symbol("molt_compile_builtin") is None
    assert feature_for_symbol("molt_ast_parse") == "stdlib_ast"


def test_ssl_context_abi_is_core_and_socket_operations_are_networking() -> None:
    # asyncio imports ssl eagerly; contexts and constants need no socket, so
    # only the socket operations require the networking provider.
    assert feature_for_symbol("molt_ssl_context_new") is None
    assert feature_for_symbol("molt_ssl_wrap_socket") == "stdlib_net"


def test_import_system_resources_are_always_defined() -> None:
    # importlib.machinery binds its resource-reader intrinsics eagerly, so they
    # belong to the import system, not to an optional feature.
    assert (
        feature_for_symbol("molt_importlib_resources_reader_contents_from_roots")
        is None
    )
    assert "stdlib_importlib_extra" not in DECLARED_FEATURES


def test_no_feature_only_hides_resolver_arms() -> None:
    # A resolver-only feature hid defined symbols from the resolver, so a
    # reached intrinsic failed at run time. Every declared feature now removes
    # its definitions (the generated absence probes prove it), and the empty
    # groups that gated nothing are gone.
    cargo = _cargo_feature_graph()
    for retired in (
        "stdlib_logging",
        "stdlib_concurrent",
        "stdlib_importlib_extra",
        "stdlib_signal",
    ):
        assert retired not in cargo
        assert retired not in DECLARED_FEATURES
    assert DECLARED_FEATURES <= set(cargo)


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


def _socket_module_reaching_socketpair() -> list[dict[str, object]]:
    # socket.py binds its intrinsics by name, as `require_intrinsic` lowers.
    return [
        {
            "name": "molt_main",
            "params": [],
            "ops": [{"kind": "const_str", "s_value": "molt_socketpair", "out": "v0"}],
        }
    ]


def test_explicit_profile_refuses_reached_networking_at_build_time() -> None:
    # HF-487: a micro runtime linked a stub that raised "networking not
    # available" when the program ran. Now the build refuses and names the
    # symbol, the feature and the smallest tier that provides it.
    message = BACKEND_IR._reachability_feature_refusal(
        RF.reached_intrinsic_symbols(_socket_module_reaching_socketpair()),
        stdlib_profile="micro",
        target=admit_target("native"),
    )
    assert message is not None
    assert "stdlib_net: reached intrinsic molt_socketpair" in message
    assert "--stdlib-profile server" in message
    assert "MOLT_STDLIB_PROFILE=server" in message


@pytest.mark.parametrize("target_triple", [None, "wasm32-wasip1"])
def test_auto_selects_the_networking_tier(target_triple: str | None) -> None:
    reached = RF.reached_intrinsic_symbols(_socket_module_reaching_socketpair())
    required = RF.required_link_features(reached)
    assert required == {"stdlib_net"}
    assert (
        RUNTIME_FEATURES.runtime_stdlib_profile_for_required_features(
            "auto", required, target_triple=target_triple
        )
        == "server"
    )
    assert (
        _refusal_for_reached_symbols(
            "molt_socketpair", profile="auto", target_triple=target_triple
        )
        is None
    )


def test_target_without_a_networking_provider_refuses_at_build_time() -> None:
    message = _refusal_for_reached_symbols(
        "molt_socketpair", profile="full", target_triple="x86_64-pc-windows-msvc"
    )
    assert message is not None
    assert "has no runtime provider" in message
    assert "molt_socketpair (unsupported on target OS windows)" in message


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
            "molt_capabilities_has", profile="micro", target_triple=None
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


@pytest.mark.parametrize(
    "target_triple,available",
    [
        ("x86_64-pc-windows-msvc", True),
        ("x86_64-pc-windows-gnu", True),
        ("aarch64-pc-windows-msvc", True),
        ("x86_64-unknown-linux-gnu", False),
        ("aarch64-apple-darwin", False),
        ("wasm32-wasip1", False),
        ("wasm32-unknown-unknown", False),
    ],
)
def test_reached_windows_locale_provider_uses_explicit_target(
    target_triple: str, available: bool, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import native_target

    # A Windows host must not grant its API to a cross-target, and a Linux
    # host must not hide the API for a real Windows cross-target.
    monkeypatch.setattr(
        native_target,
        "_host_target_triple",
        lambda **_kwargs: (
            "x86_64-unknown-linux-gnu" if available else "x86_64-pc-windows-msvc"
        ),
    )
    message = _refusal_for_reached_symbols(
        "molt_locale_getdefaultlocale", profile="full", target_triple=target_triple
    )
    if available:
        assert message is None
    else:
        assert message is not None
        assert "molt_locale_getdefaultlocale" in message
        assert "requires target OS windows" in message


@pytest.mark.parametrize(
    "host_triple,available",
    [
        ("x86_64-pc-windows-msvc", True),
        ("aarch64-apple-darwin", False),
        ("aarch64-unknown-linux-gnu", False),
    ],
)
def test_reached_windows_locale_provider_resolves_implicit_native_host(
    host_triple: str, available: bool, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import native_target

    calls = []

    def host_target(**_kwargs):
        calls.append(host_triple)
        return host_triple

    monkeypatch.setattr(native_target, "_host_target_triple", host_target)
    message = _refusal_for_reached_symbols(
        "molt_locale_getdefaultlocale", profile="full", target_triple=None
    )
    assert (message is None) is available
    assert calls == [host_triple]


def test_unreached_windows_locale_provider_needs_no_target_os_resolution(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import native_target

    def unexpected_resolution(*_args, **_kwargs):
        pytest.fail(
            "unrestricted/rejected dead functions must not resolve a host target"
        )

    monkeypatch.setattr(
        native_target, "resolve_native_target_spec", unexpected_resolution
    )
    functions = _functions_reaching("molt_locale_getencoding")
    functions.append(
        {
            "name": "unused_windows_api",
            "params": [],
            "ops": [_builtin_func("windows_only", "molt_locale_getdefaultlocale")],
        }
    )
    assert (
        RF.reachability_profile_feature_refusal(
            RF.reached_intrinsic_symbols(functions),
            profile_name="full",
            profile_features=_full_features(),
        )
        is None
    )


def test_all_target_and_feature_requirements_include_windows_locale() -> None:
    message = _refusal_for_reached_symbols(
        "molt_locale_getdefaultlocale",
        "molt_sqlite3_connect",
        "molt_hash_new",
        profile="micro",
        target_triple="wasm32-wasip1",
    )
    assert message is not None
    for required in (
        "molt_locale_getdefaultlocale",
        "molt_sqlite3_connect",
        "molt_hash_new",
    ):
        assert required in message
    assert "requires target OS windows" in message
    assert "unsupported on wasm32" in message
    assert "stdlib_crypto: reached intrinsic molt_hash_new" in message


@pytest.mark.parametrize("target", ["wasm", "wasm-freestanding", "wasm32-wasip1"])
def test_backend_refusal_combines_missing_target_and_profile_requirements(
    target: str,
) -> None:
    message = BACKEND_IR._reachability_feature_refusal(
        RF.reached_intrinsic_symbols(
            _functions_reaching(
                "molt_sqlite3_connect", "molt_hash_new", "molt_ast_parse"
            )
        ),
        stdlib_profile="micro",
        target=admit_target(target),
    )

    assert message is not None
    assert "has no runtime provider" in message
    assert "wasm32" in message
    assert [line for line in message.splitlines() if ": reached " in line] == [
        "  sqlite: reached intrinsic molt_sqlite3_connect",
        "  stdlib_ast: reached intrinsic molt_ast_parse",
        "  stdlib_crypto: reached intrinsic molt_hash_new",
    ]
    assert "--stdlib-profile full" in message
    assert "MOLT_STDLIB_PROFILE=full" in message
    assert "does not provide symbols unavailable on the selected target" in message


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
        "stdlib_xml",
        "stdlib_ipaddress",
    }
)


def _cargo_feature_graph() -> dict[str, list[str]]:
    cargo = tomllib.loads(
        (MOLT_ROOT / "runtime" / "molt-runtime" / "Cargo.toml").read_text(
            encoding="utf-8"
        )
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
        "builtin_fcntl",
    }
    micro_base = {
        "stdlib_asyncio",
        "stdlib_collections",
        "stdlib_fs_extra",
        "stdlib_logging_ext",
    }
    old_domain = {
        "stdlib_tk",
        "stdlib_net",
        "stdlib_asyncio",
        "stdlib_email",
        "stdlib_decimal",
        "stdlib_logging_ext",
        "stdlib_dbm",
        "stdlib_csv",
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
    # The refusal names the excluded feature and a remedy that really provides it.
    assert message is not None
    assert "'stdlib_ast'" in message
    assert "--stdlib-profile full" in message


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
        reached_symbols: frozenset[str],
        *,
        profile_name: str,
        profile_features: frozenset[str],
        target_triple: str | None,
    ) -> None:
        del profile_name, profile_features
        assert reached_symbols == frozenset()
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
            frozenset(),
            stdlib_profile="full",
            target=admit_target("native"),
            target_triple=target_triple,
        )
        is None
    )
    assert observed == [("profile", target_triple), ("refusal", target_triple)]


def test_enabled_runtime_features_follow_the_cargo_feature_graph() -> None:
    cargo = tomllib.loads(
        (MOLT_ROOT / "runtime" / "molt-runtime" / "Cargo.toml").read_text(
            encoding="utf-8"
        )
    )["features"]
    micro = RUNTIME_FEATURES.runtime_enabled_cargo_features(
        ("no-default-features", "stdlib_micro")
    )
    # Every direct Cargo member of stdlib_micro is enabled; nothing from a
    # higher tier is, so a micro runtime's ABI cannot claim edge symbols.
    assert {"stdlib_micro", *(f for f in cargo["stdlib_micro"] if "/" not in f)} <= (
        micro
    )
    assert "stdlib_math" not in micro and "stdlib_decimal" not in micro
    defaults = RUNTIME_FEATURES.runtime_enabled_cargo_features(("default-features",))
    assert {"stdlib_full", "stdlib_micro", "stdlib_decimal"} <= defaults
    assert "default" not in defaults


@pytest.mark.parametrize(
    "request_features",
    [("stdlib_micro",), ("default-features", "no-default-features", "stdlib_micro")],
)
def test_enabled_runtime_features_require_one_default_marker(
    request_features: tuple[str, ...],
) -> None:
    with pytest.raises(ValueError, match="exactly one default-feature marker"):
        RUNTIME_FEATURES.runtime_enabled_cargo_features(request_features)


@pytest.mark.parametrize(
    ("requested", "expected_domains"),
    [
        (("stdlib_net",), {"stdlib_net", "stdlib_select"}),
        (
            ("stdlib_micro", "stdlib_net"),
            {"stdlib_asyncio", "stdlib_net", "stdlib_select"},
        ),
        (("stdlib_micro",), {"stdlib_asyncio"}),
        (("stdlib_asyncio",), {"stdlib_asyncio"}),
        (("stdlib_select",), {"stdlib_select"}),
    ],
)
def test_network_runtime_feature_request_includes_fd_readiness_provider(
    requested: tuple[str, ...], expected_domains: set[str]
) -> None:
    # Fd watchers are compiled with networking and call the select primitive.
    # Neither non-network scheduling nor select alone needs the network driver.
    enabled = RUNTIME_FEATURES.runtime_enabled_cargo_features(
        ("no-default-features", *requested)
    )
    assert enabled & {"stdlib_asyncio", "stdlib_net", "stdlib_select"} == (
        expected_domains
    )
    assert "stdlib_server" not in enabled and "stdlib_full" not in enabled

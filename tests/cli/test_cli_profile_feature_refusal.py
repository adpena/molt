"""Loud compile-time refusal for feature-gated stdlib modules (task #70).

A domain-feature-gated stdlib module on a profile that excludes its feature must
produce a LOUD, actionable compile-time refusal — never a raw undefined-symbol
linker error. These guards pin that doctrine against the REAL stdlib modules and
the REAL profile feature surface, so the refusal can never silently regress into
a link-time failure.
"""

from __future__ import annotations

from pathlib import Path
import tomllib

import molt.cli as cli
from molt.cli import module_stdlib_policy as cli_module_stdlib_policy
from molt.cli import runtime_features as RUNTIME_FEATURES
from molt._runtime_feature_gates import (
    LINK_AFFECTING_FEATURES,
    feature_gate_for_symbol,
    link_affecting_feature_gate_for_symbol,
)


MOLT_ROOT = cli._compiler_root()
STDLIB_ROOT = MOLT_ROOT / "src" / "molt" / "stdlib"


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


# --- the mapping itself: module -> required gating feature -----------------


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


def test_ast_module_requires_stdlib_ast_gate() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(STDLIB_ROOT / "ast.py", _micro_features())
    assert "stdlib_ast" in gap
    # The gap names the concrete intrinsics that would be undefined at link.
    assert any(sym.startswith("molt_ast_") for sym in gap["stdlib_ast"])


def test_ast_module_buildable_on_full_profile() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(STDLIB_ROOT / "ast.py", _full_features())
    assert gap == {}


def test_second_gated_module_hashlib_requires_stdlib_crypto() -> None:
    # Negative-control completeness: the refusal names the RIGHT feature for a
    # different gated module, proving it is data-driven, not ast-special-cased.
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "hashlib.py", _micro_features()
    )
    assert set(gap) == {"stdlib_crypto"}


def test_third_gated_module_sqlite_requires_sqlite_feature() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "_sqlite3.py", _micro_features()
    )
    assert set(gap) == {"sqlite"}


def test_sqlite_module_buildable_on_full_profile() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "_sqlite3.py", _full_features()
    )
    assert gap == {}


def test_stringprep_module_requires_stdlib_stringprep_gate() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "stringprep.py", _micro_features()
    )
    assert set(gap) == {"stdlib_stringprep"}
    assert any(sym.startswith("molt_stringprep_") for sym in gap["stdlib_stringprep"])


def test_stringprep_module_buildable_on_full_profile() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "stringprep.py", _full_features()
    )
    assert gap == {}


def test_html_module_requires_stdlib_text_gate() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "html" / "__init__.py", _micro_features()
    )
    assert set(gap) == {"stdlib_text"}
    assert any(sym.startswith("molt_html_") for sym in gap["stdlib_text"])


def test_unicodedata_module_requires_stdlib_text_gate() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "unicodedata.py", _micro_features()
    )
    assert set(gap) == {"stdlib_text"}
    assert any(sym.startswith("molt_unicodedata_") for sym in gap["stdlib_text"])


def test_text_modules_buildable_on_full_profile() -> None:
    assert (
        cli_module_stdlib_policy._profile_feature_gap_for_module(
            STDLIB_ROOT / "html" / "__init__.py", _full_features()
        )
        == {}
    )
    assert (
        cli_module_stdlib_policy._profile_feature_gap_for_module(
            STDLIB_ROOT / "unicodedata.py", _full_features()
        )
        == {}
    )


def test_zoneinfo_module_requires_stdlib_zoneinfo_gate() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "zoneinfo" / "__init__.py", _micro_features()
    )
    assert set(gap) == {"stdlib_zoneinfo"}
    assert any(sym.startswith("molt_zoneinfo_") for sym in gap["stdlib_zoneinfo"])


def test_zoneinfo_module_buildable_on_full_profile() -> None:
    assert (
        cli_module_stdlib_policy._profile_feature_gap_for_module(
            STDLIB_ROOT / "zoneinfo" / "__init__.py", _full_features()
        )
        == {}
    )


def test_tinygrad_package_requires_gpu_primitives_gate_on_micro_profile() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "tinygrad" / "__init__.py", _micro_features()
    )
    assert set(gap) == {"molt_gpu_primitives"}
    assert gap["molt_gpu_primitives"] == ["molt_gpu_prim_device"]


def test_tinygrad_package_buildable_on_full_profile() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "tinygrad" / "__init__.py", _full_features()
    )
    assert gap == {}


def test_ungated_ssl_abi_is_never_refused() -> None:
    # ssl keeps a deliberately always-linkable ABI (asyncio imports it eagerly
    # even on micro); importing it must NOT trigger a feature refusal.
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(STDLIB_ROOT / "ssl.py", _micro_features())
    assert gap == {}


# --- resolver-only features must NOT cause false refusals ------------------


def test_importlib_extra_is_resolver_only_not_link_affecting() -> None:
    # importlib.machinery requires molt_importlib_resources_* intrinsics whose
    # resolver arm is gated by stdlib_importlib_extra, but their
    # #[unsafe(no_mangle)] definitions are compiled unconditionally — the
    # symbols are ALWAYS in the archive. The resolver-arm gate is therefore NOT
    # a link gate, so importlib.machinery must build on micro.
    assert "stdlib_importlib_extra" not in LINK_AFFECTING_FEATURES
    # The raw resolver-arm gate still attributes the symbol to the feature ...
    sym = "molt_importlib_resources_reader_contents_from_roots"
    assert feature_gate_for_symbol(sym) == "stdlib_importlib_extra"
    # ... but the link-affecting predicate (the one the refusal uses) returns
    # None, so it is never refused.
    assert link_affecting_feature_gate_for_symbol(sym) is None
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "importlib" / "machinery.py", _micro_features()
    )
    assert gap == {}


def test_micro_build_importing_importlib_machinery_is_allowed() -> None:
    rc, message = _run_pass(
        [("importlib.machinery", STDLIB_ROOT / "importlib" / "machinery.py")],
        "micro",
        "native",
    )
    assert rc is None
    assert message is None


def test_empty_cargo_group_features_are_resolver_only() -> None:
    # The empty `[]` Cargo feature groups gate only resolver arms; none of them
    # may be classified link-affecting (doing so re-creates the false-positive
    # class this distinction exists to prevent).
    resolver_only = {
        "stdlib_logging",
        "stdlib_concurrent",
        "stdlib_dbm",
        "stdlib_importlib_extra",
        "stdlib_signal",
        "stdlib_select",
    }
    assert resolver_only.isdisjoint(LINK_AFFECTING_FEATURES)


def test_base64_module_requires_stdlib_serial_gate() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "base64.py", _micro_features()
    )
    assert set(gap) == {"stdlib_serial"}
    assert any(sym.startswith("molt_base64_") for sym in gap["stdlib_serial"])


def test_email_module_requires_stdlib_email_gate() -> None:
    gap = cli_module_stdlib_policy._profile_feature_gap_for_module(
        STDLIB_ROOT / "email" / "message.py", _micro_features()
    )
    assert set(gap) == {"stdlib_email"}
    assert any(sym.startswith("molt_email_") for sym in gap["stdlib_email"])


# --- the enforcement pass --------------------------------------------------


def _run_pass(module_paths, profile, target):
    captured: list[str] = []
    graph = {name: path for name, path in module_paths}
    orig_fail = cli_module_stdlib_policy._fail

    def fake_fail(message, json_output, code=2, command="molt"):
        captured.append(message)
        return code

    cli_module_stdlib_policy._fail = fake_fail  # type: ignore[assignment]
    try:
        rc = cli_module_stdlib_policy._enforce_profile_feature_availability(
            graph, STDLIB_ROOT, profile, target, json_output=False
        )
    finally:
        cli_module_stdlib_policy._fail = orig_fail  # type: ignore[assignment]
    return rc, (captured[0] if captured else None)


def test_micro_build_importing_ast_is_refused_loudly() -> None:
    rc, message = _run_pass([("ast", STDLIB_ROOT / "ast.py")], "micro", "native")
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_ast" in message
    assert "'micro'" in message
    # Actionable remedy with the repo's real knob names.
    assert "--stdlib-profile full" in message
    assert "MOLT_STDLIB_PROFILE=full" in message
    # Names the offending module.
    assert "ast" in message


def test_full_build_importing_ast_is_allowed() -> None:
    rc, message = _run_pass([("ast", STDLIB_ROOT / "ast.py")], "full", "native")
    assert rc is None
    assert message is None


def test_refusal_names_the_right_feature_for_a_second_module() -> None:
    rc, message = _run_pass(
        [("hashlib", STDLIB_ROOT / "hashlib.py")], "micro", "native"
    )
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_crypto" in message
    assert "stdlib_ast" not in message
    assert "hashlib" in message


def test_refusal_groups_multiple_blocked_modules() -> None:
    rc, message = _run_pass(
        [
            ("ast", STDLIB_ROOT / "ast.py"),
            ("hashlib", STDLIB_ROOT / "hashlib.py"),
        ],
        "micro",
        "native",
    )
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_ast" in message
    assert "stdlib_crypto" in message


def test_core_intrinsic_imports_are_unaffected() -> None:
    # A stdlib module backed only by core ungated intrinsics never refuses.
    rc, message = _run_pass(
        [("keyword", STDLIB_ROOT / "keyword.py")], "micro", "native"
    )
    assert rc is None
    assert message is None


def test_math_leaf_intrinsic_modules_refuse_on_micro_profile() -> None:
    rc, message = _run_pass(
        [("colorsys", STDLIB_ROOT / "colorsys.py")], "micro", "native"
    )
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_math" in message
    assert "colorsys" in message


def test_difflib_leaf_intrinsic_modules_refuse_on_micro_profile() -> None:
    rc, message = _run_pass(
        [("difflib", STDLIB_ROOT / "difflib.py")], "micro", "native"
    )
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_difflib" in message
    assert "difflib" in message


def test_ipaddress_leaf_intrinsic_modules_refuse_on_server_profile() -> None:
    rc, message = _run_pass(
        [("ipaddress", STDLIB_ROOT / "ipaddress.py")], "server", "native"
    )
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_ipaddress" in message
    assert "stdlib_net" not in message
    assert "ipaddress" in message


def test_xml_leaf_intrinsic_modules_refuse_on_micro_profile() -> None:
    rc, message = _run_pass(
        [("xml.etree.ElementTree", STDLIB_ROOT / "xml" / "etree" / "ElementTree.py")],
        "micro",
        "native",
    )
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_xml" in message
    assert "xml.etree.ElementTree" in message


def test_modules_outside_stdlib_root_are_ignored() -> None:
    # User modules (not under stdlib_root) are not feature-gated stdlib and must
    # be skipped even if their path does not resolve under the stdlib tree.
    rc, message = _run_pass(
        [("user_app", Path("/nonexistent/user_app.py"))], "micro", "native"
    )
    assert rc is None
    assert message is None


def test_wasm_micro_uses_wasm_feature_surface_and_refuses_ast() -> None:
    # The wasm micro surface excludes stdlib_ast (it is in the wasm-excluded
    # set), so `import ast` on a wasm micro build is also refused — and the
    # refusal computes the SAME feature surface the wasm staticlib links.
    rc, message = _run_pass([("ast", STDLIB_ROOT / "ast.py")], "micro", "wasm")
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_ast" in message


def test_wasm_micro_excludes_sqlite_and_refuses_sqlite3() -> None:
    wasm_micro = frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            "micro", target_triple="wasm32-wasip1"
        )
    )
    assert "sqlite" not in wasm_micro
    rc, message = _run_pass(
        [("_sqlite3", STDLIB_ROOT / "_sqlite3.py")], "micro", "wasm"
    )
    assert rc is not None and rc != 0
    assert message is not None
    assert "sqlite" in message
    assert "_sqlite3" in message


def test_wasm_full_excludes_sqlite_and_refuses_sqlite3() -> None:
    wasm_full = frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            "full", target_triple="wasm32-wasip1"
        )
    )
    assert "sqlite" not in wasm_full
    rc, message = _run_pass([("_sqlite3", STDLIB_ROOT / "_sqlite3.py")], "full", "wasm")
    assert rc is not None and rc != 0
    assert message is not None
    assert "sqlite" in message
    assert "_sqlite3" in message


def test_wasm_micro_includes_crypto_so_hashlib_is_allowed() -> None:
    # Unlike native micro, the wasm micro surface KEEPS stdlib_crypto (only
    # tk/net/ast/unicode_names are wasm-excluded), so hashlib must NOT be
    # refused on wasm — proving the refusal tracks the per-target feature set
    # rather than a single hardcoded exclusion list.
    wasm_micro = frozenset(
        RUNTIME_FEATURES._runtime_builtin_features_for_profile(
            "micro", target_triple="wasm32-wasip1"
        )
    )
    assert "stdlib_crypto" in wasm_micro
    rc, message = _run_pass([("hashlib", STDLIB_ROOT / "hashlib.py")], "micro", "wasm")
    assert rc is None
    assert message is None


# --- Phase 0: profile feature sets read the Cargo ladder, not a Python mirror -
#
# ``runtime_features.profile_link_features`` resolves "what link-affecting +
# builtin features does profile P provide" by transitively expanding the Cargo
# ``[features]`` chain (micro -> stdlib_micro ... full -> stdlib_full), replacing
# the hand-maintained ``_ALL_DOMAIN_FEATURES`` flat list that DRIFTED from the
# Cargo chain.  The old mirror omitted ``stdlib_regex``/``stdlib_itertools``/
# ``stdlib_path``/``stdlib_difflib``/``stdlib_xml``/``stdlib_ipaddress`` — all of
# which ``stdlib_full`` transitively links — so the Python "full" model could not
# even name the features its own archive builds.  These guards turn that
# "Python model drifts from Cargo ladder" class into a CI failure.

# The six dep-backed leaf-crate stdlib features the old ``_ALL_DOMAIN_FEATURES``
# mirror omitted but the Cargo ``stdlib_full`` chain transitively links.
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
    """Transitive feature-name closure of *seed*, recomputed from scratch.

    Deliberately a second, independent implementation of the expansion so the
    agreement test cross-checks ``profile_link_features`` against the raw
    Cargo.toml rather than against the same code under test.
    """
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
    # The headline Phase-0 assertion: the Cargo-derived "full" feature set names
    # every dep-backed leaf-crate stdlib feature the old mirror dropped.
    full = RUNTIME_FEATURES.profile_link_features("full", target_triple=None)
    assert _PREVIOUSLY_DRIFTED_FULL_FEATURES <= full, (
        "profile_link_features('full') is missing Cargo-linked features: "
        f"{sorted(_PREVIOUSLY_DRIFTED_FULL_FEATURES - full)}"
    )


def test_profile_link_features_matches_cargo_chain_for_every_ladder_tier() -> None:
    # The anti-drift gate: for EVERY ladder tier, the function's expansion must
    # equal the transitive Cargo chain recomputed independently from Cargo.toml.
    # A future Cargo edit that desyncs the Python view fails here.
    for profile, cargo_feature in _LADDER_PROFILE_TO_CARGO.items():
        derived = RUNTIME_FEATURES.profile_link_features(profile, target_triple=None)
        expected = _independent_cargo_expansion(cargo_feature)
        assert derived == expected, (
            f"profile_link_features({profile!r}) diverged from Cargo "
            f"{cargo_feature!r} chain: "
            f"only-in-derived={sorted(derived - expected)} "
            f"only-in-cargo={sorted(expected - derived)}"
        )


def test_profile_link_features_rejects_unknown_profile() -> None:
    # Fail loudly (not silently fall back) on a profile that has no ladder tier.
    import pytest

    with pytest.raises(ValueError):
        RUNTIME_FEATURES.profile_link_features("nonsense", target_triple=None)


def test_full_features_superset_includes_previously_drifted_features() -> None:
    # The composed builtin-feature set the gate/runtime_build consume must also
    # carry the previously-drifted features (not just the raw ladder helper).
    assert _PREVIOUSLY_DRIFTED_FULL_FEATURES <= _full_features()


def test_native_feature_sets_unchanged_after_cargo_migration() -> None:
    # No-regression (migration-safety invariant §6: Phase 0 only WIDENS what full
    # accepts; it never narrows micro and never drops a feature full already had).
    # micro/native is byte-identical to the documented pre-migration set, and
    # full/native is a strict SUPERSET of the documented pre-migration full set.
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


def test_wasm_feature_surface_unchanged_after_cargo_migration() -> None:
    # Phase 0b: the WASM availability surface is preserved verbatim (the native
    # ladder migrated to Cargo, the WASM path did not, to avoid narrowing the
    # WASM-micro surface and wrongly refusing working WASM builds).
    def wasm(profile: str) -> frozenset[str]:
        return frozenset(
            RUNTIME_FEATURES._runtime_builtin_features_for_profile(
                profile, target_triple="wasm32-wasip1"
            )
        )

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
    wasm_excluded = {
        "stdlib_tk",
        "stdlib_net",
        "stdlib_ast",
        "stdlib_unicode_names",
        "sqlite",
    }
    old_domain = set(RUNTIME_FEATURES._WASM_DOMAIN_AVAILABILITY_FEATURES)
    expected_wasm_micro = (builtin | old_domain | micro_base) - wasm_excluded
    assert wasm("micro") == expected_wasm_micro
    assert wasm("full") == set(RUNTIME_FEATURES._WASM_RUNTIME_FULL_FEATURES)


def test_full_build_refusing_genuinely_gated_module_names_truthful_feature() -> None:
    # The truthful-message property the drift used to break: `ast` (genuinely
    # link-affecting via stdlib_ast) is refused on micro and the remedy it points
    # to — full — now genuinely provides stdlib_ast (the Cargo-derived full set
    # contains it), so the "rebuild with --stdlib-profile full" message is no
    # longer a lie.
    assert "stdlib_ast" in _full_features()
    rc, message = _run_pass([("ast", STDLIB_ROOT / "ast.py")], "micro", "native")
    assert rc is not None and rc != 0
    assert message is not None
    assert "stdlib_ast" in message
    assert "--stdlib-profile full" in message
    # And the remedy actually works: ast builds on full.
    rc_full, _ = _run_pass([("ast", STDLIB_ROOT / "ast.py")], "full", "native")
    assert rc_full is None

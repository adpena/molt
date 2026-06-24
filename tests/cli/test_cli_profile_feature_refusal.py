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
    gap = cli._profile_feature_gap_for_module(STDLIB_ROOT / "ast.py", _micro_features())
    assert "stdlib_ast" in gap
    # The gap names the concrete intrinsics that would be undefined at link.
    assert any(sym.startswith("molt_ast_") for sym in gap["stdlib_ast"])


def test_ast_module_buildable_on_full_profile() -> None:
    gap = cli._profile_feature_gap_for_module(STDLIB_ROOT / "ast.py", _full_features())
    assert gap == {}


def test_second_gated_module_hashlib_requires_stdlib_crypto() -> None:
    # Negative-control completeness: the refusal names the RIGHT feature for a
    # different gated module, proving it is data-driven, not ast-special-cased.
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "hashlib.py", _micro_features()
    )
    assert set(gap) == {"stdlib_crypto"}


def test_third_gated_module_sqlite_requires_sqlite_feature() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "_sqlite3.py", _micro_features()
    )
    assert set(gap) == {"sqlite"}


def test_sqlite_module_buildable_on_full_profile() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "_sqlite3.py", _full_features()
    )
    assert gap == {}


def test_stringprep_module_requires_stdlib_stringprep_gate() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "stringprep.py", _micro_features()
    )
    assert set(gap) == {"stdlib_stringprep"}
    assert any(sym.startswith("molt_stringprep_") for sym in gap["stdlib_stringprep"])


def test_stringprep_module_buildable_on_full_profile() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "stringprep.py", _full_features()
    )
    assert gap == {}


def test_html_module_requires_stdlib_text_gate() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "html" / "__init__.py", _micro_features()
    )
    assert set(gap) == {"stdlib_text"}
    assert any(sym.startswith("molt_html_") for sym in gap["stdlib_text"])


def test_unicodedata_module_requires_stdlib_text_gate() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "unicodedata.py", _micro_features()
    )
    assert set(gap) == {"stdlib_text"}
    assert any(sym.startswith("molt_unicodedata_") for sym in gap["stdlib_text"])


def test_text_modules_buildable_on_full_profile() -> None:
    assert (
        cli._profile_feature_gap_for_module(
            STDLIB_ROOT / "html" / "__init__.py", _full_features()
        )
        == {}
    )
    assert (
        cli._profile_feature_gap_for_module(
            STDLIB_ROOT / "unicodedata.py", _full_features()
        )
        == {}
    )


def test_zoneinfo_module_requires_stdlib_zoneinfo_gate() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "zoneinfo" / "__init__.py", _micro_features()
    )
    assert set(gap) == {"stdlib_zoneinfo"}
    assert any(sym.startswith("molt_zoneinfo_") for sym in gap["stdlib_zoneinfo"])


def test_zoneinfo_module_buildable_on_full_profile() -> None:
    assert (
        cli._profile_feature_gap_for_module(
            STDLIB_ROOT / "zoneinfo" / "__init__.py", _full_features()
        )
        == {}
    )


def test_tinygrad_package_requires_gpu_primitives_gate_on_micro_profile() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "tinygrad" / "__init__.py", _micro_features()
    )
    assert set(gap) == {"molt_gpu_primitives"}
    assert gap["molt_gpu_primitives"] == ["molt_gpu_prim_device"]


def test_tinygrad_package_buildable_on_full_profile() -> None:
    gap = cli._profile_feature_gap_for_module(
        STDLIB_ROOT / "tinygrad" / "__init__.py", _full_features()
    )
    assert gap == {}


def test_ungated_ssl_abi_is_never_refused() -> None:
    # ssl keeps a deliberately always-linkable ABI (asyncio imports it eagerly
    # even on micro); importing it must NOT trigger a feature refusal.
    gap = cli._profile_feature_gap_for_module(STDLIB_ROOT / "ssl.py", _micro_features())
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
    gap = cli._profile_feature_gap_for_module(
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
        "stdlib_email",
        "stdlib_logging",
        "stdlib_concurrent",
        "stdlib_dbm",
        "stdlib_importlib_extra",
        "stdlib_signal",
        "stdlib_select",
    }
    assert resolver_only.isdisjoint(LINK_AFFECTING_FEATURES)


# --- the enforcement pass --------------------------------------------------


def _run_pass(module_paths, profile, target):
    captured: list[str] = []
    graph = {name: path for name, path in module_paths}
    orig_fail = cli._fail

    def fake_fail(message, json_output, code=2, command="molt"):
        captured.append(message)
        return code

    cli._fail = fake_fail  # type: ignore[assignment]
    try:
        rc = cli._enforce_profile_feature_availability(
            graph, STDLIB_ROOT, profile, target, json_output=False
        )
    finally:
        cli._fail = orig_fail  # type: ignore[assignment]
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


def test_normal_python_only_imports_are_unaffected() -> None:
    # A pure-Python stdlib module with no gated intrinsics never refuses.
    rc, message = _run_pass(
        [("colorsys", STDLIB_ROOT / "colorsys.py")], "micro", "native"
    )
    assert rc is None
    assert message is None


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

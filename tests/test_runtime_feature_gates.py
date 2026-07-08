"""Anti-drift guards for generated runtime feature-gate classification.

``src/molt/_runtime_feature_gates.py`` is generated from categories.toml,
Cargo.toml, and cfg-gated runtime modules. The refusal must fire only
for *link-affecting* features — those whose Cargo gate, when disabled, removes
intrinsic *symbol definitions* from the archive — and never for resolver-only
features (empty `[]` Cargo groups) whose ``#[unsafe(no_mangle)]`` definitions are
compiled unconditionally.

This guard re-derives the link-affecting set mechanically from the actual runtime
crate sources and asserts it equals ``LINK_AFFECTING_FEATURES``. If someone adds
a `#[cfg(feature="stdlib_new")]`-gated `mod` or a `dep:`-backed feature without
updating the classification, this fails loudly at test time — long before a
profile build hits an undefined-symbol linker error (or, worse, a wrongful
refusal of a working build).
"""

from __future__ import annotations

import re
import tomllib
from pathlib import Path

from molt._runtime_feature_gates import (
    LINK_AFFECTING_FEATURES,
    RUNTIME_FEATURE_GATES,
)

ROOT = Path(__file__).resolve().parents[1]
RUNTIME_CRATE = ROOT / "runtime" / "molt-runtime"


def _cfg_gated_mod_features(rust_source: str) -> set[str]:
    """Features that gate a `mod` declaration (the module is cfg-compiled out)."""
    pattern = re.compile(
        r'#\[cfg\(feature\s*=\s*"([^"]+)"\)\]\s*\n'
        r"\s*(?:pub\s+|pub\(crate\)\s+)?mod\b"
    )
    return set(pattern.findall(rust_source))


def _feature_expands_to_dep(name: str, features: dict, seen: set[str]) -> bool:
    """True iff *name* transitively pulls an optional crate / dependency.

    A `dep:` item, or any `crate/feature` / `crate?/feature` activation, marks
    the feature as dep-backed (the optional crate's symbols are dropped when the
    feature is off). Recurse through feature aliases defined in the same table.
    """
    if name in seen:
        return False
    seen.add(name)
    for item in features.get(name, []):
        if item.startswith("dep:"):
            return True
        if "/" in item:
            return True
        if item in features and _feature_expands_to_dep(item, features, seen):
            return True
    return False


def _mechanically_derived_link_affecting() -> set[str]:
    cargo = tomllib.loads((RUNTIME_CRATE / "Cargo.toml").read_text())
    features = cargo.get("features", {})

    mod_features = _cfg_gated_mod_features(
        (RUNTIME_CRATE / "src" / "builtins" / "mod.rs").read_text()
    ) | _cfg_gated_mod_features((RUNTIME_CRATE / "src" / "lib.rs").read_text())

    dep_features = {
        feature
        for feature in features
        if _feature_expands_to_dep(feature, features, set())
    }

    gate_features = {feature for _prefix, feature in RUNTIME_FEATURE_GATES}
    return (mod_features | dep_features) & gate_features


def test_link_affecting_features_match_runtime_crate_ground_truth() -> None:
    derived = _mechanically_derived_link_affecting()
    assert LINK_AFFECTING_FEATURES == derived, (
        "LINK_AFFECTING_FEATURES drifted from the runtime crate. "
        f"missing (now link-affecting in the crate): {sorted(derived - LINK_AFFECTING_FEATURES)}; "
        f"stale (no longer link-affecting): {sorted(LINK_AFFECTING_FEATURES - derived)}. "
        "Update Cargo.toml/cfg-gated modules, then regenerate intrinsics."
    )


def test_link_affecting_is_subset_of_gate_table_features() -> None:
    gate_features = {feature for _prefix, feature in RUNTIME_FEATURE_GATES}
    assert LINK_AFFECTING_FEATURES <= gate_features


def test_ast_feature_is_link_affecting() -> None:
    # The seeded class: ast on micro must be refused, so stdlib_ast MUST be
    # classified link-affecting.
    assert "stdlib_ast" in LINK_AFFECTING_FEATURES


def test_serial_feature_is_link_affecting() -> None:
    assert "stdlib_serial" in LINK_AFFECTING_FEATURES


def test_high_level_gpu_intrinsics_are_link_affecting() -> None:
    from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol

    for symbol in (
        "molt_gpu_linear_contiguous",
        "molt_gpu_tensor__zeros",
        "molt_gpu_interop_decode_f16_bytes_to_f32",
        "molt_gpu_prim_create_tensor",
    ):
        assert link_affecting_feature_gate_for_symbol(symbol) == "molt_gpu_primitives"


def test_empty_cargo_group_features_are_not_link_affecting() -> None:
    # Empty `[]` Cargo groups gate only resolver arms; their symbols are always
    # defined, so they must stay out of the link-affecting set.
    for feature in (
        "stdlib_logging",
        "stdlib_concurrent",
        "stdlib_dbm",
        "stdlib_importlib_extra",
        "stdlib_signal",
        "stdlib_select",
    ):
        assert feature not in LINK_AFFECTING_FEATURES


def _mod_builtins_cfg_prefix(lib_rs: str) -> str | None:
    """Return the `#[cfg(...)]` attribute gating `mod builtins;`, or None.

    Matches an optional attribute line immediately preceding the module
    declaration. When `mod builtins;` is declared unconditionally (the correct
    core-authority shape) this returns None.
    """
    match = re.search(
        r"(?:#\[cfg\(([^\]]+)\)\]\s*\n)?[ \t]*mod builtins;",
        lib_rs,
    )
    assert match is not None, "lib.rs must declare `mod builtins;`"
    return match.group(1)


def _expand_cargo_feature_names(feature: str, features: dict) -> set[str]:
    """Transitively expand *feature* to the molt-runtime feature names reached.

    Skips ``dep:crate`` and ``crate/feat`` / ``crate?/feat`` cross-crate
    activations (they select dependency features, not molt-runtime's own).
    Includes the seed feature itself so a directly-triggering seed is caught.
    """
    reached: set[str] = set()
    stack = [feature]
    while stack:
        current = stack.pop()
        if current in reached:
            continue
        reached.add(current)
        for entry in features.get(current, []):
            if entry.startswith("dep:") or "/" in entry:
                continue
            stack.append(entry)
    return reached


def test_core_builtins_authority_compiles_unconditionally() -> None:
    """`builtins` is the crate-root CORE authority and must never be gated.

    `runtime/molt-runtime/src/builtins/mod.rs` re-exports the core runtime
    symbols (`raise_exception`, `to_i64`/`to_f64`, `molt_type_new`,
    `attr_lookup_ptr`, `ellipsis_bits`, the whole exception machinery) that
    ~230 files reference UNCONDITIONALLY via `crate::…`. Gating the module
    behind a derived `molt_runtime_builtins` cfg (or any other cfg) drops it for
    feature subsets that enable no `stdlib_*`/`builtin_*` feature — e.g. the
    wasm-browser split-runtime numpy/scipy compute plan — producing thousands of
    `E0433 cannot find 'builtins' in 'crate'` / `E0432 unresolved import`
    errors. Only the OPTIONAL stdlib submodules inside `builtins/mod.rs` may be
    feature-gated (they already are, internally); the core module is
    unconditional by construction.
    """

    lib_rs = (RUNTIME_CRATE / "src" / "lib.rs").read_text(encoding="utf-8")
    build_rs = (RUNTIME_CRATE / "build.rs").read_text(encoding="utf-8")

    cfg = _mod_builtins_cfg_prefix(lib_rs)
    assert cfg is None, (
        "`mod builtins;` must compile UNCONDITIONALLY (it is the crate-root "
        "core authority), but it is gated behind "
        f"`#[cfg({cfg})]`. Remove the outer gate; keep only the per-feature "
        "gates on the OPTIONAL stdlib submodules inside builtins/mod.rs."
    )

    # The vestigial derived cfg must be fully removed, not merely unreferenced,
    # so a future edit cannot silently re-gate the core module.
    assert "molt_runtime_builtins" not in lib_rs
    assert "molt_runtime_builtins" not in build_rs


def test_zero_stdlib_wasm_browser_profile_keeps_core_builtins() -> None:
    """Regression for the witness blocker: the MINIMAL zero-stdlib wasm-browser
    split-runtime feature set must still compile the core builtins authority.

    The browser split-runtime numpy/scipy compute runtime is built with
    ``cargo rustc -p molt-runtime --target wasm32-wasip1 --lib
    --no-default-features --features wasm_freestanding`` — a feature set that
    enables NONE of the ``stdlib_*``/``builtin_*``/``sqlite``/
    ``molt_gpu_primitives`` features. This test asserts that fact from
    Cargo.toml (so the reproduction stays honest), then asserts the core
    ``builtins`` module is not gated out of such a build. Under the previous
    ``#[cfg(molt_runtime_builtins)]`` gating this feature set dropped
    ``mod builtins`` and produced 2824 compile errors that reached the witness;
    the existing gate tests never exercised a zero-stdlib profile, which is the
    coverage gap this closes.
    """

    cargo = tomllib.loads((RUNTIME_CRATE / "Cargo.toml").read_text())
    features = cargo.get("features", {})

    def _triggers_builtins(feature: str) -> bool:
        upper = feature.upper()
        return (
            upper.startswith("STDLIB_")
            or upper.startswith("BUILTIN_")
            or upper in {"SQLITE", "MOLT_GPU_PRIMITIVES"}
        )

    reached = _expand_cargo_feature_names("wasm_freestanding", features)
    triggering = sorted(f for f in reached if _triggers_builtins(f))
    assert triggering == [], (
        "wasm_freestanding unexpectedly chains builtins-triggering features "
        f"{triggering}; the zero-stdlib reproduction is no longer minimal — "
        "update this test to a feature set that truly enables no stdlib_/"
        "builtin_ feature."
    )

    lib_rs = (RUNTIME_CRATE / "src" / "lib.rs").read_text(encoding="utf-8")
    assert _mod_builtins_cfg_prefix(lib_rs) is None, (
        "The zero-stdlib wasm-browser split-runtime feature set enables no "
        "builtins-triggering feature, so any cfg gating `mod builtins;` drops "
        "the crate-root core authority and reintroduces the witness blocker."
    )

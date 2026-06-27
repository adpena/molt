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

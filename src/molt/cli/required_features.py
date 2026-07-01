"""Reachability-driven runtime-feature requirement authority (Option b).

This is the single authority that answers *which link-affecting runtime features
this program's REACHED code requires*, replacing the coarse, whole-file
``module_required_intrinsic_names`` presence scan that forced a feature the
instant a module appeared anywhere in the static import graph - even when no code
path ever reached one of its intrinsics.

Design: ``docs/design/foundation/feature_reachability_tree_shaking.md`` (Option b
in the clean Python lane).

The fact
========
A runtime intrinsic becomes a *hard link dependency* of the binary exactly when a
**reached** SimpleIR op directly references its symbol. The frontend lowers
``require_intrinsic("molt_re_compile")`` to a ``builtin_func`` op whose ``s_value``
is the intrinsic symbol; the native backend turns that into a ``func_addr`` with
``Linkage::Import`` (an ``.refptr.molt_re_compile`` in the object), so the linker
*must* resolve ``molt_re_compile`` or the build fails with an undefined symbol.
The name-based resolver path (``load_intrinsic`` / ``require_optional_intrinsic``)
instead emits a ``const_str`` intrinsic name that the per-app resolver resolves at
runtime; that name is likewise a reached-intrinsic candidate.

``ReachedIntrinsics`` is therefore: every ``builtin_func``/``const_str`` op whose
``s_value`` is a runtime intrinsic symbol, scanned over the set of **reachable**
SimpleIR functions. Reachability is the same call-graph closure the native/WASM
backends use to dead-strip functions (``molt-tir`` ``eliminate_dead_functions``):
a BFS from the program entry (``molt_main``) plus the protected runtime
entrypoints, following the reference-bearing op kinds. A function that is never
referenced from a reachable function contributes nothing - so an ``import re``
whose regex methods are never reached links zero ``molt_re_*`` symbols, while a
program that reaches ``re.compile`` link-references exactly the ``molt_re_*``
symbols on the reached path.

``RequiredLinkFeatures`` maps each reached intrinsic to its link-affecting Cargo
feature via the generated
``molt._runtime_feature_gates.link_affecting_feature_gate_for_symbol`` authority
(``None`` for core/ungated/resolver-only symbols, which are always linkable and
thus never a requirement). This is the exact "does dropping this feature remove
the symbol definition" predicate the refusal must compare against the profile's
feature ceiling.

This module is intentionally backend-uniform: it is a property of the reached
SimpleIR, identical for native/WASM/LLVM/Luau. Only the per-target profile
*ceiling* (``runtime_features._runtime_builtin_features_for_profile``, the same
available-feature authority the build uses to select the staticlib) differs.
"""

from __future__ import annotations

from typing import Iterable, Mapping, Sequence

from molt._intrinsic_symbols import (
    INTRINSIC_SYMBOL_NAMES,
    intrinsic_runtime_symbol_name,
)
from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol
from molt.cli import function_references as _function_references

_FUNCTION_REFERENCE_OP_KINDS = _function_references.FUNCTION_REFERENCE_OP_KINDS
_POLL_COMPANION_OP_KINDS = _function_references.POLL_COMPANION_OP_KINDS
_PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES = (
    _function_references.PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES
)
_PROTECTED_RUNTIME_ENTRYPOINTS = _function_references.PROTECTED_RUNTIME_ENTRYPOINTS
reachable_function_names = _function_references.reachable_function_names

_RUNTIME_INTRINSIC_SYMBOL_NAMES: frozenset[str] = frozenset(
    INTRINSIC_SYMBOL_NAMES.values()
)

# ---------------------------------------------------------------------------
# Reachability primitives - mirror ``molt-tir`` ``eliminate_dead_functions``.
# ---------------------------------------------------------------------------
#
# These two sets are the Python mirror of the Rust dead-function call-graph
# authority (``runtime/molt-tir/src/passes/dead_functions.rs`` /
# ``runtime/molt-tir/src/passes/runtime_roots.rs``). They MUST stay in lockstep
# with that authority: if the backend treats an op kind as a function reference
# (keeping the referenced function), this requirement scan must treat it as a
# reachability edge too, or the requirement could under-approximate and let an
# undefined-symbol link error slip past the compile-time refusal. The agreement
# is pinned by ``test_required_features_reachability.py`` against the Rust source.

#: Op kinds whose ``s_value`` directly references a *runtime intrinsic symbol* and
#: thus makes it a reached-intrinsic candidate. ``builtin_func`` is the direct
#: link reference (``func_addr`` / ``Linkage::Import`` -> ``.refptr.molt_*``);
#: ``const_str`` is the name the per-app resolver may resolve dynamically (the
#: ``compute_intrinsic_manifest`` candidate shape). Both are validated against the
#: feature-gate authority, so non-intrinsic strings/builtins contribute nothing.
_INTRINSIC_SYMBOL_OP_KINDS: frozenset[str] = frozenset({"builtin_func", "const_str"})


def reached_intrinsic_symbols_by_feature(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> dict[str, set[str]]:
    """Map each required link-affecting feature to the reached intrinsic symbols.

    Scans only the *reachable* functions for ``builtin_func``/``const_str`` ops
    whose ``s_value`` is a link-affecting intrinsic symbol (per
    ``link_affecting_feature_gate_for_symbol``), grouping the reached symbols by
    the feature that defines them. Core/ungated/resolver-only symbols map to
    ``None`` and are dropped (they are always linkable, never a requirement).
    """
    symbols = reached_intrinsic_symbols(functions, extra_roots=extra_roots)
    by_feature: dict[str, set[str]] = {}
    for symbol in symbols:
        feature = link_affecting_feature_gate_for_symbol(symbol)
        if feature is None:
            continue
        by_feature.setdefault(feature, set()).add(symbol)
    return by_feature


def reached_intrinsic_symbols(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> frozenset[str]:
    """Runtime intrinsic symbols referenced by reached SimpleIR.

    This is the backend-uniform symbol fact below ``RequiredLinkFeatures``: it
    keeps core/ungated intrinsic symbols as well as link-affecting ones so
    consumers such as the WASM auto-import seed can reuse the same reachability
    closure without re-scanning source modules.
    """
    reachable = reachable_function_names(functions, extra_roots=extra_roots)
    symbols: set[str] = set()
    for func in functions:
        name = func.get("name")
        if not isinstance(name, str) or name not in reachable:
            continue
        ops = func.get("ops")
        if not isinstance(ops, list):
            continue
        for op in ops:
            if not isinstance(op, Mapping):
                continue
            if op.get("kind") not in _INTRINSIC_SYMBOL_OP_KINDS:
                continue
            symbol = op.get("s_value")
            if not isinstance(symbol, str):
                continue
            canonical = intrinsic_runtime_symbol_name(symbol)
            if canonical not in _RUNTIME_INTRINSIC_SYMBOL_NAMES:
                continue
            symbols.add(canonical)
    return frozenset(symbols)


def required_link_features(
    functions: Sequence[Mapping[str, object]],
    *,
    extra_roots: Iterable[str] = (),
) -> frozenset[str]:
    """The minimal set of link-affecting Cargo features the reached code needs.

    ``RequiredLinkFeatures`` in the design: every distinct link-affecting feature
    that defines a reached intrinsic symbol. This is the single requirement
    authority - the build must link a runtime archive whose feature set is a
    superset of this, and the compile-time refusal fires when the selected
    profile's ceiling omits any of these.
    """
    return frozenset(
        reached_intrinsic_symbols_by_feature(functions, extra_roots=extra_roots)
    )


def reachability_profile_feature_refusal(
    functions: Sequence[Mapping[str, object]],
    *,
    profile_name: str,
    profile_features: frozenset[str],
    extra_roots: Iterable[str] = (),
) -> str | None:
    """Truthful compile-time refusal when reached code needs an excluded feature.

    The reachability ceiling check (``RequiredLinkFeatures <= LinkFeatures(P)``):
    compute the link-affecting features the *reached* SimpleIR needs and, for any
    that the selected profile's ceiling omits, return a refusal that names the
    exact reached intrinsic symbols (the ``molt_*`` names whose definitions the
    excluded feature provides) and the actionable remedy. Returns ``None`` when
    every required feature is within the ceiling (the build may proceed).

    This converts what would otherwise be a raw ``undefined symbol: molt_re_*``
    linker error - the native backend takes the address of each reached
    intrinsic with ``Linkage::Import`` - into an actionable, reached-path-aware
    compile-time refusal, exactly the design's "no silent divergence / no raw
    link error" contract.
    """
    by_feature = reached_intrinsic_symbols_by_feature(
        functions, extra_roots=extra_roots
    )
    blocked = {
        feature: symbols
        for feature, symbols in by_feature.items()
        if feature not in profile_features
    }
    if not blocked:
        return None

    excluded_features = sorted(blocked)
    lines: list[str] = []
    for feature in excluded_features:
        symbols = sorted(blocked[feature])
        sample = ", ".join(symbols[:6])
        if len(symbols) > 6:
            sample += f", ... (+{len(symbols) - 6} more)"
        plural = "intrinsic" if len(symbols) == 1 else "intrinsics"
        lines.append(f"  {feature}: reached {plural} {sample}")

    feature_phrase = (
        f"the {excluded_features[0]!r} runtime feature"
        if len(excluded_features) == 1
        else "runtime features " + ", ".join(repr(f) for f in excluded_features)
    )
    return (
        f"Profile '{profile_name}' excludes {feature_phrase} that this program's "
        f"REACHED code requires.\n"
        f"These runtime intrinsics are reached by executed code paths, so their "
        f"symbols would be undefined at link under '{profile_name}':\n"
        + "\n".join(lines)
        + "\n\nFeature selection is reachability-driven, not import-driven: the "
        "'micro' profile omits heavy domains (regex, ast, crypto, compression, "
        "...) to keep small binaries small, and the requirement is computed from "
        "the intrinsics your reached code actually links, not from the mere "
        "presence of a module in the import graph.\n"
        "Either remove the reached usage of these intrinsics, or rebuild with the "
        "full stdlib profile, which includes these features:\n"
        "    --stdlib-profile full\n"
        "or set the environment knob the build reads as its canonical profile:\n"
        "    MOLT_STDLIB_PROFILE=full"
    )

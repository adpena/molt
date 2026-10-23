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

``ReachedIntrinsics`` includes named external/runtime calls and first-class
function addresses whose targets are runtime intrinsics rather than compiled
definitions. ``builtin_func`` always names a runtime intrinsic, independently
of compiled definitions; ``const_str`` preserves conservative resolver candidates.
These references are scanned over **reachable** SimpleIR functions.
Reachability is the same call-graph closure the native/WASM
backends use to dead-strip functions (``molt-tir`` ``eliminate_dead_functions``):
a BFS from the program entry (``molt_main``), protected runtime entrypoints and
registry relocation roots, following reference-bearing op kinds. This is static
link reachability, not evidence that a function executes. Publishing a function
with ``func_new`` retains its body, including lazily bound intrinsic references.
Only bodies eliminated from this closure contribute no requirement; importing
``re`` can retain regex symbols even without an explicit regex call.

A build computes ``ReachedIntrinsics`` once (``reached_intrinsic_symbols``)
and derives both the refusal and ``RequiredLinkFeatures`` from that one symbol
set, so the whole-program closure runs once per build.

``RequiredLinkFeatures`` maps each reached intrinsic to the Cargo feature its
manifest entry declares, via the generated
``molt._runtime_feature_gates.feature_for_symbol`` authority (``None`` for an
always-defined symbol, which is never a requirement). The generated resolvers
prove at compile time that a runtime built without that feature defines no such
symbol, so the declaration is exactly the "does dropping this feature remove the
symbol definition" predicate the refusal compares against the profile's feature
ceiling. No runtime provides a feature-off substitute (HF-487).

This module is intentionally backend-uniform: it is a property of the reached
SimpleIR, identical for native/WASM/LLVM/Luau. Only the per-target profile
*ceiling* (``runtime_features._runtime_builtin_features_for_profile``, the same
available-feature authority the build uses to select the staticlib) differs.
"""

from __future__ import annotations

from typing import Iterable, Mapping, Sequence

from molt._intrinsic_symbols import (
    INTRINSIC_SIGNATURES,
    intrinsic_runtime_symbol_name,
)
from molt._runtime_feature_gates import (
    feature_for_symbol,
    runtime_symbol_available_on_target,
    supported_target_oses_for_symbol,
    symbol_availability_depends_on_target_os,
    target_os_from_triple,
    unsupported_target_arches_for_symbol,
    unsupported_target_oses_for_symbol,
)
from molt.cli import function_references as _function_references
from molt.cli.runtime_features import (
    profile_link_features,
    runtime_stdlib_profile_for_required_features,
)
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_CALL_TARGET_ROLES,
    SIMPLEIR_FUNCTION_REFERENCE_S_VALUE_KINDS,
)

_PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES = (
    _function_references.PROTECTED_RUNTIME_ENTRYPOINT_PREFIXES
)
_PROTECTED_RUNTIME_ENTRYPOINTS = _function_references.PROTECTED_RUNTIME_ENTRYPOINTS
reachable_function_names = _function_references.reachable_function_names

_RUNTIME_INTRINSIC_SYMBOL_NAMES: frozenset[str] = frozenset(
    symbol for symbol, _arity in INTRINSIC_SIGNATURES.values()
)

# ---------------------------------------------------------------------------
# Reachability primitives - shared generated defined-function edges.
# ---------------------------------------------------------------------------
#
# function_references and the Rust dead-function pass consume the same generated
# op-kind authority. Runtime entry roots remain owned by runtime_roots.rs; the
# exact-name graph contributes features only from statically retained bodies.

#: Op kinds whose ``s_value`` directly references a *runtime intrinsic symbol* and
#: thus makes it a reached-intrinsic candidate. ``builtin_func`` is the direct
#: link reference (``func_addr`` / ``Linkage::Import`` -> ``.refptr.molt_*``);
#: ``const_str`` is the name the per-app resolver may resolve dynamically (the
#: ``compute_app_callable_manifest`` candidate shape). Both are validated
#: against intrinsic membership, so non-intrinsic strings/builtins contribute
#: nothing. Unlike general named calls/function addresses, these runtime-only
#: and conservative resolver references are not shadowed by compiled definitions.
_INTRINSIC_SYMBOL_OP_KINDS: frozenset[str] = frozenset({"builtin_func", "const_str"})


def reached_intrinsic_symbols_by_feature(
    reached_symbols: Iterable[str],
) -> dict[str, set[str]]:
    """Map each required feature to its reached intrinsic symbols.

    Groups ``reached_symbols`` (``reached_intrinsic_symbols``) by the feature
    that defines each one (per ``feature_for_symbol``). Always-defined symbols
    map to ``None`` and are dropped (they are never a requirement).
    """
    by_feature: dict[str, set[str]] = {}
    for symbol in reached_symbols:
        feature = feature_for_symbol(symbol)
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
    compiled = {
        name
        for func in functions
        if isinstance((name := func.get("name")), str)
        and not func.get("is_extern", False)
    }
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
            kind = op.get("kind")
            if not isinstance(kind, str):
                continue
            # A typed runtime-symbol carrier (a qualified runtime callable
            # such as `sys._getframe`) names the callable it acquires; the
            # app-callable collector admits the same symbol.
            carried = op.get("runtime_symbol")
            if isinstance(carried, str) and carried in _RUNTIME_INTRINSIC_SYMBOL_NAMES:
                symbols.add(carried)
            symbol = op.get("s_value")
            if not isinstance(symbol, str):
                continue
            if kind not in _INTRINSIC_SYMBOL_OP_KINDS:
                if symbol in compiled:
                    continue
                if (
                    SIMPLEIR_CALL_TARGET_ROLES.get(kind) != "external_or_runtime"
                    and kind not in SIMPLEIR_FUNCTION_REFERENCE_S_VALUE_KINDS
                ):
                    continue
            canonical = intrinsic_runtime_symbol_name(symbol)
            if canonical not in _RUNTIME_INTRINSIC_SYMBOL_NAMES:
                continue
            symbols.add(canonical)
    return frozenset(symbols)


def required_link_features(reached_symbols: Iterable[str]) -> frozenset[str]:
    """The minimal set of Cargo features the reached code needs.

    ``RequiredLinkFeatures`` in the design: every distinct feature that defines a
    reached intrinsic symbol. This is the single requirement
    authority - the build must link a runtime archive whose feature set is a
    superset of this, and the compile-time refusal fires when the selected
    profile's ceiling omits any of these.
    """
    return frozenset(reached_intrinsic_symbols_by_feature(reached_symbols))


def reachability_profile_feature_refusal(
    reached_symbols: frozenset[str],
    *,
    profile_name: str,
    profile_features: frozenset[str],
    target_triple: str | None = None,
) -> str | None:
    """Report all target and profile requirements absent from reached code's runtime.

    The reachability ceiling check (``RequiredLinkFeatures <= LinkFeatures(P)``):
    compute the link-affecting features the *reached* SimpleIR needs and, for any
    that the selected profile's ceiling omits, return a refusal that names the
    exact reached intrinsic symbols (the ``molt_*`` names whose definitions the
    excluded feature provides) and the actionable remedy. Independently report
    every reached intrinsic with no provider on the selected target. Both checks
    read ``reached_symbols``; neither failure category hides the other, and
    neither symbol list is truncated. Returns ``None`` when all required symbols
    have target providers and their features are within the profile ceiling.

    This converts what would otherwise be a raw ``undefined symbol: molt_re_*``
    linker error - the native backend takes the address of each reached
    intrinsic with ``Linkage::Import`` - into an actionable, reached-path-aware
    compile-time refusal, exactly the design's "no silent divergence / no raw
    link error" contract.
    """
    target_os: str | None = None
    if any(
        symbol_availability_depends_on_target_os(symbol) for symbol in reached_symbols
    ):
        target_os = target_os_from_triple(target_triple)
    unavailable_symbols: list[str] = []
    blocked: dict[str, list[str]] = {}
    for symbol in reached_symbols:
        if not runtime_symbol_available_on_target(
            symbol, target_triple=target_triple, target_os=target_os
        ):
            unavailable_symbols.append(symbol)
        feature = feature_for_symbol(symbol)
        if feature is not None and feature not in profile_features:
            blocked.setdefault(feature, []).append(symbol)

    if not unavailable_symbols and not blocked:
        return None

    messages: list[str] = []
    if unavailable_symbols:
        arch = (
            target_triple.split("-", 1)[0] if target_triple is not None else "unknown"
        )
        symbol_details: list[str] = []
        for symbol in sorted(unavailable_symbols):
            restrictions: list[str] = []
            if arches := unsupported_target_arches_for_symbol(symbol):
                restrictions.append(f"unsupported on {', '.join(arches)}")
            if oses := supported_target_oses_for_symbol(symbol):
                restrictions.append(f"requires target OS {', '.join(oses)}")
            if oses := unsupported_target_oses_for_symbol(symbol):
                restrictions.append(f"unsupported on target OS {', '.join(oses)}")
            symbol_details.append(f"{symbol} ({'; '.join(restrictions)})")
        details = ", ".join(symbol_details)
        messages.append(
            f"target {target_triple!r} (architecture {arch}) has no runtime provider "
            f"for reached intrinsic symbols: {details}; choose a supported "
            "target or remove the reached operation"
        )

    if blocked:
        excluded_features = sorted(blocked)
        lines: list[str] = []
        for feature in excluded_features:
            symbols = sorted(blocked[feature])
            plural = "intrinsic" if len(symbols) == 1 else "intrinsics"
            lines.append(f"  {feature}: reached {plural} {', '.join(symbols)}")

        feature_phrase = (
            f"the {excluded_features[0]!r} runtime feature"
            if len(excluded_features) == 1
            else "runtime features " + ", ".join(repr(f) for f in excluded_features)
        )
        unprovidable = sorted(
            set(excluded_features)
            - profile_link_features("full", target_triple=target_triple)
        )
        providable = [f for f in excluded_features if f not in unprovidable]
        remedies: list[str] = []
        if providable:
            tier = runtime_stdlib_profile_for_required_features(
                "auto", providable, target_triple=target_triple
            )
            provided = (
                f"the {providable[0]!r} runtime feature"
                if len(providable) == 1
                else "runtime features " + ", ".join(repr(f) for f in providable)
            )
            remedies.append(
                "Remove the dependency that retains these bodies, or rebuild with "
                f"the smallest stdlib profile that provides {provided}:\n"
                f"    --stdlib-profile {tier}\n"
                "or set the environment knob the build reads as its canonical "
                "profile:\n"
                f"    MOLT_STDLIB_PROFILE={tier}\n"
                "or let reachability select it:\n"
                "    --stdlib-profile auto"
                + (
                    f"\nThe {tier} stdlib profile does not provide symbols "
                    "unavailable on the selected target."
                    if unavailable_symbols
                    else ""
                )
            )
        if unprovidable:
            remedies.append(
                "No stdlib profile provides "
                + ", ".join(repr(f) for f in unprovidable)
                + " on the selected target; remove the dependency that retains "
                "these bodies or choose a target that provides it."
            )
        messages.append(
            f"Profile '{profile_name}' excludes {feature_phrase} that this program's "
            f"statically retained code requires.\n"
            "These runtime intrinsics are referenced by retained function bodies, "
            "and a runtime built without the feature does not define them "
            f"under '{profile_name}':\n"
            + "\n".join(lines)
            + "\n\nRequirements follow the static link closure, including published "
            "functions and module-registry roots; they do not prove execution. "
            + "\n".join(remedies)
        )
    return "\n\n".join(messages)

from __future__ import annotations

import functools
import os
import tomllib
from typing import Collection

from molt.cli.capability_spec import _dedupe_preserve_order
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE, _coerce_bool


@functools.lru_cache(maxsize=32)
def _runtime_cargo_features_cached(
    target_triple: str | None,
    tk_raw: str | None,
    gpu_metal_raw: str | None,
    gpu_webgpu_raw: str | None,
    gpu_cuda_raw: str | None,
    gpu_hip_raw: str | None,
) -> tuple[str, ...]:
    features: list[str] = []
    if target_triple is not None and target_triple.startswith("wasm32"):
        features.append("molt_gpu_primitives")
        if (
            True
            if gpu_webgpu_raw is None or gpu_webgpu_raw.strip() == ""
            else _coerce_bool(gpu_webgpu_raw, True)
        ):
            pass
        return tuple(features)
    tk_enabled = (
        True if tk_raw is None or tk_raw.strip() == "" else _coerce_bool(tk_raw, True)
    )
    if tk_enabled:
        features.append("molt_tk_native")
    metal_enabled = (
        False
        if gpu_metal_raw is None or gpu_metal_raw.strip() == ""
        else _coerce_bool(gpu_metal_raw, False)
    )
    if metal_enabled:
        features.append("molt_gpu_metal")
    webgpu_enabled = (
        False
        if gpu_webgpu_raw is None or gpu_webgpu_raw.strip() == ""
        else _coerce_bool(gpu_webgpu_raw, False)
    )
    if webgpu_enabled:
        features.append("molt_gpu_webgpu")
    cuda_enabled = (
        False
        if gpu_cuda_raw is None or gpu_cuda_raw.strip() == ""
        else _coerce_bool(gpu_cuda_raw, False)
    )
    if cuda_enabled:
        features.append("molt_gpu_cuda")
    hip_enabled = (
        False
        if gpu_hip_raw is None or gpu_hip_raw.strip() == ""
        else _coerce_bool(gpu_hip_raw, False)
    )
    if hip_enabled:
        features.append("molt_gpu_hip")
    return tuple(features)


def _runtime_cargo_features(target_triple: str | None) -> tuple[str, ...]:
    return _runtime_cargo_features_cached(
        target_triple,
        os.environ.get("MOLT_RUNTIME_TK_NATIVE"),
        os.environ.get("MOLT_RUNTIME_GPU_METAL"),
        os.environ.get("MOLT_RUNTIME_GPU_WEBGPU"),
        os.environ.get("MOLT_RUNTIME_GPU_CUDA"),
        os.environ.get("MOLT_RUNTIME_GPU_HIP"),
    )


_GPU_PRIMITIVE_IMPLYING_MODULE_PREFIXES = (
    "molt.gpu",
    "tinygrad",
    "molt.stdlib.tinygrad",
)


def _resolved_modules_require_gpu_primitives(
    resolved_modules: set[str] | frozenset[str] | None,
) -> bool:
    if resolved_modules is None:
        return False
    return any(
        module_name == prefix or module_name.startswith(prefix + ".")
        for module_name in resolved_modules
        for prefix in _GPU_PRIMITIVE_IMPLYING_MODULE_PREFIXES
    )


_ALL_BUILTIN_FEATURES: tuple[str, ...] = (
    "builtin_set",
    "builtin_memoryview",
    "builtin_complex",
    "builtin_contextvars",
    "builtin_fcntl",
)

# The Cargo profile feature each ``--stdlib-profile`` (and ladder tier) selects.
# The runtime archive's feature ladder is a strict superset chain
# micro -> edge -> standard -> server -> full (see
# ``runtime/molt-runtime/Cargo.toml`` ``[features]``).  This is the SINGLE
# source of truth for "what link-affecting features does profile P provide"; the
# Python side no longer maintains a parallel hand-written mirror (the old
# ``_ALL_DOMAIN_FEATURES`` flat list), which drifted from the Cargo chain and
# silently refused ``import re``/``itertools``/``difflib``/``xml``/``ipaddress``/
# ``pathlib`` on every profile because the mirror omitted those features.
_PROFILE_CARGO_FEATURE: dict[str, str] = {
    "micro": "stdlib_micro",
    "edge": "stdlib_edge",
    "standard": "stdlib_standard",
    "server": "stdlib_server",
    "full": "stdlib_full",
}


@functools.lru_cache(maxsize=1)
def _runtime_cargo_feature_graph() -> dict[str, tuple[str, ...]]:
    """The ``[features]`` table of ``runtime/molt-runtime/Cargo.toml``.

    Returns each feature name mapped to the raw list of entries it activates
    (feature names, ``dep:crate`` activations, and ``crate/feat`` /
    ``crate?/feat`` cross-crate feature activations, verbatim).  Anchored at the
    compiler root via ``_compiler_root`` so the read is cwd-independent.
    """
    cargo_path = (
        _compiler_root() / "runtime" / "molt-runtime" / "Cargo.toml"
    )
    with cargo_path.open("rb") as handle:
        manifest = tomllib.load(handle)
    features = manifest.get("features", {})
    return {
        name: tuple(entries)
        for name, entries in features.items()
        if isinstance(entries, list)
    }


def _expand_cargo_feature(feature: str) -> frozenset[str]:
    """Transitively expand a Cargo feature to the set of FEATURE NAMES reached.

    Skips ``dep:crate`` optional-dependency activations and ``crate/feat`` /
    ``crate?/feat`` cross-crate feature activations — those select dependency
    crates / crate-internal features, not features of ``molt-runtime`` itself,
    so they are not part of the profile's molt-runtime feature set.  The seed
    feature itself is excluded from the result (it is the aggregator tier name,
    not a provided leaf feature); only the features it pulls in are returned.
    """
    graph = _runtime_cargo_feature_graph()
    reached: set[str] = set()
    stack: list[str] = [feature]
    while stack:
        current = stack.pop()
        if current in reached:
            continue
        reached.add(current)
        for entry in graph.get(current, ()):
            if entry.startswith("dep:") or "/" in entry:
                continue
            stack.append(entry)
    reached.discard(feature)
    return frozenset(reached)


@functools.lru_cache(maxsize=32)
def profile_link_features(
    profile: str | None,
    *,
    target_triple: str | None,
) -> frozenset[str]:
    """Link-affecting + builtin feature names provided by ``profile``.

    Reads the runtime Cargo feature ladder and transitively expands the profile
    feature (``micro`` -> ``stdlib_micro`` ... ``full`` -> ``stdlib_full``),
    collecting the reachable molt-runtime feature names.  This is the canonical
    "what does profile P build" fact; both the compile-time profile-availability
    gate (``module_stdlib_policy._enforce_profile_feature_availability``) and the
    runtime archive feature selection (``runtime_build``) read it, so they can no
    longer disagree with the Cargo chain.

    The WASM stable-target exclusions (``_WASM_RUNTIME_STABLE_EXCLUDED_FEATURES``)
    are subtracted for ``wasm32`` targets so the derived set matches the features
    the WASM staticlib actually links.
    """
    effective_profile = profile or "micro"
    cargo_feature = _PROFILE_CARGO_FEATURE.get(effective_profile)
    if cargo_feature is None:
        raise ValueError(
            "stdlib_profile must be one of "
            f"{sorted(_PROFILE_CARGO_FEATURE)}; got {profile!r}"
        )
    reached = _expand_cargo_feature(cargo_feature)
    if target_triple is not None and target_triple.startswith("wasm32"):
        reached = frozenset(
            feature
            for feature in reached
            if feature not in _WASM_RUNTIME_STABLE_EXCLUDED_FEATURES
        )
    return reached


_WASM_RUNTIME_STABLE_EXCLUDED_FEATURES = frozenset(
    {
        "stdlib_tk",
        "stdlib_net",
        "stdlib_ast",
        "stdlib_unicode_names",
        "sqlite",
    }
)

_MICRO_BASE_RUNTIME_FEATURES: tuple[str, ...] = (
    "stdlib_asyncio",
    "stdlib_collections",
    "stdlib_fs_extra",
    "stdlib_logging",
    "stdlib_logging_ext",
)


def _runtime_builtin_features_for_profile(
    stdlib_profile: str | None,
    *,
    target_triple: str | None,
) -> list[str]:
    effective_profile = stdlib_profile or DEFAULT_STDLIB_PROFILE
    if target_triple is not None and target_triple.startswith("wasm32"):
        # Phase 0b: the WASM archive is built from a coarser, separate feature
        # plan (``_wasm_runtime_feature_plan`` only ever links ``stdlib_micro``
        # or ``_WASM_RUNTIME_FULL_FEATURES``), and the WASM-micro availability
        # surface here is deliberately the full domain set minus the stable
        # WASM exclusions — NOT the narrow Cargo ``stdlib_micro`` expansion.
        # Deriving the WASM surface from the Cargo ladder would NARROW it (e.g.
        # drop ``stdlib_crypto`` from WASM-micro) and wrongly refuse working
        # WASM builds, so the WASM path stays as-is until the reachability
        # redesign (Phase 1+) gives selection a per-target ceiling. Native is
        # where the §1.3 drift bites and is fixed below.
        all_features = (
            list(_ALL_BUILTIN_FEATURES)
            + list(_WASM_DOMAIN_AVAILABILITY_FEATURES)
            + list(_MICRO_BASE_RUNTIME_FEATURES)
        )
        if effective_profile != "micro":
            return list(_WASM_RUNTIME_FULL_FEATURES)
        return [
            feature
            for feature in all_features
            if feature not in _WASM_RUNTIME_STABLE_EXCLUDED_FEATURES
        ]
    # Native: the stdlib-ladder features come straight from the Cargo feature
    # chain (the single source of truth), unioned with the orthogonal builtin
    # tree-shaking features (always linkable regardless of stdlib tier).  Order
    # is deterministic so fingerprints/diagnostics are stable.
    ladder = profile_link_features(effective_profile, target_triple=None)
    return list(_ALL_BUILTIN_FEATURES) + sorted(
        ladder.difference(_ALL_BUILTIN_FEATURES)
    )


# Phase 0b: the WASM profile-availability surface (consumed ONLY by the wasm32
# branch of ``_runtime_builtin_features_for_profile``).  Historically the
# WASM-micro gate exposed the full domain set minus the stable WASM exclusions,
# so a wasm build importing e.g. ``hashlib``/``decimal`` is NOT refused on
# WASM-micro (unlike native micro).  This list preserves that exact surface
# verbatim while the native ladder migrates to the Cargo chain; reconciling the
# WASM availability/selection model with the Cargo ladder is deferred to the
# reachability redesign (Phase 1+), which gives selection a per-target ceiling.
# Do NOT derive this from the Cargo ladder — that would narrow the WASM surface
# and wrongly refuse working WASM builds.
_WASM_DOMAIN_AVAILABILITY_FEATURES: tuple[str, ...] = (
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
)


_WASM_RUNTIME_FULL_FEATURES: tuple[str, ...] = (
    "stdlib_crypto",
    "stdlib_compression",
    "stdlib_serialization",
    "stdlib_archive",
    "stdlib_asyncio",
    "stdlib_collections",
    "stdlib_fs_extra",
    "stdlib_logging",
    "stdlib_logging_ext",
    "builtin_set",
    "builtin_complex",
    "builtin_memoryview",
    "builtin_contextvars",
    "builtin_fcntl",
)


def _wasm_runtime_feature_plan(
    *,
    stdlib_profile: str | None,
    runtime_features: tuple[str, ...],
    builtin_features: Collection[str],
    resolved_modules: set[str] | frozenset[str] | None,
) -> tuple[bool, tuple[str, ...], tuple[str, ...]]:
    effective_profile = stdlib_profile or DEFAULT_STDLIB_PROFILE
    if effective_profile == "micro":
        cargo_features = tuple(
            _dedupe_preserve_order(
                list(runtime_features) + sorted(builtin_features) + ["stdlib_micro"]
            )
        )
    else:
        full_feature_order = list(_WASM_RUNTIME_FULL_FEATURES)
        builtin_feature_set = frozenset(builtin_features)
        cargo_features = tuple(
            _dedupe_preserve_order(
                list(runtime_features)
                + [
                    feature
                    for feature in full_feature_order
                    if feature in builtin_feature_set
                ]
                + (
                    ["molt_gpu_primitives"]
                    if _resolved_modules_require_gpu_primitives(
                        frozenset(resolved_modules or ())
                    )
                    else []
                )
            )
        )
    fingerprint_features = tuple(
        _dedupe_preserve_order(list(cargo_features) + ["no-default-features"])
    )
    return True, cargo_features, fingerprint_features


def _builtin_features_from_import_graph(
    resolved_modules: Collection[str] | None,
    stdlib_profile: str | None,
) -> list[str]:
    del resolved_modules
    return _runtime_builtin_features_for_profile(
        stdlib_profile,
        target_triple=None,
    )

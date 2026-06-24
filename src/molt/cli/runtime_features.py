from __future__ import annotations

import functools
import os
from typing import Collection

from molt.cli.capability_spec import _dedupe_preserve_order
from molt.cli.config_resolution import _coerce_bool


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

_ALL_DOMAIN_FEATURES: tuple[str, ...] = (
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
    effective_profile = stdlib_profile or "micro"
    all_features = (
        list(_ALL_BUILTIN_FEATURES)
        + list(_ALL_DOMAIN_FEATURES)
        + list(_MICRO_BASE_RUNTIME_FEATURES)
    )
    if target_triple is not None and target_triple.startswith("wasm32"):
        if effective_profile != "micro":
            return list(_WASM_RUNTIME_FULL_FEATURES)
        return [
            feature
            for feature in all_features
            if feature not in _WASM_RUNTIME_STABLE_EXCLUDED_FEATURES
        ]
    if effective_profile != "micro":
        return all_features
    return list(_ALL_BUILTIN_FEATURES) + list(_MICRO_BASE_RUNTIME_FEATURES)


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
    effective_profile = stdlib_profile or "micro"
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

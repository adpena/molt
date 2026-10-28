"""Shared content-addressed cache for atomic runtime WASM generations."""

from __future__ import annotations

from pathlib import Path

from molt.cli import runtime_wasm_cache_diagnostics as _cache_diagnostics
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.process_environment import ProcessEnvironment
from molt.default_paths import (
    _default_molt_cache,
    runtime_wasm_cache_entry,
)
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.cli.runtime_wasm_generation import (
    SourceRuntimeWasmGeneration,
    _runtime_wasm_store,
    publish_runtime_wasm_generation,
    read_runtime_wasm_generation,
    runtime_wasm_generation_path,
)


def _runtime_wasm_cache_generation_dir(identity: RuntimeBuildIdentity) -> Path:
    return runtime_wasm_cache_entry(_default_molt_cache(), identity.family_digest)[0]


def _cached_pair_paths(identity: RuntimeBuildIdentity) -> tuple[Path, Path, Path]:
    root = _runtime_wasm_cache_generation_dir(identity)
    shared = root / "molt_runtime.wasm"
    reloc = root / "molt_runtime_reloc.wasm"
    return shared, reloc, runtime_wasm_generation_path(shared)


def admit_runtime_wasm_pair_from_shared_cache(
    *,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    environment: ProcessEnvironment,
) -> SourceRuntimeWasmGeneration | None:
    """Admit the exact cached family in place; the caller checks its exports.

    Codegen pins this admitted receipt before emitting app layout. The mutable
    host selection is not rewritten and no second runtime copy is needed.
    """
    _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_attempts"] = (
        int(_cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_attempts"]) + 1
    )
    _cache_shared, _cache_reloc, cache_manifest = _cached_pair_paths(shared_identity)
    if not cache_manifest.is_file():
        _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_misses"] = (
            int(_cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_misses"]) + 1
        )
        return None
    try:
        # Renew the same store that publication and codegen binding use. The
        # lock covers admission through renewal so eviction cannot win between
        # observing the members and recording their use. No member is copied.
        with _runtime_wasm_store(cache_manifest.parent) as store:
            generation = read_runtime_wasm_generation(
                cache_manifest,
                expected_shared_identity=shared_identity,
                expected_reloc_identity=reloc_identity,
            )
            if generation is None:
                raise ValueError("cached runtime WASM generation failed admission")
            generation.cargo_environment.verify(environment)
            store.use(generation.shared.name, generation.reloc.name)
    except (OSError, ValueError):
        _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_failures"] = (
            int(_cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_failures"]) + 1
        )
        return None
    _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_hits"] = (
        int(_cache_diagnostics._RUNTIME_WASM_CACHE_STATS["admit_hits"]) + 1
    )
    return generation


def publish_runtime_wasm_pair_to_shared_cache(
    *,
    shared: Path,
    reloc: Path,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    cargo_environment: CargoEnvironmentInputs,
) -> str | None:
    _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["publish_attempts"] = (
        int(_cache_diagnostics._RUNTIME_WASM_CACHE_STATS["publish_attempts"]) + 1
    )
    cache_shared, cache_reloc, _cache_manifest = _cached_pair_paths(shared_identity)
    try:
        publish_runtime_wasm_generation(
            cache_shared,
            cache_reloc,
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            cargo_environment=cargo_environment,
            source_shared=shared,
            source_reloc=reloc,
        )
    except (OSError, ValueError) as exc:
        reason = f"runtime generation publication failed: {exc}"
        _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["publish_failures"] = (
            int(_cache_diagnostics._RUNTIME_WASM_CACHE_STATS["publish_failures"]) + 1
        )
        _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["last_publish_failure"] = reason
        return reason
    _cache_diagnostics._RUNTIME_WASM_CACHE_STATS["publish_successes"] = (
        int(_cache_diagnostics._RUNTIME_WASM_CACHE_STATS["publish_successes"]) + 1
    )
    return None

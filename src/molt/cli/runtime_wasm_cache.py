"""Shared content-addressed cache for atomic runtime WASM generations."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable

from molt.cli.default_paths import _default_molt_cache
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmGeneration,
    hydrate_runtime_wasm_generation,
    publish_runtime_wasm_generation,
    read_runtime_wasm_generation,
    runtime_wasm_generation_path,
)


_RUNTIME_WASM_CACHE_STATS: dict[str, int | str] = {
    "hydrate_attempts": 0,
    "hydrate_hits": 0,
    "hydrate_misses": 0,
    "hydrate_failures": 0,
    "publish_attempts": 0,
    "publish_successes": 0,
    "publish_failures": 0,
    "last_publish_failure": "",
}


def _runtime_wasm_cache_diagnostics_snapshot() -> dict[str, Any] | None:
    hydrate_attempts = int(_RUNTIME_WASM_CACHE_STATS["hydrate_attempts"])
    publish_attempts = int(_RUNTIME_WASM_CACHE_STATS["publish_attempts"])
    if hydrate_attempts == 0 and publish_attempts == 0:
        return None
    hydrate_hits = int(_RUNTIME_WASM_CACHE_STATS["hydrate_hits"])
    publish_successes = int(_RUNTIME_WASM_CACHE_STATS["publish_successes"])
    snapshot: dict[str, Any] = {
        "hydrate_attempts": hydrate_attempts,
        "hydrate_hits": hydrate_hits,
        "hydrate_misses": int(_RUNTIME_WASM_CACHE_STATS["hydrate_misses"]),
        "hydrate_failures": int(_RUNTIME_WASM_CACHE_STATS["hydrate_failures"]),
        "hydrate_hit_rate": round(hydrate_hits / max(1, hydrate_attempts), 6),
        "publish_attempts": publish_attempts,
        "publish_successes": publish_successes,
        "publish_failures": int(_RUNTIME_WASM_CACHE_STATS["publish_failures"]),
        "publish_success_rate": round(publish_successes / max(1, publish_attempts), 6),
    }
    failure = str(_RUNTIME_WASM_CACHE_STATS["last_publish_failure"])
    if failure:
        snapshot["last_publish_failure"] = failure
    return snapshot


def _reset_runtime_wasm_cache_diagnostics() -> None:
    for key in list(_RUNTIME_WASM_CACHE_STATS):
        _RUNTIME_WASM_CACHE_STATS[key] = "" if key == "last_publish_failure" else 0


def _shared_runtime_wasm_cache_root() -> Path:
    return _default_molt_cache() / "runtime_wasm_generations"


def _runtime_wasm_cache_generation_dir(identity: RuntimeBuildIdentity) -> Path:
    return _shared_runtime_wasm_cache_root() / identity.pair_digest


def _cached_pair_paths(identity: RuntimeBuildIdentity) -> tuple[Path, Path, Path]:
    root = _runtime_wasm_cache_generation_dir(identity)
    shared = root / "molt_runtime.wasm"
    reloc = root / "molt_runtime_reloc.wasm"
    return shared, reloc, runtime_wasm_generation_path(shared)


def hydrate_runtime_wasm_pair_from_shared_cache(
    *,
    dest_shared: Path,
    dest_reloc: Path,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    is_valid_shared: Callable[[Path], bool],
    is_valid_reloc: Callable[[Path], bool],
) -> RuntimeWasmGeneration | None:
    _RUNTIME_WASM_CACHE_STATS["hydrate_attempts"] = (
        int(_RUNTIME_WASM_CACHE_STATS["hydrate_attempts"]) + 1
    )
    _cache_shared, _cache_reloc, cache_manifest = _cached_pair_paths(shared_identity)
    if not cache_manifest.is_file():
        _RUNTIME_WASM_CACHE_STATS["hydrate_misses"] = (
            int(_RUNTIME_WASM_CACHE_STATS["hydrate_misses"]) + 1
        )
        return None
    generation = read_runtime_wasm_generation(
        cache_manifest,
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    if generation is None:
        _RUNTIME_WASM_CACHE_STATS["hydrate_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["hydrate_failures"]) + 1
        )
        return None
    if not is_valid_shared(generation.shared) or not is_valid_reloc(generation.reloc):
        _RUNTIME_WASM_CACHE_STATS["hydrate_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["hydrate_failures"]) + 1
        )
        return None
    try:
        hydrated = hydrate_runtime_wasm_generation(
            source_manifest=cache_manifest,
            dest_shared=dest_shared,
            dest_reloc=dest_reloc,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
    except (OSError, ValueError):
        _RUNTIME_WASM_CACHE_STATS["hydrate_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["hydrate_failures"]) + 1
        )
        return None
    _RUNTIME_WASM_CACHE_STATS["hydrate_hits"] = (
        int(_RUNTIME_WASM_CACHE_STATS["hydrate_hits"]) + 1
    )
    return hydrated


def publish_runtime_wasm_pair_to_shared_cache(
    *,
    shared: Path,
    reloc: Path,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
) -> str | None:
    _RUNTIME_WASM_CACHE_STATS["publish_attempts"] = (
        int(_RUNTIME_WASM_CACHE_STATS["publish_attempts"]) + 1
    )
    cache_shared, cache_reloc, _cache_manifest = _cached_pair_paths(shared_identity)
    try:
        cache_shared.parent.mkdir(parents=True, exist_ok=True)
        publish_runtime_wasm_generation(
            cache_shared,
            cache_reloc,
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            source_shared=shared,
            source_reloc=reloc,
        )
    except (OSError, ValueError) as exc:
        reason = f"runtime generation publication failed: {exc}"
        _RUNTIME_WASM_CACHE_STATS["publish_failures"] = (
            int(_RUNTIME_WASM_CACHE_STATS["publish_failures"]) + 1
        )
        _RUNTIME_WASM_CACHE_STATS["last_publish_failure"] = reason
        return reason
    _RUNTIME_WASM_CACHE_STATS["publish_successes"] = (
        int(_RUNTIME_WASM_CACHE_STATS["publish_successes"]) + 1
    )
    return None

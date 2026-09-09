"""Runtime WASM cache observations, independent of generation/build execution."""

from __future__ import annotations

from typing import Any


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

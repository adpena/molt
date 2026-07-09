from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.cli.runtime_paths import _build_state_root
from molt.compiler_analysis import (
    backend_ir_binary_image_analysis_cache_key,
    backend_ir_binary_image_analysis_payload,
)

_BACKEND_IR_ANALYSIS_CACHE_SCHEMA_VERSION = 1


@dataclass(frozen=True)
class _BackendIrAnalysisCacheResult:
    payload: dict[str, Any]
    cache_hit: bool
    cache_key: str
    cache_path: Path


def _backend_ir_analysis_cache_path(project_root: Path, cache_key: str) -> Path:
    return (
        _build_state_root(project_root)
        / "backend_ir_binary_image_analysis"
        / cache_key[:2]
        / f"{cache_key}.json"
    )


def _cached_backend_ir_binary_image_analysis_payload(
    *,
    project_root: Path,
    ir: Mapping[str, Any],
) -> _BackendIrAnalysisCacheResult:
    cache_key = backend_ir_binary_image_analysis_cache_key(ir)
    cache_path = _backend_ir_analysis_cache_path(project_root, cache_key)
    cached = _read_cached_json_object(cache_path)
    if (
        cached is not None
        and cached.get("schema_version") == _BACKEND_IR_ANALYSIS_CACHE_SCHEMA_VERSION
        and cached.get("cache_key") == cache_key
        and isinstance(cached.get("payload"), dict)
    ):
        return _BackendIrAnalysisCacheResult(
            payload=dict(cached["payload"]),
            cache_hit=True,
            cache_key=cache_key,
            cache_path=cache_path,
        )
    payload = backend_ir_binary_image_analysis_payload(ir)
    try:
        _write_cached_json_object(
            cache_path,
            {
                "schema_version": _BACKEND_IR_ANALYSIS_CACHE_SCHEMA_VERSION,
                "cache_key": cache_key,
                "payload": payload,
            },
        )
    except OSError:
        pass
    return _BackendIrAnalysisCacheResult(
        payload=payload,
        cache_hit=False,
        cache_key=cache_key,
        cache_path=cache_path,
    )

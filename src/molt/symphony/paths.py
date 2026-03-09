from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Mapping

DEFAULT_SYMPHONY_PROJECT_KEY = "molt"


def _env_get(env: Mapping[str, str] | None, key: str) -> str:
    if env is None:
        return str(os.environ.get(key) or "").strip()
    return str(env.get(key) or "").strip()


def _normalize_path(path: Path) -> Path:
    return path.expanduser().resolve()


def _platform_ext_root_candidates() -> tuple[Path, ...]:
    if os.name == "nt":
        return (
            Path("E:/APDataStore/Molt"),
            Path("D:/APDataStore/Molt"),
            Path("F:/APDataStore/Molt"),
        )
    if sys.platform == "darwin":
        return (Path("/Volumes/APDataStore/Molt"), Path("/Volumes/Molt"))
    return (
        Path("/mnt/APDataStore/Molt"),
        Path("/media/APDataStore/Molt"),
        Path("/Volumes/APDataStore/Molt"),
    )


def _autodetect_existing_root(candidates: tuple[Path, ...]) -> Path:
    for candidate in candidates:
        normalized = _normalize_path(candidate)
        if normalized.is_dir():
            return normalized
    return _normalize_path(candidates[0])


def default_molt_ext_root() -> Path:
    return _autodetect_existing_root(_platform_ext_root_candidates())


def resolve_molt_ext_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_EXT_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return default_molt_ext_root()


def default_symphony_parent_root(env: Mapping[str, str] | None = None) -> Path:
    return _normalize_path(resolve_molt_ext_root(env).parent / "symphony")


def resolve_symphony_parent_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_PARENT_ROOT") or _env_get(
        env, "MOLT_SYMPHONY_CANONICAL_ROOT"
    )
    if configured:
        return _normalize_path(Path(configured))
    return default_symphony_parent_root(env)


def resolve_symphony_project_key(env: Mapping[str, str] | None = None) -> str:
    raw = _env_get(env, "MOLT_SYMPHONY_PROJECT_KEY") or DEFAULT_SYMPHONY_PROJECT_KEY
    normalized = "".join(
        ch for ch in raw.strip().lower() if ch.isalnum() or ch in {"-", "_"}
    )
    return normalized or DEFAULT_SYMPHONY_PROJECT_KEY


def resolve_symphony_store_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_STORE_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return resolve_symphony_parent_root(env) / resolve_symphony_project_key(env)


def symphony_log_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_LOG_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return resolve_symphony_store_root(env) / "logs"


def symphony_state_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_STATE_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return resolve_symphony_store_root(env) / "state"


def symphony_artifact_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_ARTIFACT_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return resolve_symphony_store_root(env) / "artifacts"


def symphony_workspace_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_WORKSPACE_ROOT") or _env_get(
        env, "MOLT_WORKSPACE_ROOT"
    )
    if configured:
        return _normalize_path(Path(configured))
    return resolve_symphony_store_root(env) / "sessions" / "workspaces"


def symphony_durable_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_DURABLE_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_state_root(env) / "durable_memory"


def symphony_security_events_file(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_SECURITY_EVENTS_FILE")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_log_root(env) / "security" / "events.jsonl"


def symphony_api_token_file(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_API_TOKEN_FILE")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_state_root(env) / "secrets" / "dashboard_api_token"


def symphony_metrics_dir(env: Mapping[str, str] | None = None) -> Path:
    return symphony_log_root(env) / "metrics"


def symphony_readiness_dir(env: Mapping[str, str] | None = None) -> Path:
    return symphony_log_root(env) / "readiness"


def symphony_recursive_loop_dir(env: Mapping[str, str] | None = None) -> Path:
    return symphony_log_root(env) / "recursive_loop"


def symphony_perf_reports_dir(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_PERF_GUARD_REPORTS_DIR")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_log_root(env)


def symphony_dlq_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_DLQ_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_state_root(env) / "dlq"


def symphony_dlq_events_file(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_DLQ_EVENTS_FILE")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_dlq_root(env) / "events.jsonl"


def symphony_taste_memory_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_TASTE_MEMORY_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_state_root(env) / "taste_memory"


def symphony_taste_memory_events_file(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_TASTE_MEMORY_EVENTS_FILE")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_taste_memory_root(env) / "events.jsonl"


def symphony_taste_memory_distillations_dir(
    env: Mapping[str, str] | None = None,
) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_TASTE_MEMORY_DISTILLATIONS_DIR")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_taste_memory_root(env) / "distillations"


def symphony_tool_promotion_root(env: Mapping[str, str] | None = None) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_TOOL_PROMOTION_ROOT")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_state_root(env) / "tool_promotion"


def symphony_tool_promotion_events_file(
    env: Mapping[str, str] | None = None,
) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_TOOL_PROMOTION_EVENTS_FILE")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_tool_promotion_root(env) / "events.jsonl"


def symphony_tool_promotion_distillations_dir(
    env: Mapping[str, str] | None = None,
) -> Path:
    configured = _env_get(env, "MOLT_SYMPHONY_TOOL_PROMOTION_DISTILLATIONS_DIR")
    if configured:
        return _normalize_path(Path(configured))
    return symphony_tool_promotion_root(env) / "distillations"


def is_within(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError:
        return False
    return True

from __future__ import annotations

import os
import tempfile
from pathlib import Path
from typing import Any

from .errors import ConfigValidationError
from .models import (
    AgentConfig,
    CodexConfig,
    PollingConfig,
    RuntimeConfig,
    ServerConfig,
    TrackerConfig,
    WorkflowDefinition,
    WorkspaceConfig,
    WorkspaceHooks,
)


def build_runtime_config(workflow: WorkflowDefinition) -> RuntimeConfig:
    root = workflow.config
    tracker_raw = _get_map(root, "tracker")
    polling_raw = _get_map(root, "polling")
    workspace_raw = _get_map(root, "workspace")
    hooks_raw = _get_map(root, "hooks")
    agent_raw = _get_map(root, "agent")
    codex_raw = _get_map(root, "codex")
    server_raw = _get_map(root, "server")

    tracker_kind = str(tracker_raw.get("kind", "")).strip().lower()
    tracker_endpoint = str(
        tracker_raw.get("endpoint", "https://api.linear.app/graphql")
    ).strip()
    tracker_api_key = _resolve_env_token(tracker_raw.get("api_key"))
    if not tracker_api_key:
        tracker_api_key = _resolve_env_token("$LINEAR_API_KEY")
    tracker_project_slugs = _coerce_project_slugs(tracker_raw)

    active_states = _coerce_state_list(
        tracker_raw.get("active_states"), ["Todo", "In Progress"]
    )
    terminal_states = _coerce_state_list(
        tracker_raw.get("terminal_states"),
        ["Closed", "Cancelled", "Canceled", "Duplicate", "Done"],
    )

    tracker = TrackerConfig(
        kind=tracker_kind,
        endpoint=tracker_endpoint,
        api_key=tracker_api_key,
        project_slugs=tuple(tracker_project_slugs),
        active_states=tuple(active_states),
        terminal_states=tuple(terminal_states),
    )

    polling = PollingConfig(
        interval_ms=_coerce_int(polling_raw.get("interval_ms"), 30000, 1000)
    )

    workspace_root = _coerce_path(
        workspace_raw.get("root"),
        str(Path(tempfile.gettempdir()) / "symphony_workspaces"),
    )
    workspace = WorkspaceConfig(root=workspace_root)

    hooks_timeout = _coerce_int(hooks_raw.get("timeout_ms"), 60000, 1)
    hooks = WorkspaceHooks(
        after_create=_coerce_optional_script(hooks_raw.get("after_create")),
        before_run=_coerce_optional_script(hooks_raw.get("before_run")),
        after_run=_coerce_optional_script(hooks_raw.get("after_run")),
        before_remove=_coerce_optional_script(hooks_raw.get("before_remove")),
        timeout_ms=hooks_timeout,
    )

    per_state = _coerce_positive_int_map(
        agent_raw.get("max_concurrent_agents_by_state", {})
    )
    role_pools = _coerce_positive_int_map(agent_raw.get("role_pools", {}))
    default_role = _coerce_role_name(agent_raw.get("default_role"), "executor")
    agent = AgentConfig(
        max_concurrent_agents=_coerce_int(
            agent_raw.get("max_concurrent_agents"), 10, 1
        ),
        max_turns=_coerce_int(agent_raw.get("max_turns"), 20, 1),
        max_retry_backoff_ms=_coerce_int(
            agent_raw.get("max_retry_backoff_ms"), 300000, 1000
        ),
        max_concurrent_agents_by_state=per_state,
        role_pools=role_pools,
        default_role=default_role,
    )

    codex = CodexConfig(
        command=str(codex_raw.get("command", "codex app-server")).strip(),
        approval_policy=codex_raw.get(
            "approval_policy",
            {
                "reject": {
                    "sandbox_approval": True,
                    "rules": True,
                    "mcp_elicitations": True,
                }
            },
        ),
        thread_sandbox=codex_raw.get("thread_sandbox", "workspace-write"),
        turn_sandbox_policy=codex_raw.get(
            "turn_sandbox_policy",
            {
                "type": "workspaceWrite",
            },
        ),
        turn_timeout_ms=_coerce_int(codex_raw.get("turn_timeout_ms"), 3600000, 1000),
        read_timeout_ms=_coerce_int(codex_raw.get("read_timeout_ms"), 5000, 100),
        stall_timeout_ms=_coerce_int_allow_zero(
            codex_raw.get("stall_timeout_ms"), 300000
        ),
    )

    port_value = server_raw.get("port")
    server = ServerConfig(port=_coerce_optional_port(port_value))

    return RuntimeConfig(
        tracker=tracker,
        polling=polling,
        workspace=workspace,
        hooks=hooks,
        agent=agent,
        codex=codex,
        server=server,
    )


def validate_dispatch_config(config: RuntimeConfig) -> None:
    if config.tracker.kind != "linear":
        raise ConfigValidationError("unsupported_tracker_kind")
    if not config.tracker.api_key:
        raise ConfigValidationError("missing_tracker_api_key")
    if not config.tracker.project_slugs:
        raise ConfigValidationError("missing_tracker_project_slug")
    if not config.codex.command:
        raise ConfigValidationError("missing_codex_command")


def normalize_state_name(state: str) -> str:
    return state.strip().lower()


def normalize_state_list(states: tuple[str, ...]) -> set[str]:
    return {normalize_state_name(item) for item in states}


def _get_map(root: dict[str, Any], key: str) -> dict[str, Any]:
    value = root.get(key, {})
    if isinstance(value, dict):
        return value
    return {}


def _coerce_optional_script(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _coerce_state_list(value: Any, default: list[str]) -> list[str]:
    if value is None:
        return list(default)
    if isinstance(value, str):
        candidates = [part.strip() for part in value.split(",")]
    elif isinstance(value, list):
        candidates = [str(part).strip() for part in value]
    else:
        return list(default)
    normalized = [item for item in candidates if item]
    return normalized or list(default)


def _coerce_project_slugs(tracker_raw: dict[str, Any]) -> list[str]:
    explicit = tracker_raw.get("project_slugs")
    if explicit is not None:
        return _coerce_slug_list(explicit)
    legacy = tracker_raw.get("project_slug")
    return _coerce_slug_list(legacy)


def _coerce_slug_list(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        candidates: list[str] = []
        for item in value:
            resolved = _resolve_env_token(item)
            if resolved:
                candidates.extend(part.strip() for part in resolved.split(","))
    else:
        resolved = _resolve_env_token(value)
        if not resolved:
            return []
        candidates = [part.strip() for part in resolved.split(",")]

    seen: set[str] = set()
    result: list[str] = []
    for item in candidates:
        if not item:
            continue
        if item in seen:
            continue
        seen.add(item)
        result.append(item)
    return result


def _coerce_int(value: Any, default: int, minimum: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        parsed = default
    if parsed < minimum:
        return default
    return parsed


def _coerce_int_allow_zero(value: Any, default: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        parsed = default
    return parsed


def _coerce_positive_int_map(value: Any) -> dict[str, int]:
    if not isinstance(value, dict):
        return {}
    result: dict[str, int] = {}
    for raw_key, raw_val in value.items():
        key = normalize_state_name(str(raw_key))
        if not key:
            continue
        try:
            num = int(raw_val)
        except (TypeError, ValueError):
            continue
        if num <= 0:
            continue
        result[key] = num
    return result


def _coerce_role_name(value: Any, default: str) -> str:
    raw = str(value).strip().lower() if value is not None else ""
    if not raw:
        return default
    normalized = "".join(ch for ch in raw if ch.isalnum() or ch in {"-", "_"})
    return normalized or default


def _resolve_env_token(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    if not text:
        return None
    if text.startswith("$") and len(text) > 1:
        env_name = text[1:]
        resolved = os.environ.get(env_name, "").strip()
        return resolved or None
    return text


def _coerce_path(value: Any, default: str) -> Path:
    raw_value = str(value).strip() if value is not None else ""
    if not raw_value:
        raw_value = default
    expanded_vars = os.path.expandvars(raw_value)
    if "$" in expanded_vars:
        expanded_vars = os.path.expandvars(default)
    expanded = os.path.expanduser(expanded_vars)
    path = Path(expanded)
    if not path.is_absolute() and _is_bare_relative_path(expanded):
        # Preserve bare relative roots exactly as provided (spec-compatible).
        return path
    return path.resolve()


def _coerce_optional_port(value: Any) -> int | None:
    if value is None:
        return None
    try:
        port = int(value)
    except (TypeError, ValueError):
        return None
    if port < 0 or port > 65535:
        return None
    return port


def _is_bare_relative_path(value: str) -> bool:
    if not value:
        return False
    if os.path.isabs(value):
        return False
    if "/" in value or "\\" in value:
        return False
    return True

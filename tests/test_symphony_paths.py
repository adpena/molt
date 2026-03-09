from __future__ import annotations

from pathlib import Path

from molt.symphony import paths


def test_default_store_layout_is_project_scoped() -> None:
    env = {"MOLT_SYMPHONY_PROJECT_KEY": "molt"}
    parent_root = paths.resolve_symphony_parent_root(env)
    assert parent_root == paths.default_symphony_parent_root(env)
    assert paths.resolve_symphony_store_root(env) == parent_root / "molt"
    assert paths.symphony_log_root(env) == parent_root / "molt" / "logs"
    assert paths.symphony_state_root(env) == parent_root / "molt" / "state"
    assert paths.symphony_workspace_root(env) == (
        parent_root / "molt" / "sessions" / "workspaces"
    )
    assert paths.symphony_tool_promotion_events_file(env) == (
        parent_root / "molt" / "state" / "tool_promotion" / "events.jsonl"
    )
    assert paths.symphony_tool_promotion_distillations_dir(env) == (
        parent_root / "molt" / "state" / "tool_promotion" / "distillations"
    )


def test_explicit_store_root_overrides_parent_and_key(tmp_path: Path) -> None:
    parent = tmp_path / "symphony"
    explicit_store = parent / "custom-molt"
    env = {
        "MOLT_SYMPHONY_PARENT_ROOT": str(parent),
        "MOLT_SYMPHONY_PROJECT_KEY": "vertigo",
        "MOLT_SYMPHONY_STORE_ROOT": str(explicit_store),
    }
    assert paths.resolve_symphony_store_root(env) == explicit_store
    assert paths.symphony_durable_root(env) == explicit_store / "state" / "durable_memory"


def test_is_within_detects_cross_project_escape(tmp_path: Path) -> None:
    parent = tmp_path / "symphony"
    assert paths.is_within(parent / "molt" / "logs", parent) is True
    assert paths.is_within(tmp_path / "vertigo" / "logs", parent) is False

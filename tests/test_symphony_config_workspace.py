from __future__ import annotations

import os
from datetime import UTC, datetime
from pathlib import Path

import pytest

from molt.symphony.config import build_runtime_config
from molt.symphony.models import WorkflowDefinition
from molt.symphony.workspace import (
    WorkspaceError,
    WorkspaceManager,
    sanitize_workspace_key,
)


def _workflow(tmp_path: Path, config: dict) -> WorkflowDefinition:
    path = tmp_path / "WORKFLOW.md"
    return WorkflowDefinition(
        path=path,
        config=config,
        prompt_template="hello",
        loaded_at=datetime.now(UTC),
        mtime_ns=0,
    )


def test_workspace_root_supports_env_suffix(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ext_root = tmp_path / "ext"
    ext_root.mkdir(parents=True)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))

    workflow = _workflow(
        tmp_path,
        {
            "tracker": {
                "kind": "linear",
                "api_key": "token",
                "project_slug": "proj",
            },
            "workspace": {
                "root": "$MOLT_EXT_ROOT/symphony_workspaces",
            },
        },
    )
    config = build_runtime_config(workflow)
    assert config.workspace.root == (ext_root / "symphony_workspaces").resolve()


def test_workspace_sanitize() -> None:
    assert sanitize_workspace_key("MT-123") == "MT-123"
    assert sanitize_workspace_key("MT 123/abc") == "MT_123_abc"


def test_workspace_root_preserves_bare_relative_value(tmp_path: Path) -> None:
    workflow = _workflow(
        tmp_path,
        {
            "tracker": {
                "kind": "linear",
                "api_key": "token",
                "project_slug": "proj",
            },
            "workspace": {"root": "symphony_workspaces"},
        },
    )
    config = build_runtime_config(workflow)
    assert str(config.workspace.root) == "symphony_workspaces"


def test_workspace_creation_and_after_create_only_once(tmp_path: Path) -> None:
    root = tmp_path / "ws"
    manager = WorkspaceManager(
        config=build_runtime_config(
            _workflow(
                tmp_path,
                {
                    "tracker": {
                        "kind": "linear",
                        "api_key": "token",
                        "project_slug": "proj",
                    },
                    "workspace": {"root": str(root)},
                    "hooks": {
                        "after_create": "touch created_once",
                    },
                },
            )
        ).workspace,
        hooks=build_runtime_config(
            _workflow(
                tmp_path,
                {
                    "tracker": {
                        "kind": "linear",
                        "api_key": "token",
                        "project_slug": "proj",
                    },
                    "workspace": {"root": str(root)},
                    "hooks": {
                        "after_create": "touch created_once",
                    },
                },
            )
        ).hooks,
    )

    first = manager.create_for_issue("MT-10")
    assert first.created_now is True
    assert (first.path / "created_once").exists()

    os.remove(first.path / "created_once")
    second = manager.create_for_issue("MT-10")
    assert second.created_now is False
    assert not (second.path / "created_once").exists()


def test_workspace_root_containment(tmp_path: Path) -> None:
    config = build_runtime_config(
        _workflow(
            tmp_path,
            {
                "tracker": {
                    "kind": "linear",
                    "api_key": "token",
                    "project_slug": "proj",
                },
                "workspace": {"root": str(tmp_path / "root")},
            },
        )
    )
    manager = WorkspaceManager(config.workspace, config.hooks)

    with pytest.raises(WorkspaceError):
        manager.ensure_workspace_cwd(Path("/tmp"))


def test_tracker_project_slug_supports_comma_separated_values(tmp_path: Path) -> None:
    workflow = _workflow(
        tmp_path,
        {
            "tracker": {
                "kind": "linear",
                "api_key": "token",
                "project_slug": "proj-a, proj-b ,proj-a",
            },
        },
    )
    config = build_runtime_config(workflow)
    assert config.tracker.project_slugs == ("proj-a", "proj-b")


def test_agent_role_pools_and_default_role(tmp_path: Path) -> None:
    workflow = _workflow(
        tmp_path,
        {
            "tracker": {
                "kind": "linear",
                "api_key": "token",
                "project_slug": "proj",
            },
            "agent": {
                "default_role": "Fixer",
                "role_pools": {
                    "triage": 2,
                    "fixer": 4,
                    "formalizer": 0,
                    "reviewer": "3",
                },
            },
        },
    )
    config = build_runtime_config(workflow)
    assert config.agent.default_role == "fixer"
    assert config.agent.role_pools == {
        "triage": 2,
        "fixer": 4,
        "reviewer": 3,
    }

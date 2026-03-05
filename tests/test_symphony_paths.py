from __future__ import annotations

from pathlib import Path

from molt.symphony import paths


def test_default_store_layout_is_project_scoped() -> None:
    env = {"MOLT_SYMPHONY_PROJECT_KEY": "molt"}
    assert paths.resolve_symphony_parent_root(env) == Path("/Volumes/APDataStore/symphony")
    assert paths.resolve_symphony_store_root(env) == Path("/Volumes/APDataStore/symphony/molt")
    assert paths.symphony_log_root(env) == Path("/Volumes/APDataStore/symphony/molt/logs")
    assert paths.symphony_state_root(env) == Path("/Volumes/APDataStore/symphony/molt/state")
    assert paths.symphony_workspace_root(env) == Path(
        "/Volumes/APDataStore/symphony/molt/sessions/workspaces"
    )


def test_explicit_store_root_overrides_parent_and_key() -> None:
    env = {
        "MOLT_SYMPHONY_PARENT_ROOT": "/Volumes/APDataStore/symphony",
        "MOLT_SYMPHONY_PROJECT_KEY": "vertigo",
        "MOLT_SYMPHONY_STORE_ROOT": "/Volumes/APDataStore/symphony/custom-molt",
    }
    assert paths.resolve_symphony_store_root(env) == Path(
        "/Volumes/APDataStore/symphony/custom-molt"
    )
    assert paths.symphony_durable_root(env) == Path(
        "/Volumes/APDataStore/symphony/custom-molt/state/durable_memory"
    )


def test_is_within_detects_cross_project_escape() -> None:
    parent = Path("/Volumes/APDataStore/symphony")
    assert paths.is_within(Path("/Volumes/APDataStore/symphony/molt/logs"), parent) is True
    assert paths.is_within(Path("/Volumes/APDataStore/vertigo/logs"), parent) is False

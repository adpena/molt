from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import tools.symphony_bootstrap as symphony_bootstrap


def test_parse_env_file_reads_key_values(tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        "# comment\nLINEAR_API_KEY=abc123\nMOLT_LINEAR_PROJECT_SLUG = molt-runtime\n",
        encoding="utf-8",
    )
    parsed = symphony_bootstrap._parse_env_file(env_file)
    assert parsed["LINEAR_API_KEY"] == "abc123"
    assert parsed["MOLT_LINEAR_PROJECT_SLUG"] == "molt-runtime"


def test_sync_env_defaults_fills_external_paths(monkeypatch, tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    ext_root = tmp_path / "ext"
    ext_root.mkdir()
    env_file = tmp_path / "symphony.env"

    monkeypatch.setattr(
        symphony_bootstrap,
        "_git_origin",
        lambda _: "git@github.com:org/molt.git",
    )

    summary = symphony_bootstrap._sync_env_defaults(
        repo_root=repo_root,
        ext_root=ext_root,
        env_file=env_file,
        project_slug="molt-project",
        source_repo_url=None,
    )
    loaded = symphony_bootstrap._parse_env_file(env_file)

    assert summary["missing_required"] == ["LINEAR_API_KEY"]
    assert loaded["MOLT_LINEAR_PROJECT_SLUG"] == "molt-project"
    assert loaded["MOLT_SOURCE_REPO_URL"] == "git@github.com:org/molt.git"
    assert loaded["MOLT_SYMPHONY_SYNC_REMOTE"] == "origin"
    assert loaded["MOLT_SYMPHONY_SYNC_BRANCH"] == "main"
    assert loaded["MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS"] == "adpena,symphony"
    assert loaded["MOLT_SYMPHONY_API_TOKEN_FILE"] == str(
        ext_root / "logs" / "symphony" / "secrets" / "dashboard_api_token"
    )
    assert loaded["MOLT_SYMPHONY_ENFORCE_ORIGIN"] == "1"
    assert loaded["MOLT_SYMPHONY_REQUIRE_CSRF_HEADER"] == "1"
    assert loaded["MOLT_SYMPHONY_MAX_HTTP_CONNECTIONS"] == "96"
    assert loaded["MOLT_SYMPHONY_MAX_STREAM_CLIENTS"] == "16"
    assert loaded["MOLT_SYMPHONY_STREAM_MAX_AGE_SECONDS"] == "300"
    assert loaded["MOLT_SYMPHONY_EVENT_QUEUE_MAX"] == "8192"
    assert loaded["MOLT_SYMPHONY_EVENT_QUEUE_DROP_LOG_INTERVAL"] == "250"
    assert loaded["MOLT_EXT_ROOT"] == str(ext_root)


def test_ensure_git_hooks_path_sets_default(monkeypatch, tmp_path: Path) -> None:
    calls: list[list[str]] = []

    def _fake_run(cmd: list[str], *, cwd: Path | None = None):  # type: ignore[no-untyped-def]
        calls.append(list(cmd))
        if cmd[:5] == ["git", "config", "--local", "--get", "core.hooksPath"]:
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        if cmd[:4] == ["git", "config", "--local", "core.hooksPath"]:
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        raise AssertionError(f"unexpected command: {cmd}")

    monkeypatch.setattr(symphony_bootstrap, "_run", _fake_run)
    result = symphony_bootstrap._ensure_git_hooks_path(
        repo_root=tmp_path,
        force_path=False,
    )
    assert result["ok"] is True
    assert result["updated"] is True
    assert result["path"] == ".githooks"
    assert calls[-1] == ["git", "config", "--local", "core.hooksPath", ".githooks"]


def test_ensure_git_hooks_path_preserves_existing_custom_path(
    monkeypatch, tmp_path: Path
) -> None:
    calls: list[list[str]] = []

    def _fake_run(cmd: list[str], *, cwd: Path | None = None):  # type: ignore[no-untyped-def]
        calls.append(list(cmd))
        if cmd[:5] == ["git", "config", "--local", "--get", "core.hooksPath"]:
            return SimpleNamespace(returncode=0, stdout="custom-hooks\n", stderr="")
        raise AssertionError(f"unexpected command: {cmd}")

    monkeypatch.setattr(symphony_bootstrap, "_run", _fake_run)
    result = symphony_bootstrap._ensure_git_hooks_path(
        repo_root=tmp_path,
        force_path=False,
    )
    assert result["ok"] is True
    assert result["updated"] is False
    assert result["path"] == "custom-hooks"
    assert result["reason"] == "preserved_existing"
    assert calls == [["git", "config", "--local", "--get", "core.hooksPath"]]

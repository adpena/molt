from __future__ import annotations

import json
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


def test_write_env_file_quotes_whitespace_values(tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    symphony_bootstrap._write_env_file(
        env_file,
        {
            "MOLT_QUINT_NODE_FALLBACK": "npx -y node@22",
            "MOLT_EXT_ROOT": "/Volumes/APDataStore/Molt",
        },
    )
    text = env_file.read_text(encoding="utf-8")
    assert "MOLT_QUINT_NODE_FALLBACK='npx -y node@22'" in text
    parsed = symphony_bootstrap._parse_env_file(env_file)
    assert parsed["MOLT_QUINT_NODE_FALLBACK"] == "npx -y node@22"


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
    monkeypatch.setattr(
        symphony_bootstrap,
        "_default_java_home",
        lambda: "/opt/java/home",
    )

    summary = symphony_bootstrap._sync_env_defaults(
        repo_root=repo_root,
        ext_root=ext_root,
        env_file=env_file,
        project_slug="molt-project",
        source_repo_url=None,
    )
    loaded = symphony_bootstrap._parse_env_file(env_file)
    parent_root = ext_root.parent / "symphony"
    store_root = parent_root / "molt"

    assert summary["missing_required"] == ["LINEAR_API_KEY"]
    assert loaded["MOLT_LINEAR_PROJECT_SLUG"] == "molt-project"
    assert loaded["MOLT_SOURCE_REPO_URL"] == "git@github.com:org/molt.git"
    assert loaded["MOLT_SYMPHONY_SYNC_REMOTE"] == "origin"
    assert loaded["MOLT_SYMPHONY_SYNC_BRANCH"] == "main"
    assert loaded["MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS"] == "symphony"
    assert loaded["MOLT_SYMPHONY_TOOL_STATE_DETAIL"] == "compact"
    assert loaded["MOLT_SYMPHONY_MAX_CODEX_EVENT_COUNTERS"] == "64"
    assert loaded["MOLT_SYMPHONY_DURABLE_MEMORY"] == "1"
    assert loaded["MOLT_SYMPHONY_PARENT_ROOT"] == str(parent_root)
    assert loaded["MOLT_SYMPHONY_PROJECT_KEY"] == "molt"
    assert loaded["MOLT_SYMPHONY_STORE_ROOT"] == str(store_root)
    assert loaded["MOLT_SYMPHONY_DURABLE_ROOT"] == str(
        store_root / "state" / "durable_memory"
    )
    assert loaded["MOLT_SYMPHONY_DLQ_EVENTS_FILE"] == str(
        store_root / "state" / "dlq" / "events.jsonl"
    )
    assert loaded["MOLT_SYMPHONY_TASTE_MEMORY_EVENTS_FILE"] == str(
        store_root / "state" / "taste_memory" / "events.jsonl"
    )
    assert loaded["MOLT_SYMPHONY_TASTE_MEMORY_DISTILLATIONS_DIR"] == str(
        store_root / "state" / "taste_memory" / "distillations"
    )
    assert loaded["MOLT_SYMPHONY_TOOL_PROMOTION_EVENTS_FILE"] == str(
        store_root / "state" / "tool_promotion" / "events.jsonl"
    )
    assert loaded["MOLT_SYMPHONY_TOOL_PROMOTION_DISTILLATIONS_DIR"] == str(
        store_root / "state" / "tool_promotion" / "distillations"
    )
    assert loaded["MOLT_SYMPHONY_DURABLE_SYNC_SECONDS"] == "180"
    assert loaded["MOLT_SYMPHONY_DSPY_ENABLE"] == "0"
    assert loaded["MOLT_SYMPHONY_DSPY_MODEL"] == "openai/gpt-4.1-mini"
    assert loaded["MOLT_SYMPHONY_DSPY_API_KEY_ENV"] == "OPENAI_API_KEY"
    assert (
        loaded["MOLT_QUINT_NODE_FALLBACK"]
        == symphony_bootstrap._default_quint_node_fallback()
    )
    assert loaded["JAVA_HOME"] == "/opt/java/home"
    assert loaded["MOLT_SYMPHONY_API_TOKEN_FILE"] == str(
        store_root / "state" / "secrets" / "dashboard_api_token"
    )
    assert loaded["MOLT_SYMPHONY_SECURITY_PROFILE"] == "local"
    assert loaded["MOLT_SYMPHONY_BIND_HOST"] == "127.0.0.1"
    assert loaded["MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND"] == "0"
    assert loaded["MOLT_SYMPHONY_ALLOW_QUERY_TOKEN"] == "1"
    assert loaded["MOLT_SYMPHONY_DISABLE_DASHBOARD_UI"] == "0"
    assert loaded["MOLT_SYMPHONY_SECURITY_EVENTS_FILE"] == str(
        store_root / "logs" / "security" / "events.jsonl"
    )
    assert loaded["MOLT_SYMPHONY_ENFORCE_ORIGIN"] == "1"
    assert loaded["MOLT_SYMPHONY_REQUIRE_CSRF_HEADER"] == "1"
    assert loaded["MOLT_SYMPHONY_MAX_HTTP_CONNECTIONS"] == "96"
    assert loaded["MOLT_SYMPHONY_MAX_STREAM_CLIENTS"] == "16"
    assert loaded["MOLT_SYMPHONY_STREAM_MAX_AGE_SECONDS"] == "300"
    assert loaded["MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS"] == "240"
    assert loaded["MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS"] == "60"
    assert loaded["MOLT_SYMPHONY_EVENT_QUEUE_MAX"] == "8192"
    assert loaded["MOLT_SYMPHONY_EVENT_QUEUE_DROP_LOG_INTERVAL"] == "250"
    assert loaded["MOLT_APALACHE_WORK_DIR"] == str(ext_root / "tmp" / "apalache")
    assert loaded["MOLT_EXT_ROOT"] == str(ext_root)


def test_sync_env_defaults_upgrades_legacy_quint_fallback(
    monkeypatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    ext_root = tmp_path / "ext"
    ext_root.mkdir()
    env_file = tmp_path / "symphony.env"
    env_file.write_text("MOLT_QUINT_NODE_FALLBACK=npx -y node@22\n", encoding="utf-8")

    monkeypatch.setattr(
        symphony_bootstrap,
        "_default_quint_node_fallback",
        lambda: "/opt/node22/bin/node",
    )

    symphony_bootstrap._sync_env_defaults(
        repo_root=repo_root,
        ext_root=ext_root,
        env_file=env_file,
        project_slug="molt-project",
        source_repo_url="git@github.com:org/molt.git",
    )
    loaded = symphony_bootstrap._parse_env_file(env_file)
    assert loaded["MOLT_QUINT_NODE_FALLBACK"] == "/opt/node22/bin/node"


def test_formal_toolchain_report_detects_fallback_viability(
    monkeypatch, tmp_path: Path
) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        "MOLT_QUINT_NODE_FALLBACK=npx -y node@22\nJAVA_HOME=/opt/java/home\n",
        encoding="utf-8",
    )

    def _fake_which(name: str) -> str | None:
        mapping = {
            "node": "/usr/bin/node",
            "quint": "/usr/bin/quint",
            "lake": "/usr/bin/lake",
            "java": "/usr/bin/java",
        }
        return mapping.get(name)

    def _fake_probe(
        cmd: list[str],
        *,
        cwd: Path | None = None,
        timeout_seconds: int = 10,
        env: dict[str, str] | None = None,
    ):  # type: ignore[no-untyped-def]
        if cmd == ["/usr/bin/node", "--version"]:
            return {"ok": True, "returncode": 0, "stdout": "v25.8.0", "stderr": ""}
        if cmd == ["/usr/bin/java", "-version"]:
            return {"ok": True, "returncode": 0, "stdout": "openjdk 21", "stderr": ""}
        if cmd == ["/usr/bin/lake", "--version"]:
            return {"ok": True, "returncode": 0, "stdout": "Lake 4", "stderr": ""}
        if cmd == ["/usr/bin/quint", "--version"]:
            return {"ok": False, "returncode": 1, "stdout": "", "stderr": "esm"}
        if cmd == ["npx", "-y", "node@22", "/usr/bin/quint", "--version"]:
            return {"ok": True, "returncode": 0, "stdout": "0.31.0", "stderr": ""}
        return {"ok": False, "returncode": 1, "stdout": "", "stderr": "unknown"}

    monkeypatch.setattr(symphony_bootstrap.shutil, "which", _fake_which)
    monkeypatch.setattr(symphony_bootstrap, "_probe_command", _fake_probe)

    report = symphony_bootstrap._formal_toolchain_report(tmp_path, env_file)
    assert report["status"] == "warn"
    assert report["quint"]["direct_probe"]["ok"] is False
    assert report["quint"]["fallback_probe"]["ok"] is True
    assert report["quint"]["fallback_command"] == ["npx", "-y", "node@22"]


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


def test_configure_lin_cli_reports_missing_binary(monkeypatch, tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text("LINEAR_API_KEY=abc123\n", encoding="utf-8")
    monkeypatch.setattr(symphony_bootstrap.shutil, "which", lambda _: None)
    result = symphony_bootstrap._configure_lin_cli(env_file=env_file, home_dir=tmp_path)
    assert result["configured"] is False
    assert result["reason"] == "lin_not_found"


def test_configure_lin_cli_requires_api_key(monkeypatch, tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text("", encoding="utf-8")
    monkeypatch.setattr(symphony_bootstrap.shutil, "which", lambda _: "/usr/bin/lin")
    result = symphony_bootstrap._configure_lin_cli(env_file=env_file, home_dir=tmp_path)
    assert result["configured"] is False
    assert result["reason"] == "missing_linear_api_key"


def test_configure_lin_cli_writes_store_file(monkeypatch, tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text("LINEAR_API_KEY=test-token\n", encoding="utf-8")
    monkeypatch.setattr(symphony_bootstrap.shutil, "which", lambda _: "/usr/bin/lin")
    result = symphony_bootstrap._configure_lin_cli(env_file=env_file, home_dir=tmp_path)
    assert result["configured"] is True
    assert result["reason"] == "ok"
    assert result["lin_path"] == "/usr/bin/lin"
    assert result["changed"] is True
    store_file = tmp_path / ".lin" / "store.apiKey.json"
    assert store_file.exists()
    assert json.loads(store_file.read_text(encoding="utf-8")) == "test-token"

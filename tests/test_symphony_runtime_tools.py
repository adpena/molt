from __future__ import annotations

import os
from pathlib import Path
from types import SimpleNamespace

import pytest

import tools.symphony_launchd as symphony_launchd
import tools.symphony_run as symphony_run


def test_symphony_run_load_env_file_parses_key_values(tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        "# comment\n"
        "LINEAR_API_KEY=abc123\n"
        "MOLT_LINEAR_PROJECT_SLUG = molt-main\n"
        "EMPTY=\n",
        encoding="utf-8",
    )
    loaded = symphony_run._load_env_file(env_file)
    assert loaded["LINEAR_API_KEY"] == "abc123"
    assert loaded["MOLT_LINEAR_PROJECT_SLUG"] == "molt-main"
    assert loaded["EMPTY"] == ""


def test_symphony_run_main_uses_env_file_and_launches(
    monkeypatch, tmp_path: Path
) -> None:
    java_home = tmp_path / "jdk"
    (java_home / "bin").mkdir(parents=True)

    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        "LINEAR_API_KEY=abc123\n"
        "MOLT_LINEAR_PROJECT_SLUG=molt-main\n"
        f"MOLT_EXT_ROOT={tmp_path / 'ext'}\n"
        "MOLT_SOURCE_REPO_URL=git@github.com:org/molt.git\n",
        encoding="utf-8",
    )

    launched: dict[str, object] = {}

    def _fake_run(cmd, *, env, check):  # type: ignore[no-untyped-def]
        launched["cmd"] = cmd
        launched["env"] = env
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(symphony_run, "ensure_external_root", lambda _: None)
    monkeypatch.setattr(
        symphony_run,
        "_default_repo_url",
        lambda _: "git@github.com:org/molt.git",
    )
    monkeypatch.setattr(symphony_run, "_default_java_home", lambda: str(java_home))
    monkeypatch.setattr(symphony_run.shutil, "which", lambda _: "/usr/bin/uv")
    monkeypatch.setattr(symphony_run.subprocess, "run", _fake_run)

    rc = symphony_run.main(
        [
            "WORKFLOW.md",
            "--once",
            "--env-file",
            str(env_file),
            "--exec-mode",
            "python",
        ]
    )
    assert rc == 0
    assert launched["cmd"] == [  # type: ignore[index]
        "/usr/bin/uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "-m",
        "molt.symphony",
        "WORKFLOW.md",
        "--once",
    ]
    env = launched["env"]  # type: ignore[index]
    assert isinstance(env, dict)
    assert env["LINEAR_API_KEY"] == "abc123"
    assert env["MOLT_LINEAR_PROJECT_SLUG"] == "molt-main"
    assert env["MOLT_SYMPHONY_EXEC_MODE"] == "python"
    assert env["MOLT_SYMPHONY_SYNC_REMOTE"] == "origin"
    assert env["MOLT_SYMPHONY_SYNC_BRANCH"] == "main"
    assert env["MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS"] == "adpena,symphony"
    assert (
        env["MOLT_QUINT_NODE_FALLBACK"] == symphony_run._default_quint_node_fallback()
    )
    assert env["MOLT_APALACHE_WORK_DIR"] == str(
        Path(env["MOLT_EXT_ROOT"]) / "tmp" / "apalache"
    )
    assert env["JAVA_HOME"] == str(java_home)
    assert env["PATH"].split(os.pathsep)[0] == str(java_home / "bin")
    assert env["MOLT_SYMPHONY_ENFORCE_ORIGIN"] == "1"
    assert env["MOLT_SYMPHONY_REQUIRE_CSRF_HEADER"] == "1"
    assert env["MOLT_SYMPHONY_EVENT_QUEUE_MAX"] == "8192"
    assert env["MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS"] == "240"
    assert env["MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS"] == "60"
    assert env["MOLT_SYMPHONY_SECURITY_PROFILE"] == "local"
    assert env["MOLT_SYMPHONY_BIND_HOST"] == "127.0.0.1"
    assert env["MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND"] == "0"
    assert env["MOLT_SYMPHONY_ALLOW_QUERY_TOKEN"] == "1"
    assert env["MOLT_SYMPHONY_DISABLE_DASHBOARD_UI"] == "0"
    assert env["MOLT_SYMPHONY_API_TOKEN"]
    assert Path(env["MOLT_SYMPHONY_API_TOKEN_FILE"]).exists()
    assert Path(env["MOLT_SYMPHONY_SECURITY_EVENTS_FILE"]).parent.exists()


def test_symphony_run_main_requires_linear_token(monkeypatch, tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        f"MOLT_EXT_ROOT={tmp_path / 'ext'}\nMOLT_LINEAR_PROJECT_SLUG=molt-main\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(symphony_run, "ensure_external_root", lambda _: None)
    monkeypatch.setattr(symphony_run, "_default_repo_url", lambda _: None)
    monkeypatch.setattr(symphony_run.shutil, "which", lambda _: "/usr/bin/uv")
    with pytest.raises(RuntimeError):
        symphony_run.main(["WORKFLOW.md", "--env-file", str(env_file)])


def test_symphony_launchd_build_program_includes_env_file(tmp_path: Path) -> None:
    args = symphony_launchd.build_program(
        repo_root=tmp_path,
        python_bin="/usr/bin/python3",
        port=8089,
        env_file=tmp_path / "symphony.env",
        exec_mode="python",
        molt_profile="dev",
        molt_build_args=[],
        compiled_output=None,
    )
    assert args == [
        "/usr/bin/python3",
        "tools/symphony_run.py",
        "WORKFLOW.md",
        "--port",
        "8089",
        "--env-file",
        str(tmp_path / "symphony.env"),
        "--exec-mode",
        "python",
        "--molt-profile",
        "dev",
    ]


def test_symphony_launchd_watchdog_program_includes_timing(tmp_path: Path) -> None:
    args = symphony_launchd.build_watchdog_program(
        repo_root=tmp_path,
        python_bin="/usr/bin/python3",
        env_file=tmp_path / "symphony.env",
        interval_ms=1500,
        quiet_ms=1200,
        cooldown_ms=5000,
        state_url="http://127.0.0.1:8089/api/v1/state",
        state_timeout_ms=600,
        defer_log_interval_ms=12000,
        restart_when_idle=True,
        perf_check=True,
        perf_interval_ms=86_400_000,
        perf_timeout_ms=1_800_000,
        perf_command=None,
        perf_defer_when_busy=True,
    )
    assert args == [
        "/usr/bin/python3",
        "tools/symphony_watchdog.py",
        "--repo-root",
        str(tmp_path),
        "--env-file",
        str(tmp_path / "symphony.env"),
        "--service-label",
        "com.molt.symphony",
        "--interval-ms",
        "1500",
        "--quiet-ms",
        "1200",
        "--cooldown-ms",
        "5000",
        "--state-url",
        "http://127.0.0.1:8089/api/v1/state",
        "--state-timeout-ms",
        "600",
        "--defer-log-interval-ms",
        "12000",
        "--perf-interval-ms",
        "86400000",
        "--perf-timeout-ms",
        "1800000",
        "--perf-check",
        "--perf-defer-when-busy",
        "--restart-when-idle",
    ]


def test_symphony_run_main_exec_mode_molt_run(monkeypatch, tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        "LINEAR_API_KEY=abc123\n"
        "MOLT_LINEAR_PROJECT_SLUG=molt-main\n"
        f"MOLT_EXT_ROOT={tmp_path / 'ext'}\n",
        encoding="utf-8",
    )
    launched: dict[str, object] = {}

    def _fake_run(cmd, *, env, check):  # type: ignore[no-untyped-def]
        launched["cmd"] = cmd
        launched["env"] = env
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(symphony_run, "ensure_external_root", lambda _: None)
    monkeypatch.setattr(symphony_run, "_default_repo_url", lambda _: None)
    monkeypatch.setattr(symphony_run.shutil, "which", lambda _: "/usr/bin/uv")
    monkeypatch.setattr(symphony_run.subprocess, "run", _fake_run)

    rc = symphony_run.main(
        [
            "WORKFLOW.md",
            "--once",
            "--env-file",
            str(env_file),
            "--exec-mode",
            "molt-run",
            "--molt-profile",
            "dev",
        ]
    )
    assert rc == 0
    assert launched["cmd"] == [  # type: ignore[index]
        "/usr/bin/uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "-m",
        "molt.cli",
        "run",
        "tools/symphony_entry.py",
        "--profile",
        "dev",
        "--build-arg",
        "--respect-pythonpath",
        "--",
        "WORKFLOW.md",
        "--once",
    ]


def test_symphony_run_main_exec_mode_molt_bin_rebuild(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        "LINEAR_API_KEY=abc123\n"
        "MOLT_LINEAR_PROJECT_SLUG=molt-main\n"
        f"MOLT_EXT_ROOT={tmp_path / 'ext'}\n",
        encoding="utf-8",
    )
    calls: list[list[str]] = []

    def _fake_run(cmd, *, env, check):  # type: ignore[no-untyped-def]
        calls.append(list(cmd))
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(symphony_run, "ensure_external_root", lambda _: None)
    monkeypatch.setattr(symphony_run, "_default_repo_url", lambda _: None)
    monkeypatch.setattr(symphony_run.shutil, "which", lambda _: "/usr/bin/uv")
    monkeypatch.setattr(symphony_run.subprocess, "run", _fake_run)

    rc = symphony_run.main(
        [
            "WORKFLOW.md",
            "--once",
            "--env-file",
            str(env_file),
            "--exec-mode",
            "molt-bin",
            "--rebuild-binary",
            "--compiled-output",
            str(tmp_path / "bin" / "symphony_molt"),
        ]
    )
    assert rc == 0
    assert len(calls) == 2
    assert calls[0][:11] == [
        "/usr/bin/uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "-m",
        "molt.cli",
        "build",
        "tools/symphony_entry.py",
        "--profile",
        "dev",
    ]
    assert calls[1][0] == str(tmp_path / "bin" / "symphony_molt")


def test_dashboard_security_defaults_generate_token_file(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    env: dict[str, str] = {}
    monkeypatch.setattr(symphony_run.secrets, "token_urlsafe", lambda _n: "tok-123")

    symphony_run._ensure_dashboard_security_defaults(
        env=env,
        ext_root=tmp_path,
        port=8089,
    )

    token_file = Path(env["MOLT_SYMPHONY_API_TOKEN_FILE"])
    assert token_file.exists()
    assert token_file.read_text(encoding="utf-8").strip() == "tok-123"
    assert env["MOLT_SYMPHONY_API_TOKEN"] == "tok-123"
    assert env["MOLT_SYMPHONY_DASHBOARD_TOKEN"] == "tok-123"
    assert env["MOLT_SYMPHONY_ALLOWED_ORIGINS"] == (
        "http://127.0.0.1:8089,http://localhost:8089"
    )


def test_dashboard_security_defaults_preserve_existing_token(
    tmp_path: Path,
) -> None:
    env: dict[str, str] = {"MOLT_SYMPHONY_API_TOKEN": "preset-token"}

    symphony_run._ensure_dashboard_security_defaults(
        env=env,
        ext_root=tmp_path,
        port=8090,
    )

    assert env["MOLT_SYMPHONY_API_TOKEN"] == "preset-token"
    assert env["MOLT_SYMPHONY_DASHBOARD_TOKEN"] == "preset-token"
    assert "MOLT_SYMPHONY_API_TOKEN_FILE" not in env

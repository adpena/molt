from __future__ import annotations

from pathlib import Path

import molt.dx as dx
from molt.dx import (
    CANONICAL_RUN_ENV_KEYS,
    DX_ENV_KEYS,
    DxProject,
    RunContext,
    render_env,
)
from tools import run_context_env


def test_run_context_installs_repo_local_defaults(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="test").canonical_env(
        {"PATH": "/usr/bin"},
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["CARGO_INCREMENTAL"] == "0"
    assert env["MOLT_CACHE"] == str(tmp_path.resolve() / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(tmp_path.resolve() / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(tmp_path.resolve() / "tmp")
    assert env["UV_CACHE_DIR"] == str(tmp_path.resolve() / ".uv-cache")
    assert env["TMPDIR"] == str(tmp_path.resolve() / "tmp")
    assert env["MOLT_SESSION_ID"].startswith("test-")


def test_run_context_preserves_explicit_root_and_session(tmp_path: Path) -> None:
    explicit_root = tmp_path / "external"
    explicit_target = tmp_path / "target-custom"
    env = RunContext(tmp_path, session_prefix="test").canonical_env(
        {
            "MOLT_EXT_ROOT": str(explicit_root),
            "CARGO_TARGET_DIR": str(explicit_target),
            "CARGO_INCREMENTAL": "1",
            "MOLT_SESSION_ID": "caller-session",
        },
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(explicit_root.resolve())
    assert env["CARGO_TARGET_DIR"] == str(explicit_target)
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(explicit_target)
    assert env["CARGO_INCREMENTAL"] == "1"
    assert env["MOLT_SESSION_ID"] == "caller-session"


def test_run_context_prefers_healthy_external_artifact_root(tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-ssd" / "Molt"
    repo_root.mkdir()
    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "TMPDIR": "/var/folders/example/T/",
        },
        create_dirs=True,
    )

    resolved_external = external_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["CARGO_TARGET_DIR"] == str(resolved_external / "target")
    assert env["MOLT_DIFF_TMPDIR"] == str(resolved_external / "tmp")
    assert resolved_external.is_dir()


def test_run_context_prefers_windows_local_appdata_artifact_root_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    local_appdata = tmp_path / "local-appdata"
    repo_root.mkdir()
    local_appdata.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "LOCALAPPDATA": str(local_appdata),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    resolved_external = (local_appdata / "Molt").resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["CARGO_TARGET_DIR"] == str(resolved_external / "target")
    assert env["MOLT_DIFF_TMPDIR"] == str(resolved_external / "tmp")
    assert env["TMPDIR"] == str(resolved_external / "tmp")
    assert resolved_external.is_dir()


def test_run_context_skips_unhealthy_windows_local_appdata_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    local_appdata = tmp_path / "local-appdata"
    temp_root = tmp_path / "temp"
    repo_root.mkdir()
    local_appdata.mkdir()
    temp_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")

    def fake_accepts_child_dirs(path: Path, *, create_dirs: bool) -> bool:
        del create_dirs
        return path != local_appdata / "Molt"

    monkeypatch.setattr(
        dx, "_artifact_root_accepts_child_dirs", fake_accepts_child_dirs
    )

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "LOCALAPPDATA": str(local_appdata),
            "TEMP": str(temp_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    resolved_external = (temp_root / "Molt").resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["TMPDIR"] == str(resolved_external / "tmp")


def test_run_context_preserves_nonambient_tmpdir_with_external_root(
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-ssd" / "Molt"
    explicit_tmp = tmp_path / "explicit-tmp"
    repo_root.mkdir()
    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "TMPDIR": str(explicit_tmp),
        },
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(external_root.resolve())
    assert env["TMPDIR"] == str(explicit_tmp)


def test_run_context_can_force_repo_defaults_except_explicit_keys(
    tmp_path: Path,
) -> None:
    ambient_root = tmp_path / "ambient"
    explicit_cache = tmp_path / "cache"
    forced_keys = tuple(key for key in CANONICAL_RUN_ENV_KEYS if key != "MOLT_CACHE")
    env = RunContext(tmp_path, session_prefix="forced").canonical_env(
        {
            "MOLT_EXT_ROOT": str(ambient_root),
            "MOLT_CACHE": str(explicit_cache),
            "MOLT_SESSION_ID": "ambient-session",
        },
        create_dirs=False,
        force_default_keys=forced_keys,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")
    assert env["MOLT_CACHE"] == str(explicit_cache)
    assert env["MOLT_SESSION_ID"].startswith("forced-")


def test_run_context_shell_exports_are_eval_safe(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="quote").canonical_env(
        {
            "MOLT_SESSION_ID": 'session-"$`\\',
        },
        create_dirs=False,
    )

    shell = run_context_env.emit_shell_exports(env, ("MOLT_SESSION_ID",))

    assert shell == 'export MOLT_SESSION_ID="session-\\"\\$\\`\\\\"'


def test_run_context_dx_env_installs_cross_platform_tool_defaults(
    tmp_path: Path,
) -> None:
    env = RunContext(tmp_path, session_prefix="dx").dx_env(
        {"MOLT_BACKEND_DAEMON_SOCKET_ROOT": str(tmp_path / "sockets")},
        create_dirs=False,
    )

    assert env["MOLT_BACKEND_DAEMON_SOCKET_DIR"].startswith(
        str((tmp_path / "sockets").resolve())
    )
    assert env["SCCACHE_DIR"] == str(tmp_path.resolve() / ".sccache")
    assert env["SCCACHE_CACHE_SIZE"] == "10G"
    assert env["MOLT_USE_SCCACHE"] == "1"
    assert env["MOLT_DIFF_ALLOW_RUSTC_WRAPPER"] == "1"
    assert env["MOLT_CACHE_MAX_GB"] == "30"
    assert env["MOLT_CACHE_MAX_AGE_DAYS"] == "30"


def test_dx_env_renders_shell_neutral_and_powershell(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="quote").dx_env(
        {
            "MOLT_SESSION_ID": "session-'value'",
        },
        create_dirs=False,
    )

    dotenv = render_env(env, ("MOLT_SESSION_ID",), "dotenv")
    powershell = render_env(env, ("MOLT_SESSION_ID",), "powershell")

    assert dotenv == "MOLT_SESSION_ID=session-'value'"
    assert powershell == "$env:MOLT_SESSION_ID = 'session-''value'''"


def test_dx_project_preserves_explicit_root_with_external_defaults(
    tmp_path: Path,
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    (project_root / "pyproject.toml").write_text(
        """
[tool.molt.dx]
prefer_external_artifacts = true

[tool.molt.dx.env]
MOLT_EXT_ROOT = "{artifact_root}"
CARGO_TARGET_DIR = "{artifact_root}/target"
MOLT_DIFF_CARGO_TARGET_DIR = "{artifact_root}/target"
MOLT_CACHE = "{artifact_root}/.molt_cache"
MOLT_DIFF_ROOT = "{artifact_root}/tmp/diff"
MOLT_DIFF_TMPDIR = "{artifact_root}/tmp"
UV_CACHE_DIR = "{artifact_root}/.uv-cache"
TMPDIR = "{artifact_root}/tmp"
PYTHONPATH = "{root}/src"
""".lstrip(),
        encoding="utf-8",
    )
    explicit_root = tmp_path / "operator-root"

    env = DxProject(project_root).canonical_env(
        {"PATH": "/usr/bin", "MOLT_EXT_ROOT": str(explicit_root)},
        create_dirs=False,
    )

    resolved_root = explicit_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_root)
    assert env["CARGO_TARGET_DIR"] == str(resolved_root / "target")
    assert env["MOLT_CACHE"] == str(resolved_root / ".molt_cache")
    assert env["PYTHONPATH"] == str(project_root / "src")


def test_dx_project_dx_env_uses_same_key_authority(tmp_path: Path) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    (project_root / "pyproject.toml").write_text(
        "[tool.molt.dx]\nprefer_external_artifacts = false\n",
        encoding="utf-8",
    )

    env = DxProject(project_root).dx_env({"PATH": "/usr/bin"}, create_dirs=False)

    assert tuple(key for key in DX_ENV_KEYS if key in env)
    assert env["MOLT_EXT_ROOT"] == str(project_root.resolve())
    assert env["SCCACHE_DIR"] == str(project_root.resolve() / ".sccache")

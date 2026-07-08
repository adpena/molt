from __future__ import annotations

import json
import os
from pathlib import Path

import molt.dx as dx
import pytest
from molt.dx import (
    CANONICAL_RUN_ENV_KEYS,
    DX_ENV_KEYS,
    DxProject,
    RunContext,
    development_artifacts_requested,
    development_artifact_env,
    render_env,
)
from tools import run_context_env


def _clear_run_context_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key in set(CANONICAL_RUN_ENV_KEYS) | set(DX_ENV_KEYS):
        monkeypatch.delenv(key, raising=False)


def test_run_context_installs_repo_local_defaults(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="test").canonical_env(
        {"PATH": "/usr/bin"},
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["MOLT_SESSION_ID"].startswith("test-")
    # No explicit MOLT_SESSION_ID -> STABLE persistent target dir (survives across
    # sessions for warm incremental rebuilds), not a per-session cold dir.
    assert env["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    # Incremental is ON by default now (fast warm rebuilds against the persistent
    # target dir); it is forced to "0" only where sccache is actually wired, which
    # canonical_env does not do.
    assert env["CARGO_INCREMENTAL"] == "1"
    assert env["MOLT_CACHE"] == str(tmp_path.resolve() / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(tmp_path.resolve() / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(tmp_path.resolve() / "tmp")
    assert env["UV_CACHE_DIR"] == str(tmp_path.resolve() / ".uv-cache")
    assert env["UV_PROJECT_ENVIRONMENT"].startswith(
        str(tmp_path.resolve() / "tmp" / "uv-project-envs")
    )
    assert env["PIP_CACHE_DIR"] == str(tmp_path.resolve() / ".pip-cache")
    assert env["PYTHONPYCACHEPREFIX"] == str(tmp_path.resolve() / "tmp" / "pycache")
    assert env["TMPDIR"] == str(tmp_path.resolve() / "tmp")
    assert env["TMP"] == env["TMPDIR"]
    assert env["TEMP"] == env["TMPDIR"]


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


def test_target_dir_stable_by_default_session_scoped_only_when_pinned(
    tmp_path: Path,
) -> None:
    # The cold-every-session killer: without a caller-pinned MOLT_SESSION_ID the
    # Cargo target dir is STABLE (persistent incremental cache reused across
    # sessions/processes); a caller that pins MOLT_SESSION_ID (perf/bench/test-shard
    # isolation) still gets an isolated per-session dir. Regressing this to a
    # per-PID default reintroduces a full cold compile on every invocation.
    ctx = RunContext(tmp_path, session_prefix="test")
    stable = ctx.canonical_env({"PATH": "/usr/bin"}, create_dirs=False)
    assert stable["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")

    pinned = ctx.canonical_env(
        {"PATH": "/usr/bin", "MOLT_SESSION_ID": "shard-7"}, create_dirs=False
    )
    assert pinned["CARGO_TARGET_DIR"] == str(
        tmp_path.resolve() / "target" / "sessions" / "shard-7"
    )


def test_development_artifact_env_session_id_overrides_ambient_session(
    tmp_path: Path,
) -> None:
    env = development_artifact_env(
        tmp_path,
        {
            "MOLT_SESSION_ID": "pytest-ambient",
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
        session_prefix="test",
        session_id="stable-proof",
        create_dirs=False,
    )

    assert env["MOLT_SESSION_ID"] == "stable-proof"
    assert env["CARGO_TARGET_DIR"] == str(
        Path(env["MOLT_EXT_ROOT"]) / "target" / "sessions" / "stable-proof"
    )


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
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
            "TMPDIR": "/var/folders/example/T/",
        },
        create_dirs=True,
    )

    resolved_external = external_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["CARGO_TARGET_DIR"] == str(resolved_external / "target")
    assert env["MOLT_DIFF_TMPDIR"] == str(resolved_external / "tmp")
    assert resolved_external.is_dir()


def test_run_context_prefers_windows_external_drive_artifact_root_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-drive" / "Molt"
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(
        dx, "_default_windows_external_artifact_roots", lambda: (external_root,)
    )
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    resolved_external = external_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["CARGO_TARGET_DIR"] == str(resolved_external / "target")
    assert env["MOLT_DIFF_TMPDIR"] == str(resolved_external / "tmp")
    assert env["TMPDIR"] == str(resolved_external / "tmp")
    assert resolved_external.is_dir()


def test_run_context_skips_unhealthy_windows_external_candidate(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    unhealthy = tmp_path / "unhealthy" / "Molt"
    healthy = tmp_path / "healthy" / "Molt"
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(
        dx, "_default_windows_external_artifact_roots", lambda: (unhealthy, healthy)
    )
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

    def fake_accepts_child_dirs(path: Path, *, create_dirs: bool) -> bool:
        del create_dirs
        return path != unhealthy

    monkeypatch.setattr(
        dx, "_artifact_root_accepts_child_dirs", fake_accepts_child_dirs
    )

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    resolved_external = healthy.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["TMPDIR"] == str(resolved_external / "tmp")


def test_run_context_rejects_windows_c_drive_artifact_root_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    c_root = tmp_path / "c-artifacts"
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: True)

    with pytest.raises(dx.DxConfigError, match="must not be placed on C"):
        RunContext(
            repo_root,
            session_prefix="test",
            prefer_external_artifacts=True,
        ).canonical_env(
            {
                "MOLT_EXT_ROOT": str(c_root),
                "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
                "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            },
            create_dirs=False,
        )


def test_run_context_prefers_external_without_rejecting_explicit_user_output_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    user_output_root = repo_root / "build" / "wasm" / "case"
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: True)

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXT_ROOT": str(user_output_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=False,
    )

    resolved_output_root = user_output_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_output_root)
    assert env["CARGO_TARGET_DIR"] == str(resolved_output_root / "target")


def test_run_context_require_external_artifacts_forces_candidate(
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-drive" / "Molt"
    repo_root.mkdir()

    env = RunContext(repo_root, session_prefix="test").canonical_env(
        {
            "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    assert env["MOLT_EXT_ROOT"] == str(external_root.resolve())
    assert env["CARGO_TARGET_DIR"] == str(external_root.resolve() / "target")


def test_development_artifacts_requested_is_explicit_dev_control_plane() -> None:
    assert not development_artifacts_requested({})
    assert not development_artifacts_requested({"MOLT_REQUIRE_EXTERNAL_ARTIFACTS": ""})
    assert development_artifacts_requested({"MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1"})
    assert development_artifacts_requested({"MOLT_PREFER_EXTERNAL_ARTIFACTS": "true"})
    assert development_artifacts_requested({"MOLT_USE_EXTERNAL_ARTIFACTS": "yes"})


def test_run_context_rejects_explicit_c_drive_canonical_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-drive" / "Molt"
    c_target = tmp_path / "c-drive-target"
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(
        dx,
        "_is_windows_c_drive_path",
        lambda path: path == c_target.resolve(),
    )

    with pytest.raises(dx.DxConfigError, match="CARGO_TARGET_DIR resolved"):
        RunContext(repo_root, session_prefix="test").canonical_env(
            {
                "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
                "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
                "MOLT_EXTERNAL_MIN_FREE_GB": "0",
                "CARGO_TARGET_DIR": str(c_target),
            },
            create_dirs=False,
        )


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
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
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
    assert env["MOLT_SESSION_ID"].startswith("forced-")
    assert env["CARGO_TARGET_DIR"] == str(
        tmp_path.resolve() / "target" / "sessions" / env["MOLT_SESSION_ID"]
    )
    assert env["MOLT_CACHE"] == str(explicit_cache)


def test_run_context_shell_exports_are_eval_safe(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="quote").canonical_env(
        {
            "MOLT_SESSION_ID": 'session-"$`\\',
        },
        create_dirs=False,
    )

    shell = run_context_env.emit_shell_exports(env, ("MOLT_SESSION_ID",))

    assert shell == 'export MOLT_SESSION_ID="session-\\"\\$\\`\\\\"'


def test_run_context_env_dx_uses_stable_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _clear_run_context_env(monkeypatch)
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--dx",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["MOLT_SESSION_ID"].startswith("run-")
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        tmp_path.resolve() / "tmp" / "uv-project-envs" / "dx__py3.12"
    )


def test_run_context_env_can_emit_session_scoped_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _clear_run_context_env(monkeypatch)
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--dx",
                "--session-scoped-uv-project-env",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        tmp_path.resolve() / "tmp" / "uv-project-envs" / env["MOLT_SESSION_ID"]
    )


def test_run_context_env_session_id_overrides_missing_ambient_session(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _clear_run_context_env(monkeypatch)
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--session-id",
                "witness-warm",
                "--dx",
                "--session-scoped-uv-project-env",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["MOLT_SESSION_ID"] == "witness-warm"
    assert env["CARGO_TARGET_DIR"] == str(
        tmp_path.resolve() / "target" / "sessions" / "witness-warm"
    )
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        tmp_path.resolve() / "tmp" / "uv-project-envs" / "witness-warm"
    )


def test_run_context_env_preserves_explicit_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    explicit = tmp_path / "custom-venv"
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(explicit))
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--dx",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["UV_PROJECT_ENVIRONMENT"] == str(explicit.resolve())


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
    # sccache is off-by-default on Windows (0 hits + mid-compile crashes there);
    # auto-on elsewhere. The default is platform-derived, so assert accordingly.
    assert env["MOLT_USE_SCCACHE"] == ("0" if os.name == "nt" else "1")
    assert env["MOLT_DIFF_ALLOW_RUSTC_WRAPPER"] == "1"
    assert env["MOLT_CACHE_MAX_GB"] == "30"
    assert env["MOLT_CACHE_MAX_AGE_DAYS"] == "30"


def test_dx_env_sets_uv_copy_link_mode_for_windows_exfat_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / dx.DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(
        dx, "_default_windows_external_artifact_roots", lambda: (external_root,)
    )
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)
    monkeypatch.setattr(dx, "_artifact_root_is_windows_exfat", lambda _path: True)

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).dx_env(
        {
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    assert env["MOLT_EXT_ROOT"] == str(external_root.resolve())
    assert env["UV_LINK_MODE"] == "copy"


def test_dx_env_preserves_explicit_uv_link_mode_on_exfat_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / dx.DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(
        dx, "_default_windows_external_artifact_roots", lambda: (external_root,)
    )
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)
    monkeypatch.setattr(dx, "_artifact_root_is_windows_exfat", lambda _path: True)

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).dx_env(
        {
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "UV_LINK_MODE": "hardlink",
        },
        create_dirs=True,
    )

    assert env["UV_LINK_MODE"] == "hardlink"


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
        {
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(explicit_root),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
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


def test_default_windows_artifact_roots_selects_only_preferred_label(
    monkeypatch,
    tmp_path: Path,
) -> None:
    # Label-only selection: the APDataStore-labeled volume is the ONLY default
    # candidate; a non-preferred (legacy E:) volume is EXCLUDED, not merely
    # ranked behind — the drive-letter-order fallback is deleted, not layered.
    apdatastore = tmp_path / "apdatastore"
    legacy = tmp_path / "legacy"
    apdatastore.mkdir()
    legacy.mkdir()
    labels = {apdatastore: "APDataStore", legacy: "BAT00_01"}
    monkeypatch.setattr(dx, "_windows_drive_roots", lambda: (apdatastore, legacy))
    monkeypatch.setattr(dx, "_windows_volume_label", lambda root: labels.get(root))

    roots = dx._default_windows_external_artifact_roots()

    assert roots == (apdatastore / dx.DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME,)


def test_default_toolchain_root_is_child_of_artifact_root(tmp_path: Path) -> None:
    molt_root = tmp_path / dx.DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME
    assert dx._default_toolchain_root_for_artifact_root(molt_root) == (
        molt_root / dx.DEFAULT_TARGET_ROOT_DIRNAME
    )
    other = tmp_path / "custom-root"
    assert dx._default_toolchain_root_for_artifact_root(other) == (
        other / dx.DEFAULT_TARGET_ROOT_DIRNAME
    )


@pytest.mark.skipif(os.name != "nt", reason="drive-letter rehoming is Windows-only")
def test_should_rehome_offvolume_toolchain_root(monkeypatch) -> None:
    monkeypatch.setattr(dx.os, "name", "nt")
    assert dx._should_rehome_toolchain_root(r"E:\molt-target", Path(r"D:\Molt"), {})
    # Same-volume legacy sibling is still stale: the APDataStore authority is
    # D:\Molt\target-root, not the empty old D:\molt-target default.
    assert dx._should_rehome_toolchain_root(r"D:\molt-target", Path(r"D:\Molt"), {})
    assert not dx._should_rehome_toolchain_root(
        r"D:\Molt\target-root", Path(r"D:\Molt"), {}
    )
    assert not dx._should_rehome_toolchain_root(
        r"D:\custom-toolchains", Path(r"D:\Molt"), {}
    )
    # Explicit operator opt-out preserves an intentional off-volume root.
    assert not dx._should_rehome_toolchain_root(
        r"E:\molt-target", Path(r"D:\Molt"), {"MOLT_PRESERVE_TARGET_ROOT": "1"}
    )


@pytest.mark.skipif(os.name != "nt", reason="drive-letter rehoming is Windows-only")
def test_canonical_env_rehomes_stale_target_root_and_adds_ruff_cache(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / dx.DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(
        dx, "_default_windows_external_artifact_roots", lambda: (external_root,)
    )
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

    env = RunContext(
        repo_root, session_prefix="test", prefer_external_artifacts=True
    ).canonical_env(
        {"MOLT_EXTERNAL_MIN_FREE_GB": "0", "MOLT_TARGET_ROOT": r"E:\molt-target"},
        create_dirs=True,
    )

    resolved = external_root.resolve()
    # A stale off-volume E:\molt-target is rehomed under the selected artifact
    # root — the legacy fallback is not honored.
    assert env["MOLT_TARGET_ROOT"] == str(resolved / dx.DEFAULT_TARGET_ROOT_DIRNAME)
    assert env["RUFF_CACHE_DIR"] == str(resolved / ".ruff-cache")


@pytest.mark.skipif(os.name != "nt", reason="drive-letter rehoming is Windows-only")
def test_canonical_env_rehomes_same_volume_legacy_target_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / dx.DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME
    legacy_sibling = external_root.parent / dx.LEGACY_WINDOWS_TARGET_ROOT_DIRNAME
    repo_root.mkdir()
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(
        dx, "_default_windows_external_artifact_roots", lambda: (external_root,)
    )
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

    env = RunContext(
        repo_root, session_prefix="test", prefer_external_artifacts=True
    ).canonical_env(
        {
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "MOLT_TARGET_ROOT": str(legacy_sibling),
        },
        create_dirs=True,
    )

    resolved = external_root.resolve()
    assert env["MOLT_TARGET_ROOT"] == str(resolved / dx.DEFAULT_TARGET_ROOT_DIRNAME)


def test_uv_project_env_is_stable_across_sessions(tmp_path: Path) -> None:
    """The uv project env authority must be STABLE across sessions.

    The DX churn fix: repeated `uv run --active` proofs (each a fresh
    MOLT_SESSION_ID) reuse ONE uv project environment instead of minting a fresh
    `.venv` per session. The env is keyed on (purpose, python), never the session.
    """
    ctx = RunContext(tmp_path, session_prefix="proof")
    base = {"MOLT_EXT_ROOT": str(tmp_path)}
    env_a = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-aaa-111"})
    env_b = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-bbb-222"})
    assert env_a == env_b
    assert env_a == (tmp_path / "tmp" / "uv-project-envs" / "dx__py3.12").resolve()
    assert "sess-aaa" not in str(env_a) and "sess-bbb" not in str(env_b)


def test_uv_project_env_session_scoped_opt_in(tmp_path: Path) -> None:
    """MOLT_UV_PROJECT_ENV_SESSION_SCOPED restores per-session isolation on demand."""
    ctx = RunContext(tmp_path, session_prefix="proof")
    base = {
        "MOLT_EXT_ROOT": str(tmp_path),
        "MOLT_UV_PROJECT_ENV_SESSION_SCOPED": "1",
    }
    env_a = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-aaa-111"})
    env_b = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-bbb-222"})
    assert env_a != env_b
    assert env_a == (tmp_path / "tmp" / "uv-project-envs" / "sess-aaa-111").resolve()


def test_uv_project_env_explicit_override_is_honored(tmp_path: Path) -> None:
    ctx = RunContext(tmp_path, session_prefix="proof")
    explicit = tmp_path / "explicit-venv"
    env = ctx.uv_project_env_dir(
        {"MOLT_EXT_ROOT": str(tmp_path), "UV_PROJECT_ENVIRONMENT": str(explicit)}
    )
    assert env == explicit.resolve()


def test_uv_project_env_custom_purpose_and_python(tmp_path: Path) -> None:
    ctx = RunContext(tmp_path, session_prefix="proof")
    env = ctx.uv_project_env_dir(
        {
            "MOLT_EXT_ROOT": str(tmp_path),
            "MOLT_UV_PROJECT_PURPOSE": "witness",
            "MOLT_UV_PROJECT_PYTHON": "3.13",
            "MOLT_SESSION_ID": "sess-ignored",
        }
    )
    assert env == (tmp_path / "tmp" / "uv-project-envs" / "witness__py3.13").resolve()


def test_auto_janitor_throttled_and_optout(monkeypatch, tmp_path):
    # Stale artifacts are cleaned BY DEFAULT: canonical_env fires a throttled,
    # detached, best-effort janitor sweep. Verify it spawns once, then throttles,
    # and honors the opt-out.
    import molt.dx as dx

    popen_calls = []
    monkeypatch.setattr(dx.subprocess, "Popen", lambda *a, **k: popen_calls.append(a))
    monkeypatch.delenv("MOLT_DISABLE_AUTO_JANITOR", raising=False)

    dx._maybe_sweep_stale_artifacts(tmp_path)
    assert len(popen_calls) == 1
    assert (tmp_path / ".molt_janitor_last_run").exists()

    # Immediately again -> throttled (recent marker), no second spawn.
    dx._maybe_sweep_stale_artifacts(tmp_path)
    assert len(popen_calls) == 1

    # Opt-out on a fresh root -> never spawns.
    other = tmp_path / "other"
    other.mkdir()
    monkeypatch.setenv("MOLT_DISABLE_AUTO_JANITOR", "1")
    dx._maybe_sweep_stale_artifacts(other)
    assert len(popen_calls) == 1
    assert not (other / ".molt_janitor_last_run").exists()


def test_onedrive_paths_rejected_fail_closed():
    # Nothing may ever drift back onto OneDrive — checkout OR artifacts fail closed.
    import pytest as _pytest
    import molt.dx as dx
    from pathlib import Path

    with _pytest.raises(dx.DxConfigError, match="OneDrive"):
        dx._reject_onedrive(Path(r"C:\Users\x\OneDrive\Documents\molt"), "checkout")
    with _pytest.raises(dx.DxConfigError, match="OneDrive"):
        dx._reject_onedrive(Path(r"C:\Users\x\OneDrive\molt\target"), "artifacts")
    # Canonical paths pass.
    dx._reject_onedrive(Path(r"C:\Molt\molt-src"), "checkout")
    dx._reject_onedrive(Path(r"C:\Molt"), "artifacts")
    assert dx._is_onedrive_path(Path(r"C:\Users\x\OneDrive\Documents\molt")) is True
    assert dx._is_onedrive_path(Path(r"C:\Molt\molt-src")) is False

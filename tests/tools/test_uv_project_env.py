from __future__ import annotations

from pathlib import Path

from tools import uv_project_env


def test_project_environment_path_uses_dx_root_and_versioned_session(
    tmp_path: Path,
) -> None:
    artifact_root = tmp_path / "external"
    path = uv_project_env.project_environment_path(
        python="3.14",
        purpose="output startup/size",
        repo_root=tmp_path,
        env={
            "MOLT_EXT_ROOT": str(artifact_root),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
    )

    assert path == (
        artifact_root / "tmp" / "uv-project-envs" / "output-startup-size__py3.14"
    )


def test_uv_project_env_sets_project_environment(tmp_path: Path) -> None:
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        env={
            "PATH": "x",
            "MOLT_EXT_ROOT": str(tmp_path),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
        repo_root=tmp_path,
    )

    assert env["PATH"] == "x"
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        tmp_path / "tmp" / "uv-project-envs" / "audit__py3.14"
    )


def test_uv_project_env_uses_external_artifact_root(tmp_path: Path) -> None:
    artifact_root = tmp_path / "external"
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        env={
            "MOLT_EXT_ROOT": str(artifact_root),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
        repo_root=tmp_path / "repo",
    )

    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        artifact_root / "tmp" / "uv-project-envs" / "audit__py3.14"
    )


def test_uv_project_env_accepts_explicit_relative_path(tmp_path: Path) -> None:
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="ignored",
        env={
            "MOLT_EXT_ROOT": str(tmp_path),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
        repo_root=tmp_path,
        explicit="tmp/custom-env",
    )

    assert env["UV_PROJECT_ENVIRONMENT"] == str(tmp_path / "tmp" / "custom-env")


def test_parse_command_strips_separator() -> None:
    assert uv_project_env._parse_command(["--", "uv", "run"]) == ["uv", "run"]


def test_run_command_waits_for_child_on_windows(monkeypatch) -> None:
    calls = []

    def fake_call(command, *, env):  # type: ignore[no-untyped-def]
        calls.append((command, env))
        return 7

    monkeypatch.setattr(uv_project_env.os, "name", "nt")
    monkeypatch.setattr(uv_project_env.subprocess, "call", fake_call)

    assert uv_project_env.run_command(["uv", "--version"], env={"X": "1"}) == 7
    assert calls == [(["uv", "--version"], {"X": "1"})]

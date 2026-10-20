from __future__ import annotations

import os
from pathlib import Path
import sys

import pytest
from tests.process_guard_common import (
    install_module_os_view,
    install_module_view,
    run_guarded_test_process,
)

from molt import dx, process_spawn
from molt.process_environment import NameRule, ProcessEnvironment
from tools import uv_project_env
import subprocess

ROOT = Path(__file__).resolve().parents[2]


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
        },
    )

    assert path == dx.stable_uv_project_env_dir(
        artifact_root,
        purpose="output startup/size",
        python="3.14",
        source_root=tmp_path,
    )


def test_uv_project_env_sets_project_environment(tmp_path: Path) -> None:
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        env={
            "PATH": "x",
            "MOLT_EXT_ROOT": str(tmp_path),
        },
        repo_root=tmp_path,
    )

    assert env["PATH"] == "x"
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        dx.stable_uv_project_env_dir(
            tmp_path, purpose="audit", python="3.14", source_root=tmp_path
        )
    )


def test_uv_project_env_uses_external_artifact_root(tmp_path: Path) -> None:
    artifact_root = tmp_path / "external"
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="audit",
        env={
            "MOLT_EXT_ROOT": str(artifact_root),
        },
        repo_root=repo_root,
    )

    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        dx.stable_uv_project_env_dir(
            artifact_root,
            purpose="audit",
            python="3.14",
            source_root=repo_root,
        )
    )


def test_uv_project_env_accepts_explicit_relative_path(tmp_path: Path) -> None:
    env = uv_project_env.uv_project_env(
        python="3.14",
        purpose="ignored",
        env={
            "MOLT_EXT_ROOT": str(tmp_path),
        },
        repo_root=tmp_path,
        explicit="tmp/custom-env",
    )

    assert env["UV_PROJECT_ENVIRONMENT"] == str(tmp_path / "tmp" / "custom-env")


@pytest.mark.parametrize("name_rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_uv_wrapper_selectors_keep_the_captured_environment_rule(tmp_path, name_rule):
    root = tmp_path / "repo"
    root.mkdir()
    artifact_root = tmp_path / "artifacts"
    alias = tmp_path / "caller-environment"
    environment = ProcessEnvironment(
        {
            "MOLT_EXT_ROOT": str(artifact_root),
            "uv_project_environment": str(alias),
            "Path": "retained path",
        },
        rule=name_rule,
    )
    expected = (
        dx.stable_uv_project_env_dir(
            artifact_root, purpose="audit", python="3.14", source_root=root
        )
        if name_rule is NameRule.EXACT
        else alias
    )
    assert (
        uv_project_env.project_environment_path(
            python="3.14", purpose="audit", repo_root=root, env=environment
        )
        == expected
    )
    selected = uv_project_env.uv_project_env(
        python="3.14", purpose="audit", repo_root=root, env=environment
    )
    assert selected.rule is name_rule
    assert selected["UV_PROJECT_ENVIRONMENT"] == str(expected)
    assert selected["MOLT_UV_PROJECT_PURPOSE"] == "audit"
    assert selected["MOLT_UV_PROJECT_PYTHON"] == "3.14"
    assert selected["Path"] == "retained path"
    assert dict(environment) == {
        "MOLT_EXT_ROOT": str(artifact_root),
        "uv_project_environment": str(alias),
        "Path": "retained path",
    }


def test_parse_command_strips_separator() -> None:
    assert uv_project_env._parse_command(["--", "uv", "run"]) == ["uv", "run"]


@pytest.mark.parametrize("cwd", [None, "forwarded working directory"])
def test_forward_command_waits_for_child_on_windows(monkeypatch, cwd) -> None:
    calls = []

    def fake_run(command, *, cwd, env, check):  # type: ignore[no-untyped-def]
        assert check is False
        calls.append((command, cwd, env))
        return subprocess.CompletedProcess(command, 7)

    install_module_os_view(monkeypatch, process_spawn, name="nt")
    install_module_view(
        monkeypatch, "subprocess", subprocess, process_spawn, run=fake_run
    )

    assert (
        uv_project_env.forward_command(["uv", "--version"], cwd=cwd, env={"X": "1"})
        == 7
    )
    assert calls == [(["uv", "--version"], cwd, {"X": "1"})]


@pytest.mark.parametrize("status", [0, 7])
def test_uv_project_env_real_command_exit_status_propagates(
    tmp_path: Path, status: int
) -> None:
    env = os.environ.copy()
    env["MOLT_EXT_ROOT"] = str(tmp_path / "artifacts")
    result = run_guarded_test_process(
        [
            sys.executable,
            str(ROOT / "tools" / "uv_project_env.py"),
            "--python",
            "3.12",
            "--purpose",
            "forwarding-test",
            "--",
            sys.executable,
            "-c",
            f"raise SystemExit({status})",
        ],
        cwd=ROOT,
        env=env,
        timeout=60,
    )
    assert result.returncode == status, result.stderr

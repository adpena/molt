from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
import inspect
from typing import Any, cast

import molt.cli as cli
import pytest


def test_prepare_build_config_uses_dev_runtime_profile_for_dev_builds(
    tmp_path: Path,
) -> None:
    prepared, error = cli._prepare_build_config(
        project_root=tmp_path,
        warnings=[],
        json_output=False,
        profile="dev",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == "dev-fast"


def test_prepare_build_config_uses_release_runtime_profile_for_release_builds(
    tmp_path: Path,
) -> None:
    prepared, error = cli._prepare_build_config(
        project_root=tmp_path,
        warnings=[],
        json_output=False,
        profile="release",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
    )

    assert error is None
    assert prepared is not None
    assert prepared.runtime_cargo_profile == "release-output"


@pytest.mark.parametrize("profile", ["dev", "release"])
def test_build_profile_flag_routes_to_build_profile(
    profile: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    seen_profiles: list[str] = []
    build_signature = inspect.signature(cli.build)

    def fake_build(*args: Any, **kwargs: Any) -> int:
        bound = build_signature.bind_partial(*args, **kwargs)
        seen_profiles.append(cast(str, bound.arguments["profile"]))
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(
        sys,
        "argv",
        ["molt", "build", "--profile", profile, str(entry)],
    )

    assert cli.main() == 0
    assert seen_profiles == [profile]


def test_build_args_profile_detection_keeps_platform_profile_separate() -> None:
    assert not cli._build_args_has_profile_flag(["--profile", "browser"])
    assert not cli._build_args_has_profile_flag(["--profile=browser"])
    assert cli._build_args_has_profile_flag(["--profile", "dev"])
    assert cli._build_args_has_profile_flag(["--profile=release"])
    assert cli._build_args_has_profile_flag(["--build-profile", "dev"])
    assert cli._build_args_has_profile_flag(
        ["--profile", "browser", "--build-profile", "dev"]
    )


def test_nested_build_keeps_platform_profile_and_forwards_dev_build_profile(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[project]\nname = "demo"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
    entry = project / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    output_binary = tmp_path / "bin" / "main_molt"
    output_binary.parent.mkdir(parents=True)
    output_binary.write_text("", encoding="utf-8")
    payload = cli._json_payload(
        "build",
        "ok",
        data={
            "output": str(output_binary),
            "consumer_output": str(output_binary),
        },
    )
    build_cmds: list[list[str]] = []

    def fake_subprocess_run(
        cmd: list[str],
        **kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")

    monkeypatch.setattr(cli, "_find_project_root", lambda start: project)
    monkeypatch.setattr(cli, "_find_molt_root", lambda start, cwd=None: project)
    monkeypatch.setattr(cli.subprocess, "run", fake_subprocess_run)
    monkeypatch.setattr(cli, "_run_command", lambda cmd, **kwargs: 0)

    rc = cli.run_script(
        str(entry),
        None,
        [],
        build_args=["--profile", "browser"],
        build_profile="dev",
        json_output=False,
    )

    assert rc == 0
    assert build_cmds[-1:] == [
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--json",
            "--profile",
            "browser",
            "--build-profile",
            "dev",
            str(entry),
        ]
    ]

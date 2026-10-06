from __future__ import annotations

from pathlib import Path

import pytest

from tools.ci_cargo_cache import configure_cargo_cache


@pytest.mark.parametrize("selection", ["default", "relative", "absolute", "external"])
def test_cache_projection_preserves_actual_cargo_target(tmp_path: Path, selection: str):
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    github_env = tmp_path / "github-env"
    github_output = tmp_path / "github-output"
    env = {
        "GITHUB_WORKSPACE": str(workspace),
        "GITHUB_ENV": str(github_env),
        "GITHUB_OUTPUT": str(github_output),
        "RUNNER_TEMP": str(tmp_path / "runner-temp"),
        "CARGO_INCREMENTAL": "1",
    }
    # The default lives outside the checkout (verified ephemeral custody
    # rejects in-checkout targets) at a run-stable path for actions/cache.
    expected = tmp_path / "runner-temp" / "molt-cargo-target"
    if selection == "relative":
        env["CARGO_TARGET_DIR"] = "target/sessions/wasm-ci"
        expected = workspace / "target" / "sessions" / "wasm-ci"
    elif selection == "absolute":
        expected = workspace / "explicit target"
        env["CARGO_TARGET_DIR"] = str(expected)
    elif selection == "external":
        expected = tmp_path / "external target"
        env["CARGO_TARGET_DIR"] = str(expected)
    assert configure_cargo_cache(env) == expected
    values = dict(
        line.split("=", 1)
        for line in github_env.read_text(encoding="utf-8").splitlines()
    )
    outputs = dict(
        line.split("=", 1)
        for line in github_output.read_text(encoding="utf-8").splitlines()
    )
    assert Path(values["CARGO_TARGET_DIR"]) == expected
    assert values["CARGO_INCREMENTAL"] == "0"
    assert Path(outputs["target-dir"]) == expected
    assert expected.is_dir()
    if selection in ("default", "external"):
        assert not expected.is_relative_to(workspace)


@pytest.mark.parametrize("character", ["\r", "\n", "\0"])
def test_cache_projection_rejects_environment_file_injection(
    tmp_path: Path, character: str
):
    env = {
        "GITHUB_WORKSPACE": str(tmp_path),
        "GITHUB_ENV": str(tmp_path / "github-env"),
        "GITHUB_OUTPUT": str(tmp_path / "github-output"),
        "CARGO_TARGET_DIR": f"target{character}CARGO_INCREMENTAL=1",
    }
    with pytest.raises(ValueError, match="control character"):
        configure_cargo_cache(env)
    assert not (tmp_path / "github-env").exists()
    assert not (tmp_path / "github-output").exists()

from __future__ import annotations

from pathlib import Path

import pytest

from tools.ci_cargo_cache import (
    configure_cargo_cache,
    prune_workspace_artifacts,
    workspace_crate_names,
)


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


def test_prune_keeps_dependency_units_and_drops_workspace_units(tmp_path: Path):
    workspace = workspace_crate_names(
        {
            "packages": [
                {
                    "name": "molt-runtime",
                    "targets": [
                        {"name": "molt_runtime", "kind": ["lib"]},
                        {"name": "build-script-build", "kind": ["custom-build"]},
                    ],
                },
                {"name": "molt-backend", "targets": [{"name": "molt-backend"}]},
            ]
        }
    )
    assert "build_script_build" not in workspace
    hash_ = "0123456789abcdef"
    kept = [
        f"debug/deps/libserde-{hash_}.rlib",
        f"debug/deps/serde-{hash_}.d",
        f"debug/deps/libc-{hash_}.rmeta",
        f"debug/.fingerprint/serde-{hash_}/lib-serde",
        f"debug/build/libc-{hash_}/out/marker",
        f"wasm32-wasip1/release-output/deps/libhashbrown-{hash_}.rlib",
    ]
    dropped = [
        f"debug/deps/libmolt_runtime-{hash_}.rlib",
        f"debug/deps/molt_backend-{hash_}.exe",
        f"debug/deps/molt_backend-{hash_}.pdb",
        "debug/deps/molt_backend.exe",
        "debug/deps/libmolt_runtime.rlib",
        f"debug/.fingerprint/molt-runtime-{hash_}/lib-molt_runtime",
        f"debug/build/molt-runtime-{hash_}/out/generated.rs",
        "debug/incremental/molt_runtime-x/s-1",
        "debug/molt-backend.exe",
        "debug/libmolt_runtime.d",
        f"wasm32-wasip1/release-output/deps/libmolt_runtime-{hash_}.rlib",
        "wasm32-wasip1/release-output/molt_runtime.wasm",
    ]
    for relative in (*kept, *dropped):
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x")
    (tmp_path / "wasm32-wasip1" / "release-output" / ".fingerprint").mkdir()

    assert prune_workspace_artifacts(tmp_path, workspace) > 0

    for relative in kept:
        assert (tmp_path / relative).is_file(), relative
    for relative in dropped:
        assert not (tmp_path / relative).exists(), relative


def test_workspace_names_require_cargo_package_metadata():
    with pytest.raises(ValueError, match="no package list"):
        workspace_crate_names({})

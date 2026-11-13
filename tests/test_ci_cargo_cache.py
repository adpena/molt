from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from tools.ci_cargo_cache import (
    configure_cargo_cache,
    prune_workspace_artifacts,
    superseded_cache_ids,
    workspace_clean_commands,
)

REPO_ROOT = Path(__file__).resolve().parents[1]


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


def _cargo_units(workspace: Path, target: Path) -> dict[str, bool]:
    """Build the workspace; map each unit's target name to Cargo's ``fresh``."""
    from tests.process_guard_common import run_guarded_test_process

    completed = run_guarded_test_process(
        ["cargo", "build", "--offline", "--message-format=json"],
        cwd=workspace,
        env={**os.environ, "CARGO_TARGET_DIR": str(target)},
        timeout=300,
        check=True,
    )
    return {
        message["target"]["name"]: message["fresh"]
        for line in completed.stdout.splitlines()
        if (message := json.loads(line)).get("reason") == "compiler-artifact"
    }


def test_prune_drops_workspace_units_and_keeps_dependency_units(tmp_path: Path) -> None:
    """Cargo itself decides what the prune removed: after it, the workspace
    member recompiles and its path dependency outside the workspace stays fresh."""
    workspace = tmp_path / "workspace"
    for path, text in {
        "Cargo.toml": '[workspace]\nmembers = ["app"]\nresolver = "2"\n'
        '[profile.ci-fast]\ninherits = "dev"\n',
        # The repository's pinned toolchain, so the test installs nothing.
        "rust-toolchain.toml": (REPO_ROOT / "rust-toolchain.toml").read_text(
            encoding="utf-8"
        ),
        "app/Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\n'
        'edition = "2024"\n[dependencies]\nleaf = { path = "../../leaf" }\n',
        "app/src/lib.rs": "pub fn app() -> u32 { leaf::leaf() }\n",
        "../leaf/Cargo.toml": '[package]\nname = "leaf"\nversion = "0.1.0"\n'
        'edition = "2024"\n[workspace]\n',
        "../leaf/src/lib.rs": "pub fn leaf() -> u32 { 7 }\n",
    }.items():
        (workspace / path).parent.mkdir(parents=True, exist_ok=True)
        (workspace / path).write_text(text, encoding="utf-8")
    target = tmp_path / "target"
    assert _cargo_units(workspace, target) == {"leaf": False, "app": False}

    commands = workspace_clean_commands(workspace, target)
    assert {command[command.index("--profile") + 1] for command in commands} == {
        "ci-fast",
        "dev",
        "release",
    }
    assert prune_workspace_artifacts(workspace, target) == len(commands)

    assert _cargo_units(workspace, target) == {"leaf": True, "app": False}


def test_retention_keeps_the_newest_main_entry_per_cache_group():
    old_inputs, new_inputs = "a" * 64, "b" * 64

    def entry(cache_id, key, created, ref="refs/heads/main"):
        return {"id": cache_id, "key": key, "createdAt": created, "ref": ref}

    entries = [
        entry(
            1, f"cargo-rust-Linux-X64-1.99.0-{old_inputs}-100-1", "2026-10-10T00:00:00Z"
        ),
        entry(
            2, f"cargo-rust-Linux-X64-1.99.0-{new_inputs}-101-1", "2026-10-11T00:00:00Z"
        ),
        # A retried run is newer still; restores fall back across input hashes.
        entry(
            3, f"cargo-rust-Linux-X64-1.99.0-{new_inputs}-101-2", "2026-10-11T01:00:00Z"
        ),
        # Sole entry of its namespace, and another platform: both stay.
        entry(
            4,
            f"cargo-wasm-ci-Linux-X64-1.99.0-{old_inputs}-90-1",
            "2026-10-09T00:00:00Z",
        ),
        entry(
            5,
            f"cargo-rust-macOS-ARM64-1.99.0-{old_inputs}-100-1",
            "2026-10-10T00:00:00Z",
        ),
        # Only main saves count; other refs and cache families are untouched.
        entry(
            6,
            f"cargo-rust-Linux-X64-1.99.0-{old_inputs}-102-1",
            "2026-10-12T00:00:00Z",
            ref="refs/pull/5/merge",
        ),
        entry(7, "wasi-sdk-34-Linux", "2026-10-01T00:00:00Z"),
    ]
    assert superseded_cache_ids(entries) == [1, 2]

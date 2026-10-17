from __future__ import annotations

from pathlib import Path

import pytest

from molt.cargo_workspace import workspace_package_names
from tools import check_changed_clippy as gate
from tools.proof_plan import ProofCommand, ProofPlan

ROOT = Path(__file__).resolve().parents[2]


def _package(root: Path, directory: str, name: str, dependencies: str = "") -> None:
    manifest = root / directory / "Cargo.toml"
    manifest.parent.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        f'[package]\nname = "{name}"\n[dependencies]\n{dependencies}', encoding="utf-8"
    )


@pytest.fixture
def workspace(tmp_path: Path) -> gate.Workspace:
    # core <- runtime <- backend (normal edges); tool has core as a dev edge;
    # vendored is a member that patches a registry crate; nested lives inside
    # core's directory but is its own member.
    (tmp_path / "Cargo.toml").write_text(
        "[workspace]\n"
        'members = ["crates/core", "crates/core/nested", "crates/runtime",'
        ' "crates/backend", "crates/tool", "vendor/zlib"]\n'
        "[patch.crates-io]\n"
        'zlib = { path = "vendor/zlib" }\n',
        encoding="utf-8",
    )
    _package(tmp_path, "crates/core", "core")
    _package(tmp_path, "crates/core/nested", "nested")
    _package(tmp_path, "crates/runtime", "runtime", 'core = { path = "../core" }\n')
    _package(
        tmp_path, "crates/backend", "backend", 'runtime = { path = "../runtime" }\n'
    )
    _package(
        tmp_path,
        "crates/tool",
        "tool",
        '[dev-dependencies]\ncore = { path = "../core" }\n',
    )
    _package(tmp_path, "vendor/zlib", "zlib")
    # A standalone project with its own clippy command, and an excluded one
    # with none: neither belongs to the root workspace.
    _package(tmp_path, "deploy/simd", "simd")
    _package(tmp_path, "crates/mlir", "mlir")
    return gate.Workspace.load(tmp_path)


def _command(identifier: str, argv: list[str], **data: object) -> ProofCommand:
    return ProofCommand(identifier, {"argv": argv, "tiers": ["pre-push", "pr"], **data})


COMMANDS = (
    _command("ws", ["cargo", "clippy", "--workspace", "--", "-D", "warnings"]),
    _command(
        "runtime-micro",
        [
            "cargo",
            "clippy",
            "-p",
            "runtime",
            "--features",
            "micro",
            "--",
            "-D",
            "warnings",
        ],
    ),
    _command(
        "backend", ["cargo", "clippy", "--package=backend", "--", "-D", "warnings"]
    ),
    _command("simd", ["cargo", "clippy", "--", "-D", "warnings"], cwd="deploy/simd"),
)


def _selected(workspace: gate.Workspace, *paths: str) -> dict[str, str]:
    change = gate.reach(workspace, paths)
    return {command.id: reason for command, reason in gate.select(COMMANDS, change)}


def test_change_reaches_its_member_and_every_dependent(workspace) -> None:
    change = gate.reach(workspace, ["crates/core/src/lib.rs"])
    assert change.packages == {"core", "runtime", "backend", "tool"}
    assert not change.whole_workspace
    assert _selected(workspace, "crates/core/src/lib.rs") == {
        "ws": "backend, core, runtime, tool",
        "runtime-micro": "runtime",
        "backend": "backend",
    }


def test_leaf_change_selects_only_commands_that_lint_it(workspace) -> None:
    assert _selected(workspace, "crates/backend/src/main.rs") == {
        "ws": "backend",
        "backend": "backend",
    }
    # The deepest member owns a nested path; its parent member is not reached.
    assert gate.reach(workspace, ["crates/core/nested/src/lib.rs"]).packages == {
        "nested"
    }


def test_non_rust_paths_outside_members_reach_nothing(workspace) -> None:
    assert _selected(workspace, "docs/README.md", "src/molt/cli.py") == {}


@pytest.mark.parametrize(
    "path",
    [
        "Cargo.toml",
        "Cargo.lock",
        # Shared build-script support included by #[path] has no owning member.
        "runtime/build_support/target_errno.rs",
        # A patched member replaces a registry crate that local edges never name.
        "vendor/zlib/src/lib.rs",
    ],
)
def test_workspace_inputs_reach_every_member(workspace, path: str) -> None:
    change = gate.reach(workspace, [path])
    assert change.whole_workspace and not change.toolchain
    assert set(_selected(workspace, path)) == {"ws", "runtime-micro", "backend"}


@pytest.mark.parametrize(
    "path", ["rust-toolchain.toml", "clippy.toml", ".cargo/config.toml"]
)
def test_toolchain_inputs_also_reach_standalone_projects(workspace, path: str) -> None:
    assert set(_selected(workspace, path)) == {"ws", "runtime-micro", "backend", "simd"}


def test_separate_project_change_selects_only_its_own_command(workspace) -> None:
    assert _selected(workspace, "deploy/simd/src/lib.rs") == {
        "simd": "standalone project deploy/simd"
    }
    assert _selected(workspace, "crates/mlir/src/lib.rs") == {}


def test_clippy_command_without_a_package_scope_is_refused() -> None:
    command = _command("bad", ["cargo", "clippy", "--", "-D", "warnings"])
    with pytest.raises(ValueError, match="--workspace, -p or a cwd"):
        gate.clippy_packages(command)


def test_every_local_proof_plan_clippy_command_names_real_packages() -> None:
    """The gate reads the live plan; a renamed crate must not drop a cell."""
    commands = gate.local_clippy_commands(ProofPlan.load())
    assert {command.id for command in commands} >= {
        "rust.clippy.workspace-default",
        "rust.clippy.runtime-micro",
        "rust.clippy.wasi32",
        "llvm.clippy.backend",
        "rust.clippy.browser-simd",
    }
    members = set(workspace_package_names(ROOT))
    for command in commands:
        if command.data.get("cwd") is not None:
            assert (ROOT / command.data["cwd"] / "Cargo.toml").is_file(), command.id
            continue
        packages = gate.clippy_packages(command)
        assert packages is None or packages <= members, command.id

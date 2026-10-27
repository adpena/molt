from __future__ import annotations

from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cargo_execution_policy import load_ci_cargo_policy
from molt.cargo_workspace import workspace_package_names
from tools import check_changed_clippy as gate, proof_plan
from tools.proof_plan import ProofCommand, ProofPlan
from tests.process_guard_common import install_module_view

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
    for directory in ("deploy/simd", "crates/mlir"):
        manifest = tmp_path / directory / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8") + "[workspace]\n", encoding="utf-8"
        )
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


def test_all_selected_boundaries_are_checked_before_the_first_clippy(
    workspace, monkeypatch, capsys
):
    manifest = workspace.root / "deploy/simd/Cargo.toml"
    manifest.write_text(
        manifest.read_text(encoding="utf-8").removesuffix("[workspace]\n"),
        encoding="utf-8",
    )
    monkeypatch.setattr(gate, "ROOT", workspace.root)
    monkeypatch.setattr(
        gate,
        "changed_paths",
        lambda _base: ["Cargo.toml", "deploy/simd/Cargo.toml"],
    )
    # This ownership fixture has no historical lock evidence; remain global
    # without invoking Git outside the fixture's modeled boundary.
    monkeypatch.setattr(gate, "_git", lambda _arguments: "")
    monkeypatch.setattr(gate, "local_clippy_commands", lambda _plan: COMMANDS)
    monkeypatch.setattr(gate.ProofPlan, "load", lambda: object())

    def no_build(*_args, **_kwargs):
        pytest.fail("a later invalid project must refuse before the first clippy")

    monkeypatch.setattr(gate, "run", no_build)
    assert gate.main([]) == 1
    assert "explicit [workspace]" in capsys.readouterr().err
    manifest.write_text(
        manifest.read_text(encoding="utf-8") + "[workspace]\n", encoding="utf-8"
    )
    assert gate.main(["--dry-run"]) == 0


@pytest.mark.parametrize(
    ("failed_index", "unprovisioned", "exitcode"),
    [(0, False, 1), (1, False, 1), (None, False, 0), (1, True, 2)],
    ids=["first-failure", "middle-failure", "all-pass", "not-provisioned"],
)
def test_selected_cells_stop_on_failure_and_all_run_on_success(
    workspace, monkeypatch, capsys, failed_index, unprovisioned, exitcode
):
    monkeypatch.setattr(gate, "ROOT", workspace.root)
    monkeypatch.setattr(gate, "changed_paths", lambda _base: ["rust-toolchain.toml"])
    monkeypatch.setattr(gate, "local_clippy_commands", lambda _plan: COMMANDS)
    plan = object()
    monkeypatch.setattr(gate.ProofPlan, "load", lambda: plan)
    calls = []

    def run(command, actual_plan):
        index = len(calls)
        assert failed_index is None or index <= failed_index, (
            "a failed selected cell must prevent later launches"
        )
        assert actual_plan is plan
        assert command is COMMANDS[index]
        calls.append(command)
        if index == failed_index:
            if unprovisioned:
                raise gate.ToolchainNotProvisioned("provider absent")
            return 42
        return 0

    monkeypatch.setattr(gate, "run", run)
    assert gate.main([]) == exitcode
    count = len(COMMANDS) if failed_index is None else failed_index + 1
    assert calls == list(COMMANDS[:count])
    output = capsys.readouterr()
    if failed_index is None:
        assert "changed-clippy: OK (4 command(s))" in output.out
        assert not output.err
    else:
        assert "changed-clippy: OK" not in output.out
        assert COMMANDS[failed_index].id in output.err
        assert ("provider absent" if unprovisioned else "FAILED") in output.err


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


def test_every_local_proof_plan_clippy_command_names_real_packages(monkeypatch) -> None:
    """The gate reads the live plan; a renamed crate must not drop a cell."""
    install_module_view(
        monkeypatch,
        "platform",
        proof_plan.platform,
        proof_plan,
        system=lambda: "Linux",
        machine=lambda: "x86_64",
    )
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


@pytest.mark.parametrize(
    ("system", "machine", "workspace_id", "budget"),
    [
        ("Darwin", "arm64", "portability.rust.macos.clippy-workspace", "suite"),
        (
            "Linux",
            "aarch64",
            "portability.rust.linux-aarch64.clippy-workspace",
            "suite",
        ),
        ("Linux", "x86_64", "rust.clippy.workspace-default", "cross-check"),
        ("Windows", "AMD64", "rust.clippy.workspace-default", "cross-check"),
    ],
)
def test_local_clippy_reuses_only_the_declared_host_execution(
    monkeypatch, system: str, machine: str, workspace_id: str, budget: str
) -> None:
    install_module_view(
        monkeypatch,
        "platform",
        proof_plan.platform,
        proof_plan,
        system=lambda: system,
        machine=lambda: machine,
    )
    plan = ProofPlan.load()
    by_id = {command.id: command for command in plan.commands}
    defaults = tuple(
        command
        for command in plan.commands
        if (
            command.argv[:2] == ("cargo", "clippy")
            or command is by_id["rust.clippy.wasi32"]
        )
        and "pre-push" in command.data["tiers"]
    )
    expected = tuple(
        by_id[workspace_id]
        if command.id == "rust.clippy.workspace-default"
        else command
        for command in defaults
    )
    actual = gate.local_clippy_commands(plan)
    assert len(actual) == len(expected)
    assert all(selected is declared for selected, declared in zip(actual, expected))
    workspace = by_id[workspace_id]
    assert workspace.data["timeout_budget"] == budget
    assert workspace.data[
        "timeout_seconds"
    ] == load_ci_cargo_policy().execution_budgets.timeout_seconds(budget)
    # A matching host policy is selected before reach filtering, so the real
    # workspace input still reaches the same packages without altering its cell.
    reached = gate.Reach(frozenset({"molt-ir"}), False, False, frozenset())
    selected = dict(
        (command.id, command) for command, _ in gate.select(actual, reached)
    )
    assert selected[workspace_id] is workspace


@pytest.mark.parametrize(
    "different",
    [
        {"argv": ["cargo", "clippy", "--workspace", "--all-features"]},
        {"cwd": "."},
        {"toolchains": ["rustc", "cargo", "python"]},
        {"cargo_native_units": {"host": ["c", "c++"]}},
        {"env": {"CARGO_INCREMENTAL": "1"}},
        {"dependencies": ["rust.test.tir"]},
        {"resource_class": "python-tests"},
        {"cache_domain": "none"},
        {"evidence_outputs": ["different-evidence.json"]},
        {"future_execution_field": True},
    ],
)
def test_same_argv_host_cell_cannot_change_execution_contract(
    monkeypatch, different
) -> None:
    install_module_view(
        monkeypatch,
        "platform",
        proof_plan.platform,
        proof_plan,
        system=lambda: "Darwin",
        machine=lambda: "arm64",
    )
    plan = ProofPlan.load()
    default = next(c for c in plan.commands if c.id == "rust.clippy.workspace-default")
    host = next(
        c for c in plan.commands if c.id == "portability.rust.macos.clippy-workspace"
    )
    changed = ProofCommand(host.id, {**host.data, **different})
    bounded = replace(plan, commands=(default, changed))
    assert gate.local_clippy_commands(bounded) == (default,)


def test_ambiguous_host_execution_is_rejected(monkeypatch) -> None:
    install_module_view(
        monkeypatch,
        "platform",
        proof_plan.platform,
        proof_plan,
        system=lambda: "Darwin",
        machine=lambda: "arm64",
    )
    plan = ProofPlan.load()
    default = next(c for c in plan.commands if c.id == "rust.clippy.workspace-default")
    host = next(
        c for c in plan.commands if c.id == "portability.rust.macos.clippy-workspace"
    )
    duplicate = ProofCommand("duplicate-host", host.data)
    bounded = replace(plan, commands=(default, host, duplicate))
    with pytest.raises(
        ValueError, match="multiple equivalent clippy cells for macos/aarch64"
    ):
        gate.local_clippy_commands(bounded)


class _Completed:
    returncode = 0


def test_run_gives_a_cold_target_the_cargo_policy_cold_budget(monkeypatch) -> None:
    plan = ProofPlan.load()
    command = next(c for c in plan.commands if c.id == "rust.clippy.workspace-default")
    seen: list[float] = []
    monkeypatch.setattr(gate, "admit_cargo_build", lambda argv, **kwargs: None)
    monkeypatch.setattr(gate, "toolchain_environment", lambda *args, **kwargs: {})
    monkeypatch.setattr(
        gate,
        "COMMANDS",
        SimpleNamespace(
            run=lambda argv, **kwargs: seen.append(kwargs["timeout"]) or _Completed()
        ),
    )
    assert gate.run(command, plan) == 0
    cold = load_ci_cargo_policy().execution_budgets.timeout_seconds("cold")
    # CI's warm budget is shorter than a landing's first cold run.
    assert int(command.data["timeout_seconds"]) < cold
    assert seen == [float(cold)]


def _impact_documents(
    *, patched: str | None, backend_dependency: str = "codec"
) -> dict[str, str]:
    root = '[workspace]\nmembers = ["backend", "runtime"]\n'
    if patched is not None:
        root += f'[patch.crates-io]\ncodec = {{ path = "{patched}" }}\n'
    source = (
        '\nsource = "registry+https://example.invalid/index"' if patched is None else ""
    )
    documents = {
        "Cargo.toml": root,
        "backend/Cargo.toml": '[package]\nname = "backend"\nversion = "1.0.0"\n',
        "runtime/Cargo.toml": '[package]\nname = "runtime"\nversion = "1.0.0"\n',
        "Cargo.lock": (
            'version = 4\n[[package]]\nname = "backend"\nversion = "1.0.0"\n'
            + (
                f'dependencies = ["{backend_dependency}"]\n'
                if backend_dependency
                else ""
            )
            + '[[package]]\nname = "runtime"\nversion = "1.0.0"\n'
            + '[[package]]\nname = "codec"\nversion = "2.0.0"'
            + source
            + "\n"
        ),
    }
    if patched is not None:
        documents[patched + "/Cargo.toml"] = (
            '[package]\nname = "codec"\nversion = "2.0.0"\n'
        )
    return documents


def _impact_selection(tmp_path, before, after, paths):
    for relative, text in after.items():
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    workspace = gate.Workspace.load(tmp_path)
    change = gate.dependency_reach(workspace, paths, before.__getitem__)
    return change, tuple(command for command, _ in gate.select(COMMANDS, change))


@pytest.mark.parametrize("old_path", [None, "vendor/old-codec"])
def test_patch_source_transition_reaches_consumers_without_runtime(tmp_path, old_path):
    before = _impact_documents(patched=old_path)
    after = _impact_documents(patched="vendor/codec")
    change, selected = _impact_selection(
        tmp_path, before, after, ["Cargo.toml", "Cargo.lock", "vendor/codec/src/lib.rs"]
    )
    assert change.packages == {"backend"}
    assert not change.whole_workspace
    assert selected == (COMMANDS[0], COMMANDS[2])


def test_patch_source_only_preserves_declared_commands(tmp_path):
    documents = _impact_documents(patched="vendor/codec")
    change, selected = _impact_selection(
        tmp_path, documents, documents, ["vendor/codec/src/lib.rs"]
    )
    assert change.packages == {"backend"}
    assert selected == (COMMANDS[0], COMMANDS[2])
    assert selected[0] is COMMANDS[0]  # --workspace/features are not rewritten.


def test_removed_dependency_uses_base_and_current_closures(tmp_path):
    before = _impact_documents(patched="vendor/codec")
    after = _impact_documents(patched="vendor/codec", backend_dependency="")
    change, selected = _impact_selection(tmp_path, before, after, ["Cargo.lock"])
    assert change.packages == {"backend"}
    assert selected == (COMMANDS[0], COMMANDS[2])


def test_lock_projection_disambiguates_versions_and_git_source_refs(tmp_path):
    before = _impact_documents(patched=None)
    before["Cargo.lock"] = (
        before["Cargo.lock"]
        .replace(
            'dependencies = ["codec"]',
            'dependencies = ["codec 2.0.0 (git+https://example.invalid/codec?branch=main)"]',
        )
        .replace(
            "registry+https://example.invalid/index",
            "git+https://example.invalid/codec?branch=main#aaaaaaaa",
        )
        + '[[package]]\nname = "codec"\nversion = "1.0.0"\n'
    )
    after = dict(before)
    after["Cargo.lock"] = before["Cargo.lock"].replace("#aaaaaaaa", "#bbbbbbbb")
    change, selected = _impact_selection(tmp_path, before, after, ["Cargo.lock"])
    assert change.packages == {"backend"}
    assert selected == (COMMANDS[0], COMMANDS[2])


@pytest.mark.parametrize(
    "defect", ["missing-lock", "malformed-lock", "missing-package", "policy"]
)
def test_unproven_or_global_changes_keep_workspace_coverage(tmp_path, defect):
    before = _impact_documents(patched="vendor/codec")
    after = dict(before)
    paths = ["Cargo.lock"]
    if defect == "missing-lock":
        del before["Cargo.lock"]
    elif defect == "malformed-lock":
        after["Cargo.lock"] = 'version = 4\npackage = "invalid"\n'
    elif defect == "missing-package":
        after["Cargo.lock"] = (
            'version = 4\n[[package]]\nname = "runtime"\nversion = "1.0.0"\n'
        )
    else:
        after["Cargo.toml"] += "[profile.release]\nopt-level = 2\n"
        paths = ["Cargo.toml"]
    change, selected = _impact_selection(tmp_path, before, after, paths)
    assert change.whole_workspace
    assert selected == COMMANDS[:3]


@pytest.mark.parametrize("dependencies", ['"stale"', "[]"])
def test_unclassified_lock_changes_keep_cargo_validation(tmp_path, dependencies):
    before = _impact_documents(patched="vendor/codec")
    after = dict(before)
    after["Cargo.lock"] += (
        '[[package]]\nname = "unrelated"\nversion = "1.0.0"\n'
        f"dependencies = {dependencies}\n"
    )
    change, selected = _impact_selection(tmp_path, before, after, ["Cargo.lock"])
    assert change.whole_workspace
    assert selected == COMMANDS[:3]


@pytest.mark.parametrize(
    "declaration",
    [
        '[dependencies]\ncodec = { version = "2", optional = true }\n',
        '[dev-dependencies]\ncodec = "2"\n',
        '[build-dependencies]\ncodec = "2"\n',
        "[target.'cfg(windows)'.dependencies]\ncodec = \"2\"\n",
    ],
)
def test_patch_reach_retains_optional_dev_build_and_target_edges(tmp_path, declaration):
    documents = _impact_documents(patched="vendor/codec")
    documents["backend/Cargo.toml"] += declaration
    change, selected = _impact_selection(
        tmp_path, documents, documents, ["vendor/codec/src/lib.rs"]
    )
    assert change.packages == {"backend"}
    assert selected == (COMMANDS[0], COMMANDS[2])


def test_registered_wasi_lint_driver_retains_scope_and_execution_protocol(monkeypatch):
    plan = ProofPlan.load()
    command = next(row for row in plan.commands if row.id == "rust.clippy.wasi32")
    operation = gate.clippy_argv(command)
    assert operation == (
        "cargo",
        "clippy",
        "--locked",
        "--target",
        "wasm32-wasip1",
        "--lib",
        "-p",
        "molt-runtime",
        "-p",
        "molt-tir",
        "--message-format=json",
        "--",
        "-D",
        "warnings",
    )
    assert gate.clippy_packages(command) == frozenset({"molt-runtime", "molt-tir"})
    change = gate.Reach(frozenset({"molt-tir"}), False, False, frozenset())
    assert gate.select((command,), change) == ((command, "molt-tir"),)
    unrelated = gate.Reach(frozenset({"molt-wasm-host"}), False, False, frozenset())
    assert gate.select((command,), unrelated) == ()
    lifecycle = next(
        row for row in plan.commands if row.id == "rust.test.wasi-runtime-lifecycle"
    )
    assert gate.clippy_argv(lifecycle) is None

    observed = []
    monkeypatch.setattr(gate, "toolchain_environment", lambda *args, **kwargs: {})
    monkeypatch.setattr(
        gate,
        "admit_cargo_build",
        lambda *args, **kwargs: pytest.fail(
            "the driver admits its actual Cargo operation"
        ),
    )

    def run(argv, **kwargs):
        assert (
            argv == command.argv
        )  # Keep the admitted driver, not the scope projection.
        assert kwargs["timeout"] == 1200.0
        observed.append(argv)
        return SimpleNamespace(returncode=42)

    monkeypatch.setattr(gate, "COMMANDS", SimpleNamespace(run=run))
    assert gate.run(command, plan) == 42
    assert observed == [command.argv]


def test_a_policy_argument_cannot_grant_another_python_source_lint_authority(tmp_path):
    plan = ProofPlan.load()
    command = next(row for row in plan.commands if row.id == "rust.clippy.wasi32")
    decoy = tmp_path / "run_runtime_test_gate.py"
    decoy.write_text("raise SystemExit('must never execute')\n", encoding="utf-8")
    argv = [
        str(decoy) if value == "tools/run_runtime_test_gate.py" else value
        for value in command.argv
    ]
    changed = ProofCommand(command.id, {**command.data, "argv": argv})
    try:
        projected = gate.clippy_argv(changed)
    except ValueError:
        projected = None  # Admission may refuse before the source discriminator.
    assert projected is None


def test_raw_clippy_scope_uses_typed_cargo_options_and_forwarding():
    command = _command(
        "typed",
        [
            "cargo",
            "+stable",
            "--manifest-path",
            "clippy",
            "clippy",
            "-pbackend",
            "--",
            "--package=ignored",
        ],
    )
    assert gate.clippy_argv(command) == command.argv
    assert gate.clippy_packages(command) == frozenset({"backend"})
    build = _command(
        "build", ["cargo", "--manifest-path", "clippy", "build", "-pbackend"]
    )
    assert gate.clippy_argv(build) is None

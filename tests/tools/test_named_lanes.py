"""Teeth for queue-owned named lanes: one registered argv, one toolchain closure."""

from __future__ import annotations

import argparse
from dataclasses import replace
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools import proof_plan
from tools.proof_queue_pkg import command_admission
from tools.proof_queue_pkg import pact

ROOT = Path(__file__).resolve().parents[2]
PLAN = proof_plan.ProofPlan.load()
LANE_IDS = (
    "pact.witness.acceptance",
    "pact.witness.oracle",
)


def test_registered_named_lanes_validate_and_are_distinct() -> None:
    assert PLAN.validate() == []
    assert tuple(lane.id for lane in PLAN.named_lanes) == LANE_IDS
    argvs = {lane.argv for lane in PLAN.named_lanes}
    assert len(argvs) == len(PLAN.named_lanes)
    command_argvs = {tuple(map(str, command.argv)) for command in PLAN.commands}
    assert not argvs & command_argvs
    for lane in PLAN.named_lanes:
        assert "python" in lane.toolchains
        # No host path may ever ride in a registered argv.
        assert not any(":\\" in value or value.startswith("/") for value in lane.argv)
        # Outputs go to the run's scratch root: no repository-relative output
        # root may ride in a registered argv.
        assert not any(value.startswith("tmp/") for value in lane.argv)


@pytest.mark.parametrize("lane_id", LANE_IDS)
def test_named_lane_argv_is_admitted_with_a_declared_closure(lane_id: str) -> None:
    lane = PLAN.named_lane(lane_id)
    envelope = command_admission.envelope_for_command(list(lane.argv))
    assert envelope["kind"] == "named-lane"
    assert envelope["proof_plan_command_ids"] == [lane_id]
    closure = envelope["process_closure"]
    assert closure["kind"] == "named-lane"
    assert closure["descendants"] == "declared-toolchains"
    assert set(lane.toolchains) <= set(closure["toolchains"])


def test_drifted_named_lane_argv_cannot_silently_become_a_leaf() -> None:
    lane = PLAN.named_lane("pact.witness.oracle")
    drifted = [*lane.argv, "--unexpected"]
    with pytest.raises(ValueError, match="must match its registered command exactly"):
        command_admission.envelope_for_command(drifted)


def test_unprepared_producer_is_refused_not_degraded() -> None:
    with pytest.raises(ValueError, match="direct locked interpreter with -P"):
        command_admission.envelope_for_command(
            ["python", "-m", "molt", "extension", "produce-set"]
        )


def test_plain_python_payload_stays_a_leaf() -> None:
    envelope = command_admission.envelope_for_command(["python", "-c", "print(1)"])
    assert envelope["process_closure"]["descendants"] == "forbidden"


def test_pact_specs_take_their_argv_from_the_plan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class Lane:
        def __init__(self, argv: tuple[str, ...]) -> None:
            self.argv = argv
            self.data = {
                "description": "d",
                "resource_family": "wasm-browser",
                "contention_key": "k",
                "timeout_seconds": 5,
            }

    class Plan:
        def named_lane(self, lane_id: str) -> Lane:
            return Lane(("python", "tools/registered.py", lane_id))

    monkeypatch.setattr(
        pact.proof_plan.ProofPlan, "load", classmethod(lambda cls: Plan())
    )
    assert pact.named_lane_argv("x.y") == ["python", "tools/registered.py", "x.y"]
    spec = pact._pact_witness_oracle_spec()
    assert spec["command"] == ["python", "tools/registered.py", "pact.witness.oracle"]


def test_named_lane_spec_takes_argv_from_the_plan(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class Lane:
        argv = ("python", "tools/registered.py")
        data = {
            "description": "seal",
            "resource_family": "wasm-source-extension",
            "contention_key": "wasm:numpy-seal",
            "timeout_seconds": 7200,
        }

    class Plan:
        def named_lane(self, lane_id: str) -> Lane:
            assert lane_id == "pact.witness.oracle"
            return Lane()

    monkeypatch.setattr(
        pact.proof_plan.ProofPlan, "load", classmethod(lambda cls: Plan())
    )
    spec = pact._named_lane_spec("pact.witness.oracle", repo_root=tmp_path)
    assert spec["logical_id"] == "pact-witness-oracle"
    assert spec["command"] == list(Lane.argv)
    assert spec["timeout"] == 7200.0
    assert spec["scopes"] == ["tools/proof_plan.toml"]


def test_named_lane_cli_subcommand_is_registered() -> None:
    from tools.proof_queue_pkg import cli

    parser = cli._build_parser()
    args = parser.parse_args(["named-lane", "pact.witness.oracle", "--print-spec"])
    assert isinstance(args, argparse.Namespace)
    assert args.pact_handler == "_cmd_named_lane"
    assert args.lane_id == "pact.witness.oracle"


def test_rust_target_is_only_read_from_rust_tool_argv() -> None:
    from tools.proof_queue_pkg import command_identity

    payload = ["python", "-m", "molt", "extension", "produce-set", "--target", "wasm"]
    assert command_identity._rust_target(payload, {}) is None
    assert (
        command_identity._rust_target(payload, {"CARGO_BUILD_TARGET": "wasm32-wasip1"})
        == "wasm32-wasip1"
    )
    assert (
        command_identity._rust_target(
            ["cargo", "build", "--target", "wasm32-wasip1", "-p", "molt-tir"], {}
        )
        == "wasm32-wasip1"
    )
    assert (
        command_identity._rust_target(
            ["rustc", "--target=x86_64-pc-windows-msvc", "a.rs"], {}
        )
        == "x86_64-pc-windows-msvc"
    )


def test_llvm_family_lanes_prefer_the_canonical_sdk_prefix(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import os

    from tools.proof_queue_pkg import guarded_execution as ge

    prefix = tmp_path / "llvm-22.1.8"
    (prefix / "bin").mkdir(parents=True)

    class Discovery:
        pass

    Discovery.prefix = prefix
    import molt.llvm_toolchain as llvm_toolchain

    monkeypatch.setattr(
        llvm_toolchain,
        "discover_llvm_toolchain",
        lambda root, environ=None: Discovery(),
    )
    ambient = os.pathsep.join(
        [str(tmp_path / "system-llvm" / "bin"), str(tmp_path / "other")]
    )
    env, found = ge.prefer_canonical_llvm_prefix(
        {"PATH": ambient}, ["python", "clang"], cwd=tmp_path
    )
    assert found == str(prefix)
    assert env["PATH"].split(os.pathsep)[0] == str((prefix / "bin").resolve())
    assert env["PATH"].split(os.pathsep)[1:] == ambient.split(os.pathsep)

    untouched, found = ge.prefer_canonical_llvm_prefix(
        {"PATH": ambient}, ["python", "uv"], cwd=tmp_path
    )
    assert found is None and untouched["PATH"] == ambient

    monkeypatch.setattr(
        llvm_toolchain, "discover_llvm_toolchain", lambda root, environ=None: None
    )
    untouched, found = ge.prefer_canonical_llvm_prefix(
        {"PATH": ambient}, ["python", "wasm-ld"], cwd=tmp_path
    )
    assert found is None and untouched["PATH"] == ambient


def test_llvm_family_is_derived_from_the_release_manifest_evidence() -> None:
    from tools.proof_queue_pkg import guarded_execution as ge

    family = ge.llvm_family_toolchains(PLAN)
    assert {"clang", "wasm-ld", "llvm-config", "ld.lld"} <= family
    assert "python" not in family and "cargo" not in family


def test_tool_release_lanes_run_the_pinned_release_first_on_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import os

    from molt import tool_releases
    from tools.proof_queue_pkg import guarded_execution as ge

    assert "wasm-tools" in ge.tool_release_toolchains(PLAN)
    assert "python" not in ge.tool_release_toolchains(PLAN)

    release = tool_releases.tool_release("wasm-tools")
    toolchain_root = tmp_path / "target-root"
    prefix = tool_releases.tool_prefix(toolchain_root, release)
    executable = tool_releases.tool_executable(prefix, release)
    executable.parent.mkdir(parents=True)
    executable.write_bytes(b"pinned")
    provisioned: list[str] = []

    def provision(requested, root):
        provisioned.append(requested.name)
        assert root == toolchain_root
        return tool_releases.ToolDiscovery(
            release=requested,
            prefix=prefix,
            executable=executable,
            executable_sha256="0" * 64,
            asset=next(iter(requested.assets.values())),
        )

    class Custody:
        pass

    Custody.toolchain_root = toolchain_root
    monkeypatch.setattr(tool_releases, "provision_tool", provision)
    monkeypatch.setattr(
        "molt.dx.checkout_custody", lambda root, env=None, **_kwargs: Custody()
    )
    ambient = os.pathsep.join([str(tmp_path / "cargo-bin"), str(tmp_path / "other")])
    env, prefixes = ge.prefer_tool_release_prefixes(
        {"PATH": ambient}, ["python", "wasm-tools"], cwd=tmp_path
    )
    assert provisioned == ["wasm-tools"]
    assert prefixes == {"wasm-tools": str(prefix)}
    assert env["PATH"].split(os.pathsep)[0] == str(executable.parent.resolve())
    assert env["PATH"].split(os.pathsep)[1:] == ambient.split(os.pathsep)

    untouched, prefixes = ge.prefer_tool_release_prefixes(
        {"PATH": ambient}, ["python", "uv"], cwd=tmp_path
    )
    assert prefixes == {} and untouched["PATH"] == ambient


def test_named_lane_entry_routes_dedicated_lanes_through_their_aperture(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # `named-lane pact.witness.acceptance` must produce the same run as the
    # dedicated `pact-witness-acceptance` command: the acceptance tool fails
    # closed without the provenance pins that spec carries.
    seen: list[str] = []

    def acceptance(args: argparse.Namespace) -> int:
        seen.append(args.lane_id)
        return 0

    monkeypatch.setitem(
        pact._DEDICATED_NAMED_LANE_HANDLERS, "pact.witness.acceptance", acceptance
    )
    args = argparse.Namespace(lane_id="pact.witness.acceptance", timeout=None)
    assert pact._cmd_named_lane(args) == 0
    assert seen == ["pact.witness.acceptance"]
    assert set(pact._DEDICATED_NAMED_LANE_HANDLERS) == {
        "pact.witness.acceptance",
        "pact.witness.oracle",
    }


def test_obsolete_dynamic_environment_field_is_rejected() -> None:
    lane = PLAN.named_lane("pact.witness.oracle")
    changed = replace(lane, data={**lane.data, "derived_environments": []})
    plan = replace(
        PLAN,
        named_lanes=tuple(
            changed if item.id == lane.id else item for item in PLAN.named_lanes
        ),
    )
    assert any("unknown named lane fields" in error for error in plan.validate())


@pytest.mark.parametrize("lifetime", ["terminal-success", True, None])
def test_python_named_recipe_cannot_discard_deferred_cargo_outputs(lifetime) -> None:
    lane = PLAN.named_lane("pact.witness.oracle")
    changed = replace(lane, data={**lane.data, "cargo_output_lifetime": lifetime})
    plan = replace(
        PLAN,
        named_lanes=tuple(
            changed if item.id == lane.id else item for item in PLAN.named_lanes
        ),
    )
    assert any("must retain Cargo outputs" in error for error in plan.validate())


def test_disposable_named_override_is_refused_before_environment_provision(
    monkeypatch,
) -> None:
    spec = pact._named_lane_spec("pact.witness.oracle")
    spec["prepared_named_lane"] = "pact.witness.oracle"
    monkeypatch.setattr(
        pact,
        "source_build_environment",
        lambda *args, **kwargs: pytest.fail("provisioned rejected output consumer"),
    )
    with pytest.raises(ValueError, match="explicit Cargo"):
        pact._run_named_spec(
            argparse.Namespace(cargo_output_lifetime="terminal-success"), spec
        )


@pytest.mark.parametrize("lane_id", LANE_IDS)
def test_prepared_named_lane_keeps_exact_registered_payload_and_closure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, lane_id: str
) -> None:
    from molt.cli import source_build_environment as environment_authority

    custody = tmp_path / "environments"
    root = custody / ("a" * 64)
    python = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    python.parent.mkdir(parents=True)
    python.write_bytes(b"prepared-image")
    (root / environment_authority.SOURCE_BUILD_ENVIRONMENT_MANIFEST).write_text(
        "{}", encoding="utf-8"
    )
    monkeypatch.setattr(
        environment_authority, "_source_build_custody_root", lambda _repo: custody
    )
    command = command_admission.prepared_named_lane_command(lane_id, python)
    envelope = command_admission.envelope_for_command(command)
    assert envelope["kind"] == "named-lane"
    assert envelope["proof_plan_command_ids"] == [lane_id]
    assert envelope["typed_command"]["environment_root"] == str(root)
    assert set(PLAN.named_lane(lane_id).toolchains) <= set(envelope["toolchains"])
    with pytest.raises(ValueError, match="must match its registered command exactly"):
        command_admission.envelope_for_command([*command, "--changed"])
    with pytest.raises(ValueError, match="direct locked interpreter with -P"):
        command_admission.envelope_for_command([command[0], *command[2:]])
    (root / environment_authority.SOURCE_BUILD_ENVIRONMENT_MANIFEST).unlink()
    with pytest.raises(ValueError, match="content-addressed locked"):
        command_admission.envelope_for_command(command)


@pytest.mark.parametrize("print_spec", [False, True])
def test_witness_setup_precedes_submission_and_never_runs_during_print(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, print_spec: bool
) -> None:
    root = tmp_path / "environment"
    python = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    events = []
    captured = {}

    def prepare(repo, group, *, provision):
        assert group == pact.PACT_WITNESS_DEPENDENCY_GROUP
        events.append(("prepare", provision))
        return SimpleNamespace(root=root, python_executable=python)

    def queue(_args, **kwargs):
        events.append(("queue", None))
        captured.update(kwargs)
        return 0, "queued"

    monkeypatch.setattr(pact, "source_build_environment", prepare)
    monkeypatch.setattr(pact.runner, "_queue_one", queue)
    args = argparse.Namespace(
        env=[], print_spec=print_spec, repo_root=ROOT, queue_only=True
    )
    assert pact._run_named_spec(args, pact._pact_witness_oracle_spec()) == 0
    assert events == (
        [("prepare", False)] if print_spec else [("prepare", True), ("queue", None)]
    )
    if print_spec:
        import json

        captured.update(json.loads(capsys.readouterr().out))
    assert captured["command"] == [str(python), "-P", "tools/pact_witness_oracle.py"]
    assert captured["env_overrides"]["VIRTUAL_ENV"] == str(root.resolve())


@pytest.mark.parametrize(
    "name", ["PATH", "VIRTUAL_ENV", "PYTHONUTF8", "PYTHONIOENCODING"]
)
def test_witness_refuses_environment_redirection_before_setup(
    monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    monkeypatch.setattr(
        pact,
        "source_build_environment",
        lambda *a, **k: pytest.fail("setup preceded admission"),
    )
    args = argparse.Namespace(env=[f"{name}=redirect"], print_spec=False)
    with pytest.raises(SystemExit, match="locked environment custody"):
        pact._run_named_spec(args, pact._pact_witness_oracle_spec())


def test_uncaptured_environment_image_cannot_borrow_a_prepared_image_identity(
    tmp_path: Path,
) -> None:
    import hashlib
    from molt.cli.source_build_environment import SOURCE_BUILD_ENVIRONMENT_MANIFEST
    from tools.proof_queue_pkg import execution_custody

    captured = tmp_path / "captured" / "python.exe"
    copied = tmp_path / "uncaptured" / "python.exe"
    for path in (captured, copied):
        path.parent.mkdir()
        path.write_bytes(b"same-image")
    (copied.parent / SOURCE_BUILD_ENVIRONMENT_MANIFEST).write_text(
        "{}", encoding="utf-8"
    )
    # The decision primitive needs no listener or running process.
    server = object.__new__(execution_custody.ChildCustodyEventServer)
    server.policy = {
        "descendants": "declared-toolchains",
        "allowed": [
            {
                "toolchain": "python",
                "path": os.path.normcase(os.path.abspath(captured)),
                "sha256": hashlib.sha256(b"same-image").hexdigest(),
            }
        ],
    }
    assert server._decide_child({"requested": str(captured)})["admitted"] is True
    rejected = server._decide_child({"requested": str(copied)})
    assert rejected["admitted"] is False
    assert rejected["reason"] == "outside-declared-toolchain-closure"
    captured.write_bytes(b"changed-image")
    assert server._decide_child({"requested": str(captured)})["admitted"] is False

"""Teeth for queue-owned named lanes: one registered argv, one toolchain closure."""

from __future__ import annotations

import argparse
from pathlib import Path

import pytest

from tools import proof_plan
from tools.proof_queue_pkg import command_admission
from tools.proof_queue_pkg import pact

ROOT = Path(__file__).resolve().parents[2]
PLAN = proof_plan.ProofPlan.load()
LANE_IDS = (
    "pact.witness.acceptance",
    "pact.witness.oracle",
    "pact.seal.numpy.produce",
    "pact.seal.scipy.produce",
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
        for root in lane.scratch_roots:
            assert root.startswith("tmp/")


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
    lane = PLAN.named_lane("pact.seal.numpy.produce")
    drifted = [*lane.argv, "--expected-identity-sha256", "0" * 64]
    with pytest.raises(ValueError, match="must match its registered command exactly"):
        command_admission.envelope_for_command(drifted)


def test_unregistered_molt_cli_argv_is_refused_not_degraded() -> None:
    # `python -m molt` is a named-lane entrypoint: any other argv through it
    # would spawn children as a leaf, so admission refuses it loudly.
    with pytest.raises(ValueError, match="named-lane entrypoint argv must match"):
        command_admission.envelope_for_command(
            ["python", "-m", "molt", "extension", "audit", "--help"]
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
            self.scratch_roots = ()
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


def test_named_lane_spec_resets_declared_scratch_roots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = tmp_path / "repo"
    stale = repo / "tmp" / "pact_seal_build" / "numpy" / "stale.o"
    stale.parent.mkdir(parents=True)
    stale.write_bytes(b"old")

    class Lane:
        argv = ("python", "-m", "molt", "extension", "produce-set")
        scratch_roots = ("tmp/pact_seal_build/numpy",)
        data = {
            "description": "seal",
            "resource_family": "wasm-source-extension",
            "contention_key": "wasm:numpy-seal",
            "timeout_seconds": 7200,
        }

    class Plan:
        def named_lane(self, lane_id: str) -> Lane:
            assert lane_id == "pact.seal.numpy.produce"
            return Lane()

    monkeypatch.setattr(
        pact.proof_plan.ProofPlan, "load", classmethod(lambda cls: Plan())
    )
    spec = pact._named_lane_spec("pact.seal.numpy.produce", repo_root=repo)
    assert not stale.parent.exists()
    assert spec["logical_id"] == "pact-seal-numpy-produce"
    assert spec["command"] == list(Lane.argv)
    assert spec["timeout"] == 7200.0
    assert "tmp/pact_seal_build/numpy" in spec["scopes"]


def test_scratch_roots_outside_tmp_are_refused(tmp_path: Path) -> None:
    with pytest.raises(SystemExit, match="outside tmp/"):
        pact._clear_scratch_roots(tmp_path, ("src/molt",))
    with pytest.raises(SystemExit, match="outside tmp/"):
        pact._clear_scratch_roots(tmp_path, ("tmp",))


def test_named_lane_cli_subcommand_is_registered() -> None:
    from tools.proof_queue_pkg import cli

    parser = cli._build_parser()
    args = parser.parse_args(["named-lane", "pact.seal.numpy.produce", "--print-spec"])
    assert isinstance(args, argparse.Namespace)
    assert args.pact_handler == "_cmd_named_lane"
    assert args.lane_id == "pact.seal.numpy.produce"


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

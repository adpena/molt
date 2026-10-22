"""Teeth for queue-owned named lanes: one registered argv, one toolchain closure."""

from __future__ import annotations

import argparse
import hashlib
from dataclasses import replace
import os
from pathlib import Path
import shutil
from types import SimpleNamespace

import pytest

from molt.process_environment import NameRule, ProcessEnvironment

from tests.executable_test_support import custody_spelling
from tests.process_guard_common import current_thread_only
from tools import proof_plan
from tools.proof_queue_pkg import command_admission
from tools.proof_queue_pkg import pact

ROOT = Path(__file__).resolve().parents[2]
PLAN = proof_plan.ProofPlan.load()
PYTHON_LANE_IDS = (
    "native.capability-cache-policy",
    "runtime.allocation-observer",
    "pact.witness.acceptance.native",
    "pact.witness.acceptance.wasm",
    "pact.witness.oracle",
    "r6.target-version-parity.py312",
    "r6.target-version-parity.py313",
    "r6.target-version-parity.py314",
)


def test_registered_named_lanes_validate_and_are_distinct() -> None:
    assert PLAN.validate() == []
    assert len({lane.id for lane in PLAN.named_lanes}) == len(PLAN.named_lanes)
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


@pytest.mark.parametrize("lane_id", [lane.id for lane in PLAN.named_lanes])
def test_named_lane_argv_is_admitted_with_a_declared_closure(lane_id: str) -> None:
    lane = PLAN.named_lane(lane_id)
    envelope = command_admission.envelope_for_command(list(lane.argv))
    assert envelope["kind"] == "named-lane"
    assert envelope["proof_plan_command_ids"] == [lane_id]
    closure = envelope["process_closure"]
    assert closure["kind"] == "named-lane"
    assert closure["descendants"] == "declared-toolchains"
    assert set(lane.toolchains) <= set(closure["toolchains"])
    assert envelope["cargo_native_units"] == lane.data.get("cargo_native_units", {})
    assert command_admission.cargo_cache_domain(envelope) == lane.data["cache_domain"]


@pytest.mark.parametrize(
    "lane_id",
    (
        "runtime.abi-fixture-authorities",
        *PYTHON_LANE_IDS,
    ),
)
def test_runtime_and_reference_lanes_have_the_required_native_languages(
    lane_id: str,
) -> None:
    lane = PLAN.named_lane(lane_id)
    envelope = command_admission.envelope_for_command(list(lane.argv))
    # Native differential/Pact lanes cold-build the runtime, including mimalloc.
    # WASM-only and reference-only lanes must not acquire native C custody.
    native_runtime = lane_id in {
        "native.capability-cache-policy",
        "runtime.abi-fixture-authorities",
        "runtime.allocation-observer",
        "pact.witness.acceptance.native",
        "r6.target-version-parity.py312",
        "r6.target-version-parity.py313",
        "r6.target-version-parity.py314",
    }
    assert envelope["cargo_native_units"] == (
        {
            "target": ["c"]
            if lane_id == "runtime.abi-fixture-authorities"
            else ["c", "c++"]
        }
        if native_runtime
        else {}
    )


@pytest.mark.parametrize(
    ("lane_id", "languages"),
    (("runtime.abi-fixture-authorities", ["c"]),),
)
def test_native_test_images_declare_compilers_through_cargo_wrapping(
    lane_id: str, languages: list[str]
) -> None:
    from tools.proof_queue_pkg.policy import _canonical_cargo_proof_command

    lane = PLAN.named_lane(lane_id)
    wrapped = _canonical_cargo_proof_command(list(lane.argv[1:]))
    envelope = command_admission.envelope_for_command(wrapped)
    assert envelope["cargo_native_units"] == {"target": languages}
    assert envelope["delegated"]["cargo_native_units"] == {"target": languages}
    assert {"cargo", "rustc", "python"} <= set(envelope["toolchains"])
    spec = pact._named_lane_spec(lane.id)
    assert spec["command"] == wrapped
    assert "prepared_named_lane" not in spec
    with pytest.raises(ValueError, match="has no Python payload"):
        command_admission.prepared_named_lane_command(lane.id, Path("python"))


def test_allocation_observer_lane_owns_build_and_exact_execution() -> None:
    lane = PLAN.named_lane("runtime.allocation-observer")
    assert "tools/run_runtime_test_gate.py" in lane.argv
    assert lane.argv[-2:] == ("--policy", "allocation-observer")
    assert "--no-run" not in lane.argv
    assert not any(
        item.id == "runtime.allocation-observer-image" for item in PLAN.named_lanes
    )
    assert lane.data["cargo_native_units"] == {"target": ["c", "c++"]}


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


def test_rust_target_is_only_read_from_rust_tool_argv(tmp_path) -> None:
    from tools.proof_queue_pkg import command_identity

    payload = ["python", "-c", "pass", "--target", "wasm"]
    assert (
        command_identity._rust_target(
            command_admission.envelope_for_command(payload), {}, cwd=tmp_path
        )
        is None
    )
    assert (
        command_identity._rust_target(
            command_admission.envelope_for_command(payload),
            {"CARGO_BUILD_TARGET": "wasm32-wasip1"},
            cwd=tmp_path,
        )
        == "wasm32-wasip1"
    )
    assert (
        command_identity._rust_target(
            command_admission.envelope_for_command(
                ["cargo", "build", "--target", "wasm32-wasip1", "-p", "molt-tir"]
            ),
            {},
            cwd=tmp_path,
        )
        == "wasm32-wasip1"
    )
    assert (
        command_identity._rust_target(
            command_admission.envelope_for_command(
                ["rustc", "--target=x86_64-pc-windows-msvc", "a.rs"]
            ),
            {},
            cwd=tmp_path,
        )
        == "x86_64-pc-windows-msvc"
    )


def test_llvm_family_lanes_project_the_complete_declared_sdk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import llvm_toolchain
    from tools.proof_queue_pkg import guarded_execution as ge

    prefix = tmp_path / "llvm SDK"
    config = tmp_path / "external config/bin/llvm-config"
    ambient = os.pathsep.join(
        [str(tmp_path / "system-llvm/bin"), str(tmp_path / "other")]
    )
    pin = llvm_toolchain.required_llvm_backend_pin(ge.state.ROOT)
    assert pin is not None
    verification = SimpleNamespace(
        prefix=prefix, llvm_config=config, version=pin.default_release
    )
    observed = []

    def verify(root, *, environ, cwd):
        observed.append((root, dict(environ), cwd))
        return verification

    monkeypatch.setattr(llvm_toolchain, "verify_available_llvm_toolchain", verify)
    if llvm_toolchain.sys.platform == "darwin":
        from molt import platform_toolchain

        monkeypatch.setattr(
            platform_toolchain,
            "select_darwin_toolchain",
            lambda _env: SimpleNamespace(
                environment=lambda: {
                    "SDKROOT": "/selected Apple SDK",
                    "DEVELOPER_DIR": "/selected Xcode",
                }
            ),
        )
    selected = {"PATH": ambient}
    actual = ge.project_declared_sdk_environment(
        selected, ["python", "clang", "llvm-config", "llvm-sdk"], cwd=tmp_path
    )
    assert observed == [(ge.state.ROOT, selected, tmp_path)]
    assert actual["PATH"].split(os.pathsep) == [
        str(prefix / "bin"),
        *ambient.split(os.pathsep),
    ]
    assert actual["MOLT_LLVM_PREFIX"] == str(prefix)
    assert actual["LLVM_CONFIG_PATH"] == str(config)
    assert actual[pin.env_var] == str(config.parent.parent)
    assert actual[llvm_toolchain.mlir_sys_prefix_env_var(pin.major)] == str(prefix)
    assert actual[llvm_toolchain.tablegen_prefix_env_var(pin.major)] == str(prefix)
    assert selected == {"PATH": ambient}
    assert ge.project_declared_sdk_environment(
        selected, ["python", "uv"], cwd=tmp_path
    ) == ProcessEnvironment(selected, rule=NameRule.host())
    assert len(observed) == 1

    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda _root, *, environ, cwd: None,
    )
    with pytest.raises(ValueError, match="LLVM SDK.*ld.lld"):
        ge.project_declared_sdk_environment(
            selected, ["python", "ld.lld"], cwd=tmp_path
        )


@pytest.mark.parametrize("with_native", [False, True])
def test_sdk_role_selection_does_not_promote_sdk_tools_to_native_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, with_native: bool
) -> None:
    from molt import llvm_toolchain
    from tests.test_llvm_toolchain import _write_wasi_sdk_installation
    from tools.proof_queue_pkg import guarded_execution as ge

    sdk_prefix = _write_wasi_sdk_installation(tmp_path / "toolchains")
    installation = llvm_toolchain.load_wasi_sdk_installation(
        ge.state.ROOT, sdk_prefix, verify_tree=False
    )
    native = tmp_path / "llvm-native"
    ambient = str(tmp_path / "ambient")
    selected = {"PATH": ambient, "WASI_SDK_PATH": str(installation.sdk)}
    pin = llvm_toolchain.required_llvm_backend_pin(ge.state.ROOT)
    assert pin is not None
    observed = []

    def native_sdk(root, *, environ, cwd):
        assert cwd == tmp_path
        assert with_native, "a WASM-only declaration must not discover native LLVM"
        assert root == ge.state.ROOT
        assert environ == {**selected, "MOLT_WASM_LD": str(installation.wasm_ld)}
        observed.append("native")
        return SimpleNamespace(
            prefix=native,
            llvm_config=native / "bin/llvm-config",
            version=pin.default_release,
        )

    monkeypatch.setattr(llvm_toolchain, "verify_available_llvm_toolchain", native_sdk)
    if llvm_toolchain.sys.platform == "darwin":
        from molt import platform_toolchain

        monkeypatch.setattr(
            platform_toolchain,
            "select_darwin_toolchain",
            lambda _env: SimpleNamespace(
                environment=lambda: {
                    "SDKROOT": "/selected Apple SDK",
                    "DEVELOPER_DIR": "/selected Xcode",
                }
            ),
        )
    declared = ["wasm-ld", "ld.lld"] if with_native else ["wasm-ld"]
    actual = ge.project_declared_sdk_environment(selected, declared, cwd=tmp_path)
    assert actual["MOLT_WASM_LD"] == str(installation.wasm_ld)
    assert actual["WASI_SDK_PATH"] == str(installation.sdk)
    assert actual["PATH"] == (
        os.pathsep.join((str(native / "bin"), ambient)) if with_native else ambient
    )
    assert not any(name.startswith("CARGO_TARGET_WASM32_") for name in actual)
    assert "CC" not in actual and "CXX" not in actual and "RUSTFLAGS" not in actual
    assert observed == (["native"] if with_native else [])
    assert selected == {"PATH": ambient, "WASI_SDK_PATH": str(installation.sdk)}


def test_missing_declared_sdk_role_fails_without_native_path_fallback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import llvm_toolchain
    from tools.proof_queue_pkg import guarded_execution as ge

    def missing(*_args, **_kwargs):
        raise llvm_toolchain.LlvmToolchainConfigError(
            "SDK must be provisioned explicitly"
        )

    monkeypatch.setattr(llvm_toolchain, "resolve_wasi_sdk_tool", missing)
    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda *_args, **_kwargs: pytest.fail("native LLVM cannot supply the SDK role"),
    )
    with pytest.raises(
        llvm_toolchain.LlvmToolchainConfigError, match="provisioned explicitly"
    ):
        ge.project_declared_sdk_environment(
            {"PATH": "native-llvm"}, ["wasm-ld"], cwd=tmp_path
        )


def test_declared_external_wasm_role_needs_no_native_or_managed_sdk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import llvm_toolchain
    from tools.proof_queue_pkg import guarded_execution as ge

    request = tmp_path / "request"
    tool = request / "external" / "wasm-ld"
    tool.parent.mkdir(parents=True)
    tool.write_bytes(b"selected external linker")
    environment = {"MOLT_WASM_LD": "external/wasm-ld", "PATH": "ambient"}
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda *_args, **_kwargs: pytest.fail(
            "a WASM role must not discover native LLVM"
        ),
    )
    monkeypatch.setattr(
        llvm_toolchain,
        "selected_wasi_sdk_installation",
        lambda *_args, **_kwargs: pytest.fail(
            "an explicit role must not require a managed SDK"
        ),
    )
    actual = ge.project_declared_sdk_environment(environment, ["wasm-ld"], cwd=request)
    assert actual == ProcessEnvironment(
        {**environment, "MOLT_WASM_LD": str(tool)}, rule=NameRule.host()
    )
    assert environment["MOLT_WASM_LD"] == "external/wasm-ld"


@pytest.mark.parametrize("selected_sdk_version", [True, False])
def test_queue_sdk_role_launch_and_attestation_share_absolute_selection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, selected_sdk_version: bool
) -> None:
    import hashlib
    import subprocess
    import molt.llvm_toolchain as llvm_toolchain
    import tomllib
    from tools.proof_queue_pkg import command_identity

    release = tomllib.loads(
        (ROOT / "config/llvm_toolchain_releases.toml").read_text(encoding="utf-8")
    )
    version = release["wasi_sdk"]["llvm_version"] if selected_sdk_version else "22.1.8"
    selected = tmp_path / ("wasm-ld.exe" if os.name == "nt" else "wasm-ld")
    selected.write_bytes(b"SDK entrypoint")
    selected.chmod(0o755)
    environment = {"MOLT_WASM_LD": str(selected), "PATH": "wrong-native-llvm"}

    def sdk_role(root, role, *, environ, cwd):
        assert cwd == tmp_path
        assert root == proof_plan.ROOT and role == "wasm-ld"
        assert environ == ProcessEnvironment(environment, rule=NameRule.host())
        return selected

    monkeypatch.setattr(llvm_toolchain, "resolve_wasi_sdk_tool", sdk_role)
    # A tripwire must catch this call from any module, so it stays process-wide.
    # Only this thread meets the fake; other threads reach the real function.
    monkeypatch.setattr(
        shutil,
        "which",
        current_thread_only(
            lambda *_a, **_k: pytest.fail("SDK role must not resolve through PATH"),
            shutil.which,
        ),
    )
    calls = []

    def run(command, **_kwargs):
        calls.append(tuple(command))
        assert list(command) == [custody_spelling(selected), "--version"]
        return subprocess.CompletedProcess(command, 0, f"LLD {version}", "")

    monkeypatch.setattr(command_identity, "_run_captured", run)
    envelope = {"argv": ["wasm-ld", "--version"], "python": None}
    exact = command_identity._exact_command(envelope, cwd=tmp_path, env=environment)
    # Launch selection retains the resolver's spelling; captured image identity
    # and its version probe use custody spelling.
    assert exact == [str(selected), "--version"]
    # An enclosing uv environment cannot send the role back through its PATH.
    assert (
        command_identity._which_in_command_environment(
            "wasm-ld",
            {"python": {"kind": "uv"}},
            ["uv", "run"],
            cwd=tmp_path,
            env=environment,
        )
        == selected
    )
    identity = command_identity._tool_identity(
        PLAN, "wasm-ld", envelope, exact, cwd=tmp_path, env=environment
    )
    assert identity["path"] == custody_spelling(selected)
    assert (
        identity["executable_sha256"] == hashlib.sha256(b"SDK entrypoint").hexdigest()
    )
    assert calls == [(custody_spelling(selected), "--version")]
    if selected_sdk_version:
        command_identity._validate_toolchain_identity(PLAN, "wasm-ld", identity)
    else:
        with pytest.raises(ValueError, match="violates canonical policy"):
            command_identity._validate_toolchain_identity(PLAN, "wasm-ld", identity)


def test_sdk_projection_survives_queue_environment_and_binds_tool_bytes(
    tmp_path: Path,
) -> None:
    from molt.wasi_sdk_identity import SDK_CARGO_TARGETS, SDK_CARGO_TOOLS
    from tools.proof_queue_pkg import execution_environment

    binary = tmp_path / "tool"
    binary.write_bytes(b"selected SDK bytes")
    source = {
        "PATH": "native-tools",
        "WASI_SDK_PATH": str(tmp_path / "sdk"),
        "WASI_SYSROOT": str(tmp_path / "sdk" / "share" / "wasi-sysroot"),
        "MOLT_WASM_LD": str(binary),
        "MOLT_LLVM_NM": str(binary),
    }
    executable_names = {"MOLT_WASM_LD", "MOLT_LLVM_NM"}
    for target in SDK_CARGO_TARGETS:
        for spelling in (target, target.replace("-", "_")):
            for role, _name in SDK_CARGO_TOOLS:
                source[f"{role}_{spelling}"] = str(binary)
                executable_names.add(f"{role}_{spelling}")
            for role in ("CFLAGS", "CXXFLAGS"):
                source[f"{role}_{spelling}"] = "--no-default-config"
    selected, _contract = execution_environment._deterministic_execution_environment(
        source, override_names=[]
    )
    assert all(selected[name] == value for name, value in source.items())
    before = execution_environment._execution_environment_executable_identities(
        selected, cwd=tmp_path
    )
    assert set(before) == executable_names
    binary.write_bytes(b"changed SDK bytes")
    after = execution_environment._execution_environment_executable_identities(
        selected, cwd=tmp_path
    )
    assert all(
        before[name]["executable"]["sha256"] != after[name]["executable"]["sha256"]
        for name in executable_names
    )


def test_tool_release_lanes_run_the_pinned_release_first_on_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import os

    from molt import dx, tool_releases
    from tools.proof_queue_pkg import guarded_execution as ge

    assert {"wasm-tools", "uv"} <= ge.tool_release_toolchains(PLAN)
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

    custody = dx.CheckoutCustody(
        source_root=tmp_path,
        custody_root=tmp_path,
        toolchain_root=toolchain_root,
        kind="durable",
    )
    monkeypatch.setattr(tool_releases, "provision_tool", provision)
    monkeypatch.setattr(
        "molt.dx.checkout_custody", lambda root, env=None, **_kwargs: custody
    )
    ambient = os.pathsep.join([str(tmp_path / "cargo-bin"), str(tmp_path / "other")])
    env, prefixes = ge.prefer_tool_release_prefixes(
        {"PATH": ambient}, ["python", "wasm-tools"], cwd=tmp_path
    )
    assert provisioned == ["wasm-tools"]
    assert prefixes == {"wasm-tools": str(prefix)}
    assert env["PATH"].split(os.pathsep)[0] == str(executable.parent.resolve())
    assert env["PATH"].split(os.pathsep)[1:] == ambient.split(os.pathsep)

    # A selection without a tool release provisions nothing and keeps PATH.
    untouched, prefixes = ge.prefer_tool_release_prefixes(
        {"PATH": ambient}, ["python", "rustc"], cwd=tmp_path
    )
    assert prefixes == {} and untouched["PATH"] == ambient


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_named_lane_entry_routes_dedicated_lanes_through_their_aperture(
    monkeypatch: pytest.MonkeyPatch,
    target: str,
) -> None:
    # A named acceptance lane must produce the same target-qualified run as the
    # dedicated `pact-witness-acceptance` command: the acceptance tool fails
    # closed without the provenance pins that spec carries.
    seen: list[str] = []

    def acceptance(args: argparse.Namespace) -> int:
        seen.append(args.lane_id)
        return 0

    lane_id = f"pact.witness.acceptance.{target}"
    monkeypatch.setitem(pact._DEDICATED_NAMED_LANE_HANDLERS, lane_id, acceptance)
    args = argparse.Namespace(lane_id=lane_id, timeout=None)
    assert pact._cmd_named_lane(args) == 0
    assert seen == [lane_id]
    assert set(pact._DEDICATED_NAMED_LANE_HANDLERS) == {
        "pact.witness.acceptance.native",
        "pact.witness.acceptance.wasm",
        "pact.witness.oracle",
    }


@pytest.mark.parametrize("target", ["native", "wasm"])
def test_named_acceptance_dispatch_preserves_target(
    monkeypatch: pytest.MonkeyPatch, target: str
) -> None:
    seen: list[tuple[str, str]] = []

    def spec(selected: str, _timeout, _repo_root):
        return {"target": selected}

    def run(args: argparse.Namespace, selected_spec: dict[str, str]) -> int:
        seen.append((args.lane_id, selected_spec["target"]))
        return 0

    monkeypatch.setattr(pact, "_pact_witness_acceptance_spec", spec)
    monkeypatch.setattr(pact, "_run_named_spec", run)
    args = argparse.Namespace(
        lane_id=f"pact.witness.acceptance.{target}",
        timeout=None,
        env=[],
        repo_root=None,
    )
    assert pact._cmd_named_lane(args) == 0
    assert seen == [(args.lane_id, target)]


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


@pytest.mark.parametrize("lane_id", PYTHON_LANE_IDS)
def test_prepared_named_lane_keeps_exact_registered_payload_and_closure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, lane_id: str
) -> None:
    from molt.cli import source_build_environment as environment_authority
    from molt.cli import source_build_environment_schema as environment_authority_schema

    custody = tmp_path / "environments"
    root = custody / ("a" * 64)
    python = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    python.parent.mkdir(parents=True)
    python.write_bytes(b"prepared-image")
    (root / environment_authority_schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST).write_text(
        "{}", encoding="utf-8"
    )
    monkeypatch.setattr(
        environment_authority, "_source_build_custody_root", lambda _repo: custody
    )
    if lane_id == "native.capability-cache-policy":
        # This lane migrated to pytest's module entrypoint. Preparation admits
        # plain scripts only; it must not silently rewrite the module payload.
        with pytest.raises(ValueError, match="not a plain Python script"):
            command_admission.prepared_named_lane_command(lane_id, python)
        return
    command = command_admission.prepared_named_lane_command(lane_id, python)
    envelope = command_admission.envelope_for_command(command)
    assert envelope["kind"] == "named-lane"
    assert envelope["proof_plan_command_ids"] == [lane_id]
    assert envelope["typed_command"]["environment_root"] == str(root)
    assert (
        command_admission.cargo_cache_domain(envelope)
        == PLAN.named_lane(lane_id).data["cache_domain"]
    )
    assert set(PLAN.named_lane(lane_id).toolchains) <= set(envelope["toolchains"])
    with pytest.raises(ValueError, match="must match its registered command exactly"):
        command_admission.envelope_for_command([*command, "--changed"])
    with pytest.raises(ValueError, match="direct locked interpreter with -P"):
        command_admission.envelope_for_command([command[0], *command[2:]])
    (root / environment_authority_schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST).unlink()
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
    from molt.cli.source_build_environment_schema import (
        SOURCE_BUILD_ENVIRONMENT_MANIFEST,
    )
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
                "path": custody_spelling(captured),
                "sha256": hashlib.sha256(b"same-image").hexdigest(),
            }
        ],
    }
    # The Node hook launches the broker's selection, so an absolute selection
    # is the image on every host.
    admitted = server._decide_child({"requested": str(captured)}, "node")
    assert admitted["admitted"] is True
    rejected = server._decide_child({"requested": str(copied)}, "node")
    assert rejected["admitted"] is False
    assert rejected["reason"] == "outside-declared-toolchain-closure"
    captured.write_bytes(b"changed-image")
    changed = server._decide_child({"requested": str(captured)}, "node")
    assert changed["admitted"] is False


def test_sdk_role_and_generation_use_request_cwd_separately_from_probe_cwd(
    tmp_path, monkeypatch
):
    import subprocess
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        provisioned_wasi_sdk_fixture,
    )
    from tools.proof_queue_pkg import command_identity

    request = tmp_path / "request"
    (request / "relative-tools").mkdir(parents=True)
    installation = provisioned_wasi_sdk_fixture(
        RuntimeFixtureRoot(request / "relative-tools")
    )
    ambient = tmp_path / "ambient"
    ambient.mkdir()
    monkeypatch.chdir(ambient)
    policy = next(item for item in PLAN.toolchain_policies if item.name == "wasi-clang")
    selected_policy = replace(policy, data={**policy.data, "probe_cwd": "tools"})
    plan = SimpleNamespace(toolchain_policies=(selected_policy,))
    env = {"MOLT_TARGET_ROOT": "relative-tools/toolchains"}
    probes = []

    def version(command, *, cwd, env):
        assert cwd == proof_plan.ROOT / "tools"
        assert command[0] == custody_spelling(
            installation.sdk / installation.tool_fact("clang")["path"]
        )
        probes.append(tuple(command))
        return subprocess.CompletedProcess(
            command, 0, "clang version " + installation.asset.llvm_version, ""
        )

    monkeypatch.setattr(command_identity, "_run_captured", version)
    identity = command_identity._tool_identity(
        plan,
        "wasi-clang",
        {"python": None},
        ["clang", "--version"],
        cwd=request,
        env=env,
    )
    assert identity["path"] == custody_spelling(
        installation.sdk / installation.tool_fact("clang")["path"]
    )
    # The SDK closure reports the installation, not an executable custody key.
    assert identity["wasi_sdk"]["sdk"] == str(installation.sdk)
    assert identity["probe_cwd"] == str(proof_plan.ROOT / "tools")
    assert probes == [
        (
            custody_spelling(
                installation.sdk / installation.tool_fact("clang")["path"]
            ),
            "--version",
        )
    ]
    assert not (ambient / "relative-tools").exists()


DIFFERENTIAL_LANES = tuple(
    lane.id for lane in PLAN.named_lanes if lane.differential_cases
)


@pytest.fixture
def selected_diff_sources(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    suite = tmp_path / "tests" / "differential" / "stdlib"
    suite.mkdir(parents=True)
    first = suite / "a.py"
    second = suite / "b.py"
    first.write_bytes(b"print('a')\n")
    second.write_bytes(b"print('b')\n")
    monkeypatch.setattr(command_admission, "_REPO_ROOT", tmp_path)
    return tmp_path, first, second


@pytest.mark.parametrize("lane_id", DIFFERENTIAL_LANES)
def test_default_differential_lanes_keep_canonical_recipe_source_authority(lane_id):
    lane = PLAN.named_lane(lane_id)
    envelope = command_admission.envelope_for_command(lane.argv)
    assert envelope["kind"] == "named-lane"
    assert envelope["typed_command"] is None
    assert "--" not in lane.argv
    assert command_admission.differential_source_bindings(envelope) == {}


@pytest.mark.parametrize("lane_id", DIFFERENTIAL_LANES)
def test_differential_file_and_list_selection_freezes_exact_case_closure(
    selected_diff_sources, lane_id
):
    root, first, second = selected_diff_sources
    selected_list = root / "selected.txt"
    selected_list.write_text(
        "# subset\n\n" + first.relative_to(root).as_posix() + "\n", encoding="utf-8"
    )
    lane = PLAN.named_lane(lane_id)
    spec = pact._named_lane_spec(
        lane_id,
        repo_root=root,
        cases=[second.relative_to(root).as_posix()],
        case_lists=["selected.txt"],
    )
    files = ["tests/differential/stdlib/a.py", "tests/differential/stdlib/b.py"]
    assert spec["command"] == [*lane.command_prefix, "--", *files]
    assert spec["scopes"] == ["tools/proof_plan.toml", *files]
    assert spec["resource_family"] == lane.data["resource_family"]
    assert spec["contention_key"] == lane.data["contention_key"]
    envelope = command_admission.envelope_for_command(spec["command"])
    assert envelope["proof_plan_command_ids"] == [lane_id]
    assert envelope["process_closure"]["kind"] == "named-lane"
    assert envelope["cargo_native_units"] == lane.data["cargo_native_units"]
    assert set(lane.toolchains) <= set(envelope["toolchains"])
    assert command_admission.differential_source_bindings(envelope) == {
        files[0]: hashlib.sha256(b"print('a')\n").hexdigest(),
        files[1]: hashlib.sha256(b"print('b')\n").hexdigest(),
    }
    # The persisted file closure, not this mutable submission list, is replayed.
    selected_list.write_text("outside.py\n", encoding="utf-8")
    command_admission.validate_envelope(envelope, spec["command"])
    first.write_bytes(b"print('z')\n")
    with pytest.raises(ValueError, match="envelope"):
        command_admission.validate_envelope(envelope, spec["command"])


@pytest.mark.parametrize(
    "platform,expected",
    [
        ("linux", {"posix-sh", "posix-uname"}),
        ("macos", {"posix-sh", "posix-uname", "posix-file"}),
        ("windows", set()),
    ],
)
def test_differential_host_tools_follow_selected_cases_and_platform(
    selected_diff_sources, monkeypatch, platform, expected
):
    from tools.compat import test_policy

    root, first, second = selected_diff_sources
    first.write_text(
        "# MOLT_META: host_toolchains=macos:posix-file,posix:posix-sh,posix:posix-uname\n"
        "print('a')\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(test_policy, "current_platform_name", lambda: platform)
    lane = PLAN.named_lane("runtime.public-consumers.native")
    base = set(PLAN.toolchain_closure(lane.toolchains))
    # Admission adds the Cargo build generator the lane declares; the static
    # lane closure has no Cargo context (docs/agent/PROOF_QUEUE.md).
    generator = (
        set(PLAN.toolchain_closure(["build-python"]))
        if lane.data.get("cargo_build_python") is True
        else set()
    )
    for case, extra in ((first, expected), (second, set())):
        relative = case.relative_to(root).as_posix()
        command = command_admission.selected_differential_lane_command(
            lane.id, cases=[relative]
        )
        envelope = command_admission.envelope_for_command(command)
        assert set(envelope["toolchains"]) == base | extra | generator
        assert envelope["process_closure"]["toolchains"] == envelope["toolchains"]
        # Default recipes consume the same declaration as explicit selections.
        default = replace(lane, data={**lane.data, "differential_cases": [relative]})
        assert set(command_admission._named_lane_toolchains(default)) == base | extra


def test_differential_host_tools_reject_all_unknown_policies(
    selected_diff_sources, monkeypatch
):
    from tools.compat import test_policy

    root, first, second = selected_diff_sources
    first.write_text(
        "# MOLT_META: host_toolchains=posix:missing-posix,windows:missing-windows\n"
        "print('a')\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(test_policy, "current_platform_name", lambda: "linux")
    command = command_admission.selected_differential_lane_command(
        "runtime.public-consumers.native", cases=[first.relative_to(root).as_posix()]
    )
    with pytest.raises(ValueError, match="missing-posix, missing-windows"):
        command_admission.envelope_for_command(command)


def test_differential_suite_expansion_is_frozen_before_queueing(selected_diff_sources):
    root, first, second = selected_diff_sources
    command = command_admission.selected_differential_lane_command(
        "runtime.public-consumers.native", cases=["tests/differential/stdlib"]
    )
    assert command[-3:] == [
        "--",
        "tests/differential/stdlib/a.py",
        "tests/differential/stdlib/b.py",
    ]
    envelope = command_admission.envelope_for_command(command)
    (first.parent / "c.py").write_text("print('new member')\n", encoding="utf-8")
    command_admission.validate_envelope(envelope, command)
    assert len(command_admission.differential_source_bindings(envelope)) == 2


@pytest.mark.parametrize(
    "mutation", ["foreign", "link", "duplicate", "unknown", "empty-case", "empty"]
)
def test_differential_selection_rejects_unowned_or_ambiguous_inputs(
    selected_diff_sources, mutation
):
    root, first, second = selected_diff_sources
    cases = [first.relative_to(root).as_posix()]
    lists = []
    if mutation == "foreign":
        outside = root.parent / (root.name + "-foreign.py")
        outside.write_text("print('foreign')\n", encoding="utf-8")
        cases = [str(outside)]
    elif mutation == "link":
        link = root / "alias.py"
        try:
            link.symlink_to(first)
        except OSError as exc:
            pytest.skip(f"host cannot create symlink control: {exc}")
        cases = ["alias.py"]
    elif mutation == "duplicate":
        cases *= 2
    elif mutation == "unknown":
        cases = ["--unknown"]
    elif mutation == "empty-case":
        # Path("") would otherwise expand this real root-level program.
        (root / "accidental.py").write_text("print('wrong case')\n", encoding="utf-8")
        cases = [""]
    else:
        (root / "empty.txt").write_text("# no cases\n", encoding="utf-8")
        cases, lists = [], ["empty.txt"]
    with pytest.raises(ValueError):
        command_admission.selected_differential_lane_command(
            "runtime.public-consumers.native", cases=cases, case_lists=lists
        )


@pytest.mark.parametrize(
    "option,value", [("--target", "llvm"), ("--jobs", "2"), ("--build-profile", "dev")]
)
def test_differential_selection_cannot_change_fixed_lane_options(
    selected_diff_sources, option, value
):
    command = command_admission.selected_differential_lane_command(
        "runtime.public-consumers.native", cases=["tests/differential/stdlib/a.py"]
    )
    command[command.index(option) + 1] = value
    with pytest.raises(ValueError, match="registered command exactly"):
        command_admission.envelope_for_command(command)


def test_differential_admission_rejects_unexpanded_tail_and_unknown_arguments(
    selected_diff_sources,
):
    lane = PLAN.named_lane("runtime.public-consumers.native")
    for tail in (["tests/differential/stdlib"], ["--files-from", "selected.txt"], []):
        with pytest.raises(ValueError):
            command_admission.envelope_for_command([*lane.command_prefix, "--", *tail])
    with pytest.raises(ValueError, match="differential named lane"):
        pact._named_lane_spec(
            "runtime.allocation-observer", cases=["tests/differential/stdlib/a.py"]
        )


def test_differential_list_must_be_owned_and_cli_rejects_unknown_options(
    selected_diff_sources,
):
    from tools.proof_queue_pkg import cli

    root, first, second = selected_diff_sources
    outside = root.parent / (root.name + "-list.txt")
    outside.write_text(first.relative_to(root).as_posix() + "\n", encoding="utf-8")
    with pytest.raises(ValueError, match="escapes"):
        command_admission.selected_differential_lane_command(
            "runtime.public-consumers.native", cases=[], case_lists=[str(outside)]
        )
    parser = cli._build_parser()
    args = parser.parse_args(
        [
            "named-lane",
            "runtime.public-consumers.native",
            "--case",
            "a.py",
            "--cases-from",
            "cases.txt",
            "--print-spec",
        ]
    )
    assert args.case == ["a.py"] and args.cases_from == ["cases.txt"]
    with pytest.raises(SystemExit):
        parser.parse_args(
            ["named-lane", "runtime.public-consumers.native", "--unknown"]
        )


def test_prepared_differential_lane_keeps_selected_files(
    selected_diff_sources, monkeypatch
):
    from molt.cli import source_build_environment as environment_authority
    from molt.cli.source_build_environment_schema import (
        SOURCE_BUILD_ENVIRONMENT_MANIFEST,
    )

    root, first, second = selected_diff_sources
    first.write_text(
        "# MOLT_META: host_toolchains=posix:posix-sh\nprint('a')\n", encoding="utf-8"
    )
    custody = root / "environments"
    environment = custody / ("a" * 64)
    python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    python.parent.mkdir(parents=True)
    python.write_bytes(b"prepared-interpreter")
    (environment / SOURCE_BUILD_ENVIRONMENT_MANIFEST).write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        environment_authority, "_source_build_custody_root", lambda _root: custody
    )
    command = command_admission.selected_differential_lane_command(
        "runtime.public-consumers.native", cases=["tests/differential/stdlib/a.py"]
    )
    expected = command_admission.envelope_for_command(command)
    prepared = command_admission.prepared_named_lane_command(
        "runtime.public-consumers.native", python, command=command
    )
    assert prepared[:3] == [str(python), "-P", "tests/molt_diff.py"]
    actual = command_admission.envelope_for_command(prepared)
    assert actual["typed_command"]["family"] == "prepared-named-lane"
    assert (
        actual["typed_command"]["selected_cases"]
        == expected["typed_command"]["selected_cases"]
    )
    assert actual["toolchains"] == expected["toolchains"]
    with pytest.raises(ValueError, match="another named lane"):
        command_admission.prepared_named_lane_command(
            "runtime.public-consumers.wasm", python, command=command
        )


def test_cargo_workspace_admission_requires_registered_development_domain():
    from tools.proof_queue_pkg.cargo_output_environment import CargoOutputEnvironment

    command = next(
        c for c in PLAN.commands if c.id == "rust.test.runtime-cold-lifecycle"
    )
    registered = command_admission.envelope_for_command(command.argv)
    for envelope in (registered, {"delegated": registered}):
        assert (
            CargoOutputEnvironment.for_envelope(envelope).cache_domain
            == "cargo-workspace"
        )
    direct = command_admission.envelope_for_command(
        ["cargo", "test", "--lib", "unregistered_filter"]
    )
    assert CargoOutputEnvironment.for_envelope(direct).cache_domain == "none"
    producer = command_admission.envelope_for_command(
        list(PLAN.named_lane("runtime.lifecycle-cost-image").argv)
    )
    assert CargoOutputEnvironment.for_envelope(producer).cache_domain == "none"
    with pytest.raises(ValueError, match="unknown"):
        command_admission.cargo_cache_domain(
            {**registered, "proof_plan_command_ids": ["foreign"]}
        )
    with pytest.raises(ValueError, match="disagree"):
        command_admission.cargo_cache_domain(
            {
                "kind": "named-lane",
                "proof_plan_command_ids": [
                    "runtime.lifecycle-cost-image",
                    "runtime.allocation-observer",
                ],
            }
        )

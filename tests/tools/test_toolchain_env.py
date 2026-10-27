"""tools/toolchain_env.py projects the proof plan's pinned toolchains (HF-231)."""

from __future__ import annotations

import os
from pathlib import Path
import re
import sys
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[2]
for _path in (str(ROOT), str(ROOT / "src")):
    if _path not in sys.path:
        sys.path.insert(0, _path)

from molt import llvm_toolchain, tool_releases, wasm_optimizer_identity  # noqa: E402
from molt.wasi_sdk_identity import SDK_CARGO_TARGETS  # noqa: E402
from tests.runtime_build_identity_helper import (  # noqa: E402
    RuntimeFixtureRoot,
    provisioned_wasi_sdk_fixture,
)
from tools import proof_plan, toolchain_env  # noqa: E402

PLAN = proof_plan.ProofPlan.load()
# The WASI SDK's projection: the selectors cc-rs, Cargo and Molt read.
WASI_KEY = re.compile(
    r"^(MOLT_)?WASI_|^(CC|CXX|AR|RANLIB|CFLAGS|CXXFLAGS)_wasm32|"
    r"^CARGO_TARGET_WASM32_|^MOLT_WASM_LD$|^MOLT_LLVM_NM$|^MOLT_WASI_C_ABI_PLAN$"
)


def _fake_providers(monkeypatch: pytest.MonkeyPatch, *, installed: bool) -> None:
    """Fake each provider where the command reads it; a fake never installs."""

    def llvm_delta(_root, _verification, *, environ):
        return {
            "MOLT_LLVM_PREFIX": "/llvm",
            "PATH": os.pathsep.join((str(Path("/llvm/bin")), environ["PATH"])),
        }

    def wasm_delta(_installation, *, environ, rust_target=None):
        assert rust_target is None
        del environ
        return {"WASI_SDK_PATH": "/wasi/sdk", "CC_wasm32_wasip1": "/wasi/bin/clang"}

    def discover(name, _root, *, environ, cwd):
        del environ, cwd
        executable = Path(f"/tools/{name}/bin/{name}")
        return SimpleNamespace(executable=executable) if installed else None

    def refuse_download(*_args, **_kwargs):
        raise AssertionError("toolchain_env must never download")

    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda _root, *, environ, cwd: object() if installed else None,
    )
    monkeypatch.setattr(llvm_toolchain, "llvm_toolchain_environment_delta", llvm_delta)
    monkeypatch.setattr(
        llvm_toolchain,
        "selected_wasi_sdk_installation",
        lambda _root, *, environ, cwd: object() if installed else None,
    )
    monkeypatch.setattr(llvm_toolchain, "wasm_toolchain_environment_delta", wasm_delta)
    monkeypatch.setattr(tool_releases, "discover_pinned_tool", discover)
    monkeypatch.setattr(tool_releases, "provision_tool", refuse_download)
    monkeypatch.setattr(
        wasm_optimizer_identity,
        "managed_wasm_opt",
        lambda *, environment: "/binaryen/bin/wasm-opt" if installed else None,
    )


def _delta(**selection: object) -> dict[str, str]:
    names = toolchain_env.selected_toolchains(
        PLAN,
        commands=list(selection.get("commands", [])),
        family=selection.get("family"),
        matrix_cell=None,
        toolchains=list(selection.get("toolchains", [])),
    )
    return toolchain_env.toolchain_environment(
        PLAN, names, environ={"PATH": str(Path("/usr/bin"))}
    )


def test_projection_selects_native_and_release_tools_in_the_request_context(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "compiler source"
    request_cwd = tmp_path / "request"
    environment = {"PATH": "caller-bin", "MOLT_TARGET_ROOT": "request-tools"}
    verification = object()
    calls = []

    def verify(selected_root, *, environ, cwd):
        assert selected_root == root
        assert environ == environment
        assert cwd == request_cwd
        calls.append("native")
        return verification

    def llvm_delta(selected_root, selected, *, environ):
        assert selected_root == root
        assert selected is verification
        assert environ == environment
        return {"MOLT_LLVM_PREFIX": "selected-native", "PATH": "native-bin"}

    def discover(name, selected_root, *, environ, cwd):
        assert name == "wasm-tools"
        assert selected_root == root
        assert environ == {
            **environment,
            "MOLT_LLVM_PREFIX": "selected-native",
            "PATH": "native-bin",
        }
        assert cwd == request_cwd
        calls.append("release")
        return SimpleNamespace(executable=tmp_path / "release-bin" / "wasm-tools")

    monkeypatch.setattr(llvm_toolchain, "verify_available_llvm_toolchain", verify)
    monkeypatch.setattr(llvm_toolchain, "llvm_toolchain_environment_delta", llvm_delta)
    monkeypatch.setattr(tool_releases, "discover_pinned_tool", discover)
    monkeypatch.setenv("MOLT_TARGET_ROOT", "unrelated controller tools")

    projected = proof_plan.project_toolchains(
        PLAN.toolchain_policies,
        ("clang", "wasm-tools"),
        environ=environment,
        root=root,
        cwd=request_cwd,
    )

    assert projected.missing == ()
    assert projected.delta["MOLT_LLVM_PREFIX"] == "selected-native"
    assert projected.delta["PATH"] == os.pathsep.join(
        (str(tmp_path / "release-bin"), "native-bin")
    )
    assert calls == ["native", "release"]
    assert environment == {"PATH": "caller-bin", "MOLT_TARGET_ROOT": "request-tools"}


def test_wasi_variables_appear_only_when_the_selection_needs_the_wasi_sdk(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _fake_providers(monkeypatch, installed=True)
    native = _delta(toolchains=["ld.lld", "wasm-tools"])
    assert native["MOLT_LLVM_PREFIX"] == "/llvm"
    assert native["PATH"].split(os.pathsep) == [
        str(Path("/tools/wasm-tools/bin")),
        str(Path("/llvm/bin")),
        str(Path("/usr/bin")),
    ]
    assert not [key for key in native if WASI_KEY.search(key)]
    wasm = _delta(commands=["wasm.build.backend"])
    assert wasm["WASI_SDK_PATH"] == "/wasi/sdk"
    assert wasm["CC_wasm32_wasip1"] == "/wasi/bin/clang"
    # A command that declares only host tools projects nothing.
    assert _delta(commands=["rust.check.fuzz"]) == {}
    # uv is a pinned tool release (a9f61e4f3): a command that runs it puts the
    # pinned uv first, and nothing else.
    assert _delta(commands=["nightly.determinism.ir-hashseed"]) == {
        "PATH": os.pathsep.join([str(Path("/tools/uv/bin")), str(Path("/usr/bin"))])
    }


def test_binaryen_projection_pins_the_managed_wasm_opt(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _fake_providers(monkeypatch, installed=True)
    assert _delta(toolchains=["wasm-opt"]) == {
        "MOLT_WASM_OPT": "/binaryen/bin/wasm-opt"
    }


def test_missing_providers_name_their_install_commands_and_never_download(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    _fake_providers(monkeypatch, installed=False)
    selection = ["--toolchain", "ld.lld", "--toolchain", "wasm-tools"]
    assert toolchain_env.main([*selection, "--toolchain", "wasm-ld"]) == 2
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "never downloads" in captured.err
    assert "LLVM SDK (ld.lld)" in captured.err
    assert "python3 -m molt.tool_releases provision wasm-tools" in captured.err
    assert "wasi-sdk (wasm-ld): python3 tools/provision_wasi_sdk.py" in captured.err


@pytest.mark.skipif(os.name != "posix", reason="POSIX decoy executables")
def test_admission_puts_pinned_providers_ahead_of_the_ambient_shell(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # HF-231: Homebrew's LLD 23.1.3 and wasm-tools 1.262.0 first on PATH.
    if llvm_toolchain.verify_available_llvm_toolchain(ROOT) is None:
        pytest.skip("no verified LLVM SDK is provisioned")
    if tool_releases.discover_pinned_tool("wasm-tools", ROOT) is None:
        pytest.skip("the pinned wasm-tools release is not provisioned")
    decoys = tmp_path / "homebrew" / "bin"
    decoys.mkdir(parents=True)
    for name, banner in (
        ("ld.lld", "Homebrew LLD 23.1.3 (compatible with GNU linkers)"),
        ("wasm-tools", "wasm-tools 1.262.0"),
    ):
        decoy = decoys / name
        decoy.write_text(f"#!/bin/sh\necho '{banner}'\n", encoding="utf-8")
        decoy.chmod(0o755)
    monkeypatch.setenv("PATH", f"{decoys}{os.pathsep}{os.environ['PATH']}")
    names = PLAN.toolchain_closure(["ld.lld", "wasm-tools"])
    # The version contract refuses the ambient tools ...
    with pytest.raises(ValueError, match="toolchain contract violation"):
        proof_plan.toolchain_fingerprints(
            PLAN.toolchain_policies,
            names,
            environment=os.environ,
            python_executable=Path(sys.executable),
        )
    # ... and every executor admits through the projection CI uses, so no
    # shell needs projecting by hand. The projection lives in the result:
    # admission leaves the process environment exactly as it found it.
    before = dict(os.environ)
    admitted = proof_plan.admit_toolchains(
        PLAN.toolchain_policies,
        names,
        root=ROOT,
        environ=os.environ,
        python_executable=Path(sys.executable),
    )
    assert dict(os.environ) == before
    assert set(names) <= set(admitted.fingerprints)
    assert not [key for key in admitted.environment if WASI_KEY.search(key)]
    assert not admitted.environment["PATH"].startswith(str(decoys))
    assert admitted.environment == toolchain_env.toolchain_environment(
        PLAN, names, environ=dict(os.environ)
    )


def test_llvm_projection_matches_what_ci_exports(tmp_path: Path) -> None:
    verification = llvm_toolchain.verify_available_llvm_toolchain(ROOT)
    if verification is None:
        pytest.skip("no verified LLVM SDK is provisioned")
    github_env = tmp_path / "github-env"
    assert llvm_toolchain.main(["--verify", "--github-env", str(github_env)]) == 0
    exported = dict(
        line.split("=", 1)
        for line in github_env.read_text(encoding="utf-8").splitlines()
    )
    names = PLAN.toolchain_closure(["ld.lld"])
    projected = toolchain_env.toolchain_environment(
        PLAN, names, environ=dict(os.environ)
    )
    assert projected == exported


@pytest.mark.parametrize("rust_target", [None, *SDK_CARGO_TARGETS])
def test_wasm_projection_sets_only_the_keys_its_selection_clear_removes(
    tmp_path: Path, rust_target: str | None
) -> None:
    # clear_wasm_toolchain_selection removes wasm_toolchain_environment_keys();
    # a projected key outside that set would leak into tests that select
    # their own SDK.
    install = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    projected = llvm_toolchain.project_wasm_toolchain_environment(
        install, environ={}, rust_target=rust_target
    )
    assert set(projected) <= set(llvm_toolchain.wasm_toolchain_environment_keys())


@pytest.mark.parametrize("explicit_target_flags", [False, True])
def test_generic_sdk_projection_keeps_rust_only_provider_startup_independent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, explicit_target_flags: bool
) -> None:
    from functools import partial

    from molt.cli import runtime_cargo_plan as plans
    from tests.runtime_build_identity_helper import runtime_cargo_plan
    from tools.proof_queue_pkg.toolchain_capture import (
        resolve_runtime_gate_cargo_operation,
    )
    from tools.runtime_test_gate_plan import cargo_operations

    fixture_root = RuntimeFixtureRoot(tmp_path)
    installation = provisioned_wasi_sdk_fixture(fixture_root)
    monkeypatch.setattr(
        llvm_toolchain,
        "selected_wasi_sdk_installation",
        lambda _root, *, environ, cwd=None: installation,
    )
    target_flags = "CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS"
    target_linker = "CARGO_TARGET_WASM32_WASIP1_LINKER"
    baseline = {"PATH": os.environ.get("PATH", ""), "RUSTFLAGS": "--cfg user_preserved"}
    if explicit_target_flags:
        baseline[target_flags] = "--cfg explicit_target_preserved"
        baseline[target_linker] = str(tmp_path / "explicit-user-linker")
    original_baseline = dict(baseline)
    original_process = dict(os.environ)
    # Keep the real SDK projection: faking this delta would hide the original
    # outer-command leak into the provider that deliberately owns no C ABI.
    delta = toolchain_env.toolchain_environment(
        PLAN,
        PLAN.toolchain_closure(["wasi-clang", "wasm-ld"]),
        environ=baseline,
    )
    environment = {**baseline, **delta}
    assert target_flags not in delta
    assert target_linker not in delta
    assert environment.get(target_flags) == baseline.get(target_flags)
    assert environment.get(target_linker) == baseline.get(target_linker)
    assert "CARGO_ENCODED_RUSTFLAGS" not in environment
    assert environment["RUSTFLAGS"] == "--cfg user_preserved"
    assert environment["WASI_SDK_PATH"] == str(installation.sdk)

    # The existing fixture retains the real RuntimeCargoPlan and ABI/resource
    # checks, replacing compiler discovery with pytest-owned tool images only.
    monkeypatch.setattr(
        plans,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=fixture_root),
    )
    operations = cargo_operations(
        "wasi-lifecycle",
        "dev-fast",
        cargo="cargo",
        compiler_host="x86_64-unknown-linux-gnu",
    )
    selected = {
        operation.role: resolve_runtime_gate_cargo_operation(
            operation,
            cwd=tmp_path,
            env=environment,
            compiler_host="x86_64-unknown-linux-gnu",
        )
        for operation in operations
    }
    runtime_command, runtime_environment, runtime_plan = selected["runtime"]
    assert runtime_plan is not None
    assert runtime_command[runtime_command.index("-p") + 1] == "molt-runtime"
    assert "link-self-contained=no" in runtime_plan.rustflags
    assert "link-arg=--export-table" in runtime_plan.rustflags
    assert "user_preserved" in runtime_plan.rustflags
    assert runtime_environment[target_linker] == str(installation.wasm_ld)
    for role in ("host", "provider"):
        command, child_environment, plan = selected[role]
        assert command == list(
            next(row.command for row in operations if row.role == role)
        )
        assert plan is None
        assert dict(child_environment) == environment
        assert child_environment.get(target_flags) == baseline.get(target_flags)
        assert child_environment.get(target_linker) == baseline.get(target_linker)
        assert "CARGO_ENCODED_RUSTFLAGS" not in child_environment
    assert dict(os.environ) == original_process
    assert baseline == original_baseline

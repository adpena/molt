"""tools/toolchain_env.py projects the proof plan's pinned toolchains (HF-231)."""

from __future__ import annotations

import os
from pathlib import Path
import re
import sys
from types import SimpleNamespace

import pytest
from unittest import mock

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
        return {"MOLT_LLVM_PREFIX": "/llvm", "PATH": f"/llvm/bin:{environ['PATH']}"}

    def wasm_delta(_installation, *, environ):
        del environ
        return {"WASI_SDK_PATH": "/wasi/sdk", "CC_wasm32_wasip1": "/wasi/bin/clang"}

    def discover(name, _root, *, environ):
        del environ
        executable = Path(f"/tools/{name}/bin/{name}")
        return SimpleNamespace(executable=executable) if installed else None

    def refuse_download(*_args, **_kwargs):
        raise AssertionError("toolchain_env must never download")

    monkeypatch.setattr(
        llvm_toolchain,
        "verify_available_llvm_toolchain",
        lambda _root: object() if installed else None,
    )
    monkeypatch.setattr(llvm_toolchain, "llvm_toolchain_environment_delta", llvm_delta)
    monkeypatch.setattr(
        llvm_toolchain,
        "selected_wasi_sdk_installation",
        lambda _root, *, environ: object() if installed else None,
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
        PLAN, names, environ={"PATH": "/usr/bin"}
    )


def test_wasi_variables_appear_only_when_the_selection_needs_the_wasi_sdk(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _fake_providers(monkeypatch, installed=True)
    native = _delta(toolchains=["ld.lld", "wasm-tools"])
    assert native["MOLT_LLVM_PREFIX"] == "/llvm"
    assert native["PATH"].split(os.pathsep)[:2] == [
        "/tools/wasm-tools/bin",
        "/llvm/bin",
    ]
    assert not [key for key in native if WASI_KEY.search(key)]
    wasm = _delta(commands=["wasm.build.backend"])
    assert wasm["WASI_SDK_PATH"] == "/wasi/sdk"
    assert wasm["CC_wasm32_wasip1"] == "/wasi/bin/clang"
    # A command that declares only host tools projects nothing.
    assert _delta(commands=["nightly.determinism.ir-hashseed"]) == {}


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
        proof_plan.toolchain_fingerprints(PLAN.toolchain_policies, names)
    # ... and every executor admits through the projection CI uses, so no
    # shell needs projecting by hand.
    with mock.patch.dict(os.environ):
        admitted = proof_plan.admit_toolchains(PLAN.toolchain_policies, names)
        assert set(names) <= set(admitted.fingerprints)
        assert not [key for key in admitted.environment if WASI_KEY.search(key)]
        assert {
            key: os.environ[key] for key in admitted.environment
        } == admitted.environment
        assert not os.environ["PATH"].startswith(str(decoys))
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


@pytest.mark.parametrize("rust_target", SDK_CARGO_TARGETS)
def test_wasm_projection_sets_only_the_keys_its_selection_clear_removes(
    tmp_path: Path, rust_target: str
) -> None:
    # clear_wasm_toolchain_selection removes wasm_toolchain_environment_keys();
    # a projected key outside that set would leak into tests that select
    # their own SDK.
    install = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    projected = llvm_toolchain.project_wasm_toolchain_environment(
        install, environ={}, rust_target=rust_target
    )
    assert set(projected) <= set(llvm_toolchain.wasm_toolchain_environment_keys())

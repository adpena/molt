"""Verified LLVM SDK tools for real-toolchain object-format proofs.

Tests that compile, archive, read or link real objects take their tools from
the verified LLVM SDK (`molt.llvm_toolchain`), never from ambient PATH guesses,
and run them in the environment the product projects for that SDK (PATH, and
on macOS SDKROOT and DEVELOPER_DIR), never in the ambient shell's (HF-231).
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace
import functools
import hashlib
import os
from pathlib import Path
import platform
import sys

import pytest


# Real-toolchain proofs compile one object family per object format. The LLVM
# SDK contract (config/llvm_toolchain_arches.toml) guarantees only the host
# architecture and WebAssembly backends, so every format uses the host
# architecture: an Apple-silicon SDK builds AArch64, not X86.
_OBJECT_FORMAT_TRIPLE_SUFFIXES = {
    "elf": "unknown-linux-gnu",
    "coff": "pc-windows-msvc",
    "macho": "apple-darwin",
}
_TRIPLE_ARCHITECTURES = {"x86_64": "x86_64", "aarch64": "aarch64"}
# ld64 names architectures by its own spelling, not by the LLVM triple.
_MACHO_ARCH_NAMES = {"x86_64": "x86_64", "aarch64": "arm64"}


@dataclass(frozen=True)
class VerifiedLlvmTools:
    """One coherent clang/llvm-ar/llvm-nm/lld family from the verified LLVM SDK.

    Ambient PATH tools are not a substitute: Apple clang has no WebAssembly
    backend and BSD ar rejects GNU modifiers such as ``D``.
    """

    bin_dir: Path
    architecture: str
    clang: str
    ar: str
    nm: str
    environment: Mapping[str, str]

    def native_target(self, object_format: str) -> str:
        return f"{self.architecture}-{_OBJECT_FORMAT_TRIPLE_SUFFIXES[object_format]}"

    @property
    def macho_arch(self) -> str:
        return _MACHO_ARCH_NAMES[self.architecture]

    def linker(self, role: str) -> str:
        from molt.llvm_wasi_tools import llvm_linker_candidates

        return str(
            _sdk_tool(
                self.bin_dir,
                role,
                llvm_linker_candidates(role, sibling_directories=(self.bin_dir,)),
            )
        )


def _sdk_tool(bin_dir: Path, role: str, candidates: Sequence[Path]) -> Path:
    key = os.path.normcase(os.path.abspath(bin_dir))
    for candidate in candidates:
        if os.path.normcase(os.path.abspath(candidate.parent)) == key:
            return candidate
    raise AssertionError(f"verified LLVM SDK {bin_dir} has no {role} entrypoint")


@functools.cache
def _verified_llvm_tools() -> VerifiedLlvmTools | str:
    from molt.llvm_wasi_tools import llvm_tool_candidates
    from molt.llvm_toolchain import (
        llvm_bootstrap_command,
        llvm_host_architecture,
        project_llvm_toolchain_environment,
        required_llvm_backend_pin,
        verify_available_llvm_toolchain,
    )
    from molt.source_root import compiler_source_root

    root = compiler_source_root()
    # A configured but invalid SDK raises LlvmToolchainConfigError: fail loud.
    verification = verify_available_llvm_toolchain(root)
    if verification is None:
        pin = required_llvm_backend_pin(root)
        provision = (
            ""
            if pin is None
            else f"; provision it with `{llvm_bootstrap_command(pin)}`"
        )
        return f"no verified LLVM SDK is configured or provisioned{provision}"
    host = llvm_host_architecture(root, platform.machine())
    assert host is not None  # verification already required the host target
    architecture = _TRIPLE_ARCHITECTURES.get(host.id)
    if architecture is None:
        raise AssertionError(
            f"no cross-format object triple is defined for host architecture {host.id}"
        )
    bin_dir = verification.prefix / "bin"
    clang, ar, nm = (
        str(
            _sdk_tool(
                bin_dir,
                role,
                llvm_tool_candidates(role, sibling_directories=(bin_dir,)),
            )
        )
        for role in ("cc", "ar", "nm")
    )
    environment = project_llvm_toolchain_environment(
        root, verification, environ=dict(os.environ)
    )
    return VerifiedLlvmTools(bin_dir, architecture, clang, ar, nm, environment)


def verified_llvm_tools() -> VerifiedLlvmTools:
    """Return the verified SDK family, or skip when no SDK is provisioned."""
    tools = _verified_llvm_tools()
    if isinstance(tools, str):
        pytest.skip(tools)
    return tools


def native_link_selection_fixture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """Captured Linux link inputs, independent of the execution host.

    Only the compiler search query is replaced; selection, byte admission and
    serialization use the production authorities.
    """
    from molt import llvm_link_plan, llvm_toolchain
    from molt.process_environment import ProcessEnvironment
    from tests.process_guard_common import install_module_view

    install_module_view(monkeypatch, "sys", sys, llvm_link_plan, platform="linux")
    monkeypatch.setattr(
        llvm_toolchain, "llvm_sdk_probe_environment", ProcessEnvironment
    )
    prefix, system = tmp_path / "sdk", tmp_path / "system libraries"
    for path, content in (
        (prefix / "bin/llvm-config", b"configuration"),
        (prefix / "bin/clang++", b"selected driver"),
        (prefix / "lib/libLLVMCore.a", b"LLVM archive"),
        (system / "libcompression.so", b"shared compression"),
        (system / "libcompression.a", b"static compression"),
        (system / "libm.so", b"system math"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        path.chmod(0o755)
    driver = prefix / "bin/clang++"
    verification = llvm_toolchain.LlvmPrefixVerification(
        prefix=prefix,
        llvm_config=prefix / "bin/llvm-config",
        version="22.1.8",
        host_target="aarch64-unknown-linux-gnu",
        targets=("AArch64", "WebAssembly"),
        assets=(),
        tool_versions=(
            llvm_toolchain.LlvmToolVersionFact(
                "clang++",
                "bin/clang++",
                "22.1.8",
                driver.stat().st_size,
                hashlib.sha256(driver.read_bytes()).hexdigest(),
            ),
        ),
        library_facts=(),
        content_facts=(),
        content_digest=None,
        link_closure=("lib/libLLVMCore.a", "system:-lcompression", "system:-lm"),
        link_probe=(),
        release=None,
    )
    monkeypatch.setattr(
        llvm_link_plan, "_driver_library_directories", lambda *_args: (system,)
    )
    capability = (
        {
            "tool": "auditwheel",
            "version": "6.8.2",
            "platform": "linux",
            "arch": "aarch64",
            "policy_name": "manylinux_2_39_aarch64",
            "policy_sha256": "a" * 64,
            "shared_libraries": ("libm.so.6",),
        },
        frozenset({"libm.so.6"}),
    )
    return llvm_link_plan, verification, system, capability


def native_sdk_projection_fixture(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    source_root: Path,
    llvm_config: Path | None = None,
):
    from molt import llvm_toolchain

    # Reuse the owning link-selector fixture's captured driver bytes and real
    # archive/library paths. Only the compiler's external search query is mocked;
    # native link selection, image-generation checks and serialization stay real.
    _selector, selected, system, _capability = native_link_selection_fixture(
        tmp_path, monkeypatch
    )
    (system / "libstdc++.a").write_bytes(b"selected C++ ABI archive")
    pin = llvm_toolchain.required_llvm_backend_pin(source_root)
    assert pin is not None
    config = selected.llvm_config if llvm_config is None else llvm_config
    if llvm_config is not None:
        config.parent.mkdir(parents=True)
        config.write_bytes(b"external split-layout configuration")
    return replace(
        selected,
        llvm_config=config,
        version=pin.default_release,
        tool_versions=tuple(
            replace(fact, version=pin.default_release)
            for fact in selected.tool_versions
        ),
    )

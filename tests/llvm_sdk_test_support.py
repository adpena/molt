"""Verified LLVM SDK tools for real-toolchain object-format proofs.

Tests that compile, archive, read or link real objects take their tools from
the verified LLVM SDK (`molt.llvm_toolchain`), never from ambient PATH guesses.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
import functools
import os
from pathlib import Path
import platform

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

    def native_target(self, object_format: str) -> str:
        return f"{self.architecture}-{_OBJECT_FORMAT_TRIPLE_SUFFIXES[object_format]}"

    @property
    def macho_arch(self) -> str:
        return _MACHO_ARCH_NAMES[self.architecture]

    def linker(self, role: str) -> str:
        from molt.cli.llvm_wasi_tools import llvm_linker_candidates

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
    from molt.cli.llvm_wasi_tools import llvm_tool_candidates
    from molt.llvm_toolchain import (
        llvm_bootstrap_command,
        llvm_host_architecture,
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
    return VerifiedLlvmTools(bin_dir, architecture, clang, ar, nm)


def verified_llvm_tools() -> VerifiedLlvmTools:
    """Return the verified SDK family, or skip when no SDK is provisioned."""
    tools = _verified_llvm_tools()
    if isinstance(tools, str):
        pytest.skip(tools)
    return tools

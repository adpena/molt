from __future__ import annotations

import os
from pathlib import Path
from typing import Literal, TypeGuard


LlvmLinkerRole = Literal["wasm-ld", "ld.lld", "ld64.lld", "lld-link"]

_LINKER_ROLES = frozenset[LlvmLinkerRole]({"wasm-ld", "ld.lld", "ld64.lld", "lld-link"})


def lexical_executable_path(path: Path) -> Path:
    """Make an executable path absolute without dereferencing its entrypoint.

    LLVM installs each role-specific linker name as a symlink or hardlink to the
    generic ``lld`` driver on some platforms. The invoked basename selects the
    driver's emulation, so resolving that alias changes the executable contract.
    """

    return Path(os.path.abspath(os.fspath(path.expanduser())))


def executable_entrypoint_name(path: Path) -> str:
    name = os.fspath(path).replace("\\", "/").rsplit("/", 1)[-1].lower()
    return name.removesuffix(".exe")


def executable_selects_linker_role(path: Path, role: LlvmLinkerRole) -> bool:
    """Return whether the lexical executable name selects exactly ``role``."""

    return role in _LINKER_ROLES and executable_entrypoint_name(path) == role


def is_llvm_linker_role(value: str) -> TypeGuard[LlvmLinkerRole]:
    return value in _LINKER_ROLES


def llvm_linker_role_for_object_format(object_format: str) -> LlvmLinkerRole:
    normalized = object_format.strip().lower()
    if normalized == "elf":
        return "ld.lld"
    if normalized in {"macho", "mach-o"}:
        return "ld64.lld"
    if normalized == "coff":
        return "lld-link"
    raise ValueError(f"unsupported LLVM linker object format: {object_format!r}")


def host_llvm_linker_role(system: str) -> LlvmLinkerRole:
    normalized = system.strip().lower()
    if normalized == "windows":
        return "lld-link"
    if normalized == "darwin":
        return "ld64.lld"
    if normalized == "linux":
        return "ld.lld"
    raise ValueError(f"unsupported LLVM linker host platform: {system!r}")

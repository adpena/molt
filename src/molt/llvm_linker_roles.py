from __future__ import annotations

import os
import stat
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

    lexical = Path(os.path.abspath(os.fspath(path.expanduser())))
    if os.name == "nt" and lexical.name != lexical.name.lower():
        # PATHEXT and explicit selectors can spell .EXE in uppercase, while
        # build tools may dispatch by a case-sensitive driver basename. Only
        # canonicalize an alias that names the same lexical file: do not resolve
        # symlinks or change selection in case-sensitive Windows directories.
        canonical = lexical.with_name(lexical.name.lower())
        try:
            original = lexical.lstat()
            normalized = canonical.lstat()
        except FileNotFoundError:
            return lexical
        if (stat.S_ISREG(original.st_mode) or stat.S_ISLNK(original.st_mode)) and (
            os.path.samestat(original, normalized)
        ):
            return canonical
    return lexical


def executable_entrypoint_name(path: Path) -> str:
    name = os.fspath(path).replace("\\", "/").rsplit("/", 1)[-1].lower()
    return name.removesuffix(".exe")


def executable_selects_linker_role(path: Path, role: LlvmLinkerRole) -> bool:
    """Return whether the lexical executable name selects exactly ``role``."""

    return role in _LINKER_ROLES and executable_entrypoint_name(path) == role


def is_llvm_linker_role(value: str) -> TypeGuard[LlvmLinkerRole]:
    return value in _LINKER_ROLES


def host_llvm_linker_role(system: str) -> LlvmLinkerRole:
    normalized = system.strip().lower()
    if normalized == "windows":
        return "lld-link"
    if normalized == "darwin":
        return "ld64.lld"
    if normalized == "linux":
        return "ld.lld"
    raise ValueError(f"unsupported LLVM linker host platform: {system!r}")

from __future__ import annotations

import os
from pathlib import Path
from typing import Literal, TypeGuard


LlvmLinkerRole = Literal["wasm-ld", "ld.lld", "ld64.lld", "lld-link"]

_LINKER_ROLES = frozenset[LlvmLinkerRole]({"wasm-ld", "ld.lld", "ld64.lld", "lld-link"})


def lexical_path_identity(path: Path) -> Path:
    """Name an actual lexical entry without collapsing parent or alias traversal.

    Windows case equivalence is established by directory entries, not inode or
    content equivalence: distinct hardlink/symlink names keep distinct keys.
    No cached directory state outlives this lookup.
    """
    path = path.expanduser()
    lexical = path.absolute()
    if os.name != "nt":
        return lexical
    raw = os.fspath(path)
    if not raw.startswith("\\\\?\\"):
        raw = os.fspath(lexical)
    original = Path(raw)
    original_identity = None
    if raw.startswith("\\\\?\\"):
        # Verbatim paths disable Win32 parsing. Bind the original coordinate
        # before projecting its prefix, and never project names whose trailing
        # dot/space would select a different entry (even a same-inode alias).
        if any(part.endswith((".", " ")) for part in raw[4:].split("\\")):
            raise ValueError(
                "proof path custody does not support verbatim trailing-dot/space components"
            )
        if raw[4:8].casefold() == "unc\\":
            projected = Path("\\\\" + raw[8:])
        elif len(raw) >= 7 and raw[4].isalpha() and raw[5:7] == ":\\":
            projected = Path(raw[4:])
        else:
            raise ValueError("proof path custody requires a drive or UNC coordinate")
        if not projected.is_absolute():
            raise ValueError(
                "proof path custody requires an absolute prefix projection"
            )
        original_identity = original.lstat()
        lexical = projected
        if not os.path.samestat(original_identity, lexical.lstat()):
            raise ValueError("Windows prefix projection changed its entry")
    if not lexical.is_absolute():
        raise ValueError("proof path custody requires an absolute coordinate")
    anchor = Path(lexical.anchor)
    normalized_anchor = Path(lexical.anchor.lower())
    if not os.path.samestat(anchor.lstat(), normalized_anchor.lstat()):
        raise ValueError("Windows lexical root spelling changed its entry")
    current = normalized_anchor
    for component in lexical.parts[1:]:
        if component == "..":
            # Product execution retains the kernel traversal. Proof admission
            # separately refuses this unsupported watch coordinate.
            current /= component
            continue
        requested = current / component
        before = requested.lstat()
        selected = None
        ambiguous = False
        with os.scandir(current) as entries:
            for entry in entries:
                # DirEntry.stat omits inode/device identity on Windows. Fresh
                # path lstat retains both that identity and alias entry type.
                if entry.name == component:
                    if not os.path.samestat(before, Path(entry.path).lstat()):
                        raise ValueError("Windows lexical entry changed during lookup")
                    selected = entry.name
                    ambiguous = False
                    break
                if entry.name.casefold() == component.casefold() and os.path.samestat(
                    before, Path(entry.path).lstat()
                ):
                    ambiguous = selected is not None
                    selected = entry.name
        if selected is None or ambiguous:
            raise ValueError("Windows lexical entry spelling is absent or ambiguous")
        actual = current / selected
        if not os.path.samestat(before, requested.lstat()) or not os.path.samestat(
            before, actual.lstat()
        ):
            raise ValueError("Windows lexical entry changed during lookup")
        current = actual
    if original_identity is not None and (
        not os.path.samestat(original_identity, original.lstat())
        or not os.path.samestat(original_identity, current.lstat())
    ):
        raise ValueError("Windows verbatim entry changed during lookup")
    return current


def lexical_executable_path(path: Path) -> Path:
    """Make argv absolute while preserving the selected entrypoint and traversal."""
    path = path.expanduser()
    lexical = path.absolute()
    if (
        os.name == "nt"
        and ".." not in lexical.parts
        and lexical.name != lexical.name.lower()
    ):
        # Keep the existing driver spelling convention only when lowercasing
        # names this same entry, not a separate same-inode hardlink or symlink.
        canonical = lexical.with_name(lexical.name.lower())
        try:
            if str(lexical_path_identity(lexical)) == str(
                lexical_path_identity(canonical)
            ):
                return canonical
        except (OSError, ValueError):
            # Optional product spelling only: valid short-name coordinates or
            # inaccessible/changed lookup evidence must not rewrite argv or
            # turn a compiler path into a proof-custody admission decision.
            pass
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

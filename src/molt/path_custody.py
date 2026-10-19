"""Host-neutral canonical path and containment checks.

Foreign path syntax can be compared without constructing a concrete host Path.
Actual source/custody roles belong to the verified CheckoutCustody owner.
"""

from __future__ import annotations

import ntpath
import os
from pathlib import Path, PurePath, PurePosixPath, PureWindowsPath
from typing import TypeAlias


PathInput: TypeAlias = str | os.PathLike[str]


class PathCustodyError(ValueError):
    pass


def _text(raw: PathInput) -> str:
    return os.fspath(raw).strip()


def _looks_windows_absolute(raw: PathInput) -> bool:
    rendered = _text(raw)
    path = PureWindowsPath(rendered)
    return bool(path.drive and path.root)


def pure_path(raw: PathInput) -> PurePath:
    """Parse foreign paths without binding them to the review host's OS."""

    rendered = _text(raw)
    if _looks_windows_absolute(rendered):
        return PureWindowsPath(rendered)
    return PurePosixPath(rendered)


def _pure_key(path: PurePath) -> tuple[str, ...]:
    parts = path.parts
    if isinstance(path, PureWindowsPath):
        return PureWindowsPath(_windows_path_key(str(path))).parts
    return parts


def _windows_path_key(path: str) -> str:
    key = ntpath.normcase(path)
    # Compare in the verbatim namespace, retaining unusual but real names
    # (for example trailing dots). Stripping a prefix and opening that result
    # could select a different Win32 file. These keys are never used for I/O.
    if key.startswith(("\\\\?\\", "\\\\.\\")):
        return key
    if key.startswith("\\\\"):
        return "\\\\?\\unc\\" + key[2:]
    drive, tail = ntpath.splitdrive(key)
    if len(drive) == 2 and drive[1] == ":" and tail.startswith("\\"):
        return "\\\\?\\" + key
    return key


def resolved_host_path_key(path: PathInput) -> str:
    """Compare already-resolved paths without another filesystem traversal.

    Windows DOS/UNC and verbatim spellings share a key. Callers retain the
    original path for I/O and establish realpath/symlink semantics themselves.
    """
    rendered = os.fspath(path)
    return _windows_path_key(rendered) if os.name == "nt" else rendered


def pure_path_is_within(path: PathInput, parent: PathInput) -> bool:
    """Lexically compare absolute paths using the paths' own syntax."""

    child = pure_path(path)
    root = pure_path(parent)
    if (
        type(child) is not type(root)
        or not child.is_absolute()
        or not root.is_absolute()
    ):
        return False
    child_key = _pure_key(child)
    root_key = _pure_key(root)
    return len(child_key) >= len(root_key) and child_key[: len(root_key)] == root_key


def same_host_path(left: PathInput, right: PathInput) -> bool:
    """Compare real host paths, retaining foreign-path safety for simulations."""

    if os.name != "nt" and (
        _looks_windows_absolute(left) or _looks_windows_absolute(right)
    ):
        left_path = pure_path(left)
        right_path = pure_path(right)
        return type(left_path) is type(right_path) and _pure_key(
            left_path
        ) == _pure_key(right_path)
    return resolved_host_path_key(
        Path(left).expanduser().resolve(strict=False)
    ) == resolved_host_path_key(Path(right).expanduser().resolve(strict=False))


def host_path_is_within(path: PathInput, parent: PathInput) -> bool:
    """Symlink-aware containment for host paths; lexical for foreign Windows."""

    if os.name != "nt" and (
        _looks_windows_absolute(path) or _looks_windows_absolute(parent)
    ):
        return pure_path_is_within(path, parent)
    child = resolved_host_path_key(Path(path).resolve(strict=False))
    root = resolved_host_path_key(Path(parent).resolve(strict=False))
    try:
        return os.path.commonpath((child, root)) == root
    except ValueError:
        return False


def canonical_host_path(
    raw: PathInput,
    *,
    authority: str,
    require_exists: bool = False,
) -> Path:
    """Return one absolute, resolved host spelling or reject path aliases.

    Drive letters and directory names do not establish custody. The lexical
    absolute spelling must equal the filesystem's
    resolved spelling.  This rejects ``..`` traversal, symlinks, and Windows
    junction aliases instead of silently promoting them into custody.
    """

    expanded = Path(raw).expanduser()
    if not expanded.is_absolute():
        raise PathCustodyError(f"{authority} must be absolute: {raw}")
    if ".." in expanded.parts:
        raise PathCustodyError(f"{authority} cannot contain '..' aliases: {raw}")
    lexical = Path(os.path.abspath(expanded))
    try:
        resolved = expanded.resolve(strict=require_exists)
    except OSError as exc:
        raise PathCustodyError(f"{authority} cannot be resolved: {raw}: {exc}") from exc
    if os.path.normcase(os.fspath(lexical)) != os.path.normcase(os.fspath(resolved)):
        raise PathCustodyError(
            f"{authority} must use its canonical filesystem spelling: "
            f"{raw} resolves to {resolved}"
        )
    return resolved

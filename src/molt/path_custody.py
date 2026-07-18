"""Host-neutral path roles for Molt source, custody, and scratch locations.

Path syntax and custody policy are deliberately separate.  A Windows path can
be inspected on a POSIX review host without constructing ``WindowsPath`` (which
is not supported there), and hosted-runner paths are validated under their
ephemeral role instead of being mistaken for durable Molt authority.
"""

from __future__ import annotations

from enum import Enum
import ntpath
import os
from pathlib import Path, PurePath, PurePosixPath, PureWindowsPath
from typing import TypeAlias


PathInput: TypeAlias = str | os.PathLike[str]


class CustodyPathRole(str, Enum):
    DURABLE_AUTHORITY = "durable-authority"
    HOSTED_SOURCE = "hosted-source"
    HOSTED_EXECUTION = "hosted-execution"
    EXPLICIT_SCRATCH = "explicit-scratch"


class PathCustodyError(ValueError):
    pass


# D: is poison for durable Molt authority. Hosted Windows runners use D:\a for
# source and per-run temp storage; those paths are accepted only because their
# role is explicitly non-durable, never because the drive was allowlisted.
_FORBIDDEN_DURABLE_WINDOWS_DRIVES = frozenset({"D:"})


def _text(raw: PathInput) -> str:
    return os.fspath(raw).strip()


def windows_drive(raw: PathInput) -> str:
    """Return a lexical Windows drive on every host, without concrete Paths."""

    rendered = _text(raw).replace("/", "\\")
    # pathlib retains Win32 device prefixes in ``drive`` (``\\?\D:`` and
    # ``\\.\D:``), while the NT object-manager spelling ``\??\D:`` is parsed
    # as merely rooted. Normalize these namespaces before classification.
    for prefix in ("\\\\?\\", "\\\\.\\", "\\??\\", "\\\\??\\"):
        if rendered.startswith(prefix):
            rendered = rendered[len(prefix) :]
            break
    if len(rendered) >= 2 and rendered[0].isalpha() and rendered[1] == ":":
        return rendered[:2].upper()
    return PureWindowsPath(rendered).drive.upper()


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
        return tuple(ntpath.normcase(part) for part in parts)
    return parts


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
    return Path(left).expanduser().resolve(strict=False) == Path(
        right
    ).expanduser().resolve(strict=False)


def host_path_is_within(path: PathInput, parent: PathInput) -> bool:
    """Symlink-aware containment for host paths; lexical for foreign Windows."""

    if os.name != "nt" and (
        _looks_windows_absolute(path) or _looks_windows_absolute(parent)
    ):
        return pure_path_is_within(path, parent)
    child = os.path.normcase(str(Path(path).resolve(strict=False)))
    root = os.path.normcase(str(Path(parent).resolve(strict=False)))
    try:
        return os.path.commonpath((child, root)) == root
    except ValueError:
        return False


def forbidden_for_role(raw: PathInput, role: CustodyPathRole) -> bool:
    """Return whether ``raw`` violates the named custody role."""

    if role is not CustodyPathRole.DURABLE_AUTHORITY:
        return False
    return windows_drive(raw) in _FORBIDDEN_DURABLE_WINDOWS_DRIVES


def validate_path_role(
    raw: PathInput, role: CustodyPathRole, *, authority: str
) -> None:
    if forbidden_for_role(raw, role):
        raise PathCustodyError(
            f"{authority} cannot use forbidden D: durable authority: {raw}"
        )

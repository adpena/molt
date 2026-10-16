"""Canonical Git identities and source inventories with caller-owned execution."""

from __future__ import annotations

from collections.abc import Callable
import os
from pathlib import Path
from typing import TypeGuard


_HEX = frozenset("0123456789abcdef")
GIT_OBJECT_ID_LENGTHS = frozenset({40, 64})


def repository_source_paths(
    cwd: Path,
    *,
    run_git: Callable[[Path, list[str]], bytes],
    include_untracked: bool = False,
) -> list[Path]:
    """List Git-admitted source, retaining missing paths for the caller to handle.

    The checked reader owns command admission and environment. Git owns ignored
    and nested-repository boundaries; NUL framing preserves literal filenames.
    This inventory does not assert file existence or byte identity.
    """
    options = ["--others", "--exclude-standard"] if include_untracked else []
    output = run_git(cwd, ["ls-files", "--cached", *options, "--full-name", "-z"])
    root_output = run_git(cwd, ["rev-parse", "--show-toplevel"])
    if not root_output.endswith(b"\n"):
        raise ValueError("Git source root is not newline terminated")
    # Match host text newline translation without stripping legal POSIX names.
    root_bytes = root_output[:-1]
    if os.name == "nt" and root_bytes.endswith(b"\r"):
        root_bytes = root_bytes[:-1]
    root = Path(os.fsdecode(root_bytes))
    if not root.is_absolute():
        raise ValueError("Git source root is not absolute")
    root = root.resolve(strict=True)
    if output and not output.endswith(b"\0"):
        raise ValueError("Git source inventory is not NUL terminated")
    paths: set[Path] = set()
    for raw in output.split(b"\0"):
        if not raw:
            continue
        relative = Path(os.fsdecode(raw))
        candidate = Path(os.path.abspath(root / relative))
        if relative.is_absolute() or not candidate.is_relative_to(root):
            raise ValueError("Git source input escaped its root")
        paths.add(candidate)
    return sorted(paths, key=lambda path: path.as_posix())


def clean_checkout_status_arguments(
    *,
    porcelain_version: str = "v1",
    null_terminated: bool = False,
    untracked_files: str = "all",
) -> tuple[str, ...]:
    """Return config-independent arguments for an exact dirty-tree decision."""

    if porcelain_version not in {"v1", "v2"}:
        raise ValueError(f"unsupported Git porcelain version: {porcelain_version}")
    if untracked_files not in {"all", "no"}:
        raise ValueError(f"unsupported Git untracked-file policy: {untracked_files}")
    return (
        "--no-optional-locks",
        "status",
        f"--porcelain={porcelain_version}",
        *(("-z",) if null_terminated else ()),
        f"--untracked-files={untracked_files}",
        "--ignore-submodules=none",
    )


def is_git_object_id(value: object) -> TypeGuard[str]:
    """Return whether *value* is one canonical lowercase Git object ID."""

    return (
        isinstance(value, str)
        and len(value) in GIT_OBJECT_ID_LENGTHS
        and all(character in _HEX for character in value)
    )


def require_git_object_id(value: object, *, label: str = "Git object ID") -> str:
    if not is_git_object_id(value):
        raise ValueError(f"{label} must be lowercase 40- or 64-hex")
    return value


def changed_paths(
    base: str,
    head: str,
    *,
    run_git: Callable[[list[str]], str],
    three_dot: bool = False,
) -> list[str]:
    """Return every changed path, including both sides of renames and copies.

    The caller supplies its repository-bound Git reader and source identities.
    NUL framing preserves names that Git would otherwise quote or escape.
    """
    if not base or not head or set(base) == {"0"} or set(head) == {"0"}:
        raise RuntimeError("event does not provide two non-null commit identities")
    separator = "..." if three_dot else ".."
    output = run_git(
        [
            "diff",
            "--name-status",
            "-z",
            "--diff-filter=ACDMRTUXB",
            f"{base}{separator}{head}",
        ]
    )
    tokens = output.split("\0")
    paths: list[str] = []
    seen: set[str] = set()
    index = 0
    while index < len(tokens):
        status = tokens[index]
        index += 1
        if not status:
            continue
        path_count = 2 if status[0] in {"R", "C"} else 1
        if index + path_count > len(tokens):
            raise RuntimeError(f"malformed git diff record for status {status!r}")
        for path in tokens[index : index + path_count]:
            if path and path not in seen:
                seen.add(path)
                paths.append(path)
        index += path_count
    return paths

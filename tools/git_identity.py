"""Canonical Git object-ID grammar for SHA-1 and SHA-256 repositories."""

from __future__ import annotations


_HEX = frozenset("0123456789abcdef")
GIT_OBJECT_ID_LENGTHS = frozenset({40, 64})


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


def is_git_object_id(value: object) -> bool:
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

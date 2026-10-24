"""Canonical Git identities and source inventories with caller-owned execution."""

from __future__ import annotations

from collections.abc import Callable, Iterator, Mapping, Sequence
import hashlib
import os
from pathlib import Path
from typing import BinaryIO, TypeGuard

from molt.process_environment import ProcessEnvironmentBuilder


_HEX = frozenset("0123456789abcdef")
GIT_OBJECT_ID_LENGTHS = frozenset({40, 64})


def immutable_git_environment(
    environment: Mapping[str, str],
) -> ProcessEnvironmentBuilder:
    """Read original Git objects and ancestry, without replacement refs or grafts."""
    env = ProcessEnvironmentBuilder(environment)
    env["GIT_NO_REPLACE_OBJECTS"] = "1"
    env["GIT_GRAFT_FILE"] = os.devnull
    return env


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


def branch_changed_paths(
    base_ref: str, *, run_git: Callable[[list[str]], str]
) -> list[str]:
    """Return every path the checked-out branch changes against *base_ref*.

    That is the committed changes since the merge base with *base_ref*, plus
    staged, unstaged and untracked paths. *base_ref* is the integration
    target, never the branch's upstream: a pushed branch's upstream is its
    own tip, so a diff against it is empty.
    """
    merge_base = run_git(["merge-base", base_ref, "HEAD"]).strip()
    head = run_git(["rev-parse", "HEAD"]).strip()
    paths = dict.fromkeys(changed_paths(merge_base, head, run_git=run_git))
    for arguments in (
        ["diff", "--name-only", "-z", "HEAD"],
        ["ls-files", "--others", "--exclude-standard", "-z"],
    ):
        paths.update(
            dict.fromkeys(path for path in run_git(arguments).split("\0") if path)
        )
    return sorted(paths)


def read_git_blob_batch(
    output: BinaryIO, objects: Sequence[tuple[str, int]]
) -> Iterator[tuple[bytes, str]]:
    """Yield verified blob bytes and SHA256 from Git's default --batch framing.

    The caller owns execution and must exhaust this iterator to validate EOF.
    Only one blob is retained by the reader; release consumers may stream it
    directly to their unpublished destination. Protocol: git-scm.com/docs/git-cat-file.
    """
    for blob_oid, declared_size in objects:
        require_git_object_id(blob_oid)
        if type(declared_size) is not int or declared_size < 0:
            raise ValueError("Git source snapshot blob size is invalid")
        expected_header = f"{blob_oid} blob {declared_size}\n".encode("ascii")
        if output.readline(len(expected_header) + 1) != expected_header:
            raise ValueError(f"Git source snapshot blob header is invalid: {blob_oid}")
        data = output.read(declared_size)
        if len(data) != declared_size or output.read(1) != b"\n":
            raise ValueError(f"Git source snapshot blob framing is invalid: {blob_oid}")
        object_hash = hashlib.new({40: "sha1", 64: "sha256"}[len(blob_oid)])
        object_hash.update(f"blob {declared_size}\0".encode("ascii"))
        object_hash.update(data)
        if object_hash.hexdigest() != blob_oid:
            raise ValueError(
                f"Git source snapshot blob content does not match its object ID: {blob_oid}"
            )
        yield data, hashlib.sha256(data).hexdigest()
    if output.read(1):
        raise ValueError("Git source snapshot has trailing blob data")

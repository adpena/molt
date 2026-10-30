"""Canonical source admission for checkout and archive identity consumers.

Git owns admission, including tracked dot files and unignored new sources.
Explicit file roots are declared build inputs. Outside Git, Cargo's dot-file
rule owns directory admission. Admission is separate from stable byte custody.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
import os
from pathlib import Path
import stat
from functools import lru_cache

from molt.file_publication import metadata_is_link_like
from molt.process_guard import run_completed_command


def _version(path: Path) -> tuple[int, ...] | None:
    try:
        value = path.lstat()
    except FileNotFoundError:
        return None
    return (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def _git_generation(checkout: Path) -> tuple[bytes, tuple[int, ...] | None]:
    git = checkout / ".git"
    if git.is_file():
        text = git.read_text(encoding="utf-8").removesuffix("\n")
        if not text.startswith("gitdir: "):
            raise ValueError("invalid Git worktree metadata")
        git = (checkout / text.removeprefix("gitdir: ")).resolve()
    head = (git / "HEAD").read_bytes()
    if head.startswith(b"ref: "):
        common_file = git / "commondir"
        common = (
            (git / common_file.read_text(encoding="utf-8").removesuffix("\n")).resolve()
            if common_file.is_file()
            else git
        )
        ref = common / os.fsdecode(head.removeprefix(b"ref: ").removesuffix(b"\n"))
        head += (
            ref.read_bytes()
            if ref.exists()
            else (common / "packed-refs").read_bytes()
            if (common / "packed-refs").exists()
            else b""
        )
    return head, _version(git / "index")


@lru_cache(maxsize=32)
def _inventory_paths(
    root: Path, output: bytes, generation: tuple[object, ...]
) -> tuple[Path, ...]:
    # HEAD/index scope the memo; the current Git result is also essential.
    # Untracked additions and ignore edits do not necessarily change either.
    # One live ls-files query replaces recursive scandir/lstat on every capture.
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
    return tuple(sorted(paths, key=lambda path: path.as_posix()))


def repository_source_paths(
    cwd: Path,
    *,
    run_git: Callable[[Path, list[str]], bytes],
    include_untracked: bool = False,
    source_root: Path | None = None,
    roots: Sequence[Path] = (),
) -> list[Path]:
    """List Git-admitted source, retaining missing paths for the caller to handle.

    The checked reader owns command admission and environment. Git owns ignored
    and nested-repository boundaries; NUL framing preserves literal filenames.
    This inventory does not assert file existence or byte identity.
    """
    options = ["--others", "--exclude-standard"] if include_untracked else []
    arguments = ["ls-files", "--cached", *options, "--full-name", "-z"]
    if roots:
        arguments += [
            "--",
            *(f":(top,literal){path.relative_to(cwd).as_posix()}" for path in roots),
        ]
    output = run_git(cwd, arguments)
    root_output = (
        os.fsencode(source_root) + b"\n"
        if source_root is not None
        else run_git(cwd, ["rev-parse", "--show-toplevel"])
    )
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
    generation = _git_generation(root) if source_root is not None else ()
    return list(_inventory_paths(root, output, generation))


def _checkout_root(path: Path) -> Path | None:
    for parent in (path, *path.parents):
        if (parent / ".git").exists():
            return parent
    return None


def _read_git(root: Path, arguments: list[str]) -> bytes:
    result = run_completed_command(
        ["git", "--no-optional-locks", "-C", str(root), *arguments],
        cwd=root,
        capture_output=True,
        text=False,
        memory_guard_prefix=None,
    )
    if result.returncode:
        raise OSError(f"Git source admission failed: {os.fsdecode(result.stderr)}")
    return result.stdout


def source_directories(
    files: Sequence[Path], roots: Sequence[Path]
) -> tuple[Path, ...]:
    """Directory topology derives from admitted files, never empty debris."""
    directories: set[Path] = set()
    root_set = frozenset(roots)
    root_set = frozenset(
        root
        for root in root_set
        if not any(parent in root_set for parent in root.parents)
    )
    for path in files:
        parent = path.parent
        pending: list[Path] = []
        while parent not in directories:
            pending.append(parent)
            if parent in root_set:
                directories.update(pending)
                break
            next_parent = parent.parent
            if next_parent == parent:
                break
            parent = next_parent
        else:
            directories.update(pending)
    return tuple(sorted(directories))


def _archive_files(root: Path) -> list[Path]:
    """Cargo's source-archive dot-file rule; no growing debris-name list."""
    files: list[Path] = []

    def scan_error(error: OSError) -> None:
        raise error

    for directory, children, names in os.walk(
        root, followlinks=False, onerror=scan_error
    ):
        children[:] = sorted(name for name in children if not name.startswith("."))
        current = Path(directory)
        for name in children:
            if metadata_is_link_like((current / name).lstat()):
                raise ValueError(
                    f"source input path alias escaped logical root: {current / name}"
                )
        files.extend(current / name for name in names if not name.startswith("."))
    return files


def admitted_source_files(
    roots: Sequence[Path], *, follow_file_symlinks: bool = False
) -> tuple[Path, ...]:
    """Capture the admitted union with one Git query per source checkout.

    A file root is an explicit declaration, even when ignored. A directory
    declaration never admits ignored children; Cargo dep-info refuses such
    undeclared reads at publication. Missing tracked files are not bytes.
    Python producers follow file symlinks while retaining the import-visible
    path. Direct-file custody consumers keep refusing aliases. Neither policy
    traverses directory aliases into another source owner.
    """
    files: set[Path] = set()
    regular: set[Path] = set()
    checkouts: dict[Path, list[Path]] = {}
    selected = frozenset(Path(os.path.abspath(path)) for path in roots)
    for path in sorted(selected):
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            continue
        if metadata_is_link_like(metadata):
            if not follow_file_symlinks or not stat.S_ISREG(path.stat().st_mode):
                raise ValueError(f"source input root alias is forbidden: {path}")
            files.add(path)
            regular.add(path)
            continue
        if stat.S_ISREG(metadata.st_mode):
            files.add(path)
            regular.add(path)
        elif stat.S_ISDIR(metadata.st_mode):
            checkout = _checkout_root(path)
            if checkout is None:
                files.update(_archive_files(path))
            else:
                checkouts.setdefault(checkout, []).append(path)
    for checkout, directories in checkouts.items():
        files.update(
            repository_source_paths(
                checkout,
                run_git=_read_git,
                include_untracked=True,
                source_root=checkout,
                roots=directories,
            )
        )
    result: list[Path] = []
    checked: set[Path] = set(selected)
    for path in sorted(files):
        # Git lists a symlink as a blob of link text, not its target bytes.
        # A Python producer reads the target through the lexical file path;
        # directory aliases still cannot expand Git's admitted membership.
        parent = path
        while parent not in checked:
            try:
                metadata = parent.lstat()
            except FileNotFoundError:
                break
            if metadata_is_link_like(metadata):
                if (
                    not follow_file_symlinks
                    or parent != path
                    or not stat.S_ISREG(path.stat().st_mode)
                ):
                    raise ValueError(
                        f"source input path alias escaped logical root: {parent}"
                    )
                regular.add(path)
            checked.add(parent)
            if parent == path and stat.S_ISREG(metadata.st_mode):
                regular.add(path)
            parent = parent.parent
        if path in regular:
            result.append(path)
    return tuple(result)

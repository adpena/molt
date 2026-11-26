"""Shared lexical selection of live toolchain resources.

LLVM verification, Cargo and build records depend on the same selected file
entries and directory and alias observations. Content custody belongs to the
consumer of this live selection.
"""

from __future__ import annotations

import os
import stat
import time
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

from molt.file_hashing import DIGEST_RACY_WINDOW_NS
from molt.platform_toolchain import DARWIN_SDK_SETTINGS
from molt.toolchain_identity import executable_content_path


@dataclass(frozen=True, slots=True)
class CargoResourceEntry:
    """One selected file: its label, lexical entrypoint and resolved content."""

    label: str
    entrypoint: Path
    content: Path


_DirectoryToken = tuple[int, int, int, int]
_ResourceTarget = tuple[Path, int]


def _resource_alias_target(path: Path) -> _ResourceTarget | None:
    try:
        target = path.resolve(strict=True)
        return target, stat.S_IFMT(target.lstat().st_mode)
    except (FileNotFoundError, NotADirectoryError):
        return None


def _resolve_file_parent(path: Path, parents: dict[Path, Path]) -> Path:
    parent = path.parent
    resolved = parents.get(parent)
    if resolved is None:
        resolved = parent.resolve(strict=True)
        parents[parent] = resolved
    return resolved / path.name


def _directory_token(path: Path) -> _DirectoryToken:
    """Adding, removing or renaming an entry changes its directory's mtime."""
    metadata = path.stat()
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


@dataclass(frozen=True, slots=True)
class CargoResourceWalk:
    """One root's selected files, and every directory the walk listed."""

    entries: tuple[CargoResourceEntry, ...]
    resolved_root: Path | None
    directories: tuple[tuple[Path, _DirectoryToken], ...]
    # A directory changed within the racy window of the walk may change again
    # within one timestamp tick, so its token cannot prove an unchanged list.
    racy: bool = False
    # A single non-link POSIX file can share its parent's resolution with
    # sibling roots. Directory walks and leaf aliases keep their own fence.
    direct_file: bool = False
    # An alias may traverse directories outside the enumerated root. Its
    # selected target can change while every listed directory stays intact.
    aliases: tuple[tuple[Path, _ResourceTarget | None], ...] = ()


class CargoResourceIdentity(Enum):
    """What proves a resource root's content."""

    # Every file under the root.
    CONTENT = "content"
    # A toolchain header tree includes regular-file content through directory
    # aliases. Dangling aliases and special entries do not supply headers.
    REGULAR_TREE = "regular-tree"
    # An Apple SDK's settings, the identity `platform_toolchain.select_darwin_sdk`
    # and Clang use: Xcode and the Command Line Tools install each SDK version
    # whole, and Bazel's apple_support keys actions on that version too. The
    # alternative stats every file: 69,329 in the macOS 26.5 SDK.
    APPLE_SDK = "apple-sdk"


def is_apple_sdk(path: Path) -> bool:
    return (path / DARWIN_SDK_SETTINGS).is_file()


@dataclass(frozen=True, slots=True)
class CargoResourceRoot:
    label: str
    path: Path
    recursive: bool = True
    required: bool = True
    dynamic_libraries_only: bool = False
    identity: CargoResourceIdentity = CargoResourceIdentity.CONTENT

    def _walk_apple_sdk(self) -> CargoResourceWalk:
        started_ns = time.time_ns()
        resolved_root = self.path.resolve(strict=True)
        settings = resolved_root / DARWIN_SDK_SETTINGS
        if not settings.is_file():
            raise ValueError(f"Apple SDK has no {DARWIN_SDK_SETTINGS}: {self.path}")
        token = _directory_token(resolved_root)
        return CargoResourceWalk(
            (
                CargoResourceEntry(
                    f"{self.label}/{DARWIN_SDK_SETTINGS}",
                    self.path / DARWIN_SDK_SETTINGS,
                    settings.resolve(strict=True),
                ),
            ),
            resolved_root,
            ((resolved_root, token),),
            racy=max(token[2], token[3]) >= started_ns - DIGEST_RACY_WINDOW_NS,
        )

    def walk(
        self, *, resolved_parents: dict[Path, Path] | None = None
    ) -> CargoResourceWalk:
        """Capture lexical file edges without unfolding directory backedges.

        Each directory is resolved once; a file reached through real
        directories resolves to its resolved directory plus its name, so only
        symlinks need their own resolution.
        """
        if self.identity is CargoResourceIdentity.APPLE_SDK:
            return self._walk_apple_sdk()
        try:
            metadata = self.path.lstat()
        except FileNotFoundError:
            if self.required:
                raise ValueError(
                    f"required runtime resource is missing: {self.path}"
                ) from None
            return CargoResourceWalk((), None, ())
        direct_file = os.name != "nt" and stat.S_ISREG(metadata.st_mode)
        if direct_file or stat.S_ISREG(metadata.st_mode) or self.path.is_file():
            content = (
                _resolve_file_parent(self.path, resolved_parents)
                if direct_file and resolved_parents is not None
                else executable_content_path(self.path, label=self.label)
            )
            return CargoResourceWalk(
                (CargoResourceEntry(self.label, self.path, content),),
                content,
                (),
                direct_file=direct_file,
            )
        if not self.path.is_dir():
            raise ValueError(f"runtime resource root is not a directory: {self.path}")
        result: list[CargoResourceEntry] = []
        directories: list[tuple[Path, _DirectoryToken]] = []
        aliases: list[tuple[Path, _ResourceTarget | None]] = []
        regular_tree = self.identity is CargoResourceIdentity.REGULAR_TREE

        def visit(
            directory: Path,
            resolved: Path,
            relative: str,
            ancestors: frozenset[Path],
        ) -> None:
            if resolved in ancestors:
                return
            ancestors = ancestors | {resolved}
            directories.append((resolved, _directory_token(resolved)))
            with os.scandir(directory) as iterator:
                entries = sorted(
                    iterator, key=lambda entry: os.path.normcase(entry.name)
                )
            for entry in entries:
                name = entry.name
                if self.dynamic_libraries_only and not (
                    Path(name).suffix.casefold() in {".dll", ".dylib", ".so"}
                    or ".so." in name
                ):
                    continue
                lexical = directory / name
                if entry.is_symlink() or entry.is_junction():
                    alias = _resource_alias_target(lexical)
                    aliases.append((lexical, alias))
                    if alias is None:
                        if regular_tree:
                            continue
                        raise ValueError(
                            f"runtime resource is not a regular file: {lexical}"
                        )
                    target, kind = alias
                    is_file = kind == stat.S_IFREG
                    is_dir = kind == stat.S_IFDIR
                else:
                    target = resolved / name
                    is_file = entry.is_file(follow_symlinks=False)
                    is_dir = not is_file and entry.is_dir(follow_symlinks=False)
                if is_file:
                    result.append(
                        CargoResourceEntry(
                            self.label + "/" + relative + name, lexical, target
                        )
                    )
                elif is_dir:
                    if self.recursive:
                        visit(lexical, target, relative + name + "/", ancestors)
                elif not regular_tree:
                    raise ValueError(
                        f"runtime resource is not a regular file: {lexical}"
                    )

        started_ns = time.time_ns()
        resolved_root = self.path.resolve(strict=True)
        visit(self.path, resolved_root, "", frozenset())
        return CargoResourceWalk(
            tuple(result),
            resolved_root,
            tuple(directories),
            racy=any(
                max(token[2], token[3]) >= started_ns - DIGEST_RACY_WINDOW_NS
                for _path, token in directories
            ),
            aliases=tuple(aliases),
        )


# This process's walks by root. Root sets overlap (one SDK search root appears
# in several Cargo plans), so a capture of a new set reuses an unchanged walk.
_WALKED_ROOTS: dict[CargoResourceRoot, CargoResourceWalk] = {}


def resource_walk_unchanged(
    root: CargoResourceRoot,
    walk: CargoResourceWalk,
    resolved_parents: dict[Path, Path],
    *,
    verify_files: bool = False,
) -> bool:
    """Unchanged directory tokens prove an unchanged listing on POSIX, as git's
    untracked cache does; a racy walk or Windows always walks again."""
    if os.name == "nt" or walk.racy:
        return False
    if walk.resolved_root is None:
        return not (root.path.exists() or root.path.is_symlink())
    try:
        if walk.direct_file:
            content = _resolve_file_parent(root.path, resolved_parents)
            # verify() checks leaf generations immediately after selection.
            # A cached walk reused for capture has no prior leaf proof to check.
            return content == walk.resolved_root and (
                verify_files or stat.S_ISREG(content.lstat().st_mode)
            )
        if root.path.resolve(strict=True) != walk.resolved_root:
            return False
        return all(
            _directory_token(path) == token for path, token in walk.directories
        ) and all(
            _resource_alias_target(path) == target for path, target in walk.aliases
        )
    except OSError:
        return False


def _current_walk(
    root: CargoResourceRoot, resolved_parents: dict[Path, Path]
) -> CargoResourceWalk:
    walk = _WALKED_ROOTS.get(root)
    if walk is None or not resource_walk_unchanged(root, walk, resolved_parents):
        walk = root.walk(resolved_parents=resolved_parents)
        _WALKED_ROOTS[root] = walk
    return walk


def observe_cargo_resource_roots(
    roots: tuple[CargoResourceRoot, ...],
) -> tuple[CargoResourceWalk, ...]:
    """Select live paths without hashing the file bodies."""
    resolved_parents: dict[Path, Path] = {}
    return tuple(_current_walk(root, resolved_parents) for root in roots)

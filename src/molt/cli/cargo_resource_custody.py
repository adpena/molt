"""Live custody of the files a Cargo build reads outside its own sources.

A runtime or backend Cargo plan names its toolchain and C resources by root.
This module captures each root's selected files, proves them current at every
fence of a build, and reports them to a build record. It is live host state,
never part of a portable receipt.
"""

from __future__ import annotations

import os
import stat
import time
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path

from molt import input_report as _input_report
from molt.cache_eviction import resolve_budget
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.cli.wasm_link_args import runtime_link_response_arguments
from molt.default_paths import _default_molt_cache, file_digest_cache_path
from molt.exact_json import canonical_json_sha256
from molt.file_hashing import DIGEST_RACY_WINDOW_NS
from molt.platform_toolchain import DARWIN_SDK_SETTINGS
from molt.toolchain_identity import (
    ExecutableIdentity,
    StableRegularFileIdentity,
    capture_stable_regular_file,
    executable_content_path,
    file_digest_cache,
    stable_native_executable_probe,
    verify_stable_regular_file_identity,
    verify_stable_regular_file_metadata,
)


@dataclass(frozen=True, slots=True)
class CargoFileCustody:
    """Live lexical selection plus direct content generation, never portable."""

    label: str
    entrypoint: Path
    identity: StableRegularFileIdentity

    def verify(self) -> None:
        if (
            executable_content_path(self.entrypoint, label=self.label)
            != self.identity.path
        ):
            raise ValueError(f"{self.label} selected content changed")
        verify_stable_regular_file_identity(self.identity, label=self.label)

    def content_record(self) -> dict[str, str | int]:
        return ExecutableIdentity(
            self.entrypoint,
            self.identity.path,
            self.identity.size,
            self.identity.sha256,
            "",
        ).content_record()

    def _report_to_build_record(self) -> None:
        if not _input_report.active():
            return
        token = self.identity.stat_token()
        _input_report.report_captured_file(self.entrypoint, token, self.identity.sha256)
        if self.identity.path != self.entrypoint:
            _input_report.report_probe(self.entrypoint)
            _input_report.report_captured_file(
                self.identity.path, token, self.identity.sha256
            )


@dataclass(frozen=True, slots=True)
class CargoExecutableCustody(CargoFileCustody):
    """Native-tool admission over the shared lexical/content generation fence."""

    @classmethod
    def capture(cls, label: str, path: Path) -> CargoExecutableCustody:
        with stable_native_executable_probe(path, label=label) as (
            entrypoint,
            identity,
        ):
            result = cls(label, entrypoint, identity)
            result._report_to_build_record()
            return result


@dataclass(frozen=True, slots=True)
class CargoResourceEntry:
    """One selected file: its label, lexical entrypoint and resolved content."""

    label: str
    entrypoint: Path
    content: Path


_DirectoryToken = tuple[int, int, int, int]


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


class CargoResourceIdentity(Enum):
    """What proves a resource root's content."""

    # Every file under the root.
    CONTENT = "content"
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

    def walk(self) -> CargoResourceWalk:
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
        if stat.S_ISREG(metadata.st_mode) or self.path.is_file():
            content = executable_content_path(self.path, label=self.label)
            return CargoResourceWalk(
                (CargoResourceEntry(self.label, self.path, content),), content, ()
            )
        if not self.path.is_dir():
            raise ValueError(f"runtime resource root is not a directory: {self.path}")
        result: list[CargoResourceEntry] = []
        directories: list[tuple[Path, _DirectoryToken]] = []

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
                    try:
                        target = lexical.resolve(strict=True)
                    except OSError:
                        raise ValueError(
                            f"runtime resource is not a regular file: {lexical}"
                        ) from None
                    is_file = target.is_file() and not target.is_symlink()
                    is_dir = not is_file and target.is_dir()
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
                else:
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
        )


# This process's captures by root set; see CargoResourceCustody.capture.
_CAPTURED_RESOURCES: dict[tuple[CargoResourceRoot, ...], CargoResourceCustody] = {}
# This process's walks by root. Root sets overlap (one SDK search root appears
# in several Cargo plans), so a capture of a new set reuses an unchanged walk.
_WALKED_ROOTS: dict[CargoResourceRoot, CargoResourceWalk] = {}


def _walk_unchanged(root: CargoResourceRoot, walk: CargoResourceWalk) -> bool:
    """Unchanged directory tokens prove an unchanged listing on POSIX, as git's
    untracked cache does; a racy walk or Windows always walks again."""
    if os.name == "nt" or walk.racy:
        return False
    if walk.resolved_root is None:
        return not (root.path.exists() or root.path.is_symlink())
    try:
        if root.path.resolve(strict=True) != walk.resolved_root:
            return False
        return all(_directory_token(path) == token for path, token in walk.directories)
    except OSError:
        return False


def _current_walk(root: CargoResourceRoot) -> CargoResourceWalk:
    walk = _WALKED_ROOTS.get(root)
    if walk is None or not _walk_unchanged(root, walk):
        walk = root.walk()
        _WALKED_ROOTS[root] = walk
    return walk


@dataclass(frozen=True, slots=True)
class CargoResourceCustody:
    roots: tuple[CargoResourceRoot, ...]
    files: tuple[CargoFileCustody, ...]
    walks: tuple[CargoResourceWalk, ...]
    # content_identity() of these immutable captures, computed once:
    # (digest, file count, total size, sorted root labels).
    _content: tuple[str, int, int, tuple[str, ...]] | None = field(
        default=None, init=False, compare=False, repr=False
    )

    @classmethod
    def capture(
        cls,
        roots: tuple[CargoResourceRoot, ...],
    ) -> CargoResourceCustody:
        """Capture these roots, or reuse this process's still-current capture.

        A build captures the same toolchain roots at several fences. A verified
        earlier capture is the same generation; one that no longer verifies is
        a new generation and is captured afresh.
        """
        cached = _CAPTURED_RESOURCES.get(roots)
        if cached is not None:
            try:
                cached.verify()
            except ValueError:
                _CAPTURED_RESOURCES.pop(roots, None)
            else:
                cached._report_to_build_record()
                return cached
        result = cls._capture(roots)
        _CAPTURED_RESOURCES[roots] = result
        result._report_to_build_record()
        return result

    def _report_to_build_record(self) -> None:
        """Report this capture's files and listings to an active build record.

        Each use is this build's input, but a walk shared by several captures
        reports once per recording, with each file's captured token.
        """
        if not _input_report.active():
            return
        start = 0
        for root, walk in zip(self.roots, self.walks, strict=True):
            files = self.files[start : start + len(walk.entries)]
            start += len(walk.entries)
            if not _input_report.first_report(walk):
                continue
            if walk.racy:
                _input_report.refuse(f"racy Cargo resource walk: {root.path}")
            if walk.resolved_root is None:
                _input_report.report_probe(root.path)
            for path, _token in walk.directories:
                _input_report.report_directory(path)
            for item in files:
                item._report_to_build_record()

    @classmethod
    def _capture(
        cls,
        roots: tuple[CargoResourceRoot, ...],
    ) -> CargoResourceCustody:
        snapshots: dict[Path, StableRegularFileIdentity] = {}
        files: list[CargoFileCustody] = []
        _max_gb, max_age_days = resolve_budget(None, None, os.environ)
        digests = file_digest_cache(
            file_digest_cache_path(_default_molt_cache()),
            max_age_ns=max_age_days * 86_400 * 1_000_000_000,
        )
        walks = tuple(_current_walk(root) for root in roots)
        selected = tuple(
            (entry.label, entry.entrypoint, entry.content)
            for walk in walks
            for entry in walk.entries
        )
        response_paths = {
            content_path
            for label, _path, content_path in selected
            if label.startswith("rust/link-response/")
        }
        for label, path, content_path in selected:
            prior = snapshots.get(content_path)
            if prior is not None:
                captured = CargoFileCustody(label, path, prior)
            elif content_path in response_paths:
                # Capture and parse the same bounded bytes, even if this file
                # first occurs under another resource label. Retain only its
                # identity; no response-content cache outlives this operation.
                identity, content = capture_stable_regular_file(
                    content_path,
                    label="runtime linker response",
                    max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
                )
                runtime_link_response_arguments(content)
                captured = CargoFileCustody(label, path, identity)
            else:
                # The walk proved the entrypoint selects this content; only its
                # identity is captured, through the host's digest cache.
                captured = CargoFileCustody(
                    label, path, digests.identity(content_path, label=label)
                )
            snapshots[captured.identity.path] = captured.identity
            files.append(captured)
        digests.flush()
        result = cls(roots, tuple(files), walks)
        result.verify()
        return result

    def verify(self) -> None:
        """Prove the captured selection and content are still current.

        On POSIX, unchanged directory tokens prove an unchanged selection and
        an equal stat token proves unchanged content, as git's index does.
        Otherwise the roots are walked and compared exactly.
        """
        if not self._selection_unchanged():
            selected = tuple(
                entry for root in self.roots for entry in root.walk().entries
            )
            if tuple((entry.label, entry.entrypoint) for entry in selected) != tuple(
                (item.label, item.entrypoint) for item in self.files
            ):
                raise ValueError("runtime Rust resource selection changed during build")
            for entry, item in zip(selected, self.files, strict=True):
                if entry.content != item.identity.path:
                    raise ValueError(f"{item.label} selected content changed")
        verified: set[Path] = set()
        for item in self.files:
            if item.identity.path in verified:
                continue
            if os.name == "nt":
                verify_stable_regular_file_identity(item.identity, label=item.label)
            else:
                verify_stable_regular_file_metadata(item.identity, label=item.label)
            verified.add(item.identity.path)

    def _selection_unchanged(self) -> bool:
        return all(
            _walk_unchanged(root, walk)
            for root, walk in zip(self.roots, self.walks, strict=True)
        )

    def file_identity(self, path: Path) -> StableRegularFileIdentity:
        """Project a captured lexical resource; never select or hash a new file."""
        matches = [item.identity for item in self.files if item.entrypoint == path]
        if not matches or any(identity != matches[0] for identity in matches):
            raise ValueError(
                f"runtime resource has no unique captured identity: {path}"
            )
        return matches[0]

    def content_identity(self) -> dict[str, object]:
        """Project captured bytes, without probing or reading a second generation."""
        content = self._content
        if content is None:
            records = [
                {
                    "label": item.label,
                    "size": item.identity.size,
                    "sha256": item.identity.sha256,
                }
                for item in self.files
            ]
            content = (
                canonical_json_sha256(records),
                len(records),
                sum(item.identity.size for item in self.files),
                tuple(sorted(root.label for root in self.roots)),
            )
            object.__setattr__(self, "_content", content)
        digest, file_count, total_size, roots = content
        return {
            "digest": digest,
            "file_count": file_count,
            "total_size": total_size,
            "roots": list(roots),
            "missing": [],
        }

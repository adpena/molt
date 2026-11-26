"""Live custody of the files a Cargo build reads outside its own sources.

A runtime or backend Cargo plan names its toolchain and C resources by root.
This module captures each root's selected files, proves them current at every
fence of a build, and reports them to a build record. It is live host state,
never part of a portable receipt.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from molt import input_report as _input_report
from molt.cargo_resource_selection import (
    CargoResourceRoot,
    CargoResourceWalk,
    observe_cargo_resource_roots,
    resource_walk_unchanged,
)
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.cli.wasm_link_args import runtime_link_response_arguments
from molt.exact_json import canonical_json_sha256
from molt.toolchain_facts import host_file_digest_cache
from molt.toolchain_identity import (
    ExecutableIdentity,
    StableRegularFileIdentity,
    capture_stable_regular_file,
    executable_content_path,
    stable_native_executable_probe,
    verify_stable_regular_file_identity,
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


# This process's captures by root set; see CargoResourceCustody.capture.
_CAPTURED_RESOURCES: dict[tuple[CargoResourceRoot, ...], CargoResourceCustody] = {}


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
    # file_identity's index, built once: entrypoint -> its one identity, or
    # None when two captures of the entrypoint disagree.
    _by_entrypoint: dict[Path, StableRegularFileIdentity | None] | None = field(
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
            if walk.resolved_root != root.path:
                _input_report.report_probe(root.path)
            for alias, _target in walk.aliases:
                _input_report.report_probe(alias)
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
        digests = host_file_digest_cache()
        walks = observe_cargo_resource_roots(roots)
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
            verify_stable_regular_file_identity(item.identity, label=item.label)
            verified.add(item.identity.path)

    def _selection_unchanged(self) -> bool:
        resolved_parents: dict[Path, Path] = {}
        return all(
            resource_walk_unchanged(root, walk, resolved_parents, verify_files=True)
            for root, walk in zip(self.roots, self.walks, strict=True)
        )

    def file_identity(self, path: Path) -> StableRegularFileIdentity:
        """Project a captured lexical resource; never select or hash a new file."""
        index = self._by_entrypoint
        if index is None:
            index = {}
            for item in self.files:
                known = index.get(item.entrypoint, item.identity)
                index[item.entrypoint] = known if known == item.identity else None
            object.__setattr__(self, "_by_entrypoint", index)
        identity = index.get(path)
        if identity is None:
            raise ValueError(
                f"runtime resource has no unique captured identity: {path}"
            )
        return identity

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

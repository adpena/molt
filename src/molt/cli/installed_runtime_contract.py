"""Installed native admission values and fences, independent of their producer.

Selection, retention, publication, and runtime builds belong to installed_runtime.
This contract owns only the immutable admitted generation and its read-only fences.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from molt.cli.native_link_custody import NativeLinkCustodyObservation
from molt.cli.native_link_manifest import NativeLinkManifestFacts
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.compiler_distribution import NATIVE_CALLABLE_PROJECTION_ROLE
from molt.file_publication import resolve_owned_path
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    verify_stable_regular_file_identity,
)


class InstalledRuntimeError(ValueError):
    """A shipped runtime cell is unsupported, missing, damaged or mismatched."""


@dataclass(frozen=True, slots=True)
class InstalledNativeAdmission:
    """One build operation's content admission of a retained native generation.

    Each member was hashed once against the signed cell record, and the
    retained receipt was validated against the archive's content identity and
    custody closure. Later consumers in the same operation verify these
    stable-file fences (handle identity and content change time, never mtime
    alone) instead of re-hashing or re-admitting the shipped cell. Every field
    is an immutable value; nothing here is a process-global cache.
    """

    cell_id: str
    runtime_lib: Path
    build_identity: RuntimeBuildIdentity
    archive: StableRegularFileIdentity
    manifest: StableRegularFileIdentity
    callable_projection: StableRegularFileIdentity
    link_facts: NativeLinkManifestFacts
    custody: NativeLinkCustodyObservation
    callable_semantic_digest: str

    def members(self) -> tuple[tuple[str, StableRegularFileIdentity], ...]:
        return (
            ("runtime_archive", self.archive),
            ("native_link_manifest", self.manifest),
            (NATIVE_CALLABLE_PROJECTION_ROLE, self.callable_projection),
        )

    def verify(self) -> None:
        if resolve_owned_path(self.runtime_lib) != self.archive.path:
            raise InstalledRuntimeError(
                "installed native runtime path no longer names its admitted generation"
            )
        for role, identity in self.members():
            verify_stable_regular_file_identity(
                identity, label=f"admitted installed runtime {role}"
            )

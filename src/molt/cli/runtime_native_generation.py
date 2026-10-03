"""Atomic retention and selection of complete native runtime generations."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
import tempfile
from typing import TypeVar, cast

from molt.cli.atomic_io import _atomic_copy_file, _atomic_write_json
from molt.cli.native_link_custody import (
    NativeLinkCustodyError,
    native_link_custody_archive_path,
)
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    native_link_dependency_manifest_path,
    read_native_link_dependency_manifest,
    write_native_link_dependency_manifest,
)
from molt.cli.runtime_artifact_selection import RUNTIME_STATICLIB_ARTIFACTS
from molt.cli.runtime_identity_schema import (
    RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
    RuntimeBuildIdentity,
    require_native_runtime_staticlib_identity,
)
from molt.cli.runtime_paths import _cargo_profile_dir
from molt.exact_json import canonical_json_sha256, read_exact
from molt.file_publication import (
    durable_publish_directory_exclusive,
    is_link_like,
    resolve_owned_path,
)
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)


_SCHEMA = "molt.runtime-native-generation.v1"
_Admission = TypeVar("_Admission")


def publish_native_runtime_directory(
    staged: Path,
    destination: Path,
    *,
    verify_staged: Callable[[], None],
    admit: Callable[[Path], _Admission],
) -> _Admission:
    """Publish a complete native closure, also used by installed runtime cells.

    The destination is immutable. A concurrent winner must pass the same
    admission as this transaction's staged files; it is never overwritten.
    The caller owns staging cleanup and the content-addressed destination.
    """
    verify_staged()
    try:
        durable_publish_directory_exclusive(staged, destination)
    except OSError:
        if not destination.exists() or is_link_like(destination):
            raise
    return admit(destination)


@dataclass(frozen=True, slots=True)
class NativeRuntimeGeneration:
    runtime_lib: Path
    build_identity: RuntimeBuildIdentity
    members: tuple[tuple[str, StableRegularFileIdentity], ...]

    def verify(self) -> None:
        if resolve_owned_path(self.runtime_lib) != self.members[0][1].path:
            raise ValueError("native runtime no longer names its admitted generation")
        for role, identity in self.members:
            verify_stable_regular_file_identity(
                identity, label=f"native runtime generation {role}"
            )

    def records(self) -> list[dict[str, object]]:
        return [
            {
                "role": role,
                "name": identity.path.name,
                "size": identity.size,
                "sha256": identity.sha256,
            }
            for role, identity in self.members
        ]


def _capture_generation(
    runtime_lib: Path,
    *,
    build_identity: RuntimeBuildIdentity,
    cargo_profile: str,
    target_triple: str | None,
    expected_records: object | None = None,
) -> NativeRuntimeGeneration:
    identity = require_native_runtime_staticlib_identity(
        build_identity,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
    )
    archive = stable_regular_file_identity(runtime_lib, label="native runtime archive")
    manifest_file = stable_regular_file_identity(
        native_link_dependency_manifest_path(runtime_lib),
        label="native runtime link manifest",
    )
    manifest = read_native_link_dependency_manifest(
        runtime_lib,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        runtime_build_identity=identity,
    )
    members = [("runtime_archive", archive), ("native_link_manifest", manifest_file)]
    custody = native_link_custody_archive_path(
        runtime_lib, cast(Mapping[str, object], manifest["custody"])
    )
    if custody is not None:
        members.append(
            (
                "native_link_custody_archive",
                stable_regular_file_identity(custody, label="native runtime link custody"),
            )
        )
    generation = NativeRuntimeGeneration(runtime_lib, identity, tuple(members))
    if expected_records is not None and generation.records() != expected_records:
        raise ValueError("native runtime generation members differ from selection")
    generation.verify()
    return generation


def native_runtime_generation_path(coordinate: Path) -> Path:
    """The selection receipt; the Cargo output coordinate is never admitted."""
    return coordinate.with_name(f"{coordinate.name}.generation.json")


def _generation_material(
    generation: NativeRuntimeGeneration, *, cargo_profile: str
) -> dict[str, object]:
    return {
        "schema": _SCHEMA,
        "build_identity": generation.build_identity.to_dict(),
        "profile_dir": _cargo_profile_dir(cargo_profile),
        "runtime_name": generation.runtime_lib.name,
        "members": generation.records(),
    }


def read_native_runtime_generation(
    coordinate: Path,
    *,
    cargo_profile: str,
    target_triple: str | None,
) -> NativeRuntimeGeneration | None:
    """Admit a selected retained closure; partial, stale or corrupt selections miss.

    The returned build identity is provenance, not a current-input expectation.
    Its consumer must capture current inputs after this read, compare them, and
    close the returned member fences before accepting it.
    """
    try:
        payload = read_exact(
            native_runtime_generation_path(coordinate),
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="native runtime generation selection",
        )
        if not isinstance(payload, dict) or set(payload) != {
            "schema", "build_identity", "profile_dir", "runtime_name", "members",
            "generation",
        }:
            return None
        material = {key: value for key, value in payload.items() if key != "generation"}
        digest = canonical_json_sha256(material)
        profile_dir = _cargo_profile_dir(cargo_profile)
        if (
            payload["schema"] != _SCHEMA
            or payload["generation"] != digest
            or payload["profile_dir"] != profile_dir
            or payload["runtime_name"] != coordinate.name
        ):
            return None
        root = coordinate.parent / ".molt-native-generations" / digest
        if is_link_like(root) or is_link_like(root / profile_dir):
            return None
        return _capture_generation(
            root / profile_dir / coordinate.name,
            build_identity=RuntimeBuildIdentity.from_dict(payload["build_identity"]),
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            expected_records=payload["members"],
        )
    except (
        OSError,
        ValueError,
        TypeError,
        UnicodeError,
        NativeLinkCustodyError,
        NativeLinkDependencyManifestError,
    ):
        return None


def publish_native_runtime_generation(
    coordinate: Path,
    *,
    source_archive: Path,
    cargo_stdout: str,
    cargo_stderr: str,
    cargo_profile: str,
    target_triple: str | None,
    build_identity: RuntimeBuildIdentity,
    inputs_are_current: Callable[[], bool],
) -> NativeRuntimeGeneration | None:
    """Stage the closure, recapture inputs once, then select it atomically.

    No archive alias, standalone fingerprint, or half-published manifest can
    authorize a consumer. A failed final input check leaves the old selection
    intact. Publication never substitutes another input capture for that check.
    """
    coordinate.parent.mkdir(parents=True, exist_ok=True)
    profile_dir = _cargo_profile_dir(cargo_profile)
    generations = coordinate.parent / ".molt-native-generations"
    generations.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".native-runtime-", dir=generations) as temp:
        stage = Path(temp) / "members"
        runtime_lib = stage / profile_dir / coordinate.name
        runtime_lib.parent.mkdir(parents=True)
        source = stable_regular_file_identity(source_archive, label="Cargo runtime output")
        _atomic_copy_file(source_archive, runtime_lib, expected_sha256=source.sha256)
        verify_stable_regular_file_identity(source, label="Cargo runtime output")
        write_native_link_dependency_manifest(
            cargo_stdout,
            cargo_stderr=cargo_stderr,
            runtime_lib=runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            runtime_build_identity=build_identity,
        )
        staged = _capture_generation(
            runtime_lib,
            build_identity=build_identity,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
        )
        material = _generation_material(staged, cargo_profile=cargo_profile)
        digest = canonical_json_sha256(material)
        if not inputs_are_current():
            return None
        staged.verify()

        def admit(root: Path) -> NativeRuntimeGeneration:
            return _capture_generation(
                root / profile_dir / coordinate.name,
                build_identity=build_identity,
                cargo_profile=cargo_profile,
                target_triple=target_triple,
                expected_records=staged.records(),
            )

        published = publish_native_runtime_directory(
            stage, generations / digest, verify_staged=staged.verify, admit=admit
        )
        published.verify()
        _atomic_write_json(
            native_runtime_generation_path(coordinate),
            {**material, "generation": digest},
            sort_keys=True,
        )
        published.verify()
        return published

"""Complete native runtime generations: publication, selection and admission.

A coordinate names one selection receipt and the immutable generations beside
it. Builds use coordinates inside the host's product store
(``molt.cli.product_store``): one entry per runtime build identity, one variant
per observed Cargo environment (design 56, section 2.8).
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
import tempfile
from typing import TypeVar, cast

from molt.cli.atomic_io import _atomic_copy_file, _atomic_write_json
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli.product_store import ProductStore, ProductStoreError
from molt.cli.native_link_custody import (
    NativeLinkCustodyError,
    native_link_custody_archive_path,
)
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    native_link_dependency_manifest_path,
    read_native_link_dependency_manifest,
    read_native_link_dependency_manifest_payload,
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
from molt.portable_paths import portable_path_component
from molt.process_environment import ProcessEnvironment
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
    cargo_environment: CargoEnvironmentInputs

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
                stable_regular_file_identity(
                    custody, label="native runtime link custody"
                ),
            )
        )
    generation = NativeRuntimeGeneration(
        runtime_lib,
        identity,
        tuple(members),
        CargoEnvironmentInputs.from_dict(manifest["cargo_environment"]),
    )
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


_SELECTION_FIELDS = frozenset(
    {"schema", "build_identity", "profile_dir", "runtime_name", "members", "generation"}
)


def _selection_payload(
    payload: object, *, coordinate: Path, cargo_profile: str
) -> dict[str, object] | None:
    """A selection receipt for ``coordinate`` whose digest names its own material."""
    if not isinstance(payload, dict) or set(payload) != _SELECTION_FIELDS:
        return None
    material = {key: value for key, value in payload.items() if key != "generation"}
    if (
        payload["schema"] != _SCHEMA
        or payload["generation"] != canonical_json_sha256(material)
        or payload["profile_dir"] != _cargo_profile_dir(cargo_profile)
        or payload["runtime_name"] != coordinate.name
    ):
        return None
    return payload


def read_native_runtime_generation(
    coordinate: Path,
    *,
    cargo_profile: str,
    target_triple: str | None,
) -> NativeRuntimeGeneration | None:
    """Admit the retained closure ``coordinate`` selects, or None for no selection.

    A selection receipt is written only after its complete closure, so a
    receipt that exists but does not admit (a partial, stale or mutated
    member, a redirected digest) raises ValueError instead of missing.

    The returned build identity is provenance, not a current-input expectation.
    Its consumer must capture current inputs after this read, compare them, and
    close the returned member fences before accepting it.
    """
    selection = native_runtime_generation_path(coordinate)
    try:
        selection.lstat()
    except FileNotFoundError:
        return None
    try:
        raw = read_exact(
            selection,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="native runtime generation selection",
        )
    except (OSError, ValueError, UnicodeError) as exc:
        raise ValueError(f"unreadable selection receipt {selection}: {exc}") from exc
    try:
        payload = _selection_payload(
            raw, coordinate=coordinate, cargo_profile=cargo_profile
        )
        if payload is None:
            raise ValueError("the receipt does not name its own material")
        profile_dir = _cargo_profile_dir(cargo_profile)
        root = (
            coordinate.parent / ".molt-native-generations" / str(payload["generation"])
        )
        if is_link_like(root) or is_link_like(root / profile_dir):
            raise ValueError("the selected generation is indirect")
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
    ) as exc:
        raise ValueError(f"selection {selection} does not admit: {exc}") from exc


def native_runtime_store_generations(
    store: ProductStore,
    identity: RuntimeBuildIdentity,
    *,
    runtime_name: str,
    cargo_profile: str,
    target_triple: str | None,
) -> tuple[NativeRuntimeGeneration, ...]:
    """Every published variant of ``identity`` in ``store``, admitted.

    A variant whose receipt has not appeared yet is a publication in progress.
    A variant that exists but does not admit is corrupt: it raises
    ``ProductStoreError`` instead of missing, so a store miss never hides it.
    """
    admitted = []
    for variant in store.variants(identity.digest):
        try:
            generation = read_native_runtime_generation(
                variant / runtime_name,
                cargo_profile=cargo_profile,
                target_triple=target_triple,
            )
        except ValueError as exc:
            raise ProductStoreError(variant, str(exc)) from exc
        if generation is None:
            continue
        if generation.build_identity != identity:
            raise ProductStoreError(variant, "it holds another build identity")
        if generation.cargo_environment.digest != variant.name:
            raise ProductStoreError(
                variant, "its Cargo environment differs from its name"
            )
        admitted.append(generation)
    return tuple(admitted)


def _select_staged_generation(
    coordinate: Path,
    *,
    stage: Path,
    staged: NativeRuntimeGeneration,
    cargo_profile: str,
    target_triple: str | None,
    inputs_are_current: Callable[[], bool],
    expected_generation: object = None,
) -> NativeRuntimeGeneration | None:
    """Publish a staged closure under its digest, then select it atomically."""
    profile_dir = _cargo_profile_dir(cargo_profile)
    material = _generation_material(staged, cargo_profile=cargo_profile)
    digest = canonical_json_sha256(material)
    if expected_generation is not None and digest != expected_generation:
        raise ValueError("native runtime generation differs from its selection")
    if not inputs_are_current():
        return None
    staged.verify()

    def admit(root: Path) -> NativeRuntimeGeneration:
        return _capture_generation(
            root / profile_dir / coordinate.name,
            build_identity=staged.build_identity,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            expected_records=staged.records(),
        )

    published = publish_native_runtime_directory(
        stage,
        coordinate.parent / ".molt-native-generations" / digest,
        verify_staged=staged.verify,
        admit=admit,
    )
    published.verify()
    _atomic_write_json(
        native_runtime_generation_path(coordinate),
        {**material, "generation": digest},
        sort_keys=True,
    )
    published.verify()
    return published


def publish_native_runtime_generation(
    coordinate: Path,
    *,
    source_archive: Path,
    cargo_stdout: str,
    cargo_stderr: str,
    cargo_profile: str,
    target_triple: str | None,
    build_identity: RuntimeBuildIdentity,
    cargo_environment: CargoEnvironmentInputs,
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
    with tempfile.TemporaryDirectory(
        prefix=".native-runtime-", dir=generations
    ) as temp:
        stage = Path(temp) / "members"
        runtime_lib = stage / profile_dir / coordinate.name
        runtime_lib.parent.mkdir(parents=True)
        source = stable_regular_file_identity(
            source_archive, label="Cargo runtime output"
        )
        _atomic_copy_file(source_archive, runtime_lib, expected_sha256=source.sha256)
        verify_stable_regular_file_identity(source, label="Cargo runtime output")
        write_native_link_dependency_manifest(
            cargo_stdout,
            cargo_stderr=cargo_stderr,
            runtime_lib=runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            runtime_build_identity=build_identity,
            cargo_environment=cargo_environment,
        )
        staged = _capture_generation(
            runtime_lib,
            build_identity=build_identity,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
        )
        return _select_staged_generation(
            coordinate,
            stage=stage,
            staged=staged,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            inputs_are_current=inputs_are_current,
        )


def import_native_runtime_generation(
    store: ProductStore,
    *,
    runtime_name: str,
    selection: Path,
    members: Mapping[str, Path],
    cargo_profile: str,
    target_triple: str | None,
    build_identity: RuntimeBuildIdentity,
    environment: ProcessEnvironment,
) -> NativeRuntimeGeneration:
    """Publish a generation another host built into ``store``, through admission.

    ``selection`` and ``members`` (keyed by role) are copies of another
    host's selection receipt and generation files. ``build_identity`` is
    this checkout's current capture: a generation built from other inputs is
    refused before anything is published, and the republished closure must
    reproduce the receipt's generation digest. The closure lands in the
    variant of its recorded Cargo environment, which must match this one.
    """
    payload = _selection_payload(
        read_exact(
            selection,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="transported native runtime generation selection",
        ),
        coordinate=Path(runtime_name),
        cargo_profile=cargo_profile,
    )
    if payload is None:
        raise ValueError(
            f"transported native runtime selection is invalid: {selection}"
        )
    if RuntimeBuildIdentity.from_dict(payload["build_identity"]) != build_identity:
        raise ValueError(
            "transported native runtime was built from other inputs than this checkout"
        )
    records = payload["members"]
    if not isinstance(records, list) or any(
        not isinstance(record, dict) for record in records
    ):
        raise ValueError("transported native runtime members are invalid")
    if sorted(str(record.get("role")) for record in records) != sorted(members):
        raise ValueError("transported native runtime members differ from its selection")
    manifest = read_native_link_dependency_manifest_payload(
        members["native_link_manifest"]
    )
    cargo_environment = CargoEnvironmentInputs.from_dict(
        manifest.get("cargo_environment")
    )
    cargo_environment.verify(environment)
    coordinate = store.variant(build_identity.digest, cargo_environment) / runtime_name
    profile_dir = _cargo_profile_dir(cargo_profile)
    generations = coordinate.parent / ".molt-native-generations"
    generations.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".native-runtime-", dir=generations
    ) as temp:
        stage = Path(temp) / "members"
        (stage / profile_dir).mkdir(parents=True)
        for record in records:
            name = portable_path_component(record.get("name"))
            digest = record.get("sha256")
            if not isinstance(digest, str):
                raise ValueError("transported native runtime member digest is invalid")
            _atomic_copy_file(
                members[str(record["role"])],
                stage / profile_dir / name,
                expected_sha256=digest,
            )
        staged = _capture_generation(
            stage / profile_dir / coordinate.name,
            build_identity=build_identity,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            expected_records=records,
        )
        staged.cargo_environment.verify(environment)
        published = _select_staged_generation(
            coordinate,
            stage=stage,
            staged=staged,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            inputs_are_current=lambda: True,
            expected_generation=payload["generation"],
        )
    assert published is not None
    store.retain(build_identity.digest)
    return published

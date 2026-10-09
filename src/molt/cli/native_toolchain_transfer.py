"""Transfer of the native toolchain a source-checkout build reuses.

A ``molt build --target native --build-profile dev`` in a source checkout
reuses two retained products without Cargo: the selected native runtime
generation and the feature-tagged backend compiler with its fingerprint
receipt. Export names their files where this checkout's build admitted them.
Import admits copies into another checkout of the same commit at the
canonical coordinates every build session consults. Each import passes the
admission a build applies, against inputs captured in the importing
checkout, so a transport never derives or trusts a layout of its own.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from molt.backend_executable_names import DEFAULT_CODEGEN_BACKEND
from molt.cli.backend_binary import admitted_backend_binary, import_backend_binary
from molt.cli.backend_compile import _BackendSelection, _select_backend_binary
from molt.cli.build_inputs import _resolve_backend_compiler_profile
from molt.cli.cargo_profiles import _resolve_cargo_profile_name
from molt.cli.native_link_custody import NativeLinkCustodyError
from molt.cli.native_link_manifest import NativeLinkDependencyManifestError
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.cli.runtime_native_build import (
    canonical_native_runtime_coordinate,
    current_native_runtime_build_identity,
    native_runtime_generation_coordinates,
)
from molt.cli.runtime_native_generation import (
    import_native_runtime_generation,
    native_runtime_generation_path,
    read_native_runtime_generation,
)
from molt.cli.runtime_paths import _runtime_lib_path

STDLIB_PROFILE = "full"
_ADMISSION_ERRORS = (
    OSError,
    ValueError,
    NativeLinkCustodyError,
    NativeLinkDependencyManifestError,
)
RUNTIME_SELECTION_ROLE = "runtime_generation_selection"
BACKEND_EXECUTABLE_ROLE = "backend_executable"
BACKEND_RECEIPT_ROLE = "backend_fingerprint"
RUNTIME_ARCHIVE_ROLE = "runtime_archive"
NATIVE_LINK_MANIFEST_ROLE = "native_link_manifest"
NATIVE_LINK_CUSTODY_ROLE = "native_link_custody_archive"
REQUIRED_ROLES = (
    RUNTIME_SELECTION_ROLE,
    RUNTIME_ARCHIVE_ROLE,
    NATIVE_LINK_MANIFEST_ROLE,
    BACKEND_EXECUTABLE_ROLE,
    BACKEND_RECEIPT_ROLE,
)
OPTIONAL_ROLES = (NATIVE_LINK_CUSTODY_ROLE,)
_BACKEND_ROLES = (BACKEND_EXECUTABLE_ROLE, BACKEND_RECEIPT_ROLE)


class NativeToolchainTransferError(RuntimeError):
    """The native toolchain cannot be exported from or admitted into a checkout."""


@dataclass(frozen=True)
class TransferMember:
    role: str
    path: Path
    executable: bool


@dataclass(frozen=True)
class NativeToolchainSelection:
    """The host-native dev runtime and backend a build of ``project_root`` uses."""

    project_root: Path
    runtime_lib: Path
    runtime_cargo_profile: str
    backend: _BackendSelection

    @classmethod
    def current(cls, project_root: Path) -> NativeToolchainSelection:
        runtime_cargo_profile, runtime_error = _resolve_cargo_profile_name("dev")
        _profile, backend_cargo_profile, backend_error = (
            _resolve_backend_compiler_profile()
        )
        if runtime_error or backend_error:
            raise NativeToolchainTransferError(str(runtime_error or backend_error))
        return cls(
            project_root=project_root,
            runtime_lib=_runtime_lib_path(
                project_root,
                runtime_cargo_profile,
                None,
                stdlib_profile=STDLIB_PROFILE,
            ),
            runtime_cargo_profile=runtime_cargo_profile,
            backend=_select_backend_binary(
                molt_root=project_root,
                backend_cargo_profile=backend_cargo_profile,
                is_wasm=False,
                is_luau_transpile=False,
                is_rust_transpile=False,
                codegen_backend=DEFAULT_CODEGEN_BACKEND,
            ),
        )

    def runtime_build_identity(self) -> RuntimeBuildIdentity:
        """This checkout's current runtime inputs; never a receipt's claim."""
        try:
            return current_native_runtime_build_identity(
                self.project_root,
                self.runtime_lib,
                target_triple=None,
                cargo_profile=self.runtime_cargo_profile,
                stdlib_profile=STDLIB_PROFILE,
            )
        except (OSError, ValueError) as exc:
            raise NativeToolchainTransferError(
                f"cannot capture the native runtime build identity: {exc}"
            ) from exc


def export_native_toolchain(
    selection: NativeToolchainSelection,
) -> tuple[TransferMember, ...]:
    """The admitted runtime generation and backend files, in transfer order."""
    identity = selection.runtime_build_identity()
    members: list[TransferMember] | None = None
    for coordinate in native_runtime_generation_coordinates(
        selection.runtime_lib,
        project_root=selection.project_root,
        cargo_profile=selection.runtime_cargo_profile,
        target_triple=None,
    ):
        generation = read_native_runtime_generation(
            coordinate,
            cargo_profile=selection.runtime_cargo_profile,
            target_triple=None,
        )
        if generation is not None and generation.build_identity == identity:
            members = [
                TransferMember(
                    RUNTIME_SELECTION_ROLE,
                    native_runtime_generation_path(coordinate),
                    executable=False,
                ),
                *(
                    TransferMember(role, member.path, executable=False)
                    for role, member in generation.members
                ),
            ]
            break
    if members is None:
        raise NativeToolchainTransferError(
            "no native runtime generation is admitted for this checkout's inputs; "
            f"build {selection.runtime_lib.name} first"
        )
    try:
        backend, receipt = admitted_backend_binary(
            selection.project_root,
            binary=selection.backend.binary,
            cargo_profile=selection.backend.cargo_profile,
            backend_features=selection.backend.features,
        )
    except (OSError, ValueError) as exc:
        raise NativeToolchainTransferError(str(exc)) from exc
    members.append(TransferMember(BACKEND_EXECUTABLE_ROLE, backend, executable=True))
    members.append(TransferMember(BACKEND_RECEIPT_ROLE, receipt, executable=False))
    return tuple(members)


def import_native_toolchain(
    selection: NativeToolchainSelection, files: Mapping[str, Path]
) -> None:
    """Admit exported files at this checkout's canonical coordinates.

    Nothing is published unless the runtime receipt carries this checkout's
    runtime build identity and the backend receipt its backend identity.
    """
    missing = [role for role in REQUIRED_ROLES if role not in files]
    unknown = sorted(set(files) - {*REQUIRED_ROLES, *OPTIONAL_ROLES})
    if missing or unknown:
        raise NativeToolchainTransferError(
            f"native toolchain files are incomplete: missing={missing}, unknown={unknown}"
        )
    identity = selection.runtime_build_identity()
    runtime_members = {
        role: path
        for role, path in files.items()
        if role not in (RUNTIME_SELECTION_ROLE, *_BACKEND_ROLES)
    }
    try:
        import_native_runtime_generation(
            canonical_native_runtime_coordinate(
                selection.runtime_lib,
                project_root=selection.project_root,
                cargo_profile=selection.runtime_cargo_profile,
                target_triple=None,
            ),
            selection=files[RUNTIME_SELECTION_ROLE],
            members=runtime_members,
            cargo_profile=selection.runtime_cargo_profile,
            target_triple=None,
            build_identity=identity,
        )
        import_backend_binary(
            selection.project_root,
            binary=selection.backend.binary,
            cargo_profile=selection.backend.cargo_profile,
            backend_features=selection.backend.features,
            executable=files[BACKEND_EXECUTABLE_ROLE],
            receipt=files[BACKEND_RECEIPT_ROLE],
        )
    except _ADMISSION_ERRORS as exc:
        raise NativeToolchainTransferError(str(exc)) from exc

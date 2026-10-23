"""Transfer of the native toolchain a source-checkout build reuses, across hosts.

A ``molt build --target native --build-profile dev`` in a source checkout
reuses two retained products without Cargo: the native runtime generation in
the host's product store (``molt.cli.product_store``) and the feature-tagged
backend compiler with its fingerprint receipt. Every checkout on one host
already shares the store; this transfer carries the products to another host.
Export names the files of the store entry this checkout's inputs select.
Import publishes copies into the importing host's store. Each import passes
the admission a build applies, against inputs captured in the importing
checkout, so a transport never derives or trusts a layout of its own.
"""

from __future__ import annotations

from molt.compilation_target import TargetKind

import os
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
from molt.cli.product_store import ProductStoreError
from molt.cli.runtime_native_build import (
    NativeRuntimeBuildInputs,
    current_native_runtime_build_inputs,
    native_runtime_store,
)
from molt.cli.runtime_native_generation import (
    import_native_runtime_generation,
    native_runtime_generation_path,
    native_runtime_store_generation,
)
from molt.cli.runtime_paths import _runtime_lib_path
from molt.cli.rust_cpu_policy import RustCpuPolicy, resolve_rust_cpu_policy

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
        try:
            rust_cpu, _warning = resolve_rust_cpu_policy(TargetKind.NATIVE, os.environ)
        except ValueError as exc:
            raise NativeToolchainTransferError(str(exc)) from exc
        if rust_cpu is not RustCpuPolicy.BASELINE:
            # The flag names no CPU, so the identity cannot tell two hosts
            # apart. A native-arch product stays on the host that built it.
            raise NativeToolchainTransferError(
                "MOLT_PERF_PROFILE=native-arch products are specific to the "
                "build host and never transfer; unset MOLT_PERF_PROFILE"
            )
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
                rust_cpu=rust_cpu,
            ),
        )

    def runtime_build_inputs(self) -> NativeRuntimeBuildInputs:
        """This checkout's current runtime inputs; never a receipt's claim.

        The capture uses the same Cargo tool and resource admission as a
        ``molt build``; a stored receipt never supplies its own expectation.
        """
        try:
            return current_native_runtime_build_inputs(
                self.project_root,
                self.runtime_lib,
                target_triple=None,
                cargo_profile=self.runtime_cargo_profile,
                stdlib_profile=STDLIB_PROFILE,
                rust_cpu=self.backend.rust_cpu,
            )
        except (OSError, ValueError) as exc:
            raise NativeToolchainTransferError(
                f"cannot capture the native runtime build identity: {exc}"
            ) from exc


def export_native_toolchain(
    selection: NativeToolchainSelection,
) -> tuple[TransferMember, ...]:
    """The admitted runtime generation and backend files, in transfer order."""
    inputs = selection.runtime_build_inputs()
    identity = inputs.build_identity
    store = native_runtime_store(selection.project_root)
    try:
        # Renewed before the read, so no eviction races the export.
        generation = (
            native_runtime_store_generation(
                store,
                identity,
                inputs.environment,
                runtime_name=selection.runtime_lib.name,
                cargo_profile=selection.runtime_cargo_profile,
                target_triple=None,
            )
            if store.record_use(identity.digest)
            else None
        )
    except (OSError, ProductStoreError) as exc:
        raise NativeToolchainTransferError(str(exc)) from exc
    if generation is None:
        raise NativeToolchainTransferError(
            f"no native runtime generation is stored for this checkout's inputs "
            f"and Cargo environment ({identity.digest} in {store.root}); build "
            f"{selection.runtime_lib.name} first"
        )
    coordinate = (
        store.variant(identity.digest, generation.cargo_environment)
        / selection.runtime_lib.name
    )
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
    try:
        backend, receipt = admitted_backend_binary(
            selection.project_root,
            binary=selection.backend.binary,
            cargo_profile=selection.backend.cargo_profile,
            backend_features=selection.backend.features,
            rust_cpu=selection.backend.rust_cpu,
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
    inputs = selection.runtime_build_inputs()
    identity = inputs.build_identity
    runtime_members = {
        role: path
        for role, path in files.items()
        if role not in (RUNTIME_SELECTION_ROLE, *_BACKEND_ROLES)
    }
    try:
        import_native_runtime_generation(
            native_runtime_store(selection.project_root),
            runtime_name=selection.runtime_lib.name,
            selection=files[RUNTIME_SELECTION_ROLE],
            members=runtime_members,
            cargo_profile=selection.runtime_cargo_profile,
            target_triple=None,
            build_identity=identity,
            environment=inputs.environment,
        )
        import_backend_binary(
            selection.project_root,
            binary=selection.backend.binary,
            cargo_profile=selection.backend.cargo_profile,
            backend_features=selection.backend.features,
            rust_cpu=selection.backend.rust_cpu,
            executable=files[BACKEND_EXECUTABLE_ROLE],
            receipt=files[BACKEND_RECEIPT_ROLE],
        )
    except _ADMISSION_ERRORS as exc:
        raise NativeToolchainTransferError(str(exc)) from exc

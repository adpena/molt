"""Installed runtime artifact authority: select, admit and retain shipped cells.

An installed Molt (release bundle, package-manager projection or platform
wheel) is a compiler distribution, not a runtime build environment. Its signed
``release-compiler-source.json`` declares every shipped native staticlib and
WASM runtime pair as a typed cell. User compilation selects exactly one declared
cell from typed build parameters and admits the bundle bytes through the
canonical receipts: the native-link dependency manifest and the immutable WASM
generation manifest. Their ``RuntimeBuildIdentity`` must agree with the cell
key (target, profile, tier/features, SIMD/freestanding, C-API surface). Native
cells also ship the archive-derived callable projection code generation
consumes, so installed compilation never runs a symbol reader. One
content-addressed generation is retained under ``MOLT_HOME``. Each build
operation admits it once; code generation and final link reuse that admission
through stable-file mutation fences and re-derive only the cell selection, so
the shipped cell is never admitted again beneath compiled code. Nothing here
plans Cargo, consults an ambient runtime directory, or substitutes a different
cell.

Source checkouts never enter this admission path: selectors return ``None``
when no installed compiler exists, and callers keep their explicit development
Cargo workflow.
"""

from __future__ import annotations

import contextlib
from dataclasses import dataclass
import os
from pathlib import Path
from typing import Any, Collection, Mapping, Sequence, cast
import uuid

from molt._wasm_runtime_exports import wasm_cpython_abi_distribution_export_names
from molt.cli import installed_runtime_contract as _runtime_contract
from molt.cli.atomic_io import _atomic_copy_file, _remove_file_or_tree
from molt.cli.config_resolution import DEFAULT_RUNTIME_STDLIB_PROFILE
from molt.cli.default_paths import _default_molt_home
from molt.cli.native_link_custody import (
    NativeLinkCustodyError,
    NativeLinkCustodyAdmission,
    copy_native_link_custody_archive,
    native_link_custody_archive_path,
    observe_native_link_custody,
)
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    NativeLinkManifestFacts,
    native_link_dependency_manifest_path,
    validate_native_link_dependency_manifest,
)
from molt.cli.runtime_features import runtime_fingerprint_features_for_profile
from molt.cli.runtime_identity_schema import (
    RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
    RuntimeBuildIdentity,
    _freeze_json,
)
from molt.cli.runtime_paths import _runtime_lib_archive_name
from molt.cli.runtime_native_generation import publish_native_runtime_directory
from molt.cli.runtime_wasm_build_policy import _resolve_wasm_cargo_profile
from molt.cli.runtime_wasm_build_spec import runtime_wasm_distribution_features
from molt.cli.runtime_wasm_build_support import wasm_runtime_simd_enabled
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmGeneration,
    hydrate_runtime_wasm_generation,
    read_runtime_wasm_generation,
    runtime_wasm_generation_path,
    _generation_record_descriptors,
    _validate_generation_payload,
)
from molt.cli.static_archive_identity import (
    StaticArchiveIdentityError,
    artifact_content_identity,
)
from molt.compiler_distribution import (
    NATIVE_CALLABLE_PROJECTION_ROLE,
    NATIVE_RUNTIME_CELL,
    WASM_RUNTIME_CELL,
    InstalledCompiler,
    installed_compiler,
    verify_runtime_member,
)
from molt.exact_json import loads_exact, string_keyed_mapping
from molt.file_publication import (
    is_link_like,
    resolve_owned_path,
)
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from molt.release_lanes import ReleaseLane, capture_release_lanes
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    stable_regular_file_identity,
    stable_regular_file_version,
    verify_stable_regular_file_identity,
)
from molt.verified_subset import current_host_coordinate

WASM_RUNTIME_TARGET = "wasm32-wasip1"
INSTALLED_RUNTIME_STORE = "installed-runtime"
_SHARED_RUNTIME_NAME = "molt_runtime.wasm"
_RELOC_RUNTIME_NAME = "molt_runtime_reloc.wasm"
# Source-checkout runtime coordinates. An installed CLI never consults them, so
# setting one is a request the installed distribution cannot honor.
_SOURCE_RUNTIME_SELECTORS = ("MOLT_WASM_RUNTIME_DIR",)
_ADMISSION_ERRORS = (
    OSError,
    ValueError,
    TypeError,
    KeyError,
    NativeLinkDependencyManifestError,
    NativeLinkCustodyError,
    StaticArchiveIdentityError,
)


def _installed(project_root: Path) -> InstalledCompiler | None:
    installed = installed_compiler(project_root)
    if installed is None:
        return None
    selectors = sorted(
        name for name in _SOURCE_RUNTIME_SELECTORS if os.environ.get(name)
    )
    if selectors:
        raise _runtime_contract.InstalledRuntimeError(
            "installed Molt uses its shipped runtime cells; "
            + ", ".join(selectors)
            + " selects source-checkout runtime artifacts. Unset it, or set "
            "MOLT_SOURCE_ROOT to a Molt source checkout for runtime development."
        )
    return installed


def installed_runtime_active(project_root: Path) -> bool:
    """Whether runtime artifacts for this compiler root come from the bundle."""
    return _installed(project_root) is not None


def installed_runtime_store(installed: InstalledCompiler) -> Path:
    """Mutable retention root; never inside the immutable installation."""
    store = resolve_owned_path(_default_molt_home() / INSTALLED_RUNTIME_STORE)
    bundle = resolve_owned_path(installed.source_root.parent)
    if store == bundle or bundle in store.parents:
        raise _runtime_contract.InstalledRuntimeError(
            "MOLT_HOME must be outside the immutable Molt installation"
        )
    return store


@dataclass(frozen=True)
class InstalledRuntimeCell:
    """One declared runtime cell and the directory holding its members.

    ``installed`` is the admitting installation; release production and bundle
    assembly admit staged cells (``installed=None``) with the same receipts.
    """

    installed: InstalledCompiler | None
    record: Mapping[str, Any]
    members_root: Path

    @property
    def id(self) -> str:
        return cast(str, self.record["id"])

    @property
    def kind(self) -> str:
        return cast(str, self.record["kind"])

    @property
    def key(self) -> Mapping[str, Any]:
        return cast(Mapping[str, Any], self.record["key"])

    @property
    def target_triple(self) -> str | None:
        target = cast(str, self.key["target_triple"])
        return None if target == "native" else target

    def file_name(self, role: str) -> str | None:
        for entry in self.record["files"]:
            if entry["role"] == role:
                return cast(str, entry["name"])
        return None

    def file_record(self, role: str) -> Mapping[str, Any]:
        for entry in self.record["files"]:
            if entry["role"] == role:
                return cast(Mapping[str, Any], entry)
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {self.id} has no {role}"
        )

    def bundle_file(self, role: str) -> StableRegularFileIdentity:
        """Return one shipped member after content admission against the manifest."""
        try:
            return verify_runtime_member(self.members_root, self.record, role)
        except (OSError, ValueError) as exc:
            raise _runtime_contract.InstalledRuntimeError(
                f"installed runtime cell {self.id} {role} is missing or damaged; "
                f"reinstall Molt: {exc}"
            ) from exc

    def bundle_json(self, role: str) -> tuple[StableRegularFileIdentity, object]:
        try:
            path = resolve_owned_path(
                self.members_root / self.file_record(role)["name"]
            )
            identity, content = capture_stable_regular_file(
                path,
                max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
                label=f"installed runtime {role}",
            )
            self.require_signed_member(role, identity)
            return identity, loads_exact(content.decode("utf-8"))
        except (
            OSError,
            UnicodeError,
            ValueError,
            _runtime_contract.InstalledRuntimeError,
        ) as exc:
            raise _runtime_contract.InstalledRuntimeError(
                f"installed runtime cell {self.id} {role} is missing or damaged; "
                f"reinstall Molt: {exc}"
            ) from exc

    def bundle_projection(self) -> tuple[StableRegularFileIdentity, bytes]:
        role = NATIVE_CALLABLE_PROJECTION_ROLE
        try:
            path = resolve_owned_path(
                self.members_root / self.file_record(role)["name"]
            )
            identity, content = capture_stable_regular_file(
                path,
                label="installed callable projection",
                max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            )
            return self.require_signed_member(role, identity), content
        except (OSError, ValueError, _runtime_contract.InstalledRuntimeError) as exc:
            raise _runtime_contract.InstalledRuntimeError(
                f"installed runtime cell {self.id} {role} is missing or damaged; "
                f"reinstall Molt: {exc}"
            ) from exc

    @property
    def retained_root(self) -> Path:
        if self.installed is None:
            raise _runtime_contract.InstalledRuntimeError(
                "a staged runtime cell has no retention store"
            )
        return installed_runtime_store(self.installed) / self.id

    @property
    def runtime_lib(self) -> Path:
        """Retained native archive coordinate consumed by codegen and link."""
        if self.kind != NATIVE_RUNTIME_CELL:
            raise _runtime_contract.InstalledRuntimeError(
                "installed runtime cell is not native"
            )
        return self.retained_root / cast(str, self.file_name("runtime_archive"))

    def require_signed_member(
        self, role: str, identity: StableRegularFileIdentity
    ) -> StableRegularFileIdentity:
        """Admit one hashed retained member against its signed release record."""
        record = self.file_record(role)
        if (identity.sha256, identity.size) != (record["sha256"], record["size"]):
            raise _runtime_contract.InstalledRuntimeError(
                f"retained installed runtime {role} differs from its release record: "
                f"{identity.path}"
            )
        return identity

    def verify_retained_file(self, role: str, path: Path) -> StableRegularFileIdentity:
        """Hash one retained member once; the identity fences every later read."""
        return self.require_signed_member(
            role,
            stable_regular_file_identity(
                resolve_owned_path(path), label=f"retained installed runtime {role}"
            ),
        )


def _describe_cells(installed: InstalledCompiler, kind: str) -> str:
    keys = [cell["key"] for cell in installed.runtime["cells"] if cell["kind"] == kind]
    if not keys:
        return "none"
    return "; ".join(
        ", ".join(
            f"{name}={value}"
            for name, value in sorted(key.items())
            if name != "runtime_features"
        )
        for key in keys
    )


def select_installed_runtime_cell(
    installed: InstalledCompiler, *, kind: str, key: Mapping[str, Any]
) -> InstalledRuntimeCell:
    """Select exactly one declared cell by typed key; never a nearby substitute."""
    matches = [
        cell
        for cell in installed.runtime["cells"]
        if cell["kind"] == kind and cell["key"] == dict(key)
    ]
    if len(matches) != 1:
        requested = ", ".join(
            f"{name}={value}"
            for name, value in sorted(key.items())
            if name != "runtime_features"
        )
        raise _runtime_contract.InstalledRuntimeError(
            f"installed Molt does not ship a {kind} runtime for {requested} "
            f"(features: {', '.join(key.get('runtime_features', ()))}). "
            f"Shipped {kind} cells: {_describe_cells(installed, kind)}. "
            "Choose a shipped profile/target/feature cell, or set MOLT_SOURCE_ROOT "
            "to a Molt source checkout to build other runtime configurations."
        )
    return InstalledRuntimeCell(
        installed, matches[0], installed.runtime_root / matches[0]["id"]
    )


def native_runtime_cell_key(
    *,
    target_triple: str | None,
    cargo_profile: str,
    stdlib_profile: str | None,
    extra_runtime_features: Sequence[str] | None,
) -> dict[str, Any]:
    """Project the source runtime feature authority onto one cell key.

    The same projection names the Cargo features recorded in the native
    staticlib's build identity, so admission compares typed identity facts.
    """
    concrete = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE
    features = runtime_fingerprint_features_for_profile(
        concrete,
        target_triple=target_triple,
        extra_runtime_features=extra_runtime_features,
    )
    return {
        "target_triple": target_triple or "native",
        "cargo_profile": cargo_profile,
        "stdlib_profile": concrete,
        "runtime_features": sorted(set(features)),
    }


def wasm_runtime_cell_key(
    *,
    runtime_profile: str,
    stdlib_profile: str | None,
    simd_enabled: bool,
    freestanding: bool,
) -> dict[str, Any]:
    """Key the resolved physical profile, full tier ceiling and C-API surface."""
    concrete = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE
    return {
        "target_triple": WASM_RUNTIME_TARGET,
        "cargo_profile": runtime_profile,
        "stdlib_profile": concrete,
        "runtime_features": sorted(
            set(runtime_wasm_distribution_features(concrete, freestanding=freestanding))
        ),
        "simd": bool(simd_enabled),
        "freestanding": bool(freestanding),
    }


def select_installed_native_runtime(
    project_root: Path,
    *,
    target_triple: str | None,
    cargo_profile: str,
    stdlib_profile: str | None,
    extra_runtime_features: Sequence[str] | None,
) -> InstalledRuntimeCell | None:
    installed = _installed(project_root)
    if installed is None:
        return None
    key = native_runtime_cell_key(
        target_triple=target_triple,
        cargo_profile=cargo_profile,
        stdlib_profile=stdlib_profile,
        extra_runtime_features=extra_runtime_features,
    )
    cell = select_installed_runtime_cell(installed, kind=NATIVE_RUNTIME_CELL, key=key)
    expected = _runtime_lib_archive_name(key["stdlib_profile"], target_triple)
    if cell.file_name("runtime_archive") != expected:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} archive is not the {expected} member"
        )
    return cell


def select_installed_wasm_runtime(
    project_root: Path,
    *,
    cargo_profile: str,
    stdlib_profile: str | None,
    simd_enabled: bool,
    freestanding: bool,
) -> InstalledRuntimeCell | None:
    installed = _installed(project_root)
    if installed is None:
        return None
    key = wasm_runtime_cell_key(
        runtime_profile=_resolve_wasm_cargo_profile(cargo_profile),
        stdlib_profile=stdlib_profile,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    return select_installed_runtime_cell(installed, kind=WASM_RUNTIME_CELL, key=key)


def installed_runtime_profile_readiness(
    installed: InstalledCompiler,
) -> dict[ReleaseLane, bool]:
    """Reported availability joins each declared lane to delivered bytes/features.

    This is a selection report, not target execution qualification. Native and
    LLVM share runtime members but still require distinct compiler capabilities.
    """
    inventory = capture_release_lanes(installed.source_root)
    readiness: dict[ReleaseLane, bool] = {}
    for lane in inventory.lanes:
        features = lane.compiler_features
        kind = NATIVE_RUNTIME_CELL if lane.target == "native" else WASM_RUNTIME_CELL
        readiness[lane] = (
            installed.record["profile"] == lane.compiler_profile
            and set(features) <= set(installed.record["features"])
            and any(
                cell["kind"] == kind
                and cell["key"]["cargo_profile"] == lane.runtime_profile
                and cell["key"]["target_triple"]
                == ("native" if lane.target == "native" else WASM_RUNTIME_TARGET)
                for cell in installed.runtime["cells"]
            )
        )
    inventory.verify()
    return readiness


# --- canonical receipt semantics -----------------------------------------------


def _common_config(identity: RuntimeBuildIdentity) -> Mapping[str, object]:
    family = cast(Mapping[str, object], identity.payload["family"])
    compilation = cast(Mapping[str, object], family["compile"])
    return cast(Mapping[str, object], compilation["common_config"])


def _require_features(
    cell: InstalledRuntimeCell, identity: RuntimeBuildIdentity
) -> None:
    recorded = sorted(set(identity.runtime_features))
    if recorded != list(cell.key["runtime_features"]):
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} build features differ from its key"
        )


# --- native ------------------------------------------------------------------


def _shipped_native_identity(
    cell: InstalledRuntimeCell, archive: StableRegularFileIdentity
) -> tuple[
    NativeLinkManifestFacts, StableRegularFileIdentity, StableRegularFileIdentity | None
]:
    """Observe the canonical receipt once against the already-admitted archive."""
    receipt, payload = cell.bundle_json("native_link_manifest")
    if receipt.path != native_link_dependency_manifest_path(archive.path):
        raise _runtime_contract.InstalledRuntimeError(
            "installed native-link manifest is not adjacent to its archive"
        )
    facts = validate_native_link_dependency_manifest(
        cast(Mapping[str, object], payload),
        runtime_identity=artifact_content_identity(archive.path, observed=archive),
        context=str(receipt.path),
        target_triple=cell.target_triple,
        cargo_profile=cast(str, cell.key["cargo_profile"]),
    )
    _require_features(cell, facts.build_identity)
    if cell.target_triple is None:
        host = RUST_TARGET_BY_COORDINATE.get(current_host_coordinate())
        if facts.build_identity.effective_target != host:
            raise _runtime_contract.InstalledRuntimeError(
                f"installed native runtime targets {facts.build_identity.effective_target}, not this host ({host})"
            )
    custody = facts.custody
    custody_archive = native_link_custody_archive_path(archive.path, custody)
    declared = cell.file_name("native_link_custody_archive")
    if (custody_archive.name if custody_archive is not None else None) != declared:
        raise _runtime_contract.InstalledRuntimeError(
            "installed custody archive differs from its native-link manifest"
        )
    verify_stable_regular_file_identity(archive, label="shipped native archive")
    verify_stable_regular_file_identity(receipt, label="shipped native receipt")
    custody_identity = (
        cell.bundle_file("native_link_custody_archive")
        if declared is not None
        else None
    )
    return facts, receipt, custody_identity


def _retained_callable_projection(
    cell: InstalledRuntimeCell, runtime_lib: Path
) -> Path:
    """The signed projection's coordinate beside one retained or staged archive."""
    return runtime_lib.with_name(
        cast(str, cell.file_record(NATIVE_CALLABLE_PROJECTION_ROLE)["name"])
    )


def _admit_retained_native(
    cell: InstalledRuntimeCell,
    runtime_lib: Path,
    source_archive: StableRegularFileIdentity,
    source_receipt: StableRegularFileIdentity,
    source_projection: StableRegularFileIdentity,
    facts: NativeLinkManifestFacts,
    callable_semantic_digest: str,
    custody_admission: NativeLinkCustodyAdmission | None = None,
) -> _runtime_contract.InstalledNativeAdmission:
    archive = cell.verify_retained_file("runtime_archive", runtime_lib)
    receipt = cell.verify_retained_file(
        "native_link_manifest", native_link_dependency_manifest_path(runtime_lib)
    )
    projection = cell.verify_retained_file(
        NATIVE_CALLABLE_PROJECTION_ROLE,
        _retained_callable_projection(cell, runtime_lib),
    )
    for actual, source in (
        (archive, source_archive),
        (receipt, source_receipt),
        (projection, source_projection),
    ):
        if (actual.sha256, actual.size) != (source.sha256, source.size):
            raise _runtime_contract.InstalledRuntimeError(
                "retained native generation differs from observed shipped semantics"
            )
    custody = (
        observe_native_link_custody(runtime_lib, facts.custody)
        if custody_admission is None
        else custody_admission.borrow(runtime_lib, facts.custody)
    )
    admission = _runtime_contract.InstalledNativeAdmission(
        cell.id,
        runtime_lib,
        facts.build_identity,
        archive,
        receipt,
        projection,
        facts,
        custody,
        callable_semantic_digest,
    )
    admission.verify()
    return admission


def _retain_native(
    cell: InstalledRuntimeCell,
    archive: StableRegularFileIdentity,
    manifest: StableRegularFileIdentity,
    projection: StableRegularFileIdentity,
    facts: NativeLinkManifestFacts,
    callable_semantic_digest: str,
    custody_identity: StableRegularFileIdentity | None,
) -> _runtime_contract.InstalledNativeAdmission:
    root = cell.retained_root
    runtime_lib = root / archive.path.name

    def admit(
        path: Path, custody_admission: NativeLinkCustodyAdmission | None = None
    ) -> _runtime_contract.InstalledNativeAdmission:
        return _admit_retained_native(
            cell,
            path,
            archive,
            manifest,
            projection,
            facts,
            callable_semantic_digest,
            custody_admission,
        )

    if not root.exists() and not is_link_like(root):
        root.parent.mkdir(parents=True, exist_ok=True)
        stage = root.parent / f".{cell.id}.{uuid.uuid4().hex}.stage"
        stage.mkdir()
        try:
            staged_lib = stage / archive.path.name
            _atomic_copy_file(
                archive.path,
                staged_lib,
                observed=archive,
            )
            _atomic_copy_file(
                manifest.path,
                native_link_dependency_manifest_path(staged_lib),
                observed=manifest,
            )
            _atomic_copy_file(
                projection.path,
                _retained_callable_projection(cell, staged_lib),
                observed=projection,
            )
            with copy_native_link_custody_archive(
                archive.path,
                staged_lib,
                facts.custody,
                source_identity=custody_identity,
            ) as custody_admission:
                staged = admit(staged_lib, custody_admission)
            return publish_native_runtime_directory(
                stage,
                root,
                verify_staged=staged.verify,
                admit=lambda directory: admit(directory / archive.path.name),
            )
        finally:
            if stage.exists():
                with contextlib.suppress(OSError):
                    _remove_file_or_tree(stage)
    return admit(runtime_lib)


def admit_installed_native_runtime(
    cell: InstalledRuntimeCell,
) -> _runtime_contract.InstalledNativeAdmission:
    """Admit shipped bytes, retain one generation, and fence its exact members."""
    try:
        archive = cell.bundle_file("runtime_archive")
        facts, manifest, custody_identity = _shipped_native_identity(cell, archive)
        projection = _admit_shipped_callable_projection(cell, archive)
        return _retain_native(
            cell,
            archive,
            manifest,
            projection.identity,
            facts,
            projection.semantic_digest,
            custody_identity,
        )
    except _runtime_contract.InstalledRuntimeError:
        raise
    except _ADMISSION_ERRORS as exc:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed native runtime cell {cell.id} failed admission: {exc}"
        ) from exc


def installed_native_runtime_identity(
    cell: InstalledRuntimeCell, runtime_lib: Path
) -> RuntimeBuildIdentity:
    """Cold re-admission for consumers that hold no operation admission.

    A build operation reuses its ``InstalledNativeAdmission`` instead. This
    path re-hashes the shipped and retained members and never reselects.
    """
    try:
        if runtime_lib != cell.runtime_lib:
            raise _runtime_contract.InstalledRuntimeError(
                "native runtime path is not this installed cell's retained generation"
            )
        if not cell.retained_root.exists():
            raise _runtime_contract.InstalledRuntimeError(
                "installed native retained generation is missing"
            )
        return admit_installed_native_runtime(cell).build_identity
    except _runtime_contract.InstalledRuntimeError:
        raise
    except _ADMISSION_ERRORS as exc:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed native runtime cell {cell.id} failed re-admission: {exc}"
        ) from exc


def reuse_installed_native_admission(
    cell: InstalledRuntimeCell, admission: _runtime_contract.InstalledNativeAdmission
) -> None:
    """Reuse this operation's admission for the cell the request still selects.

    The caller re-derives selection and policy for the current request. A
    different cell, path or build identity is a changed selection, never a
    reason to admit again. Member bytes are not re-read: the fences prove the
    admitted generations are still the files on disk.
    """
    try:
        if (
            cell.kind != NATIVE_RUNTIME_CELL
            or cell.id != admission.cell_id
            or cell.runtime_lib != admission.runtime_lib
        ):
            raise _runtime_contract.InstalledRuntimeError(
                "installed native runtime selection changed after admission"
            )
        for role, identity in admission.members():
            cell.require_signed_member(role, identity)
        admission.verify()
    except _runtime_contract.InstalledRuntimeError:
        raise
    except _ADMISSION_ERRORS as exc:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed native runtime cell {cell.id} changed after admission: {exc}"
        ) from exc


def installed_native_callable_projection(
    cell: InstalledRuntimeCell, admission: _runtime_contract.InstalledNativeAdmission
) -> tuple[Path, str]:
    """Name the callable projection this operation admitted beside its archive.

    Its bytes were hashed against the signed cell record at admission and are
    fenced here. Canonical contents and the archive binding in its content
    address belong to the one projection authority, ``runtime_callable_symbols``.
    """
    reuse_installed_native_admission(cell, admission)
    record = cell.file_record(NATIVE_CALLABLE_PROJECTION_ROLE)
    return admission.callable_projection.path, cast(str, record["sha256"])


# --- WASM ----------------------------------------------------------------------


def _require_wasm_semantics(
    cell: InstalledRuntimeCell, identity: RuntimeBuildIdentity
) -> None:
    config = _common_config(identity)
    if (
        config["target_triple"] != WASM_RUNTIME_TARGET
        or config["cargo_profile"] != cell.key["cargo_profile"]
    ):
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} WASM target/profile differs from its key"
        )
    _require_features(cell, identity)
    flags = config.get("base_rustflags")
    if not isinstance(flags, Sequence) or wasm_runtime_simd_enabled(
        cast(Sequence[str], flags)
    ) != bool(cell.key["simd"]):
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} SIMD code generation differs from its key"
        )
    scripts = cast(Mapping[str, object], config["build_script_environment"])
    exports = scripts.get("MOLT_WASM_CPYTHON_ABI_EXPORTS")
    if not isinstance(exports, Sequence) or list(exports) != list(
        wasm_cpython_abi_distribution_export_names()
    ):
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} does not carry the distributed "
            "CPython C-API export surface"
        )


def _shipped_wasm_identities(
    cell: InstalledRuntimeCell, value: object
) -> tuple[RuntimeBuildIdentity, RuntimeBuildIdentity]:
    payload = string_keyed_mapping(value)
    receipts = string_keyed_mapping(payload.get("receipts")) if payload else None
    if receipts is None or set(receipts) != {"shared", "reloc"}:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} WASM generation receipts are invalid"
        )
    shared = RuntimeBuildIdentity.from_dict(
        cast(Mapping[str, object], receipts["shared"])["identity"]
    )
    reloc = RuntimeBuildIdentity.from_dict(
        cast(Mapping[str, object], receipts["reloc"])["identity"]
    )
    if (
        shared.payload.get("member_kind") != "shared"
        or reloc.payload.get("member_kind") != "reloc"
        or shared.family_digest != reloc.family_digest
    ):
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} WASM receipts are not one pair"
        )
    _require_wasm_semantics(cell, shared)
    _require_wasm_semantics(cell, reloc)
    return shared, reloc


def _require_link_features(
    cell: InstalledRuntimeCell, required_link_features: Collection[str]
) -> None:
    missing = sorted(
        set(required_link_features).difference(cell.key["runtime_features"])
    )
    if missing:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed WASM runtime cell {cell.id} lacks required runtime "
            f"features: {', '.join(missing)}"
        )


def _retained_wasm_member(
    cell: InstalledRuntimeCell, role: str, identity: StableRegularFileIdentity
) -> StableRegularFileIdentity:
    """Admit one member the generation reader hashed; never read it twice."""
    # Reject a link or junction anywhere on the retained path, as hashing did.
    resolve_owned_path(identity.path)
    if identity.path.parent != cell.retained_root:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed runtime cell {cell.id} {role} is not its retained "
            f"generation: {identity.path}"
        )
    return cell.require_signed_member(role, identity)


def admit_installed_wasm_runtime(
    cell: InstalledRuntimeCell,
    *,
    required_link_features: Collection[str] = (),
) -> RuntimeWasmGeneration:
    """Admit the shipped pair and hydrate one retained immutable generation."""
    try:
        manifest, payload = cell.bundle_json("wasm_generation_manifest")
        payload = _freeze_json(payload)
        shared_identity, reloc_identity = _shipped_wasm_identities(cell, payload)
        _require_link_features(cell, required_link_features)
        descriptors = _generation_record_descriptors(
            manifest.path,
            payload,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        if descriptors is None:
            raise _runtime_contract.InstalledRuntimeError(
                f"installed runtime cell {cell.id} WASM generation is invalid"
            )
        for role, (path, digest, size) in zip(
            ("wasm_shared_member", "wasm_reloc_member"), descriptors, strict=True
        ):
            record = cell.file_record(role)
            expected = resolve_owned_path(cell.members_root / record["name"])
            selected = stable_regular_file_version(
                expected, label=f"installed runtime {role}"
            )
            if (
                path.absolute() != expected
                or (digest, size) != (record["sha256"], record["size"])
                or selected.size != size
            ):
                raise _runtime_contract.InstalledRuntimeError(
                    f"installed runtime cell {cell.id} WASM generation does not name "
                    "its signed shipped members"
                )
        root = cell.retained_root
        dest_shared = root / _SHARED_RUNTIME_NAME
        dest_reloc = root / _RELOC_RUNTIME_NAME
        retained = read_runtime_wasm_generation(
            runtime_wasm_generation_path(dest_shared),
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        if retained is None:
            source = _validate_generation_payload(
                manifest.path,
                payload,
                expected_shared_identity=shared_identity,
                expected_reloc_identity=reloc_identity,
                receipt_identity=manifest,
            )
            if source is None:
                raise _runtime_contract.InstalledRuntimeError(
                    f"installed runtime cell {cell.id} WASM generation is invalid"
                )
            for role, member in (
                ("wasm_shared_member", source.shared_member_identity),
                ("wasm_reloc_member", source.reloc_member_identity),
            ):
                expected = resolve_owned_path(
                    cell.members_root / cell.file_record(role)["name"]
                )
                if member.path != expected:
                    raise _runtime_contract.InstalledRuntimeError(
                        f"installed runtime cell {cell.id} WASM generation does not "
                        "name its shipped members"
                    )
                cell.require_signed_member(role, member)
            root.mkdir(parents=True, exist_ok=True)
            retained = hydrate_runtime_wasm_generation(
                source_manifest=manifest.path,
                dest_shared=dest_shared,
                dest_reloc=dest_reloc,
                expected_shared_identity=shared_identity,
                expected_reloc_identity=reloc_identity,
                source_generation=source,
            )
        # The generation reader hashed both members; admit those identities.
        for role, member in (
            ("wasm_shared_member", retained.shared_member_identity),
            ("wasm_reloc_member", retained.reloc_member_identity),
        ):
            _retained_wasm_member(cell, role, member)
        return retained
    except _runtime_contract.InstalledRuntimeError:
        raise
    except _ADMISSION_ERRORS as exc:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed WASM runtime cell {cell.id} failed admission: {exc}"
        ) from exc


def reuse_installed_wasm_generation(
    cell: InstalledRuntimeCell,
    generation: RuntimeWasmGeneration,
    *,
    required_link_features: Collection[str] = (),
) -> None:
    """Reuse the retained pair this operation admitted and bound for codegen.

    The caller re-derives selection for the current request; required features
    are checked against that cell. Member bytes are not re-read: they were
    hashed against the signed record at admission, and their stable-file
    fences prove they are still the files the pinned receipt names.
    """
    try:
        _require_link_features(cell, required_link_features)
        for role, member in (
            ("wasm_shared_member", generation.shared_member_identity),
            ("wasm_reloc_member", generation.reloc_member_identity),
        ):
            _retained_wasm_member(cell, role, member)
            verify_stable_regular_file_identity(
                member, label=f"admitted installed runtime {role}"
            )
    except _runtime_contract.InstalledRuntimeError:
        raise
    except _ADMISSION_ERRORS as exc:
        raise _runtime_contract.InstalledRuntimeError(
            f"installed WASM runtime cell {cell.id} changed after admission: {exc}"
        ) from exc


def _admit_shipped_callable_projection(
    cell: InstalledRuntimeCell, archive: StableRegularFileIdentity
):
    """Admit a staged or shipped projection with the codegen projection authority."""
    # runtime_callable_symbols imports runtime_native_build, which imports this
    # module, so the one projection authority is bound when a receipt is admitted.
    from molt.cli.runtime_callable_symbols import _admit_runtime_callable_projection

    captured = cell.bundle_projection()
    return _admit_runtime_callable_projection(
        captured[0].path,
        runtime_lib=archive.path,
        archive_identity=archive,
        captured=captured,
        expected_sha256=cast(
            str, cell.file_record(NATIVE_CALLABLE_PROJECTION_ROLE)["sha256"]
        ),
    )


def admit_runtime_cell_receipts(
    record: Mapping[str, Any], members_root: Path
) -> tuple[RuntimeBuildIdentity, ...]:
    """Admit one staged or shipped cell's canonical receipts against its key.

    Release production and bundle assembly use the installed admission rules
    (content, receipt parsers, key agreement) without retention, then bind the
    returned identities' sources to the release source tree.
    """
    cell = InstalledRuntimeCell(None, record, members_root)
    try:
        if cell.kind == NATIVE_RUNTIME_CELL:
            archive = cell.bundle_file("runtime_archive")
            facts, _receipt, _custody = _shipped_native_identity(cell, archive)
            _admit_shipped_callable_projection(cell, archive)
            return (facts.build_identity,)
        return _shipped_wasm_identities(
            cell, _freeze_json(cell.bundle_json("wasm_generation_manifest")[1])
        )
    except _runtime_contract.InstalledRuntimeError:
        raise
    except _ADMISSION_ERRORS as exc:
        raise _runtime_contract.InstalledRuntimeError(
            f"runtime cell {cell.id} receipts failed admission: {exc}"
        ) from exc

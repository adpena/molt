"""Single source-extension receipt pipeline: structure first, admission second."""

from __future__ import annotations

import re
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Mapping, cast

from molt.cli.source_extension_set_identity import (
    SourceExtensionSetIdentity,
    ValidatedSourceExtensionIdentitySidecar,
    ValidatedSourceExtensionSetIdentityInputs,
    _source_extension_set_identity,
    _target_semantic_projection,
)
from molt.cli.source_extension_set_registry import (
    SourceExtensionRegistry,
    SourceExtensionSet,
    SourceExtensionVariant,
    load_source_extension_registry,
    require_registered_source_extension_set,
    source_extension_set_expected_identity,
)
from molt.cli.source_extension_set_validation_build import (
    validate_source_extension_build_custody,
)
from molt.cli.source_extension_set_validation_inventory import (
    validate_source_extension_installed_inventory,
)
from molt.cli.source_extension_set_validation_schema import (
    RecordedSourceExtensionSet,
    SourceExtensionSetValidationError as SourceExtensionSetValidationError,
    require_source_extension_set_registered_contract,
    validate_source_extension_set_manifest_schema,
)
from molt.cli.source_extension_set_validation_sidecars import (
    validate_source_extension_sidecars,
)
from molt.cli.source_extension_set_validation_target import (
    validate_source_extension_target_metadata,
)
from molt.cli.source_package_seal import SourcePackageSeal, verify_source_package_seal
from molt.exact_json import canonical_json_bytes, loads_exact
from molt.file_hashing import _sha256_bytes, _sha256_file


@dataclass(frozen=True, slots=True)
class ValidatedSourceExtensionSetPublishRoot:
    """Immutable snapshots from a complete structural payload validation."""

    recorded: RecordedSourceExtensionSet
    variant: SourceExtensionVariant
    set_manifest_json: bytes
    target_semantics_json: bytes
    installed_package_files: tuple[str, ...]
    sidecars: tuple[ValidatedSourceExtensionIdentitySidecar, ...]


@dataclass(frozen=True, slots=True)
class ValidatedSourceExtensionSetSeal:
    """Evidence for observed bytes, not a capability to mutate or publish them.

    Do not persist or reuse a receipt across an unverified filesystem boundary.
    A copied or recovered namespace must prove its seal bytes before rebinding.
    """

    seal: SourcePackageSeal
    validation: ValidatedSourceExtensionSetPublishRoot
    canonical_identity: SourceExtensionSetIdentity

    @property
    def payload_root(self) -> Path:
        return self.seal.payload_root

    @property
    def set_manifest_json(self) -> bytes:
        return self.validation.set_manifest_json

    def manifest_payload(self) -> dict[str, Any]:
        return cast(dict[str, Any], loads_exact(self.set_manifest_json.decode("utf-8")))

    def identity_payload(self) -> dict[str, Any]:
        return self.canonical_identity.digest_payload()


def _validate_source_extension_set_payload(
    publish_root: Path,
    set_manifest: Mapping[str, Any],
    *,
    inventory_sha256: Mapping[str, str] | None = None,
) -> ValidatedSourceExtensionSetPublishRoot:
    publish_root = publish_root.resolve()
    try:
        recorded, variant = validate_source_extension_set_manifest_schema(set_manifest)
        target = validate_source_extension_target_metadata(
            publish_root=publish_root,
            set_manifest=set_manifest,
            variant=variant,
        )
        validate_source_extension_build_custody(
            publish_root=publish_root,
            extension_set=recorded,
            set_manifest=set_manifest,
        )
        installed = validate_source_extension_installed_inventory(
            publish_root=publish_root,
            extension_set=recorded,
            set_manifest=set_manifest,
        )
        if inventory_sha256 is None:
            # Unsealed producer validation can observe installed support bytes,
            # but only seal admission grants a complete inventory receipt.
            inventory_sha256 = {
                relative: _sha256_file(publish_root / relative)
                for relative in installed
            }
        sidecars = validate_source_extension_sidecars(
            publish_root=publish_root,
            extension_set=recorded,
            variant=variant,
            set_manifest=set_manifest,
            target=target,
            inventory_sha256=inventory_sha256,
        )
        target_semantics = _target_semantic_projection(set_manifest)
    except (OSError, ValueError) as exc:
        if isinstance(exc, SourceExtensionSetValidationError):
            raise
        raise SourceExtensionSetValidationError(str(exc)) from exc
    return ValidatedSourceExtensionSetPublishRoot(
        recorded=recorded,
        variant=variant,
        set_manifest_json=canonical_json_bytes(set_manifest),
        target_semantics_json=canonical_json_bytes(target_semantics),
        installed_package_files=installed,
        sidecars=sidecars,
    )


def validate_source_extension_set_publish_root(
    *,
    publish_root: Path,
    extension_set: SourceExtensionSet,
    variant: SourceExtensionVariant,
    set_manifest: Mapping[str, Any],
) -> ValidatedSourceExtensionSetPublishRoot:
    """Validate an unsealed producer payload without granting seal custody."""

    validation = _validate_source_extension_set_payload(publish_root, set_manifest)
    require_source_extension_set_registered_contract(
        validation.recorded,
        validation.variant,
        validation.installed_package_files,
        extension_set,
        variant,
    )
    return validation


def validate_source_extension_set_seal_contents(
    root: Path,
) -> ValidatedSourceExtensionSetSeal:
    """Validate recorded structure independent of today's registry.

    Historical incumbents follow this same structural pipeline; their caller
    must pin the expected seal and canonical identity before replacement.
    """

    seal = verify_source_package_seal(root)
    inventory = {entry.relative_path: entry.sha256 for entry in seal.files}
    manifest_path = seal.payload_root / "extension_set_manifest.json"
    try:
        raw = manifest_path.read_bytes()
        if _sha256_bytes(raw) != inventory.get("extension_set_manifest.json"):
            raise SourceExtensionSetValidationError(
                "extension-set manifest bytes changed after seal verification"
            )
        payload = loads_exact(raw.decode("utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise SourceExtensionSetValidationError(
            f"cannot read source-extension set manifest {manifest_path}: {exc}"
        ) from exc
    if not isinstance(payload, Mapping):
        raise SourceExtensionSetValidationError(
            f"source-extension set manifest is not an object: {manifest_path}"
        )
    validation = _validate_source_extension_set_payload(
        seal.payload_root, payload, inventory_sha256=inventory
    )
    expected_artifacts = {entry.artifact_relative_path for entry in validation.sidecars}
    expected_sidecars = {entry.sidecar_relative_path for entry in validation.sidecars}
    actual_artifacts = {
        path for path in inventory if path.endswith((".molt.wasm", ".molt.a"))
    }
    actual_sidecars = {
        path
        for path in inventory
        if path.endswith(
            (".molt.wasm.extension_manifest.json", ".molt.a.extension_manifest.json")
        )
    }
    if expected_artifacts != actual_artifacts or expected_sidecars != actual_sidecars:
        raise SourceExtensionSetValidationError(
            "extension-set identity inventory differs from typed set"
        )
    for sidecar in validation.sidecars:
        if (
            inventory.get(sidecar.artifact_relative_path) != sidecar.artifact_sha256
            or inventory.get(sidecar.sidecar_relative_path) != sidecar.sidecar_sha256
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set artifact/sidecar bytes differ from verified inventory: {sidecar.module}"
            )
    if any(
        relative not in inventory for relative in validation.installed_package_files
    ):
        raise SourceExtensionSetValidationError(
            "extension-set installed files differ from verified inventory"
        )
    identity = _source_extension_set_identity(
        ValidatedSourceExtensionSetIdentityInputs(
            set_manifest_json=validation.set_manifest_json,
            target_semantics_json=validation.target_semantics_json,
            installed_package_files=validation.installed_package_files,
            sidecars=validation.sidecars,
            inventory=seal.files,
        )
    )
    # Receipt issuance is a custody boundary. Metadata, retained inputs and
    # installed files may have changed while their semantic consumers ran.
    # Recheck the complete seal, not merely the artifact and JSON sidecars.
    verified_seal = verify_source_package_seal(root, expected_sha256=seal.seal_sha256)
    if verified_seal.files != seal.files:
        raise SourceExtensionSetValidationError(
            "extension-set inventory changed during validation"
        )
    return ValidatedSourceExtensionSetSeal(
        seal=verified_seal,
        validation=validation,
        canonical_identity=identity,
    )


def require_source_extension_set_receipt_identity(
    receipt: ValidatedSourceExtensionSetSeal,
    expected_sha256: str,
) -> ValidatedSourceExtensionSetSeal:
    """Compare a pinned identity without re-reading or revalidating payloads."""

    if re.fullmatch(r"[0-9a-f]{64}", expected_sha256) is None:
        raise SourceExtensionSetValidationError(
            "expected source-extension identity must be lowercase SHA-256"
        )
    actual = receipt.canonical_identity.canonical_sha256
    if actual != expected_sha256:
        raise SourceExtensionSetValidationError(
            f"source-extension canonical identity mismatch: expected {expected_sha256}, got {actual}"
        )
    return receipt


def rebind_source_extension_set_receipt(
    receipt: ValidatedSourceExtensionSetSeal,
    verified_seal: SourcePackageSeal,
) -> ValidatedSourceExtensionSetSeal:
    """Relocate facts only after the new namespace proves identical seal bytes."""

    if (
        verified_seal.seal_sha256 != receipt.seal.seal_sha256
        or verified_seal.files != receipt.seal.files
    ):
        raise SourceExtensionSetValidationError(
            "cannot rebind source-extension receipt to different seal bytes"
        )
    return replace(receipt, seal=verified_seal)


def validate_source_extension_set_candidate_seal(
    root: Path,
    extension_set: SourceExtensionSet,
    *,
    variant: SourceExtensionVariant,
    registry: SourceExtensionRegistry | None = None,
) -> ValidatedSourceExtensionSetSeal:
    """Apply current package/build policy without requiring a registered cell.

    Detached candidates establish new supported variant identities before
    registration, so the explicit variant need not appear in registry.variants.
    Registered admission/promotion additionally requires the variant expectation
    through validate_source_extension_set_seal.
    """

    registered = require_registered_source_extension_set(
        extension_set, registry=registry
    )
    receipt = validate_source_extension_set_seal_contents(root)
    validation = receipt.validation
    require_source_extension_set_registered_contract(
        validation.recorded,
        validation.variant,
        validation.installed_package_files,
        registered,
        variant,
    )
    return receipt


def validate_source_extension_set_seal(
    root: Path,
    extension_set: SourceExtensionSet,
    *,
    variant: SourceExtensionVariant,
    registry: SourceExtensionRegistry | None = None,
) -> ValidatedSourceExtensionSetSeal:
    """Admit the same structural receipt by the current registered identity."""

    selected = load_source_extension_registry() if registry is None else registry
    receipt = validate_source_extension_set_candidate_seal(
        root,
        extension_set,
        variant=variant,
        registry=selected,
    )
    return require_source_extension_set_receipt_identity(
        receipt,
        source_extension_set_expected_identity(
            extension_set, variant=variant, registry=selected
        ),
    )

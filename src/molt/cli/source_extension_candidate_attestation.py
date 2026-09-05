"""Detached candidate custody for source-extension package-set admission."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from molt.exact_json import write_exact
from molt.file_hashing import _sha256_file
from molt.file_publication import is_link_like, resolve_owned_path
from molt.cli.source_extension_candidate_transaction import (
    CANDIDATE_ATTESTATION_NAME,
    CANDIDATE_SEAL_DIRECTORY,
    CANDIDATE_BUNDLE_DIRECTORY,
    SourceExtensionCandidateTransactionCustody,
    require_source_extension_candidate_transaction_custody,
    commit_source_extension_candidate_transaction,
    complete_source_extension_candidate_transaction,
)
from molt.cli.source_extension_set_registry import (
    SourceExtensionRegistry,
    SourceExtensionSet,
    SourceExtensionVariant,
    require_registered_source_extension_set,
    source_extension_custody_root,
)
from molt.cli.source_extension_set_validation import (
    ValidatedSourceExtensionSetSeal,
    rebind_source_extension_set_receipt,
)
from molt.cli.source_package_seal import (
    SealFileInventoryEntry,
    SourcePackageSeal,
    _copy_seal_candidate,
    verify_source_package_seal,
)

_CANDIDATE_ATTESTATION_SCHEMA_VERSION = 1
_CANDIDATE_VALIDATION_CONTRACT = {
    "kind": "molt-source-extension-set-validation",
    "schema_version": 1,
}
_SHA256_RE = re.compile(r"\A[0-9a-f]{64}\Z")


class SourceExtensionCandidateAttestationError(ValueError):
    """A detached candidate output violates candidate-only custody."""


@dataclass(frozen=True, slots=True)
class SourceExtensionCandidateAttestation:
    """A validated candidate that has no publication capability."""

    root: Path
    report_path: Path
    seal: SourcePackageSeal
    canonical_identity: Mapping[str, Any]
    registered_identity_sha256: str | None


def resolve_source_extension_candidate_custody_path(raw: str | Path) -> Path:
    """Resolve candidate-only ownership without asserting pre-recovery absence."""
    custody_root = source_extension_custody_root().resolve()
    candidates_root = (custody_root / "package-candidates").resolve()
    try:
        output = resolve_owned_path(Path(raw))
    except ValueError as exc:
        raise SourceExtensionCandidateAttestationError(str(exc)) from exc
    if output == candidates_root or not output.is_relative_to(candidates_root):
        raise SourceExtensionCandidateAttestationError(
            "source-extension candidate output must be a child of canonical "
            f"candidate custody {candidates_root}: {output}"
        )
    package_seals_root = (custody_root / "package-seals").resolve()
    if output == package_seals_root or output.is_relative_to(package_seals_root):
        raise SourceExtensionCandidateAttestationError(
            "source-extension candidate output cannot enter canonical package-seal "
            f"custody: {output}"
        )
    return output


def resolve_source_extension_candidate_output(raw: str | Path) -> Path:
    """Resolve a new output below candidate custody and outside package seals."""

    output = resolve_source_extension_candidate_custody_path(raw)
    if output.exists() or is_link_like(output):
        raise SourceExtensionCandidateAttestationError(
            f"source-extension candidate output already exists: {output}"
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    return output


def resolve_existing_source_extension_candidate_output(raw: str | Path) -> Path:
    """Resolve an existing detached bundle without granting publication rights."""

    output = resolve_source_extension_candidate_custody_path(raw)
    if not output.is_dir() or is_link_like(output):
        raise SourceExtensionCandidateAttestationError(
            f"source-extension candidate bundle is not a real directory: {output}"
        )
    return output


def _inventory_payload(
    inventory: tuple[SealFileInventoryEntry, ...],
) -> list[dict[str, object]]:
    return [
        {
            "path": entry.relative_path,
            "role": entry.role,
            "sha256": entry.sha256,
            "size": entry.size,
        }
        for entry in inventory
    ]


def _exact_mapping(
    value: object,
    *,
    keys: set[str],
    field: str,
) -> Mapping[str, Any]:
    if not isinstance(value, Mapping) or set(value) != keys:
        raise SourceExtensionCandidateAttestationError(
            f"candidate attestation {field} has an invalid schema"
        )
    result: dict[str, Any] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            raise SourceExtensionCandidateAttestationError(
                f"candidate attestation {field} has a non-string key"
            )
        result[key] = item
    return result


def validate_source_extension_candidate_attestation_payload(
    payload: object,
) -> Mapping[str, Any]:
    """Validate the exact stable data contract for one candidate report."""

    report = _exact_mapping(
        payload,
        keys={
            "schema_version",
            "kind",
            "status",
            "package_set",
            "variant",
            "candidate_seal",
            "canonical_identity",
            "validation",
            "registry_admission",
            "publication",
        },
        field="root",
    )
    if (
        type(report.get("schema_version")) is not int
        or report.get("schema_version") != _CANDIDATE_ATTESTATION_SCHEMA_VERSION
        or report.get("kind") != "molt-source-extension-set-candidate-attestation"
        or report.get("status") != "validated"
    ):
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation has an unsupported contract"
        )
    package_set = _exact_mapping(
        report.get("package_set"),
        keys={"package", "package_version", "name", "seal_name"},
        field="package_set",
    )
    variant = _exact_mapping(
        report.get("variant"),
        keys={"cpython", "abi_tier", "target_triple"},
        field="variant",
    )
    for field, value in (*package_set.items(), *variant.items()):
        if not isinstance(value, str) or not value:
            raise SourceExtensionCandidateAttestationError(
                f"candidate attestation requires a non-empty {field} string"
            )
    candidate_seal = _exact_mapping(
        report.get("candidate_seal"),
        keys={"root", "seal_sha256", "inventory"},
        field="candidate_seal",
    )
    if (
        candidate_seal.get("root") != CANDIDATE_SEAL_DIRECTORY
        or not isinstance(candidate_seal.get("seal_sha256"), str)
        or _SHA256_RE.fullmatch(str(candidate_seal["seal_sha256"])) is None
    ):
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation has an invalid candidate seal identity"
        )
    inventory = candidate_seal.get("inventory")
    if not isinstance(inventory, list) or not inventory:
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation seal inventory must be non-empty"
        )
    inventory_paths: list[str] = []
    for index, raw_entry in enumerate(inventory):
        entry = _exact_mapping(
            raw_entry,
            keys={"path", "role", "sha256", "size"},
            field=f"candidate_seal.inventory[{index}]",
        )
        path = entry.get("path")
        role = entry.get("role")
        sha256 = entry.get("sha256")
        size = entry.get("size")
        if (
            not isinstance(path, str)
            or not path
            or not isinstance(role, str)
            or not role
            or not isinstance(sha256, str)
            or _SHA256_RE.fullmatch(sha256) is None
            or not isinstance(size, int)
            or isinstance(size, bool)
            or size < 0
        ):
            raise SourceExtensionCandidateAttestationError(
                f"candidate attestation inventory entry {index} is invalid"
            )
        inventory_paths.append(path)
    if inventory_paths != sorted(set(inventory_paths)):
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation seal inventory is not canonical"
        )
    canonical_identity = _exact_mapping(
        report.get("canonical_identity"),
        keys={
            "schema_version",
            "target_semantic_sha256",
            "content_sha256",
            "canonical_sha256",
            "producer_attestation_sha256",
        },
        field="canonical_identity",
    )
    if (
        type(canonical_identity.get("schema_version")) is not int
        or canonical_identity.get("schema_version") != 1
        or not all(
            isinstance(canonical_identity.get(field), str)
            and _SHA256_RE.fullmatch(str(canonical_identity[field])) is not None
            for field in (
                "target_semantic_sha256",
                "content_sha256",
                "canonical_sha256",
                "producer_attestation_sha256",
            )
        )
    ):
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation canonical identity is invalid"
        )
    validation = _exact_mapping(
        report.get("validation"),
        keys={"kind", "schema_version", "result"},
        field="validation",
    )
    if type(
        validation.get("schema_version")
    ) is not int or validation != _CANDIDATE_VALIDATION_CONTRACT | {"result": "passed"}:
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation validator contract is invalid"
        )
    registry_admission = _exact_mapping(
        report.get("registry_admission"),
        keys={"required_identity_sha256"},
        field="registry_admission",
    )
    if registry_admission.get("required_identity_sha256") != canonical_identity.get(
        "canonical_sha256"
    ):
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation registry identity differs from canonical identity"
        )
    publication = _exact_mapping(
        report.get("publication"),
        keys={"performed", "publication_custody_acquired"},
        field="publication",
    )
    if any(value is not False for value in publication.values()):
        raise SourceExtensionCandidateAttestationError(
            "candidate attestation contains publication authority"
        )
    return report


def _registered_variant_identity(
    extension_set: SourceExtensionSet,
    variant: SourceExtensionVariant,
    *,
    registry: SourceExtensionRegistry,
) -> str | None:
    registered = require_registered_source_extension_set(
        extension_set,
        registry=registry,
    )
    for expectation in registered.variants:
        if expectation.variant == variant:
            return expectation.expected_identity_sha256
    return None


def source_extension_candidate_attestation_payload(
    *,
    validated_candidate: ValidatedSourceExtensionSetSeal,
    extension_set: SourceExtensionSet,
    variant: SourceExtensionVariant,
) -> dict[str, object]:
    """Return the location-neutral, recomputable candidate report."""

    candidate_identity = validated_candidate.identity_payload()
    candidate_identity_sha256 = str(candidate_identity["canonical_sha256"])
    return {
        "schema_version": _CANDIDATE_ATTESTATION_SCHEMA_VERSION,
        "kind": "molt-source-extension-set-candidate-attestation",
        "status": "validated",
        "package_set": {
            "package": extension_set.package,
            "package_version": extension_set.package_version,
            "name": extension_set.name,
            "seal_name": extension_set.seal_name,
        },
        "variant": {
            "cpython": variant.cpython,
            "abi_tier": variant.abi_tier,
            "target_triple": variant.target_triple,
        },
        "candidate_seal": {
            "root": CANDIDATE_SEAL_DIRECTORY,
            "seal_sha256": validated_candidate.seal.seal_sha256,
            "inventory": _inventory_payload(validated_candidate.seal.files),
        },
        "canonical_identity": candidate_identity,
        "validation": _CANDIDATE_VALIDATION_CONTRACT | {"result": "passed"},
        "registry_admission": {
            "required_identity_sha256": candidate_identity_sha256,
        },
        "publication": {
            "performed": False,
            "publication_custody_acquired": False,
        },
    }


def finalize_source_extension_candidate_attestation(
    *,
    transaction_root: Path,
    output: Path,
    custody: SourceExtensionCandidateTransactionCustody,
    validated_candidate: ValidatedSourceExtensionSetSeal,
    extension_set: SourceExtensionSet,
    variant: SourceExtensionVariant,
    registry: SourceExtensionRegistry,
) -> SourceExtensionCandidateAttestation:
    """Validate and atomically expose a detached, non-publishing candidate.

    This module intentionally has no dependency on source-extension publication
    custody.  It can copy a verified seal into candidate custody and report the
    exact identity needed for registry admission, but it cannot prepare or
    commit a canonical package seal.
    """

    require_source_extension_candidate_transaction_custody(custody)
    if is_link_like(transaction_root):
        raise SourceExtensionCandidateAttestationError(
            "candidate transaction root is indirect"
        )
    transaction_root = resolve_owned_path(transaction_root)
    output = resolve_source_extension_candidate_output(output)
    if output != custody.candidate_output:
        raise SourceExtensionCandidateAttestationError(
            "candidate finalization crosses exact output custody"
        )
    if transaction_root.parent != output.parent:
        raise SourceExtensionCandidateAttestationError(
            "candidate transaction and output must share one parent for atomic "
            "finalization"
        )
    candidate_identity = validated_candidate.identity_payload()
    registered_identity_sha256 = _registered_variant_identity(
        extension_set,
        variant,
        registry=registry,
    )

    bundle = transaction_root / CANDIDATE_BUNDLE_DIRECTORY
    bundle.mkdir()
    detached_seal_root = bundle / CANDIDATE_SEAL_DIRECTORY
    _copy_seal_candidate(
        validated_candidate.seal.root,
        detached_seal_root,
        validated_candidate.seal.seal_sha256,
    )
    detached_seal = verify_source_package_seal(
        detached_seal_root,
        expected_sha256=validated_candidate.seal.seal_sha256,
    )
    report = source_extension_candidate_attestation_payload(
        validated_candidate=rebind_source_extension_set_receipt(
            validated_candidate, detached_seal
        ),
        extension_set=extension_set,
        variant=variant,
    )
    validate_source_extension_candidate_attestation_payload(report)
    write_exact(
        bundle / CANDIDATE_ATTESTATION_NAME,
        report,
    )
    commit_source_extension_candidate_transaction(
        transaction_root,
        custody=custody,
        seal_sha256=detached_seal.seal_sha256,
        report_sha256=_sha256_file(bundle / CANDIDATE_ATTESTATION_NAME),
    )
    final_seal = verify_source_package_seal(
        output / CANDIDATE_SEAL_DIRECTORY,
        expected_sha256=detached_seal.seal_sha256,
    )
    complete_source_extension_candidate_transaction(transaction_root, custody=custody)
    return SourceExtensionCandidateAttestation(
        root=output,
        report_path=output / CANDIDATE_ATTESTATION_NAME,
        seal=final_seal,
        canonical_identity=candidate_identity,
        registered_identity_sha256=registered_identity_sha256,
    )

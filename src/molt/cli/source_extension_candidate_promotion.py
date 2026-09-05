"""Registry-gated promotion of an already sealed source-extension candidate."""

from __future__ import annotations

import tempfile
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from molt.file_publication import durable_remove_path, is_link_like
from molt.cli.build_locks import _acquire_file_lock, _release_file_lock
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.source_extension_candidate_attestation import (
    CANDIDATE_ATTESTATION_NAME,
    CANDIDATE_SEAL_DIRECTORY,
    SourceExtensionCandidateAttestationError,
    resolve_existing_source_extension_candidate_output,
    source_extension_candidate_attestation_payload,
    validate_source_extension_candidate_attestation_payload,
)
from molt.cli.source_extension_publication import (
    _source_extension_publication_custody,
    publish_source_extension_candidate,
    recover_and_prune_source_extension_transactions,
)
from molt.cli.source_extension_set_registry import (
    SourceExtensionVariant,
    load_source_extension_registry,
    source_extension_set,
    source_extension_set_expected_identity,
    source_extension_set_root,
)
from molt.cli.source_extension_set_validation import (
    validate_source_extension_set_seal,
    rebind_source_extension_set_receipt,
)
from molt.cli.source_package_seal import (
    SourcePackageSealError,
    SourcePackageSealVerificationError,
    commit_source_package_seal,
    prepare_source_package_seal_commit,
    verify_source_package_seal,
)
from molt.exact_json import loads_exact
from molt.target_python import _parse_target_python_version


class SourceExtensionCandidatePromotionError(ValueError):
    """A detached candidate cannot be admitted to canonical custody."""


def _load_candidate_report(root: Path) -> dict[str, Any]:
    report_path = root / CANDIDATE_ATTESTATION_NAME
    if not report_path.is_file() or is_link_like(report_path):
        raise SourceExtensionCandidatePromotionError(
            f"source-extension candidate attestation is not a regular file: {report_path}"
        )
    try:
        payload = loads_exact(report_path.read_text(encoding="utf-8", errors="strict"))
    except (OSError, ValueError) as exc:
        raise SourceExtensionCandidatePromotionError(
            f"cannot read source-extension candidate attestation {report_path}: {exc}"
        ) from exc
    if not isinstance(payload, dict):
        raise SourceExtensionCandidatePromotionError(
            f"source-extension candidate attestation is not an object: {report_path}"
        )
    validate_source_extension_candidate_attestation_payload(payload)
    return payload


def _report_string(report: Mapping[str, Any], section: str, field: str) -> str:
    container = report.get(section)
    value = container.get(field) if isinstance(container, Mapping) else None
    if not isinstance(value, str) or not value:
        raise SourceExtensionCandidatePromotionError(
            f"candidate attestation requires {section}.{field}"
        )
    return value


def publish_source_extension_set_candidate(
    *,
    candidate: str,
    expected_incumbent_seal_sha256: str | None = None,
    expected_incumbent_identity_sha256: str | None = None,
    json_output: bool = False,
) -> int:
    """Promote one exact candidate; never build or recompute package content."""

    transaction_root: Path | None = None
    producer_lock = None
    try:
        candidate_root = resolve_existing_source_extension_candidate_output(candidate)
        if {path.name for path in candidate_root.iterdir()} != {
            CANDIDATE_SEAL_DIRECTORY,
            CANDIDATE_ATTESTATION_NAME,
        }:
            raise SourceExtensionCandidatePromotionError(
                "candidate bundle must contain exactly candidate-seal and "
                f"{CANDIDATE_ATTESTATION_NAME}: {candidate_root}"
            )
        report = _load_candidate_report(candidate_root)
        package = _report_string(report, "package_set", "package")
        package_version = _report_string(report, "package_set", "package_version")
        module_set = _report_string(report, "package_set", "name")
        variant = SourceExtensionVariant(
            target_python=_parse_target_python_version(
                _report_string(report, "variant", "cpython")
            ),
            abi_tier=_report_string(report, "variant", "abi_tier"),
            target_triple=_report_string(report, "variant", "target_triple"),
        )
        registry = load_source_extension_registry()
        extension_set = source_extension_set(
            package,
            package_version,
            module_set,
            registry=registry,
        )
        expected_candidate_identity_sha256 = source_extension_set_expected_identity(
            extension_set,
            variant=variant,
            registry=registry,
        )
        validated_candidate = validate_source_extension_set_seal(
            candidate_root / CANDIDATE_SEAL_DIRECTORY,
            extension_set,
            variant=variant,
            registry=registry,
        )
        expected_report = source_extension_candidate_attestation_payload(
            validated_candidate=validated_candidate,
            extension_set=extension_set,
            variant=variant,
        )
        if report != expected_report:
            raise SourceExtensionCandidatePromotionError(
                "candidate attestation differs from the recomputed sealed-candidate "
                "report"
            )

        destination = source_extension_set_root(
            extension_set,
            variant=variant,
            registry=registry,
        )
        destination.parent.mkdir(parents=True, exist_ok=True)
        lock_path = destination.parent / f".{destination.name}.producer.lock"
        producer_lock = _acquire_file_lock(
            lock_path,
            timeout_s=300.0,
            timeout_message=(
                "timed out waiting for the canonical extension-set publication "
                f"lock {lock_path}; another publisher owns {destination}"
            ),
        )
        publication_custody = _source_extension_publication_custody(
            destination,
            producer_lock,
        )
        recover_and_prune_source_extension_transactions(
            destination,
            custody=publication_custody,
        )
        if destination.exists() and (
            expected_incumbent_identity_sha256 is None
            or expected_incumbent_seal_sha256 is None
        ):
            raise SourceExtensionCandidatePromotionError(
                "canonical extension seal already exists; promotion requires "
                "both --expected-incumbent-seal-sha256 and "
                "--expected-incumbent-identity-sha256 for compare-and-swap"
            )
        if not destination.exists() and (
            expected_incumbent_identity_sha256 is not None
            or expected_incumbent_seal_sha256 is not None
        ):
            raise SourceExtensionCandidatePromotionError(
                "incumbent seal/identity expectations require an incumbent canonical "
                "seal"
            )

        transaction_root = Path(
            tempfile.mkdtemp(
                prefix=f".{destination.name}.promote-",
                dir=destination.parent,
            )
        )
        no_op = False
        upgraded = False
        if destination.exists():
            assert expected_incumbent_identity_sha256 is not None
            assert expected_incumbent_seal_sha256 is not None
            publication = publish_source_extension_candidate(
                custody=publication_custody,
                destination=destination,
                candidate_receipt=validated_candidate,
                transaction_root=transaction_root,
                expected_incumbent_seal_sha256=expected_incumbent_seal_sha256,
                expected_incumbent_identity_sha256=(expected_incumbent_identity_sha256),
                expected_candidate_identity_sha256=(expected_candidate_identity_sha256),
            )
            no_op = bool(publication["no_op"])
            upgraded = bool(publication["upgraded"])
        else:
            package_store = transaction_root / "package-store"
            commit = prepare_source_package_seal_commit(
                package_store,
                validated_candidate.seal,
                destination,
            )
            commit_source_package_seal(commit)
        published_seal = verify_source_package_seal(
            destination,
            expected_sha256=validated_candidate.seal.seal_sha256,
        )
        rebind_source_extension_set_receipt(validated_candidate, published_seal)
        data = {
            "package": extension_set.package,
            "module_set": extension_set.name,
            "candidate": str(candidate_root),
            "root": str(destination),
            "module_root": str(published_seal.payload_root),
            "seal_sha256": published_seal.seal_sha256,
            "identity_sha256": expected_candidate_identity_sha256,
            "no_op": no_op,
            "upgraded": upgraded,
            "target": variant.target_triple,
            "abi_tier": variant.abi_tier,
        }
        try:
            durable_remove_path(transaction_root)
        except (OSError, ValueError) as exc:
            raise SourceExtensionCandidatePromotionError(
                f"candidate publication committed at {destination}; promotion transaction cleanup failed: {transaction_root}: {exc}"
            ) from exc
        transaction_root = None
        if json_output:
            _emit_json(
                _json_payload(
                    "extension-publish-set-candidate",
                    "ok",
                    data=data,
                ),
                json_output=True,
            )
        else:
            print(f"Published registered extension-set candidate: {destination}")
            print(f"Canonical identity: {expected_candidate_identity_sha256}")
        return 0
    except (
        OSError,
        RuntimeError,
        SourceExtensionCandidateAttestationError,
        SourceExtensionCandidatePromotionError,
        SourcePackageSealError,
        SourcePackageSealVerificationError,
        ValueError,
    ) as exc:
        detail = str(exc)
        if transaction_root is not None and transaction_root.exists():
            detail += f"; preserved promotion transaction: {transaction_root}"
        return _fail(
            detail,
            json_output,
            command="extension-publish-set-candidate",
        )
    finally:
        if producer_lock is not None:
            _release_file_lock(producer_lock)

"""Crash-recoverable compare-and-swap publication for extension package seals."""

from __future__ import annotations

import os
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
import threading
import warnings
from typing import Any

from molt.exact_json import loads_exact, write_exact
from molt.file_publication import (
    durable_namespace_publish_directory_exclusive,
    durable_publish_directory_exclusive,
    durable_remove_path,
    is_link_like,
    resolve_owned_path,
)
from molt.cli.build_locks import _FileLockHandle, _in_process_lock_key
from molt.cli.source_extension_set_validation import (
    ValidatedSourceExtensionSetSeal,
    rebind_source_extension_set_receipt,
    require_source_extension_set_receipt_identity,
    validate_source_extension_set_seal_contents,
)
from molt.cli.source_package_seal import (
    SourcePackageSeal,
    SourcePackageSealVerificationError,
    _copy_seal_candidate,
    recover_source_package_seal_commits,
    verify_source_package_seal,
)

_PUBLICATION_RECORD_KEYS = {
    "schema_version",
    "kind",
    "state",
    "destination",
    "candidate",
    "retired",
    "quarantined_candidate",
    "quarantined_destination",
    "incumbent_seal_sha256",
    "candidate_seal_sha256",
    "incumbent_identity_sha256",
    "candidate_identity_sha256",
}
_PUBLICATION_STATES = {
    "prepared",
    "retired",
    "published",
    "committed",
    "aborted-restored",
}
_SHA256_RE = re.compile(r"\A[0-9a-f]{64}\Z")


@dataclass(frozen=True, slots=True)
class SourceExtensionPublicationCustody:
    """Capability proving exclusive custody of one canonical destination."""

    destination: Path
    lock_path: Path
    lock_handle: _FileLockHandle
    owner_process_id: int
    owner_thread_id: int


def _source_extension_publication_custody(
    destination: Path, lock_handle: _FileLockHandle
) -> SourceExtensionPublicationCustody:
    """Bind a live producer lock to its only authorized publication target."""

    resolved = resolve_owned_path(destination)
    lock_path = resolved.parent / f".{resolved.name}.producer.lock"
    if (
        lock_handle.file.closed
        or lock_handle.registry_key != _in_process_lock_key(lock_path)
        or not lock_handle.entry.mutex.locked()
    ):
        raise SourcePackageSealVerificationError(
            f"publication custody does not own the producer lock {lock_path}"
        )
    return SourceExtensionPublicationCustody(
        destination=resolved,
        lock_path=lock_path,
        lock_handle=lock_handle,
        owner_process_id=os.getpid(),
        owner_thread_id=threading.get_ident(),
    )


def _require_publication_custody(
    custody: SourceExtensionPublicationCustody, destination: Path
) -> None:
    resolved = resolve_owned_path(destination)
    if (
        not isinstance(custody, SourceExtensionPublicationCustody)
        or custody.destination != resolved
        or custody.lock_path != resolved.parent / f".{resolved.name}.producer.lock"
        or custody.owner_process_id != os.getpid()
        or custody.owner_thread_id != threading.get_ident()
        or custody.lock_handle.file.closed
        or custody.lock_handle.registry_key != _in_process_lock_key(custody.lock_path)
        or not custody.lock_handle.entry.mutex.locked()
    ):
        raise SourcePackageSealVerificationError(
            f"publication requires live exclusive producer-lock custody for {resolved}"
        )


class _PublicationVerifier:
    """Reuse semantic receipts while rechecking bytes at each namespace boundary."""

    def __init__(self, receipts: tuple[ValidatedSourceExtensionSetSeal, ...] = ()):
        self.receipts = {receipt.seal.seal_sha256: receipt for receipt in receipts}

    def verified_at(
        self, path: Path, seal_sha256: str, identity_sha256: str
    ) -> SourcePackageSeal:
        receipt = self.receipts.get(seal_sha256)
        if receipt is None:
            receipt = validate_source_extension_set_seal_contents(path)
            self.receipts[receipt.seal.seal_sha256] = receipt
            if receipt.seal.seal_sha256 != seal_sha256:
                raise SourcePackageSealVerificationError(
                    f"publication seal differs from expected {seal_sha256}: {path}"
                )
        else:
            receipt = rebind_source_extension_set_receipt(
                receipt, verify_source_package_seal(path, expected_sha256=seal_sha256)
            )
        require_source_extension_set_receipt_identity(receipt, identity_sha256)
        return receipt.seal

    def matches(self, path: Path, seal_sha256: str, identity_sha256: str) -> bool:
        try:
            self.verified_at(path, seal_sha256, identity_sha256)
        except (OSError, ValueError):
            return False
        return True


def _record_path(transaction_root: Path) -> Path:
    return transaction_root / "identity-publication.json"


def _write_record(path: Path, record: Mapping[str, Any], state: str) -> dict[str, Any]:
    updated = dict(record)
    updated["state"] = state
    write_exact(path, updated)
    return updated


def _load_record(path: Path) -> dict[str, Any]:
    if is_link_like(path) or is_link_like(path.parent):
        raise SourcePackageSealVerificationError(
            f"publication journal has indirect custody: {path}"
        )
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise SourcePackageSealVerificationError(
            f"cannot recover extension identity publication {path}: {exc}"
        ) from exc
    transaction_root = resolve_owned_path(path.parent)
    if not isinstance(payload, dict) or set(payload) != _PUBLICATION_RECORD_KEYS:
        raise SourcePackageSealVerificationError(
            f"extension identity publication record is invalid: {path}"
        )
    if (
        type(payload.get("schema_version")) is not int
        or payload.get("schema_version") != 2
        or payload.get("kind") != "source-extension-seal-compare-and-swap"
        or not isinstance(payload.get("state"), str)
        or payload["state"] not in _PUBLICATION_STATES
        or not all(
            isinstance(payload.get(field), str)
            and _SHA256_RE.fullmatch(payload[field]) is not None
            for field in (
                "incumbent_seal_sha256",
                "candidate_seal_sha256",
                "incumbent_identity_sha256",
                "candidate_identity_sha256",
            )
        )
    ):
        raise SourcePackageSealVerificationError(
            f"extension identity publication record is invalid: {path}"
        )
    destination = payload.get("destination")
    candidate = payload.get("candidate")
    retired = payload.get("retired")
    quarantined_candidate = payload.get("quarantined_candidate")
    quarantined_destination = payload.get("quarantined_destination")

    def canonical_absolute(raw: object) -> bool:
        if not isinstance(raw, str) or not raw:
            return False
        try:
            candidate_path = Path(raw)
            return candidate_path.is_absolute() and str(candidate_path.resolve()) == raw
        except (OSError, ValueError):
            return False

    if not all(
        canonical_absolute(raw)
        for raw in (
            destination,
            candidate,
            retired,
            quarantined_candidate,
            quarantined_destination,
        )
    ):
        raise SourcePackageSealVerificationError(
            f"extension identity publication paths are not canonical: {path}"
        )
    publication_root = transaction_root / "identity-publication"
    if (
        Path(candidate) != publication_root / "candidate"
        or Path(retired) != publication_root / "retired"
        or Path(quarantined_candidate) != publication_root / "quarantined-candidate"
        or Path(quarantined_destination) != publication_root / "quarantined-destination"
    ):
        raise SourcePackageSealVerificationError(
            f"extension identity publication paths escape transaction custody: {path}"
        )
    return payload


def _quarantine_tree(source: Path, quarantine: Path) -> None:
    if not source.exists() and not is_link_like(source):
        return
    if quarantine.exists():
        raise SourcePackageSealVerificationError(
            f"publication quarantine already exists: {quarantine}"
        )
    durable_namespace_publish_directory_exclusive(source, quarantine)


def _abort_and_restore_source_extension_publication(
    *,
    verifier: _PublicationVerifier,
    record_path: Path,
    record: Mapping[str, Any],
    destination: Path,
    candidate: Path,
    retired: Path,
    quarantined_candidate: Path,
    quarantined_destination: Path,
) -> dict[str, Any]:
    """Preserve failed candidate custody and restore the exact incumbent."""

    incumbent_seal_sha256 = str(record["incumbent_seal_sha256"])
    incumbent_identity_sha256 = str(record["incumbent_identity_sha256"])
    if destination.exists() and not verifier.matches(
        destination,
        incumbent_seal_sha256,
        incumbent_identity_sha256,
    ):
        _quarantine_tree(destination, quarantined_destination)
    if candidate.exists():
        _quarantine_tree(candidate, quarantined_candidate)
    if not destination.exists():
        if not retired.exists():
            raise SourcePackageSealVerificationError(
                "publication failure lost both canonical and retired incumbent custody"
            )
        verifier.verified_at(retired, incumbent_seal_sha256, incumbent_identity_sha256)
        durable_publish_directory_exclusive(retired, destination)
    verifier.verified_at(destination, incumbent_seal_sha256, incumbent_identity_sha256)
    if candidate.exists():
        raise SourcePackageSealVerificationError(
            "publication abort retained a candidate outside quarantine custody"
        )
    return _write_record(record_path, record, "aborted-restored")


def _resume_source_extension_publication(
    record_path: Path,
    custody: SourceExtensionPublicationCustody,
    *,
    verifier: _PublicationVerifier | None = None,
) -> dict[str, Any]:
    if verifier is None:
        verifier = _PublicationVerifier()
    record = _load_record(record_path)
    transaction_root = resolve_owned_path(record_path.parent)
    destination = Path(str(record["destination"])).resolve()
    candidate = Path(str(record["candidate"])).resolve()
    retired = Path(str(record["retired"])).resolve()
    quarantined_candidate = Path(str(record["quarantined_candidate"])).resolve()
    quarantined_destination = Path(str(record["quarantined_destination"])).resolve()
    _require_publication_custody(custody, destination)
    publication_root = transaction_root / "identity-publication"
    if (
        candidate != publication_root / "candidate"
        or retired != publication_root / "retired"
        or quarantined_candidate != publication_root / "quarantined-candidate"
        or quarantined_destination != publication_root / "quarantined-destination"
    ):
        raise SourcePackageSealVerificationError(
            "identity publication record escapes its transaction custody"
        )
    incumbent_seal_sha256 = str(record["incumbent_seal_sha256"])
    candidate_seal_sha256 = str(record["candidate_seal_sha256"])
    incumbent_identity_sha256 = str(record["incumbent_identity_sha256"])
    candidate_identity_sha256 = str(record["candidate_identity_sha256"])

    if record["state"] == "aborted-restored":
        # This receipt records a past restoration, not perpetual ownership of
        # the destination. Later successful publications may supersede it.
        return record
    if record["state"] == "committed":
        verifier.verified_at(
            destination, candidate_seal_sha256, candidate_identity_sha256
        )
        if retired.exists():
            verifier.verified_at(
                retired, incumbent_seal_sha256, incumbent_identity_sha256
            )
        return record

    # A crash can follow the quarantine rename but precede the abort journal.
    # Resume restoration, never try to publish the missing quarantined candidate.
    if quarantined_candidate.exists() or quarantined_destination.exists():
        return _abort_and_restore_source_extension_publication(
            verifier=verifier,
            record_path=record_path,
            record=record,
            destination=destination,
            candidate=candidate,
            retired=retired,
            quarantined_candidate=quarantined_candidate,
            quarantined_destination=quarantined_destination,
        )

    retirement_started = retired.exists()
    try:
        if destination.exists() and verifier.matches(
            destination,
            candidate_seal_sha256,
            candidate_identity_sha256,
        ):
            if not retired.exists():
                raise SourcePackageSealVerificationError(
                    "published candidate has no retained incumbent rollback custody"
                )
            verifier.verified_at(
                retired, incumbent_seal_sha256, incumbent_identity_sha256
            )
            return _write_record(record_path, record, "committed")

        if retired.exists():
            verifier.verified_at(
                retired, incumbent_seal_sha256, incumbent_identity_sha256
            )
        elif destination.exists():
            verifier.verified_at(
                destination,
                incumbent_seal_sha256,
                incumbent_identity_sha256,
            )
            durable_publish_directory_exclusive(destination, retired)
            retirement_started = True
            verifier.verified_at(
                retired, incumbent_seal_sha256, incumbent_identity_sha256
            )
            record = _write_record(record_path, record, "retired")
        else:
            raise SourcePackageSealVerificationError(
                "identity publication lost both incumbent and retired custody"
            )

        if not candidate.exists():
            raise SourcePackageSealVerificationError(
                "identity publication candidate is missing"
            )
        verifier.verified_at(
            candidate, candidate_seal_sha256, candidate_identity_sha256
        )
        if destination.exists():
            raise SourcePackageSealVerificationError(
                "identity publication destination changed during compare-and-swap"
            )
        durable_publish_directory_exclusive(candidate, destination)
        record = _write_record(record_path, record, "published")
        verifier.verified_at(
            destination, candidate_seal_sha256, candidate_identity_sha256
        )
        verifier.verified_at(retired, incumbent_seal_sha256, incumbent_identity_sha256)
        return _write_record(record_path, record, "committed")
    except BaseException as failure:
        if retirement_started or retired.exists():
            try:
                _abort_and_restore_source_extension_publication(
                    verifier=verifier,
                    record_path=record_path,
                    record=record,
                    destination=destination,
                    candidate=candidate,
                    retired=retired,
                    quarantined_candidate=quarantined_candidate,
                    quarantined_destination=quarantined_destination,
                )
            except BaseException as restoration_failure:
                raise SourcePackageSealVerificationError(
                    "source-extension publication failed after incumbent retirement "
                    f"({failure!r}) and rollback restoration also failed "
                    f"({restoration_failure!r}); preserved journal: {record_path}"
                ) from restoration_failure
        raise failure


def publish_source_extension_candidate(
    *,
    custody: SourceExtensionPublicationCustody,
    destination: Path,
    candidate_receipt: ValidatedSourceExtensionSetSeal,
    transaction_root: Path,
    expected_incumbent_seal_sha256: str,
    expected_incumbent_identity_sha256: str,
    expected_candidate_identity_sha256: str,
) -> dict[str, Any]:
    """Publish a candidate only when both sides match declared CAS identities."""

    destination = resolve_owned_path(destination)
    transaction_root = resolve_owned_path(transaction_root)
    _require_publication_custody(custody, destination)
    incumbent_receipt = require_source_extension_set_receipt_identity(
        validate_source_extension_set_seal_contents(destination),
        expected_incumbent_identity_sha256,
    )
    incumbent = incumbent_receipt.seal
    if incumbent.seal_sha256 != expected_incumbent_seal_sha256:
        raise SourcePackageSealVerificationError(
            "incumbent seal differs from expected compare-and-swap seal: "
            f"expected {expected_incumbent_seal_sha256}, got {incumbent.seal_sha256}"
        )
    candidate_receipt = rebind_source_extension_set_receipt(
        candidate_receipt,
        verify_source_package_seal(
            candidate_receipt.seal.root,
            expected_sha256=candidate_receipt.seal.seal_sha256,
        ),
    )
    require_source_extension_set_receipt_identity(
        candidate_receipt, expected_candidate_identity_sha256
    )
    verified_candidate = candidate_receipt.seal
    if (
        incumbent_receipt.canonical_identity.canonical_sha256
        == candidate_receipt.canonical_identity.canonical_sha256
        and incumbent.seal_sha256 == verified_candidate.seal_sha256
    ):
        return {
            "state": "committed",
            "no_op": True,
            "upgraded": False,
            "incumbent_seal_sha256": incumbent.seal_sha256,
            "candidate_seal_sha256": verified_candidate.seal_sha256,
            "identity_sha256": expected_candidate_identity_sha256,
        }

    publication_root = transaction_root / "identity-publication"
    candidate = publication_root / "candidate"
    retired = publication_root / "retired"
    publication_root.mkdir(parents=True, exist_ok=False)
    _copy_seal_candidate(
        verified_candidate.root, candidate, verified_candidate.seal_sha256
    )
    path = _record_path(transaction_root)
    record = {
        "schema_version": 2,
        "kind": "source-extension-seal-compare-and-swap",
        "state": "prepared",
        "destination": str(destination),
        "candidate": str(candidate),
        "retired": str(retired),
        "quarantined_candidate": str(publication_root / "quarantined-candidate"),
        "quarantined_destination": str(publication_root / "quarantined-destination"),
        "incumbent_seal_sha256": incumbent.seal_sha256,
        "candidate_seal_sha256": verified_candidate.seal_sha256,
        "incumbent_identity_sha256": expected_incumbent_identity_sha256,
        "candidate_identity_sha256": expected_candidate_identity_sha256,
    }
    write_exact(path, record, exclusive=True)
    result = _resume_source_extension_publication(
        path,
        custody,
        verifier=_PublicationVerifier((incumbent_receipt, candidate_receipt)),
    )
    return result | {
        "no_op": False,
        "upgraded": True,
        "identity_sha256": expected_candidate_identity_sha256,
    }


def recover_source_extension_publication(
    transaction_root: Path,
    *,
    custody: SourceExtensionPublicationCustody,
) -> dict[str, Any] | None:
    _require_publication_custody(custody, custody.destination)
    if is_link_like(transaction_root):
        raise SourcePackageSealVerificationError(
            f"publication transaction has indirect custody: {transaction_root}"
        )
    path = _record_path(resolve_owned_path(transaction_root))
    if is_link_like(path) or (path.exists() and not path.is_file()):
        raise SourcePackageSealVerificationError(f"invalid publication journal: {path}")
    return (
        _resume_source_extension_publication(path, custody) if path.exists() else None
    )


def recover_and_prune_source_extension_transactions(
    destination: Path,
    *,
    custody: SourceExtensionPublicationCustody,
) -> None:
    """Recover canonical publication journals, then prune abandoned scratch."""

    destination = resolve_owned_path(destination)
    _require_publication_custody(custody, destination)
    for operation in ("produce", "promote"):
        pattern = f".{destination.name}.{operation}-*"
        for prior in sorted(destination.parent.glob(pattern)):
            if is_link_like(prior) or not prior.is_dir():
                raise SourcePackageSealVerificationError(
                    f"transaction recovery refuses indirect or non-directory custody: {prior}"
                )
            recovered_publication = recover_source_extension_publication(
                prior,
                custody=custody,
            )
            if (
                recovered_publication is not None
                and recovered_publication.get("state") == "aborted-restored"
            ):
                warnings.warn(
                    f"retained aborted publication receipt; preserved evidence: {prior}",
                    RuntimeWarning,
                    stacklevel=2,
                )
                continue
            if (
                recovered_publication is not None
                and recovered_publication.get("state") != "committed"
            ):
                raise SourcePackageSealVerificationError(
                    f"identity publication recovery did not commit: {prior}"
                )
            recovered_seals = recover_source_package_seal_commits(
                prior / "package-store",
                expected_destination=destination,
            )
            retired = prior / "retired-destination"
            if retired.exists():
                raise SourcePackageSealVerificationError(
                    "legacy source-extension transaction contains a retired "
                    "canonical destination and requires manual custody review: "
                    f"{retired}"
                )
            if recovered_publication is None and not recovered_seals:
                warnings.warn(
                    f"preserved extension transaction without a completed publication "
                    f"receipt; review required: {prior}",
                    RuntimeWarning,
                    stacklevel=2,
                )
                continue
            durable_remove_path(prior)

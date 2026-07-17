"""Crash-recoverable compare-and-swap publication for extension package seals."""

from __future__ import annotations

import json
import os
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
import threading
from typing import Any

from molt.cli.atomic_io import _atomic_write_json, _remove_file_or_tree
from molt.cli.build_locks import _FileLockHandle, _in_process_lock_key
from molt.cli.source_extension_set_identity import (
    _require_expected_source_extension_set_identity,
)
from molt.cli.source_package_seal import (
    SourcePackageSeal,
    SourcePackageSealVerificationError,
    _copy_seal_candidate,
    verify_source_package_seal,
)

_PUBLICATION_RECORD_KEYS = {
    "schema_version",
    "kind",
    "state",
    "destination",
    "candidate",
    "retired",
    "incumbent_seal_sha256",
    "candidate_seal_sha256",
    "incumbent_identity_sha256",
    "candidate_identity_sha256",
}
_PUBLICATION_STATES = {"prepared", "retired", "published", "committed"}
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

    resolved = destination.resolve()
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
    resolved = destination.resolve()
    if (
        not isinstance(custody, SourceExtensionPublicationCustody)
        or custody.destination != resolved
        or custody.owner_process_id != os.getpid()
        or custody.owner_thread_id != threading.get_ident()
        or custody.lock_handle.file.closed
        or custody.lock_handle.registry_key != _in_process_lock_key(custody.lock_path)
        or not custody.lock_handle.entry.mutex.locked()
    ):
        raise SourcePackageSealVerificationError(
            f"publication requires live exclusive producer-lock custody for {resolved}"
        )


def _inventory(seal: SourcePackageSeal) -> dict[str, str]:
    return {entry.relative_path: entry.sha256 for entry in seal.files}


def _identity(seal: SourcePackageSeal, expected: str) -> dict[str, Any]:
    return _require_expected_source_extension_set_identity(
        seal.payload_root,
        expected,
        inventory_sha256=_inventory(seal),
    )


def _record_path(transaction_root: Path) -> Path:
    return transaction_root / "identity-publication.json"


def _write_record(path: Path, record: Mapping[str, Any], state: str) -> dict[str, Any]:
    updated = dict(record)
    updated["state"] = state
    _atomic_write_json(path, updated, sort_keys=True, indent=2)
    return updated


def _load_record(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourcePackageSealVerificationError(
            f"cannot recover extension identity publication {path}: {exc}"
        ) from exc
    transaction_root = path.parent.resolve()
    if not isinstance(payload, dict) or set(payload) != _PUBLICATION_RECORD_KEYS:
        raise SourcePackageSealVerificationError(
            f"extension identity publication record is invalid: {path}"
        )
    if (
        payload.get("schema_version") != 1
        or payload.get("kind") != "source-extension-seal-compare-and-swap"
        or payload.get("state") not in _PUBLICATION_STATES
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

    def canonical_absolute(raw: object) -> bool:
        if not isinstance(raw, str) or not raw:
            return False
        try:
            candidate_path = Path(raw)
            return candidate_path.is_absolute() and str(candidate_path.resolve()) == raw
        except (OSError, ValueError):
            return False

    if not all(canonical_absolute(raw) for raw in (destination, candidate, retired)):
        raise SourcePackageSealVerificationError(
            f"extension identity publication paths are not canonical: {path}"
        )
    publication_root = transaction_root / "identity-publication"
    if (
        Path(candidate) != publication_root / "candidate"
        or Path(retired) != publication_root / "retired"
    ):
        raise SourcePackageSealVerificationError(
            f"extension identity publication paths escape transaction custody: {path}"
        )
    return payload


def _verified_at(
    path: Path, seal_sha256: str, identity_sha256: str
) -> SourcePackageSeal:
    seal = verify_source_package_seal(path, expected_sha256=seal_sha256)
    _identity(seal, identity_sha256)
    return seal


def _resume_source_extension_publication(
    record_path: Path, custody: SourceExtensionPublicationCustody
) -> dict[str, Any]:
    record = _load_record(record_path)
    transaction_root = record_path.parent.resolve()
    destination = Path(str(record["destination"])).resolve()
    candidate = Path(str(record["candidate"])).resolve()
    retired = Path(str(record["retired"])).resolve()
    _require_publication_custody(custody, destination)
    publication_root = transaction_root / "identity-publication"
    if (
        candidate != publication_root / "candidate"
        or retired != publication_root / "retired"
    ):
        raise SourcePackageSealVerificationError(
            "identity publication record escapes its transaction custody"
        )
    incumbent_seal_sha256 = str(record["incumbent_seal_sha256"])
    candidate_seal_sha256 = str(record["candidate_seal_sha256"])
    incumbent_identity_sha256 = str(record["incumbent_identity_sha256"])
    candidate_identity_sha256 = str(record["candidate_identity_sha256"])

    if destination.exists():
        try:
            _verified_at(destination, candidate_seal_sha256, candidate_identity_sha256)
        except (SourcePackageSealVerificationError, ValueError):
            _verified_at(destination, incumbent_seal_sha256, incumbent_identity_sha256)
        else:
            if retired.exists():
                _verified_at(retired, incumbent_seal_sha256, incumbent_identity_sha256)
                _remove_file_or_tree(retired)
            if candidate.exists():
                _verified_at(
                    candidate, candidate_seal_sha256, candidate_identity_sha256
                )
                _remove_file_or_tree(candidate)
            return _write_record(record_path, record, "committed")

    if retired.exists():
        _verified_at(retired, incumbent_seal_sha256, incumbent_identity_sha256)
    elif destination.exists():
        os.replace(destination, retired)
        _verified_at(retired, incumbent_seal_sha256, incumbent_identity_sha256)
        record = _write_record(record_path, record, "retired")
    else:
        raise SourcePackageSealVerificationError(
            "identity publication lost both incumbent and retired custody"
        )

    if not candidate.exists():
        if not destination.exists():
            os.replace(retired, destination)
        raise SourcePackageSealVerificationError(
            "identity publication candidate is missing; incumbent restored"
        )
    _verified_at(candidate, candidate_seal_sha256, candidate_identity_sha256)
    if destination.exists():
        raise SourcePackageSealVerificationError(
            "identity publication destination changed during compare-and-swap"
        )
    os.replace(candidate, destination)
    record = _write_record(record_path, record, "published")
    _verified_at(destination, candidate_seal_sha256, candidate_identity_sha256)
    _verified_at(retired, incumbent_seal_sha256, incumbent_identity_sha256)
    _remove_file_or_tree(retired)
    return _write_record(record_path, record, "committed")


def publish_source_extension_candidate(
    *,
    custody: SourceExtensionPublicationCustody,
    destination: Path,
    candidate_seal: SourcePackageSeal,
    transaction_root: Path,
    expected_incumbent_identity_sha256: str,
    expected_candidate_identity_sha256: str,
) -> dict[str, Any]:
    """Publish a candidate only when both sides match declared CAS identities."""

    destination = destination.resolve()
    transaction_root = transaction_root.resolve()
    _require_publication_custody(custody, destination)
    incumbent = verify_source_package_seal(destination)
    incumbent_identity = _identity(incumbent, expected_incumbent_identity_sha256)
    verified_candidate = verify_source_package_seal(
        candidate_seal.root, expected_sha256=candidate_seal.seal_sha256
    )
    candidate_identity = _identity(
        verified_candidate, expected_candidate_identity_sha256
    )
    if incumbent_identity["canonical_sha256"] == candidate_identity["canonical_sha256"]:
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
    publication_root.mkdir(parents=True, exist_ok=True)
    _copy_seal_candidate(
        verified_candidate.root, candidate, verified_candidate.seal_sha256
    )
    path = _record_path(transaction_root)
    record = {
        "schema_version": 1,
        "kind": "source-extension-seal-compare-and-swap",
        "state": "prepared",
        "destination": str(destination),
        "candidate": str(candidate),
        "retired": str(retired),
        "incumbent_seal_sha256": incumbent.seal_sha256,
        "candidate_seal_sha256": verified_candidate.seal_sha256,
        "incumbent_identity_sha256": expected_incumbent_identity_sha256,
        "candidate_identity_sha256": expected_candidate_identity_sha256,
    }
    _atomic_write_json(path, record, sort_keys=True, indent=2)
    result = _resume_source_extension_publication(path, custody)
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
    path = _record_path(transaction_root.resolve())
    return (
        _resume_source_extension_publication(path, custody) if path.is_file() else None
    )

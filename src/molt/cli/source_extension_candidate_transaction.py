"""Custodied lifecycle for failed source-extension candidate build evidence."""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path
import threading
import time
from typing import Any

from molt.cli.build_locks import _FileLockHandle, _in_process_lock_key
from molt.cli.source_package_seal import verify_source_package_seal
from molt.exact_json import loads_exact, write_exact
from molt.file_hashing import _sha256_file
from molt.file_publication import (
    resolve_owned_path,
    durable_publish_directory_exclusive,
    durable_remove_path,
    is_link_like,
)


CANDIDATE_ATTESTATION_NAME = "candidate-attestation.json"
CANDIDATE_SEAL_DIRECTORY = "candidate-seal"
CANDIDATE_BUNDLE_DIRECTORY = "bundle"
_TRANSACTION_RECORD_NAME = "candidate-transaction.json"
_TRANSACTION_KIND = "molt-source-extension-candidate-transaction"
_TRANSACTION_STATES = frozenset(
    {"building", "prepared", "committed", "failed", "interrupted"}
)
_TRANSACTION_KEYS = frozenset(
    {
        "schema_version",
        "kind",
        "state",
        "candidate_output",
        "package",
        "package_version",
        "module_set",
        "cpython",
        "abi_tier",
        "target_triple",
        "created_at_ns",
        "updated_at_ns",
        "error",
        "seal_sha256",
        "report_sha256",
    }
)
_DEFAULT_FAILED_RETENTION_SECONDS = 7 * 24 * 60 * 60
_SHA256_RE = re.compile(r"\A[0-9a-f]{64}\Z")


class SourceExtensionCandidateTransactionError(ValueError):
    """Candidate transaction evidence is malformed or lacks lock custody."""


@dataclass(frozen=True, slots=True)
class SourceExtensionCandidateTransactionCustody:
    """Capability proving exclusive custody of one detached candidate name."""

    candidate_output: Path
    lock_path: Path
    lock_handle: _FileLockHandle
    owner_process_id: int
    owner_thread_id: int


def source_extension_candidate_transaction_custody(
    candidate_output: Path,
    lock_handle: _FileLockHandle,
) -> SourceExtensionCandidateTransactionCustody:
    if is_link_like(candidate_output):
        raise SourceExtensionCandidateTransactionError("candidate output is indirect")
    output = resolve_owned_path(candidate_output)
    lock_path = output.parent / f".{output.name}.candidate.lock"
    custody = SourceExtensionCandidateTransactionCustody(
        candidate_output=output,
        lock_path=lock_path,
        lock_handle=lock_handle,
        owner_process_id=os.getpid(),
        owner_thread_id=threading.get_ident(),
    )
    require_source_extension_candidate_transaction_custody(custody)
    return custody


def require_source_extension_candidate_transaction_custody(
    custody: SourceExtensionCandidateTransactionCustody,
) -> None:
    if (
        not isinstance(custody, SourceExtensionCandidateTransactionCustody)
        or custody.owner_process_id != os.getpid()
        or custody.owner_thread_id != threading.get_ident()
        or custody.lock_handle.file.closed
        or custody.lock_path
        != custody.candidate_output.parent
        / f".{custody.candidate_output.name}.candidate.lock"
        or custody.lock_handle.registry_key != _in_process_lock_key(custody.lock_path)
        or not custody.lock_handle.entry.mutex.locked()
    ):
        raise SourceExtensionCandidateTransactionError(
            "candidate transaction requires the live exclusive candidate lock"
        )


def _record_path(transaction_root: Path) -> Path:
    return transaction_root / _TRANSACTION_RECORD_NAME


def _write_record(
    transaction_root: Path, record: dict[str, Any], *, exclusive: bool = False
) -> None:
    write_exact(_record_path(transaction_root), record, exclusive=exclusive)


def _transaction_root(
    transaction_root: Path, custody: SourceExtensionCandidateTransactionCustody
) -> Path:
    require_source_extension_candidate_transaction_custody(custody)
    if is_link_like(transaction_root) or not transaction_root.is_dir():
        raise SourceExtensionCandidateTransactionError(
            f"candidate transaction root is not a real directory: {transaction_root}"
        )
    root = resolve_owned_path(transaction_root)
    if root.parent != custody.candidate_output.parent or not root.name.startswith(
        f".{custody.candidate_output.name}.attest-"
    ):
        raise SourceExtensionCandidateTransactionError(
            f"candidate transaction root escapes exact output custody: {root}"
        )
    return root


def _timestamp(now_ns: int | None) -> int:
    value = time.time_ns() if now_ns is None else now_ns
    if type(value) is not int or value <= 0:
        raise SourceExtensionCandidateTransactionError(
            "candidate timestamp must be a positive integer"
        )
    return value


def begin_source_extension_candidate_transaction(
    transaction_root: Path,
    *,
    custody: SourceExtensionCandidateTransactionCustody,
    package: str,
    package_version: str,
    module_set: str,
    cpython: str,
    abi_tier: str,
    target_triple: str,
    now_ns: int | None = None,
) -> None:
    transaction_root = _transaction_root(transaction_root, custody)
    if any(
        not isinstance(value, str) or not value
        for value in (
            package,
            package_version,
            module_set,
            cpython,
            abi_tier,
            target_triple,
        )
    ):
        raise SourceExtensionCandidateTransactionError(
            "candidate transaction facts must be non-empty strings"
        )
    timestamp = _timestamp(now_ns)
    _write_record(
        transaction_root,
        {
            "schema_version": 2,
            "kind": _TRANSACTION_KIND,
            "state": "building",
            "candidate_output": str(custody.candidate_output),
            "package": package,
            "package_version": package_version,
            "module_set": module_set,
            "cpython": cpython,
            "abi_tier": abi_tier,
            "target_triple": target_triple,
            "created_at_ns": timestamp,
            "updated_at_ns": timestamp,
            "error": None,
            "seal_sha256": None,
            "report_sha256": None,
        },
        exclusive=True,
    )


def _load_record(
    transaction_root: Path,
    *,
    custody: SourceExtensionCandidateTransactionCustody,
) -> dict[str, Any]:
    transaction_root = _transaction_root(transaction_root, custody)
    record_path = _record_path(transaction_root)
    if not record_path.is_file() or is_link_like(record_path):
        raise SourceExtensionCandidateTransactionError(
            f"candidate transaction has no regular journal: {record_path}"
        )
    try:
        value = loads_exact(record_path.read_text(encoding="utf-8", errors="strict"))
    except (OSError, ValueError) as exc:
        raise SourceExtensionCandidateTransactionError(
            f"cannot read candidate transaction journal {record_path}: {exc}"
        ) from exc
    if not isinstance(value, dict) or set(value) != _TRANSACTION_KEYS:
        raise SourceExtensionCandidateTransactionError(
            f"candidate transaction journal has an invalid schema: {record_path}"
        )
    if (
        type(value.get("schema_version")) is not int
        or value.get("schema_version") != 2
        or value.get("kind") != _TRANSACTION_KIND
        or not isinstance(value.get("state"), str)
        or value.get("state") not in _TRANSACTION_STATES
        or value.get("candidate_output") != str(custody.candidate_output)
        or type(value.get("created_at_ns")) is not int
        or type(value.get("updated_at_ns")) is not int
        or value["created_at_ns"] <= 0
        or value["updated_at_ns"] < value["created_at_ns"]
        or any(
            not isinstance(value.get(field), str) or not value[field]
            for field in (
                "package",
                "package_version",
                "module_set",
                "cpython",
                "abi_tier",
                "target_triple",
            )
        )
        or (
            value.get("error") is not None
            and (not isinstance(value["error"], str) or not value["error"])
        )
    ):
        raise SourceExtensionCandidateTransactionError(
            f"candidate transaction journal has invalid values: {record_path}"
        )
    prepared = value["state"] in {"prepared", "committed"}
    for field in ("seal_sha256", "report_sha256"):
        digest = value[field]
        if (
            prepared
            and (not isinstance(digest, str) or _SHA256_RE.fullmatch(digest) is None)
        ) or (not prepared and digest is not None):
            raise SourceExtensionCandidateTransactionError(
                f"candidate transaction {field} is invalid: {record_path}"
            )
    return value


def fail_source_extension_candidate_transaction(
    transaction_root: Path,
    *,
    custody: SourceExtensionCandidateTransactionCustody,
    error: str,
    now_ns: int | None = None,
) -> None:
    record = _load_record(transaction_root, custody=custody)
    if not isinstance(error, str) or not error:
        raise SourceExtensionCandidateTransactionError(
            "candidate failure requires a diagnostic"
        )
    if record["state"] not in {"prepared", "committed"}:
        record["state"] = "failed"
    record["updated_at_ns"] = max(_timestamp(now_ns), record["updated_at_ns"])
    record["error"] = error
    _write_record(transaction_root, record)


def _verify_bundle(root: Path, record: dict[str, Any]) -> None:
    if not root.is_dir() or is_link_like(root):
        raise SourceExtensionCandidateTransactionError(
            f"candidate bundle is not a real directory: {root}"
        )
    if {path.name for path in root.iterdir()} != {
        CANDIDATE_ATTESTATION_NAME,
        CANDIDATE_SEAL_DIRECTORY,
    }:
        raise SourceExtensionCandidateTransactionError(
            f"candidate bundle has unexpected members: {root}"
        )
    report = root / CANDIDATE_ATTESTATION_NAME
    if (
        not report.is_file()
        or is_link_like(report)
        or _sha256_file(report) != record["report_sha256"]
    ):
        raise SourceExtensionCandidateTransactionError(
            f"candidate report differs from prepared transaction: {report}"
        )
    verify_source_package_seal(
        root / CANDIDATE_SEAL_DIRECTORY, expected_sha256=record["seal_sha256"]
    )


def _commit_prepared(
    root: Path,
    record: dict[str, Any],
    custody: SourceExtensionCandidateTransactionCustody,
) -> None:
    require_source_extension_candidate_transaction_custody(custody)
    bundle = root / CANDIDATE_BUNDLE_DIRECTORY
    if bundle.exists() or is_link_like(bundle):
        _verify_bundle(bundle, record)
        durable_publish_directory_exclusive(bundle, custody.candidate_output)
    else:
        _verify_bundle(custody.candidate_output, record)
    record["state"] = "committed"
    record["updated_at_ns"] = max(time.time_ns(), record["updated_at_ns"])
    _write_record(root, record)


def commit_source_extension_candidate_transaction(
    transaction_root: Path,
    *,
    custody: SourceExtensionCandidateTransactionCustody,
    seal_sha256: str,
    report_sha256: str,
) -> None:
    root = _transaction_root(transaction_root, custody)
    record = _load_record(root, custody=custody)
    if record["state"] != "building":
        raise SourceExtensionCandidateTransactionError(
            f"candidate transaction cannot prepare from {record['state']}: {root}"
        )
    if any(
        not isinstance(value, str) or _SHA256_RE.fullmatch(value) is None
        for value in (seal_sha256, report_sha256)
    ):
        raise SourceExtensionCandidateTransactionError(
            "candidate commit requires exact seal and report digests"
        )
    record.update(
        state="prepared",
        seal_sha256=seal_sha256,
        report_sha256=report_sha256,
        updated_at_ns=max(time.time_ns(), record["updated_at_ns"]),
    )
    _verify_bundle(root / CANDIDATE_BUNDLE_DIRECTORY, record)
    _write_record(root, record)
    _commit_prepared(root, record, custody)


def complete_source_extension_candidate_transaction(
    transaction_root: Path,
    *,
    custody: SourceExtensionCandidateTransactionCustody,
) -> None:
    root = _transaction_root(transaction_root, custody)
    record = _load_record(root, custody=custody)
    if record["state"] != "committed":
        raise SourceExtensionCandidateTransactionError(
            f"candidate transaction is not committed: {root}"
        )
    _verify_bundle(custody.candidate_output, record)
    durable_remove_path(root)


def recover_and_prune_source_extension_candidate_transactions(
    *,
    custody: SourceExtensionCandidateTransactionCustody,
    now_ns: int | None = None,
    failed_retention_seconds: int = _DEFAULT_FAILED_RETENTION_SECONDS,
) -> tuple[Path, ...]:
    """Recover prepared commits and diagnose every ambiguous journal."""

    require_source_extension_candidate_transaction_custody(custody)
    if type(failed_retention_seconds) is not int or failed_retention_seconds < 0:
        raise SourceExtensionCandidateTransactionError(
            "candidate failed-transaction retention must be a nonnegative integer"
        )
    timestamp = _timestamp(now_ns)
    retention_ns = failed_retention_seconds * 1_000_000_000
    pruned: list[Path] = []
    prefix = f".{custody.candidate_output.name}.attest-"
    for transaction_root in sorted(custody.candidate_output.parent.iterdir()):
        if not transaction_root.name.startswith(prefix):
            continue
        record = _load_record(transaction_root, custody=custody)
        if record["state"] == "prepared":
            _commit_prepared(transaction_root, record, custody)
        if record["state"] == "committed":
            complete_source_extension_candidate_transaction(
                transaction_root, custody=custody
            )
            pruned.append(transaction_root)
            continue
        if record["state"] == "building":
            record["state"] = "interrupted"
            record["updated_at_ns"] = max(timestamp, record["updated_at_ns"])
            record["error"] = (
                "prior producer no longer owns the exclusive candidate lock"
            )
            _write_record(transaction_root, record)
        age_ns = timestamp - int(record["updated_at_ns"])
        if age_ns >= retention_ns:
            durable_remove_path(transaction_root)
            pruned.append(transaction_root)
    return tuple(pruned)

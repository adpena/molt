"""Deterministic, content-addressed source-package sealing.

This module owns the package-neutral storage and publication protocol for a
closed set of source and generated files.  Callers provide every input and its
root-relative POSIX destination explicitly; the seal never walks an upstream
tree, guesses relocation roots, or carries package-specific policy.

One transaction root contains the deduplicated blob store, immutable
digest-addressed seals, publication candidates, and durable commit records.
Publication is recoverable at both meaningful crash boundaries: a prepared
candidate can be committed later, and a destination renamed into place before
the record state was updated is recognized and completed idempotently.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import shutil
from typing import Iterable, cast
import uuid

from molt.exact_json import canonical_json_bytes, loads_exact
from molt.portable_paths import portable_relative_path, portable_path_identity
from molt.file_publication import (
    atomic_write_bytes,
    durable_publish_directory_exclusive,
    durable_publish_exclusive,
    durable_remove_path,
    is_link_like,
    resolve_owned_path,
)


_MANIFEST_SCHEMA = "molt.source-package-seal/1"
_COMMIT_SCHEMA = "molt.source-package-seal-commit/1"
_MANIFEST_NAME = "source-package-seal.json"
_PAYLOAD_DIRECTORY = "files"
_SHA256_RE = re.compile(r"\A[0-9a-f]{64}\Z")
_ROLE_RE = re.compile(r"\A[a-z][a-z0-9_.-]*\Z")
_COPY_CHUNK_BYTES = 1024 * 1024


class SourcePackageSealError(ValueError):
    """The requested seal or publication transaction is invalid."""


class SourcePackageSealVerificationError(SourcePackageSealError):
    """A seal, blob, or commit record failed closed verification."""


@dataclass(frozen=True, slots=True)
class SourcePackageInput:
    """One explicitly admitted file in a source-package seal."""

    source: Path
    relative_path: str
    role: str


@dataclass(frozen=True, slots=True)
class SealFileInventoryEntry:
    """Location-independent identity for one sealed file."""

    relative_path: str
    sha256: str
    size: int
    role: str


@dataclass(frozen=True, slots=True)
class SourcePackageSeal:
    """A verified immutable seal rooted at ``root``."""

    root: Path
    seal_sha256: str
    files: tuple[SealFileInventoryEntry, ...]

    @property
    def payload_root(self) -> Path:
        """Return the only module/file tree admitted by this verified seal."""
        return self.root / _PAYLOAD_DIRECTORY


@dataclass(frozen=True, slots=True)
class SourcePackageSealCommit:
    """Durable state for one immutable, digest-addressed publication."""

    transaction_root: Path
    record_path: Path
    commit_id: str
    seal_sha256: str
    candidate_root: Path
    destination: Path
    state: str


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha256_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(_COPY_CHUNK_BYTES):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def _copy_staged_file(source: Path, destination: Path) -> None:
    """Copy private staging bytes; the directory commit owns their one flush."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def _validate_sha256(value: object, *, field: str) -> str:
    if not isinstance(value, str) or _SHA256_RE.fullmatch(value) is None:
        raise SourcePackageSealVerificationError(
            f"{field} must be a lowercase sha256 hex digest"
        )
    return value


def validate_source_package_relative_path(value: object, *, field: str) -> str:
    try:
        return portable_relative_path(value).as_posix()
    except ValueError as exc:
        raise SourcePackageSealVerificationError(
            f"{field} must be a canonical portable root-relative POSIX path: {value!r}"
        ) from exc


def _validate_role(value: object) -> str:
    if not isinstance(value, str) or _ROLE_RE.fullmatch(value) is None:
        raise SourcePackageSealVerificationError(
            "file role must be a lowercase portable identifier"
        )
    return value


def _entry_payload(entry: SealFileInventoryEntry) -> dict[str, object]:
    return {
        "path": entry.relative_path,
        "role": entry.role,
        "sha256": entry.sha256,
        "size": entry.size,
    }


def _manifest_identity_payload(
    entries: tuple[SealFileInventoryEntry, ...],
) -> dict[str, object]:
    return {
        "files": [_entry_payload(entry) for entry in entries],
        "schema": _MANIFEST_SCHEMA,
    }


def _manifest_document(
    entries: tuple[SealFileInventoryEntry, ...], seal_sha256: str
) -> dict[str, object]:
    payload = _manifest_identity_payload(entries)
    payload["seal_sha256"] = seal_sha256
    return payload


def _manifest_bytes(
    entries: tuple[SealFileInventoryEntry, ...], seal_sha256: str
) -> bytes:
    return canonical_json_bytes(_manifest_document(entries, seal_sha256)) + b"\n"


def _parse_inventory(payload: object) -> tuple[SealFileInventoryEntry, ...]:
    if not isinstance(payload, list) or not payload:
        raise SourcePackageSealVerificationError(
            "seal manifest files must be a non-empty list"
        )
    entries: list[SealFileInventoryEntry] = []
    seen_paths: set[str] = set()
    seen_portable_paths: set[str] = set()
    for index, raw in enumerate(payload):
        if not isinstance(raw, dict) or set(raw) != {"path", "role", "sha256", "size"}:
            raise SourcePackageSealVerificationError(
                f"seal manifest files[{index}] has an invalid shape"
            )
        raw_entry = cast(dict[str, object], raw)
        relative_path = validate_source_package_relative_path(
            raw_entry["path"], field=f"files[{index}].path"
        )
        portable_path = portable_path_identity(relative_path)
        if relative_path in seen_paths or portable_path in seen_portable_paths:
            raise SourcePackageSealVerificationError(
                f"seal inventory has a duplicate or case-colliding path: {relative_path}"
            )
        seen_paths.add(relative_path)
        seen_portable_paths.add(portable_path)
        size = raw_entry["size"]
        if isinstance(size, bool) or not isinstance(size, int) or size < 0:
            raise SourcePackageSealVerificationError(
                f"files[{index}].size must be a non-negative integer"
            )
        entries.append(
            SealFileInventoryEntry(
                relative_path=relative_path,
                sha256=_validate_sha256(
                    raw_entry["sha256"], field=f"files[{index}].sha256"
                ),
                size=size,
                role=_validate_role(raw_entry["role"]),
            )
        )
    expected_order = sorted(entries, key=lambda entry: entry.relative_path)
    if entries != expected_order:
        raise SourcePackageSealVerificationError(
            "seal inventory must be sorted by root-relative POSIX path"
        )
    return tuple(entries)


def _decode_canonical_document(path: Path) -> tuple[dict[str, object], bytes]:
    if not path.is_file() or is_link_like(path):
        raise SourcePackageSealVerificationError(f"missing regular file: {path}")
    try:
        raw_bytes = path.read_bytes()
        decoded = loads_exact(raw_bytes.decode("utf-8"))
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        raise SourcePackageSealVerificationError(
            f"cannot decode canonical JSON document {path}: {exc}"
        ) from exc
    if not isinstance(decoded, dict):
        raise SourcePackageSealVerificationError(
            f"canonical JSON document must be an object: {path}"
        )
    return cast(dict[str, object], decoded), raw_bytes


def _expected_payload_directories(
    entries: Iterable[SealFileInventoryEntry],
) -> set[str]:
    expected: set[str] = set()
    for entry in entries:
        parent = PurePosixPath(entry.relative_path).parent
        while str(parent) != ".":
            expected.add(str(parent))
            parent = parent.parent
    return expected


def _verify_payload_tree(
    payload_root: Path, entries: tuple[SealFileInventoryEntry, ...]
) -> None:
    if not payload_root.is_dir() or is_link_like(payload_root):
        raise SourcePackageSealVerificationError(
            f"seal payload is not a regular directory: {payload_root}"
        )

    actual_files: set[str] = set()
    actual_directories: set[str] = set()
    for directory, dirnames, filenames in os.walk(payload_root, followlinks=False):
        directory_path = Path(directory)
        for name in dirnames:
            child = directory_path / name
            relative = child.relative_to(payload_root).as_posix()
            if is_link_like(child):
                raise SourcePackageSealVerificationError(
                    f"seal payload contains a link or junction: {relative}"
                )
            actual_directories.add(relative)
        for name in filenames:
            child = directory_path / name
            relative = child.relative_to(payload_root).as_posix()
            if is_link_like(child) or not child.is_file():
                raise SourcePackageSealVerificationError(
                    f"seal payload contains a linked or non-regular file: {relative}"
                )
            actual_files.add(relative)

    expected_files = {entry.relative_path for entry in entries}
    expected_directories = _expected_payload_directories(entries)
    if actual_files != expected_files:
        missing = sorted(expected_files - actual_files)
        unexpected = sorted(actual_files - expected_files)
        raise SourcePackageSealVerificationError(
            f"seal payload inventory mismatch; missing={missing}, unexpected={unexpected}"
        )
    if actual_directories != expected_directories:
        missing = sorted(expected_directories - actual_directories)
        unexpected = sorted(actual_directories - expected_directories)
        raise SourcePackageSealVerificationError(
            "seal payload directory inventory mismatch; "
            f"missing={missing}, unexpected={unexpected}"
        )

    for entry in entries:
        path = payload_root.joinpath(*PurePosixPath(entry.relative_path).parts)
        actual_size = path.stat().st_size
        if actual_size != entry.size:
            raise SourcePackageSealVerificationError(
                f"sealed file size mismatch for {entry.relative_path}: "
                f"expected {entry.size}, got {actual_size}"
            )
        actual_sha256, _ = _sha256_file(path)
        if actual_sha256 != entry.sha256:
            raise SourcePackageSealVerificationError(
                f"sealed file sha256 mismatch for {entry.relative_path}: "
                f"expected {entry.sha256}, got {actual_sha256}"
            )


def verify_source_package_seal(
    seal_root: Path, *, expected_sha256: str | None = None
) -> SourcePackageSeal:
    """Strictly verify one seal and return its location-independent identity.

    The root may be relocated, but it must contain exactly the canonical
    manifest and the inventoried payload tree.  Missing, unexpected, aliased,
    non-regular, size-mismatched, or digest-mismatched content is rejected.
    """

    seal_root = resolve_owned_path(seal_root)
    if not seal_root.is_dir() or is_link_like(seal_root):
        raise SourcePackageSealVerificationError(
            f"seal root is not a regular directory: {seal_root}"
        )
    root_entries = {entry.name for entry in seal_root.iterdir()}
    expected_root_entries = {_MANIFEST_NAME, _PAYLOAD_DIRECTORY}
    if root_entries != expected_root_entries:
        raise SourcePackageSealVerificationError(
            "seal root inventory mismatch; "
            f"missing={sorted(expected_root_entries - root_entries)}, "
            f"unexpected={sorted(root_entries - expected_root_entries)}"
        )

    manifest_path = seal_root / _MANIFEST_NAME
    document, raw_bytes = _decode_canonical_document(manifest_path)
    if set(document) != {"schema", "seal_sha256", "files"}:
        raise SourcePackageSealVerificationError("seal manifest has an invalid shape")
    if document["schema"] != _MANIFEST_SCHEMA:
        raise SourcePackageSealVerificationError(
            f"unsupported seal manifest schema: {document['schema']!r}"
        )
    entries = _parse_inventory(document["files"])
    recorded_sha256 = _validate_sha256(document["seal_sha256"], field="seal_sha256")
    computed_sha256 = _sha256_bytes(
        canonical_json_bytes(_manifest_identity_payload(entries))
    )
    if recorded_sha256 != computed_sha256:
        raise SourcePackageSealVerificationError(
            "seal manifest identity mismatch: "
            f"expected {computed_sha256}, got {recorded_sha256}"
        )
    if expected_sha256 is not None:
        expected_sha256 = _validate_sha256(expected_sha256, field="expected_sha256")
        if recorded_sha256 != expected_sha256:
            raise SourcePackageSealVerificationError(
                f"unexpected seal identity: expected {expected_sha256}, "
                f"got {recorded_sha256}"
            )
    if raw_bytes != _manifest_bytes(entries, recorded_sha256):
        raise SourcePackageSealVerificationError(
            "seal manifest is not in canonical deterministic encoding"
        )
    _verify_payload_tree(seal_root / _PAYLOAD_DIRECTORY, entries)
    return SourcePackageSeal(
        root=seal_root,
        seal_sha256=recorded_sha256,
        files=entries,
    )


def _ingest_blob(transaction_root: Path, source: Path) -> tuple[str, int, Path]:
    if not source.is_file():
        raise SourcePackageSealError(f"seal input is not a regular file: {source}")
    incoming_root = transaction_root / "blobs" / ".incoming"
    incoming_root.mkdir(parents=True, exist_ok=True)
    incoming = incoming_root / f"{uuid.uuid4().hex}.blob"
    digest = hashlib.sha256()
    size = 0
    try:
        with source.open("rb") as source_handle, incoming.open("xb") as blob_handle:
            while chunk := source_handle.read(_COPY_CHUNK_BYTES):
                blob_handle.write(chunk)
                digest.update(chunk)
                size += len(chunk)
        sha256 = digest.hexdigest()
        blob = transaction_root / "blobs" / "sha256" / sha256[:2] / sha256
        blob.parent.mkdir(parents=True, exist_ok=True)
        if blob.exists():
            if is_link_like(blob) or not blob.is_file():
                raise SourcePackageSealVerificationError(
                    f"content-addressed blob is not a regular file: {blob}"
                )
            existing_sha256, existing_size = _sha256_file(blob)
            if existing_sha256 != sha256 or existing_size != size:
                raise SourcePackageSealVerificationError(
                    f"content-addressed blob is corrupt: {blob}"
                )
        else:
            try:
                durable_publish_exclusive(incoming, blob)
            except FileExistsError:
                if is_link_like(blob) or not blob.is_file():
                    raise SourcePackageSealVerificationError(
                        f"competing blob is not a regular file: {blob}"
                    )
                if _sha256_file(blob) != (sha256, size):
                    raise SourcePackageSealVerificationError(
                        f"competing content-addressed blob is corrupt: {blob}"
                    )
        return sha256, size, blob
    finally:
        if incoming.exists():
            incoming.unlink()


def stage_source_package_seal(
    transaction_root: Path, inputs: Iterable[SourcePackageInput]
) -> SourcePackageSeal:
    """Stage an immutable seal and return its digest-addressed location.

    Input order and absolute source locations do not affect the seal identity.
    Byte-identical inputs share one blob even when they occupy distinct payload
    paths.  Repeated identical entries are coalesced; conflicting or portable
    case-colliding destinations fail closed.
    """

    transaction_root = resolve_owned_path(transaction_root)
    transaction_root.mkdir(parents=True, exist_ok=True)
    admitted: dict[str, tuple[SealFileInventoryEntry, Path]] = {}
    portable_paths: dict[str, str] = {}
    for index, item in enumerate(inputs):
        if not isinstance(item, SourcePackageInput):
            raise SourcePackageSealError(
                f"inputs[{index}] must be a SourcePackageInput"
            )
        try:
            relative_path = validate_source_package_relative_path(
                item.relative_path, field=f"inputs[{index}].relative_path"
            )
            role = _validate_role(item.role)
        except SourcePackageSealVerificationError as exc:
            raise SourcePackageSealError(str(exc)) from exc
        portable_path = portable_path_identity(relative_path)
        previous_spelling = portable_paths.get(portable_path)
        if previous_spelling is not None and previous_spelling != relative_path:
            raise SourcePackageSealError(
                "seal inputs have a portable case collision: "
                f"{previous_spelling!r} and {relative_path!r}"
            )
        portable_paths[portable_path] = relative_path

        sha256, size, blob = _ingest_blob(transaction_root, Path(item.source))
        entry = SealFileInventoryEntry(
            relative_path=relative_path,
            sha256=sha256,
            size=size,
            role=role,
        )
        previous = admitted.get(relative_path)
        if previous is not None:
            if previous[0] != entry:
                raise SourcePackageSealError(
                    f"conflicting seal inputs target {relative_path!r}"
                )
            continue
        admitted[relative_path] = (entry, blob)

    if not admitted:
        raise SourcePackageSealError("a source-package seal requires at least one file")

    entries = tuple(
        admitted[path][0] for path in sorted(admitted, key=lambda value: value)
    )
    seal_sha256 = _sha256_bytes(
        canonical_json_bytes(_manifest_identity_payload(entries))
    )
    final_root = transaction_root / "seals" / "sha256" / seal_sha256[:2] / seal_sha256
    if final_root.exists():
        return verify_source_package_seal(final_root, expected_sha256=seal_sha256)

    staging_root = transaction_root / "staging" / uuid.uuid4().hex
    payload_root = staging_root / _PAYLOAD_DIRECTORY
    payload_root.mkdir(parents=True, exist_ok=False)
    try:
        for entry in entries:
            blob = admitted[entry.relative_path][1]
            destination = payload_root.joinpath(
                *PurePosixPath(entry.relative_path).parts
            )
            _copy_staged_file(blob, destination)
        atomic_write_bytes(
            staging_root / _MANIFEST_NAME,
            _manifest_bytes(entries, seal_sha256),
        )
        verify_source_package_seal(staging_root, expected_sha256=seal_sha256)
        final_root.parent.mkdir(parents=True, exist_ok=True)
        try:
            durable_publish_directory_exclusive(staging_root, final_root)
        except FileExistsError:
            if not final_root.exists():
                raise
            verify_source_package_seal(final_root, expected_sha256=seal_sha256)
        return verify_source_package_seal(final_root, expected_sha256=seal_sha256)
    finally:
        durable_remove_path(staging_root)


def _commit_identity_payload(seal_sha256: str, destination: Path) -> dict[str, object]:
    return {
        "destination": str(destination),
        "schema": _COMMIT_SCHEMA,
        "seal_sha256": seal_sha256,
    }


def _commit_document(commit: SourcePackageSealCommit) -> dict[str, object]:
    candidate_relative = commit.candidate_root.relative_to(
        commit.transaction_root
    ).as_posix()
    return {
        "candidate": candidate_relative,
        "commit_id": commit.commit_id,
        "destination": str(commit.destination),
        "schema": _COMMIT_SCHEMA,
        "seal_sha256": commit.seal_sha256,
        "state": commit.state,
    }


def _write_commit_record(commit: SourcePackageSealCommit) -> None:
    atomic_write_bytes(
        commit.record_path,
        canonical_json_bytes(_commit_document(commit)) + b"\n",
    )


def load_source_package_seal_commit(
    transaction_root: Path, record_path: Path
) -> SourcePackageSealCommit:
    """Load and strictly validate one durable commit record."""

    if is_link_like(transaction_root) or is_link_like(record_path):
        raise SourcePackageSealVerificationError("seal commit has indirect custody")
    transaction_root = resolve_owned_path(transaction_root)
    record_path = resolve_owned_path(record_path)
    records_root = transaction_root / "commits"
    if record_path.parent != records_root or record_path.suffix != ".json":
        raise SourcePackageSealVerificationError(
            f"commit record escapes the transaction commit root: {record_path}"
        )
    document, raw_bytes = _decode_canonical_document(record_path)
    expected_keys = {
        "candidate",
        "commit_id",
        "destination",
        "schema",
        "seal_sha256",
        "state",
    }
    if set(document) != expected_keys or document["schema"] != _COMMIT_SCHEMA:
        raise SourcePackageSealVerificationError("commit record has an invalid shape")
    seal_sha256 = _validate_sha256(document["seal_sha256"], field="commit.seal_sha256")
    commit_id = _validate_sha256(document["commit_id"], field="commit.commit_id")
    if record_path.name != f"{commit_id}.json":
        raise SourcePackageSealVerificationError(
            "commit record filename does not match its immutable identity"
        )
    destination_raw = document["destination"]
    if not isinstance(destination_raw, str):
        raise SourcePackageSealVerificationError(
            "commit destination must be an absolute path"
        )
    destination = Path(destination_raw)
    if not destination.is_absolute() or destination != destination.resolve():
        raise SourcePackageSealVerificationError(
            "commit destination must be a canonical absolute path"
        )
    expected_commit_id = _sha256_bytes(
        canonical_json_bytes(_commit_identity_payload(seal_sha256, destination))
    )
    if commit_id != expected_commit_id:
        raise SourcePackageSealVerificationError("commit identity mismatch")
    candidate_relative = validate_source_package_relative_path(
        document["candidate"], field="commit.candidate"
    )
    expected_candidate = f"commit-candidates/{commit_id}"
    if candidate_relative != expected_candidate:
        raise SourcePackageSealVerificationError(
            "commit candidate path does not match its immutable identity"
        )
    state = document["state"]
    if not isinstance(state, str) or state not in {"prepared", "committed"}:
        raise SourcePackageSealVerificationError(f"invalid commit state: {state!r}")
    commit = SourcePackageSealCommit(
        transaction_root=transaction_root,
        record_path=record_path,
        commit_id=commit_id,
        seal_sha256=seal_sha256,
        candidate_root=transaction_root.joinpath(
            *PurePosixPath(candidate_relative).parts
        ),
        destination=destination,
        state=state,
    )
    if raw_bytes != canonical_json_bytes(_commit_document(commit)) + b"\n":
        raise SourcePackageSealVerificationError(
            "commit record is not in canonical deterministic encoding"
        )
    return commit


def _copy_seal_candidate(source: Path, candidate: Path, seal_sha256: str) -> None:
    if candidate.exists():
        verify_source_package_seal(candidate, expected_sha256=seal_sha256)
        return
    candidate.parent.mkdir(parents=True, exist_ok=True)
    temporary = candidate.parent / f".{candidate.name}.{uuid.uuid4().hex}.tmp"
    try:
        verified = verify_source_package_seal(source, expected_sha256=seal_sha256)
        (temporary / _PAYLOAD_DIRECTORY).mkdir(parents=True, exist_ok=False)
        for entry in verified.files:
            source_file = source.joinpath(
                _PAYLOAD_DIRECTORY, *PurePosixPath(entry.relative_path).parts
            )
            destination_file = temporary.joinpath(
                _PAYLOAD_DIRECTORY, *PurePosixPath(entry.relative_path).parts
            )
            _copy_staged_file(source_file, destination_file)
        _copy_staged_file(source / _MANIFEST_NAME, temporary / _MANIFEST_NAME)
        verify_source_package_seal(temporary, expected_sha256=seal_sha256)
        try:
            durable_publish_directory_exclusive(temporary, candidate)
        except FileExistsError:
            if not candidate.exists():
                raise
            verify_source_package_seal(candidate, expected_sha256=seal_sha256)
    finally:
        durable_remove_path(temporary)


def prepare_source_package_seal_commit(
    transaction_root: Path,
    seal: SourcePackageSeal,
    destination: Path,
) -> SourcePackageSealCommit:
    """Durably prepare publication at one canonical immutable destination.

    ``destination`` is explicit and must share a filesystem with the transaction
    root so the candidate-to-destination rename is atomic.  An existing root is
    admitted only when it verifies as the exact same seal; publication never
    replaces a different identity behind a stable package/version name.
    The source seal may be external: its verified bytes are copied directly
    into the transaction's commit candidate, never moved or adopted in place.
    """

    if is_link_like(transaction_root) or is_link_like(destination):
        raise SourcePackageSealVerificationError(
            "seal publication has indirect custody"
        )
    transaction_root = resolve_owned_path(transaction_root)
    transaction_root.mkdir(parents=True, exist_ok=True)
    verified = verify_source_package_seal(seal.root, expected_sha256=seal.seal_sha256)
    seal_root = verified.root.resolve()
    destination = resolve_owned_path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    different_windows_volume = (
        os.name == "nt"
        and transaction_root.anchor.casefold() != destination.parent.anchor.casefold()
    )
    if (
        different_windows_volume
        or transaction_root.stat().st_dev != destination.parent.stat().st_dev
    ):
        raise SourcePackageSealError(
            "transaction root and seal destination must share a filesystem"
        )
    commit_id = _sha256_bytes(
        canonical_json_bytes(
            _commit_identity_payload(verified.seal_sha256, destination)
        )
    )
    record_path = transaction_root / "commits" / f"{commit_id}.json"
    candidate = transaction_root / "commit-candidates" / commit_id
    if record_path.exists():
        existing = load_source_package_seal_commit(transaction_root, record_path)
        if (
            existing.seal_sha256 != verified.seal_sha256
            or existing.destination != destination
        ):
            raise SourcePackageSealVerificationError(
                f"commit identity collision at {record_path}"
            )
        return existing

    state = "prepared"
    if destination.exists():
        verify_source_package_seal(destination, expected_sha256=verified.seal_sha256)
        state = "committed"
    else:
        _copy_seal_candidate(seal_root, candidate, verified.seal_sha256)
    commit = SourcePackageSealCommit(
        transaction_root=transaction_root,
        record_path=record_path,
        commit_id=commit_id,
        seal_sha256=verified.seal_sha256,
        candidate_root=candidate,
        destination=destination,
        state=state,
    )
    _write_commit_record(commit)
    return load_source_package_seal_commit(transaction_root, record_path)


def commit_source_package_seal(
    commit: SourcePackageSealCommit,
) -> SourcePackageSealCommit:
    """Finish one prepared publication, idempotently across process crashes."""

    current = load_source_package_seal_commit(
        commit.transaction_root, commit.record_path
    )
    destination_exists = current.destination.exists()
    if current.state == "committed":
        if not destination_exists:
            raise SourcePackageSealVerificationError(
                "committed seal destination is missing"
            )
        verify_source_package_seal(
            current.destination, expected_sha256=current.seal_sha256
        )
        if current.candidate_root.exists():
            verify_source_package_seal(
                current.candidate_root, expected_sha256=current.seal_sha256
            )
            durable_remove_path(current.candidate_root)
        return current

    if destination_exists:
        verify_source_package_seal(
            current.destination, expected_sha256=current.seal_sha256
        )
        if current.candidate_root.exists():
            verify_source_package_seal(
                current.candidate_root, expected_sha256=current.seal_sha256
            )
            durable_remove_path(current.candidate_root)
    else:
        verify_source_package_seal(
            current.candidate_root, expected_sha256=current.seal_sha256
        )
        current.destination.parent.mkdir(parents=True, exist_ok=True)
        try:
            durable_publish_directory_exclusive(
                current.candidate_root, current.destination
            )
        except FileExistsError:
            if not current.destination.exists():
                raise
            verify_source_package_seal(
                current.destination, expected_sha256=current.seal_sha256
            )

    committed = SourcePackageSealCommit(
        transaction_root=current.transaction_root,
        record_path=current.record_path,
        commit_id=current.commit_id,
        seal_sha256=current.seal_sha256,
        candidate_root=current.candidate_root,
        destination=current.destination,
        state="committed",
    )
    _write_commit_record(committed)
    return load_source_package_seal_commit(
        committed.transaction_root, committed.record_path
    )


def recover_source_package_seal_commits(
    transaction_root: Path,
    *,
    expected_destination: Path | None = None,
) -> tuple[SourcePackageSealCommit, ...]:
    """Complete every durable commit record directly under a transaction root."""

    if is_link_like(transaction_root):
        raise SourcePackageSealVerificationError("seal recovery has indirect custody")
    transaction_root = resolve_owned_path(transaction_root)
    records_root = transaction_root / "commits"
    if not records_root.exists():
        return ()
    if not records_root.is_dir() or is_link_like(records_root):
        raise SourcePackageSealVerificationError(
            f"transaction commit root is not a regular directory: {records_root}"
        )
    records = tuple(
        load_source_package_seal_commit(transaction_root, record_path)
        for record_path in sorted(records_root.glob("*.json"))
    )
    if expected_destination is not None:
        destination = resolve_owned_path(expected_destination)
        for record in records:
            if record.destination != destination:
                raise SourcePackageSealVerificationError(
                    f"seal recovery journal {record.record_path} targets "
                    f"{record.destination}, outside destination custody {destination}"
                )
    return tuple(commit_source_package_seal(record) for record in records)

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
import uuid
from dataclasses import dataclass
from pathlib import Path

from molt.cli.atomic_io import _atomic_write_text, _durable_replace
from molt.cli.runtime_build_identity import RuntimeBuildIdentity, _json_object_mapping


_RUNTIME_WASM_GENERATION_SCHEMA = "molt.runtime-wasm-generation.v2"
_RUNTIME_WASM_GENERATION_NAME = "molt_runtime.generation.json"
_MEMBER_SUFFIX = ".runtime-wasm-member"
_SHARED_RUNTIME_NAME = "molt_runtime.wasm"
_RELOC_RUNTIME_NAME = "molt_runtime_reloc.wasm"


@dataclass(frozen=True)
class RuntimeWasmGeneration:
    manifest: Path
    shared: Path
    reloc: Path
    shared_identity: RuntimeBuildIdentity
    reloc_identity: RuntimeBuildIdentity
    payload: dict[str, object]


def runtime_wasm_generation_path(shared: Path) -> Path:
    return shared.with_name(_RUNTIME_WASM_GENERATION_NAME)


def _hash_artifact(path: Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(8 * 1024 * 1024):
            hasher.update(chunk)
            size += len(chunk)
    return hasher.hexdigest(), size


def _is_regular_immutable_member(path: Path) -> bool:
    """Reject symlink/reparse indirection before trusting a member path."""

    try:
        member_stat = path.lstat()
    except OSError:
        return False
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    file_attributes = getattr(member_stat, "st_file_attributes", 0)
    return (
        stat.S_ISREG(member_stat.st_mode)
        and not path.is_symlink()
        and not bool(file_attributes & reparse_flag)
    )


def _stage_artifact(
    source: Path,
    staged: Path,
    *,
    published_name: str,
    identity: RuntimeBuildIdentity,
    expected_record: dict[str, object] | None = None,
) -> dict[str, object]:
    hasher = hashlib.sha256()
    size = 0
    with source.open("rb") as source_handle, staged.open("xb") as staged_handle:
        before = os.fstat(source_handle.fileno())
        while chunk := source_handle.read(8 * 1024 * 1024):
            hasher.update(chunk)
            staged_handle.write(chunk)
            size += len(chunk)
        staged_handle.flush()
        staged_stat = os.fstat(staged_handle.fileno())
        after = os.fstat(source_handle.fileno())
    digest = hasher.hexdigest()
    if (
        before.st_size != after.st_size
        or before.st_mtime_ns != after.st_mtime_ns
        or before.st_ctime_ns != after.st_ctime_ns
        or size != after.st_size
        or staged_stat.st_size != size
    ):
        raise ValueError(f"runtime artifact mutated while staging: {source.name}")
    if expected_record is not None and (
        expected_record.get("sha256") != digest or expected_record.get("size") != size
    ):
        raise ValueError(
            f"runtime artifact changed after source generation validation: {source.name}"
        )
    member_name = f"{published_name}.{digest}{_MEMBER_SUFFIX}"
    return {
        "name": published_name,
        "member": member_name,
        "sha256": digest,
        "size": size,
        "identity": identity.to_dict(),
    }


def _publish_immutable_member(staged: Path, member: Path, source: Path) -> None:
    """Publish a content-named member; same-name races can only contain same bytes."""

    if member.exists():
        if not _is_regular_immutable_member(member):
            raise ValueError(
                f"immutable runtime member is not a regular file: {member.name}"
            )
        digest, size = _hash_artifact(member)
        if digest != member.name.split(".")[-2] or size != staged.stat().st_size:
            raise ValueError(f"immutable runtime member is corrupt: {member.name}")
        staged.unlink()
        return
    _durable_replace(staged, member)
    # Content durability and namespace commit precede final (possibly read-only) mode.
    try:
        shutil.copymode(source, member)
    except OSError:
        pass


def publish_runtime_wasm_generation(
    shared: Path,
    reloc: Path,
    *,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    source_shared: Path | None = None,
    source_reloc: Path | None = None,
    expected_source_receipts: dict[str, dict[str, object]] | None = None,
) -> RuntimeWasmGeneration:
    """Atomically point at one immutable shared+reloc runtime generation.

    Content-named members are immutable authorities and the manifest replacement
    is the sole pair publication transaction. Fixed runtime filenames are not
    materialized here; only an explicit final deployment may project them.
    """

    if shared_identity.pair_digest != reloc_identity.pair_digest:
        raise ValueError("shared and reloc runtime identities are not one build pair")
    if shared.name != _SHARED_RUNTIME_NAME or reloc.name != _RELOC_RUNTIME_NAME:
        raise ValueError("runtime generation coordinates use non-canonical names")
    shared.parent.mkdir(parents=True, exist_ok=True)
    reloc.parent.mkdir(parents=True, exist_ok=True)
    token = uuid.uuid4().hex
    staged_shared = shared.with_name(f".{shared.name}.{token}.generation")
    staged_reloc = reloc.with_name(f".{reloc.name}.{token}.generation")
    actual_source_shared = source_shared or shared
    actual_source_reloc = source_reloc or reloc
    try:
        shared_record = _stage_artifact(
            actual_source_shared,
            staged_shared,
            published_name=shared.name,
            identity=shared_identity,
            expected_record=(expected_source_receipts or {}).get("shared"),
        )
        reloc_record = _stage_artifact(
            actual_source_reloc,
            staged_reloc,
            published_name=reloc.name,
            identity=reloc_identity,
            expected_record=(expected_source_receipts or {}).get("reloc"),
        )
        shared_member = shared.parent / str(shared_record["member"])
        reloc_member = reloc.parent / str(reloc_record["member"])
        _publish_immutable_member(staged_shared, shared_member, actual_source_shared)
        _publish_immutable_member(staged_reloc, reloc_member, actual_source_reloc)

        payload = {
            "schema": _RUNTIME_WASM_GENERATION_SCHEMA,
            "pair_digest": shared_identity.pair_digest,
            "receipts": {"shared": shared_record, "reloc": reloc_record},
        }
        manifest = runtime_wasm_generation_path(shared)
        _atomic_write_text(
            manifest,
            json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n",
        )
        generation = read_runtime_wasm_generation(
            manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        if generation is None:
            raise ValueError("published runtime generation failed self-validation")
        return generation
    finally:
        staged_shared.unlink(missing_ok=True)
        staged_reloc.unlink(missing_ok=True)


def _member_path(manifest: Path, record: object) -> Path | None:
    if not isinstance(record, dict):
        return None
    raw = record.get("member")
    if not isinstance(raw, str) or not raw or Path(raw).name != raw:
        return None
    path = manifest.parent / raw
    if path.parent != manifest.parent or not raw.endswith(_MEMBER_SUFFIX):
        return None
    return path


def _validate_artifact_record(
    record: object,
    *,
    manifest: Path,
    expected_name: str,
    expected_identity: RuntimeBuildIdentity,
) -> Path | None:
    if not isinstance(record, dict) or record.get("name") != expected_name:
        return None
    member = _member_path(manifest, record)
    if member is None or not _is_regular_immutable_member(member):
        return None
    try:
        recorded_identity = RuntimeBuildIdentity.from_dict(record.get("identity"))
    except ValueError:
        return None
    if recorded_identity != expected_identity:
        return None
    try:
        digest, size = _hash_artifact(member)
    except OSError:
        return None
    expected_member = f"{expected_name}.{digest}{_MEMBER_SUFFIX}"
    if (
        record.get("sha256") != digest
        or record.get("size") != size
        or member.name != expected_member
    ):
        return None
    return member


def _generation_receipts(
    value: object,
) -> dict[str, dict[str, object]] | None:
    """Return the exact typed shared/reloc receipt pair or fail closed."""

    receipts = _json_object_mapping(value)
    if receipts is None or set(receipts) != {"shared", "reloc"}:
        return None
    typed: dict[str, dict[str, object]] = {}
    for kind in ("shared", "reloc"):
        record = _json_object_mapping(receipts.get(kind))
        if record is None:
            return None
        typed[kind] = dict(record)
    return typed


def read_runtime_wasm_generation(
    manifest: Path,
    *,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
) -> RuntimeWasmGeneration | None:
    """Validate the atomically selected immutable pair against trusted identities."""

    if expected_shared_identity.pair_digest != expected_reloc_identity.pair_digest:
        return None
    try:
        payload = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if (
        not isinstance(payload, dict)
        or payload.get("schema") != _RUNTIME_WASM_GENERATION_SCHEMA
        or payload.get("pair_digest") != expected_shared_identity.pair_digest
    ):
        return None
    receipts = _generation_receipts(payload.get("receipts"))
    if receipts is None:
        return None
    shared_member = _validate_artifact_record(
        receipts.get("shared"),
        manifest=manifest,
        expected_name=_SHARED_RUNTIME_NAME,
        expected_identity=expected_shared_identity,
    )
    reloc_member = _validate_artifact_record(
        receipts.get("reloc"),
        manifest=manifest,
        expected_name=_RELOC_RUNTIME_NAME,
        expected_identity=expected_reloc_identity,
    )
    if shared_member is None or reloc_member is None:
        return None
    return RuntimeWasmGeneration(
        manifest=manifest,
        shared=shared_member,
        reloc=reloc_member,
        shared_identity=expected_shared_identity,
        reloc_identity=expected_reloc_identity,
        payload=payload,
    )


def hydrate_runtime_wasm_generation(
    *,
    source_manifest: Path,
    dest_shared: Path,
    dest_reloc: Path,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
) -> RuntimeWasmGeneration:
    """Validate and hydrate only from the source pointer's immutable members."""

    payload = read_runtime_wasm_generation(
        source_manifest,
        expected_shared_identity=expected_shared_identity,
        expected_reloc_identity=expected_reloc_identity,
    )
    if payload is None:
        raise ValueError(
            "runtime wasm source generation does not match trusted identity"
        )
    source_member_shared = payload.shared
    source_member_reloc = payload.reloc
    receipts = _generation_receipts(payload.payload.get("receipts"))
    if receipts is None:
        raise ValueError("validated runtime generation receipts are invalid")
    return publish_runtime_wasm_generation(
        dest_shared,
        dest_reloc,
        shared_identity=expected_shared_identity,
        reloc_identity=expected_reloc_identity,
        source_shared=source_member_shared,
        source_reloc=source_member_reloc,
        expected_source_receipts=receipts,
    )

from __future__ import annotations

import hashlib
import os
import shutil
from dataclasses import dataclass
from pathlib import Path

from molt.cli.atomic_io import _atomic_write_json
from molt.file_publication import durable_replace, staged_file_path
from molt.cli.runtime_build_identity import RuntimeBuildIdentity, _json_object_mapping
from molt.exact_json import read_exact
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    open_stable_regular_file,
    stable_regular_file_identity,
)


_RUNTIME_WASM_GENERATION_SCHEMA = "molt.runtime-wasm-generation.v3"
_RUNTIME_WASM_EXPECTED_PAIR_SCHEMA = "molt.runtime-wasm-expected-pair.v2"
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
    shared_member_identity: StableRegularFileIdentity
    reloc_member_identity: StableRegularFileIdentity
    payload: dict[str, object]


@dataclass(frozen=True)
class RuntimeWasmExpectedPair:
    """Caller-trusted exact build identities for one shared/reloc pair."""

    shared: RuntimeBuildIdentity
    reloc: RuntimeBuildIdentity

    def __post_init__(self) -> None:
        if (
            self.shared.payload.get("member_kind") != "shared"
            or self.reloc.payload.get("member_kind") != "reloc"
            or self.shared.family_digest != self.reloc.family_digest
        ):
            raise ValueError(
                "runtime WASM expected pair must name one shared/reloc build pair"
            )

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _RUNTIME_WASM_EXPECTED_PAIR_SCHEMA,
            "shared": self.shared.to_dict(),
            "reloc": self.reloc.to_dict(),
        }

    @classmethod
    def from_dict(cls, value: object) -> RuntimeWasmExpectedPair:
        payload = _json_object_mapping(value)
        if (
            payload is None
            or set(payload) != {"schema", "shared", "reloc"}
            or payload.get("schema") != _RUNTIME_WASM_EXPECTED_PAIR_SCHEMA
        ):
            raise ValueError("runtime WASM expected pair schema is invalid")
        return cls(
            shared=RuntimeBuildIdentity.from_dict(payload.get("shared")),
            reloc=RuntimeBuildIdentity.from_dict(payload.get("reloc")),
        )

    @classmethod
    def read(cls, path: Path) -> RuntimeWasmExpectedPair:
        try:
            payload = read_exact(
                path,
                max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
                label="runtime WASM expected pair",
            )
        except (OSError, UnicodeError, ValueError) as exc:
            raise ValueError(
                f"runtime WASM expected pair is unreadable: {path}: {exc}"
            ) from exc
        return cls.from_dict(payload)

    def write(self, path: Path) -> None:
        _atomic_write_json(path, self.to_dict(), sort_keys=True)


def runtime_wasm_generation_path(shared: Path) -> Path:
    return shared.with_name(_RUNTIME_WASM_GENERATION_NAME)


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
    with (
        open_stable_regular_file(source, label="runtime generation source") as opened,
        staged.open("xb") as staged_handle,
    ):
        while chunk := opened.stream.read(1024 * 1024):
            hasher.update(chunk)
            staged_handle.write(chunk)
            size += len(chunk)
        staged_handle.flush()
        staged_stat = os.fstat(staged_handle.fileno())
    digest = hasher.hexdigest()
    if size != opened.stat.st_size or staged_stat.st_size != size:
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
        identity = stable_regular_file_identity(
            member,
            label="existing immutable runtime member",
        )
        if (
            identity.sha256 != member.name.split(".")[-2]
            or identity.size != staged.stat().st_size
        ):
            raise ValueError(f"immutable runtime member is corrupt: {member.name}")
        staged.unlink()
        return
    durable_replace(staged, member)
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

    expected_pair = RuntimeWasmExpectedPair(shared_identity, reloc_identity)
    shared_identity = expected_pair.shared
    reloc_identity = expected_pair.reloc
    if shared.name != _SHARED_RUNTIME_NAME or reloc.name != _RELOC_RUNTIME_NAME:
        raise ValueError("runtime generation coordinates use non-canonical names")
    shared.parent.mkdir(parents=True, exist_ok=True)
    reloc.parent.mkdir(parents=True, exist_ok=True)
    staged_shared = staged_file_path(shared, purpose="generation")
    staged_reloc = staged_file_path(reloc, purpose="generation")
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
            "family_digest": shared_identity.family_digest,
            "receipts": {"shared": shared_record, "reloc": reloc_record},
        }
        manifest = runtime_wasm_generation_path(shared)
        _atomic_write_json(
            manifest,
            payload,
            sort_keys=True,
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
) -> StableRegularFileIdentity | None:
    if (
        not isinstance(record, dict)
        or set(record) != {"name", "member", "sha256", "size", "identity"}
        or record.get("name") != expected_name
    ):
        return None
    member = _member_path(manifest, record)
    if member is None:
        return None
    try:
        recorded_identity = RuntimeBuildIdentity.from_dict(record.get("identity"))
    except ValueError:
        return None
    if recorded_identity != expected_identity:
        return None
    try:
        member_identity = stable_regular_file_identity(
            member,
            label=f"immutable runtime member {expected_name}",
        )
    except (OSError, ValueError):
        return None
    expected_member = f"{expected_name}.{member_identity.sha256}{_MEMBER_SUFFIX}"
    if (
        record.get("sha256") != member_identity.sha256
        or not isinstance(record.get("size"), int)
        or isinstance(record.get("size"), bool)
        or record.get("size") != member_identity.size
        or member.name != expected_member
    ):
        return None
    return member_identity


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

    try:
        expected_pair = RuntimeWasmExpectedPair(
            expected_shared_identity,
            expected_reloc_identity,
        )
    except ValueError:
        return None
    expected_shared_identity = expected_pair.shared
    expected_reloc_identity = expected_pair.reloc
    try:
        payload = read_exact(
            manifest,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="runtime WASM generation",
        )
    except (OSError, UnicodeError, ValueError):
        return None
    if (
        not isinstance(payload, dict)
        or set(payload) != {"schema", "family_digest", "receipts"}
        or payload.get("schema") != _RUNTIME_WASM_GENERATION_SCHEMA
        or payload.get("family_digest") != expected_shared_identity.family_digest
    ):
        return None
    receipts = _generation_receipts(payload.get("receipts"))
    if receipts is None:
        return None
    shared_member_identity = _validate_artifact_record(
        receipts.get("shared"),
        manifest=manifest,
        expected_name=_SHARED_RUNTIME_NAME,
        expected_identity=expected_shared_identity,
    )
    reloc_member_identity = _validate_artifact_record(
        receipts.get("reloc"),
        manifest=manifest,
        expected_name=_RELOC_RUNTIME_NAME,
        expected_identity=expected_reloc_identity,
    )
    if shared_member_identity is None or reloc_member_identity is None:
        return None
    return RuntimeWasmGeneration(
        manifest=manifest,
        shared=shared_member_identity.path,
        reloc=reloc_member_identity.path,
        shared_identity=expected_shared_identity,
        reloc_identity=expected_reloc_identity,
        shared_member_identity=shared_member_identity,
        reloc_member_identity=reloc_member_identity,
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

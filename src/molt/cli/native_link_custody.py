from __future__ import annotations

from collections.abc import Mapping, Sequence
import contextlib
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
import tarfile
from typing import cast
import uuid

from molt.cli.atomic_io import (
    _atomic_copy_file,
    _remove_file_or_tree,
)
from molt.exact_json import canonical_json_sha256
from molt.ustar import RegularUstarTarInfo
from molt.file_publication import (
    durable_publish_directory_exclusive,
    durable_publish_exclusive,
)
from molt.portable_paths import (
    portable_path_component,
    portable_path_identity,
    portable_relative_path,
)
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    open_stable_regular_file,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)


_CUSTODY_SCHEMA = "molt.native-link-custody.v2"
_ARCHIVE_PREFIX = "molt-native-link-custody-"
_ARCHIVE_SUFFIX = ".tar"
_EXTRACTION_PREFIX = ".molt-native-link-custody-"
_SHA256_RE = re.compile(r"[0-9a-f]{64}")
_MAX_ARCHIVE_BYTES = 2 * 1024 * 1024 * 1024


class NativeLinkCustodyError(RuntimeError):
    """Native link inputs are missing, corrupt, or not safely relocatable."""


@dataclass(frozen=True, slots=True)
class NativeLinkCustodyEntry:
    identifier: str
    filename: str
    archive_path: str
    sha256: str
    size_bytes: int

    def to_dict(self) -> dict[str, object]:
        return {
            "id": self.identifier,
            "filename": self.filename,
            "archive_path": self.archive_path,
            "sha256": self.sha256,
            "size_bytes": self.size_bytes,
        }


def _safe_filename(value: str) -> str:
    try:
        return portable_path_component(value)
    except ValueError as exc:
        raise NativeLinkCustodyError(
            f"unsafe native-link custody filename: {value!r}"
        ) from exc


def _safe_archive_member(value: str) -> PurePosixPath:
    try:
        return portable_relative_path(value)
    except ValueError as exc:
        raise NativeLinkCustodyError(
            f"unsafe native-link custody member: {value!r}"
        ) from exc


def _file_identity(path: Path) -> StableRegularFileIdentity:
    try:
        with open_stable_regular_file(path, label="native link input") as opened:
            if opened.stat.st_size > _MAX_ARCHIVE_BYTES:
                raise NativeLinkCustodyError(
                    f"native link input exceeds the custody size limit: {path}"
                )
            return stable_regular_file_identity(path, label="native link input")
    except (OSError, ValueError) as exc:
        raise NativeLinkCustodyError(
            f"native link input is unavailable or not one stable regular file: {path}: {exc}"
        ) from exc


def _describe_file(
    path: Path,
) -> tuple[NativeLinkCustodyEntry, StableRegularFileIdentity]:
    identity = _file_identity(path)
    if identity.size <= 0:
        raise NativeLinkCustodyError(
            f"native link input must not be empty: {identity.path}"
        )
    if identity.size > _MAX_ARCHIVE_BYTES:
        raise NativeLinkCustodyError(
            f"native link input exceeds the custody size limit: {identity.path}"
        )
    filename = _safe_filename(identity.path.name)
    identity_payload = {
        "filename": filename,
        "sha256": identity.sha256,
        "size_bytes": identity.size,
    }
    identifier = canonical_json_sha256(identity_payload)
    return (
        NativeLinkCustodyEntry(
            identifier=identifier,
            filename=filename,
            archive_path=f"files/{identifier}/{filename}",
            sha256=identity.sha256,
            size_bytes=identity.size,
        ),
        identity,
    )


def _validated_entry(value: object, *, context: str) -> NativeLinkCustodyEntry:
    if not isinstance(value, dict) or set(value) != {
        "id",
        "filename",
        "archive_path",
        "sha256",
        "size_bytes",
    }:
        raise NativeLinkCustodyError(f"invalid native-link custody entry: {context}")
    identifier = value.get("id")
    filename = value.get("filename")
    archive_path = value.get("archive_path")
    digest = value.get("sha256")
    size = value.get("size_bytes")
    if (
        not isinstance(identifier, str)
        or _SHA256_RE.fullmatch(identifier) is None
        or not isinstance(filename, str)
        or not isinstance(archive_path, str)
        or not isinstance(digest, str)
        or _SHA256_RE.fullmatch(digest) is None
        or not isinstance(size, int)
        or isinstance(size, bool)
        or size <= 0
        or size > _MAX_ARCHIVE_BYTES
    ):
        raise NativeLinkCustodyError(f"invalid native-link custody entry: {context}")
    _safe_filename(filename)
    _safe_archive_member(archive_path)
    expected_id = canonical_json_sha256(
        {"filename": filename, "sha256": digest, "size_bytes": size}
    )
    if identifier != expected_id or archive_path != f"files/{identifier}/{filename}":
        raise NativeLinkCustodyError(
            f"non-canonical native-link custody entry: {context}"
        )
    return NativeLinkCustodyEntry(identifier, filename, archive_path, digest, size)


def validate_native_link_custody(
    value: object,
    *,
    context: str,
) -> tuple[Mapping[str, object], tuple[NativeLinkCustodyEntry, ...]]:
    if not isinstance(value, dict) or set(value) != {"schema", "archive", "entries"}:
        raise NativeLinkCustodyError(f"invalid native-link custody shape: {context}")
    typed_value = cast(dict[str, object], value)
    if typed_value.get("schema") != _CUSTODY_SCHEMA:
        raise NativeLinkCustodyError(
            f"unsupported native-link custody schema: {context}"
        )
    raw_entries = typed_value.get("entries")
    if not isinstance(raw_entries, list):
        raise NativeLinkCustodyError(
            f"native-link custody entries must be an array: {context}"
        )
    entries = tuple(
        _validated_entry(item, context=f"{context}:entries[{index}]")
        for index, item in enumerate(raw_entries)
    )
    if tuple(entry.identifier for entry in entries) != tuple(
        sorted(entry.identifier for entry in entries)
    ):
        raise NativeLinkCustodyError(
            f"native-link custody entries are not canonically ordered: {context}"
        )
    if len({entry.identifier for entry in entries}) != len(entries) or len(
        {portable_path_identity(entry.archive_path) for entry in entries}
    ) != len(entries):
        raise NativeLinkCustodyError(f"duplicate native-link custody entry: {context}")
    archive = typed_value.get("archive")
    if not entries:
        if archive is not None:
            raise NativeLinkCustodyError(
                f"empty native-link custody unexpectedly has an archive: {context}"
            )
        return typed_value, entries
    if not isinstance(archive, dict) or set(archive) != {
        "name",
        "sha256",
        "size_bytes",
    }:
        raise NativeLinkCustodyError(f"invalid native-link custody archive: {context}")
    name = archive.get("name")
    digest = archive.get("sha256")
    size = archive.get("size_bytes")
    if (
        not isinstance(name, str)
        or not isinstance(digest, str)
        or _SHA256_RE.fullmatch(digest) is None
        or name != f"{_ARCHIVE_PREFIX}{digest}{_ARCHIVE_SUFFIX}"
        or not isinstance(size, int)
        or isinstance(size, bool)
        or size <= 0
        or size > _MAX_ARCHIVE_BYTES
    ):
        raise NativeLinkCustodyError(f"invalid native-link custody archive: {context}")
    _safe_filename(name)
    return typed_value, entries


def native_link_custody_archive_path(
    runtime_lib: Path,
    custody: Mapping[str, object],
) -> Path | None:
    _value, entries = validate_native_link_custody(custody, context=str(runtime_lib))
    if not entries:
        return None
    archive = cast(Mapping[str, object], custody["archive"])
    name = archive["name"]
    assert isinstance(name, str)
    return runtime_lib.parent / name


def _tar_info(entry: NativeLinkCustodyEntry) -> tarfile.TarInfo:
    info = tarfile.TarInfo(entry.archive_path)
    info.size = entry.size_bytes
    info.mode = 0o644
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    info.type = tarfile.REGTYPE
    return info


def _archive_member_is_canonical(
    member: tarfile.TarInfo,
    entry: NativeLinkCustodyEntry,
) -> bool:
    return (
        member.name == entry.archive_path
        and member.isreg()
        and member.size == entry.size_bytes
        and member.mode == 0o644
        and member.uid == 0
        and member.gid == 0
        and member.uname == ""
        and member.gname == ""
        and member.mtime == 0
        and member.linkname == ""
        and member.devmajor == 0
        and member.devminor == 0
        and not member.pax_headers
        and not member.issparse()
    )


def _write_archive(
    stage: Path,
    entries: Sequence[NativeLinkCustodyEntry],
    sources: Mapping[str, StableRegularFileIdentity],
) -> None:
    with stage.open("xb") as raw:
        with tarfile.open(
            fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT
        ) as archive:
            for entry in entries:
                source = sources[entry.identifier]
                with open_stable_regular_file(
                    source.path, label="native custody archive source"
                ) as opened:
                    verify_stable_regular_file_identity(
                        source, label="native custody archive source"
                    )
                    archive.addfile(_tar_info(entry), opened.stream)
                    verify_stable_regular_file_identity(
                        source, label="native custody archive source"
                    )
        raw.flush()
        os.fsync(raw.fileno())


def _validate_archive_file(
    path: Path,
    *,
    archive_record: Mapping[str, object],
    entries: Sequence[NativeLinkCustodyEntry],
) -> None:
    identity = _file_identity(path)
    expected_size = archive_record["size_bytes"]
    expected_digest = archive_record["sha256"]
    if identity.size != expected_size or identity.sha256 != expected_digest:
        raise NativeLinkCustodyError(
            f"native-link custody archive identity mismatch: {path}"
        )
    expected = {entry.archive_path: entry for entry in entries}
    expected_order = tuple(entry.archive_path for entry in entries)
    try:
        with (
            open_stable_regular_file(
                identity.path, label="native custody archive"
            ) as opened,
            tarfile.open(
                fileobj=opened.stream, mode="r:", tarinfo=RegularUstarTarInfo
            ) as archive,
        ):
            verify_stable_regular_file_identity(
                identity, label="native custody archive"
            )
            observed_order: list[str] = []
            observed: set[str] = set()
            for member in archive:
                member_path = _safe_archive_member(member.name).as_posix()
                if member_path in observed:
                    raise NativeLinkCustodyError(
                        f"duplicate native-link custody archive member: {member_path}"
                    )
                observed.add(member_path)
                observed_order.append(member_path)
                entry = expected.get(member_path)
                if entry is None or not _archive_member_is_canonical(member, entry):
                    raise NativeLinkCustodyError(
                        f"invalid native-link custody archive member: {member_path}"
                    )
                stream = archive.extractfile(member)
                if stream is None:
                    raise NativeLinkCustodyError(
                        f"unreadable native-link custody archive member: {member_path}"
                    )
                digest = hashlib.sha256()
                size = 0
                while block := stream.read(1024 * 1024):
                    size += len(block)
                    digest.update(block)
                if size != entry.size_bytes or digest.hexdigest() != entry.sha256:
                    raise NativeLinkCustodyError(
                        f"native-link custody member identity mismatch: {member_path}"
                    )
            if tuple(observed_order) != expected_order:
                raise NativeLinkCustodyError(
                    "native-link custody archive member closure mismatch"
                )
            verify_stable_regular_file_identity(
                identity, label="native custody archive"
            )
    except (OSError, tarfile.TarError, ValueError) as exc:
        raise NativeLinkCustodyError(
            f"cannot validate native-link custody archive {path}: {exc}"
        ) from exc


def validate_native_link_custody_archive(
    archive_path: Path | None,
    custody: Mapping[str, object],
    *,
    context: str | None = None,
) -> None:
    """Validate exact custody bytes at an explicit staging or final path."""
    validation_context = context or (
        os.fspath(archive_path) if archive_path is not None else "native-link custody"
    )
    _value, entries = validate_native_link_custody(
        custody,
        context=validation_context,
    )
    if not entries:
        if archive_path is not None:
            raise NativeLinkCustodyError(
                "empty native-link custody unexpectedly names an archive path: "
                f"{validation_context}"
            )
        return
    if archive_path is None:
        raise NativeLinkCustodyError(
            f"native-link custody archive path is missing: {validation_context}"
        )
    archive = cast(Mapping[str, object], custody["archive"])
    _validate_archive_file(archive_path, archive_record=archive, entries=entries)


def publish_native_link_custody(
    runtime_lib: Path,
    files: Sequence[Path],
) -> tuple[dict[str, object], dict[Path, str]]:
    described: dict[str, tuple[NativeLinkCustodyEntry, StableRegularFileIdentity]] = {}
    path_to_id: dict[Path, str] = {}
    for path in files:
        entry, identity = _describe_file(path)
        existing = described.get(entry.identifier)
        if existing is not None and existing[0] != entry:
            raise NativeLinkCustodyError(
                f"native-link custody identity collision for {identity.path}"
            )
        described[entry.identifier] = (entry, identity)
        path_to_id[identity.path] = entry.identifier
    entries = tuple(described[key][0] for key in sorted(described))
    if not entries:
        return {
            "schema": _CUSTODY_SCHEMA,
            "archive": None,
            "entries": [],
        }, path_to_id

    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    stage = runtime_lib.parent / f".{_ARCHIVE_PREFIX}{uuid.uuid4().hex}.stage.tar"
    try:
        try:
            _write_archive(
                stage,
                entries,
                {
                    identifier: source
                    for identifier, (_entry, source) in described.items()
                },
            )
        except (OSError, tarfile.TarError, ValueError) as exc:
            raise NativeLinkCustodyError(
                f"cannot create deterministic native-link custody archive: {exc}"
            ) from exc
        stage_identity = _file_identity(stage)
        stage_size = stage_identity.size
        if stage_size > _MAX_ARCHIVE_BYTES:
            raise NativeLinkCustodyError(
                "native-link custody archive exceeds the size limit"
            )
        archive_digest = stage_identity.sha256
        archive_name = f"{_ARCHIVE_PREFIX}{archive_digest}{_ARCHIVE_SUFFIX}"
        archive_record: dict[str, object] = {
            "name": archive_name,
            "sha256": archive_digest,
            "size_bytes": stage_size,
        }
        _validate_archive_file(stage, archive_record=archive_record, entries=entries)
        final = runtime_lib.parent / archive_name
        if final.exists():
            _validate_archive_file(
                final, archive_record=archive_record, entries=entries
            )
        else:
            try:
                durable_publish_exclusive(stage, final)
            except OSError:
                if not final.exists():
                    raise
                _validate_archive_file(
                    final, archive_record=archive_record, entries=entries
                )
        return {
            "schema": _CUSTODY_SCHEMA,
            "archive": archive_record,
            "entries": [entry.to_dict() for entry in entries],
        }, path_to_id
    finally:
        with contextlib.suppress(OSError):
            stage.unlink()


def _validate_extracted_root(
    root: Path,
    entries: Sequence[NativeLinkCustodyEntry],
) -> dict[str, Path]:
    if not root.is_dir() or root.is_symlink() or root.is_junction():
        raise NativeLinkCustodyError(
            f"native-link custody extraction is invalid: {root}"
        )
    expected_files = {PurePosixPath(entry.archive_path) for entry in entries}
    observed_files: set[PurePosixPath] = set()
    for directory, dirnames, filenames in os.walk(root):
        directory_path = Path(directory)
        for name in dirnames:
            child = directory_path / name
            if child.is_symlink() or child.is_junction():
                raise NativeLinkCustodyError(
                    f"native-link custody extraction contains a link: {child}"
                )
        for name in filenames:
            child = directory_path / name
            if child.is_symlink() or child.is_junction():
                raise NativeLinkCustodyError(
                    f"native-link custody extraction contains a link: {child}"
                )
            relative = PurePosixPath(child.relative_to(root).as_posix())
            observed_files.add(relative)
    if observed_files != expected_files:
        raise NativeLinkCustodyError(
            f"native-link custody extraction closure mismatch: {root}"
        )
    result: dict[str, Path] = {}
    for entry in entries:
        path = root.joinpath(*PurePosixPath(entry.archive_path).parts)
        identity = _file_identity(path)
        if identity.size != entry.size_bytes or identity.sha256 != entry.sha256:
            raise NativeLinkCustodyError(
                f"native-link custody extracted file identity mismatch: {path}"
            )
        result[entry.identifier] = identity.path
    return result


def ensure_native_link_custody(
    runtime_lib: Path,
    custody: Mapping[str, object],
) -> dict[str, Path]:
    _value, entries = validate_native_link_custody(custody, context=str(runtime_lib))
    if not entries:
        return {}
    archive = cast(Mapping[str, object], custody["archive"])
    archive_path = native_link_custody_archive_path(runtime_lib, custody)
    assert archive_path is not None
    validate_native_link_custody_archive(
        archive_path,
        custody,
        context=os.fspath(runtime_lib),
    )
    digest = archive["sha256"]
    assert isinstance(digest, str)
    extraction_root = runtime_lib.parent / f"{_EXTRACTION_PREFIX}{digest}"
    if extraction_root.exists():
        return _validate_extracted_root(extraction_root, entries)

    stage = (
        runtime_lib.parent / f".{_EXTRACTION_PREFIX}{digest}.{uuid.uuid4().hex}.stage"
    )
    stage.mkdir(parents=False, exist_ok=False)
    try:
        archive_identity = _file_identity(archive_path)
        if (
            archive_identity.sha256 != archive["sha256"]
            or archive_identity.size != archive["size_bytes"]
        ):
            raise NativeLinkCustodyError(
                f"native-link custody archive changed before extracting: {archive_path}"
            )
        stage_root = stage.resolve(strict=True)
        with (
            open_stable_regular_file(
                archive_path, label="native custody extraction"
            ) as opened,
            tarfile.open(
                fileobj=opened.stream, mode="r:", tarinfo=RegularUstarTarInfo
            ) as source,
        ):
            verify_stable_regular_file_identity(
                archive_identity, label="native custody extraction"
            )
            by_path = {entry.archive_path: entry for entry in entries}
            observed_order: list[str] = []
            observed: set[str] = set()
            for member in source:
                member_path = _safe_archive_member(member.name).as_posix()
                if member_path in observed:
                    raise NativeLinkCustodyError(
                        f"duplicate native-link custody archive member: {member_path}"
                    )
                observed.add(member_path)
                observed_order.append(member_path)
                entry = by_path.get(member_path)
                if entry is None or not _archive_member_is_canonical(member, entry):
                    raise NativeLinkCustodyError(
                        f"invalid native-link custody archive member: {member_path}"
                    )
                destination = stage_root.joinpath(
                    *_safe_archive_member(entry.archive_path).parts
                )
                if not destination.resolve().is_relative_to(stage_root):
                    raise NativeLinkCustodyError(
                        f"native-link custody member escapes extraction: {entry.archive_path}"
                    )
                destination.parent.mkdir(parents=True, exist_ok=True)
                stream = source.extractfile(member)
                if stream is None:
                    raise NativeLinkCustodyError(
                        f"unreadable native-link custody member: {member.name}"
                    )
                digest_state = hashlib.sha256()
                copied = 0
                with destination.open("xb") as output:
                    while block := stream.read(1024 * 1024):
                        copied += len(block)
                        if copied > entry.size_bytes:
                            raise NativeLinkCustodyError(
                                f"native-link custody member exceeds declared size: {member.name}"
                            )
                        digest_state.update(block)
                        output.write(block)
                    output.flush()
                    os.fsync(output.fileno())
                destination.chmod(0o644)
                if (
                    copied != entry.size_bytes
                    or digest_state.hexdigest() != entry.sha256
                ):
                    raise NativeLinkCustodyError(
                        f"native-link custody member identity mismatch: {member.name}"
                    )
            if tuple(observed_order) != tuple(entry.archive_path for entry in entries):
                raise NativeLinkCustodyError(
                    "native-link custody archive member closure mismatch"
                )
            verify_stable_regular_file_identity(
                archive_identity, label="native custody extraction"
            )
        _validate_extracted_root(stage, entries)
        try:
            durable_publish_directory_exclusive(stage, extraction_root)
        except OSError:
            if not extraction_root.exists():
                raise
            _validate_extracted_root(extraction_root, entries)
        return _validate_extracted_root(extraction_root, entries)
    except (OSError, tarfile.TarError, ValueError) as exc:
        raise NativeLinkCustodyError(
            f"cannot extract native-link custody archive {archive_path}: {exc}"
        ) from exc
    finally:
        if stage.exists():
            with contextlib.suppress(OSError):
                _remove_file_or_tree(stage)


def copy_native_link_custody_archive(
    source_runtime_lib: Path,
    destination_runtime_lib: Path,
    custody: Mapping[str, object],
) -> Path | None:
    _value, entries = validate_native_link_custody(
        custody,
        context=str(source_runtime_lib),
    )
    if not entries:
        return None
    source = native_link_custody_archive_path(source_runtime_lib, custody)
    destination = native_link_custody_archive_path(destination_runtime_lib, custody)
    assert source is not None and destination is not None
    validate_native_link_custody_archive(
        source,
        custody,
        context=os.fspath(source_runtime_lib),
    )
    if destination.exists():
        validate_native_link_custody_archive(
            destination,
            custody,
            context=os.fspath(destination_runtime_lib),
        )
    else:
        _atomic_copy_file(source, destination)
        validate_native_link_custody_archive(
            destination,
            custody,
            context=os.fspath(destination_runtime_lib),
        )
    return destination

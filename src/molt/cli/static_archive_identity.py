from __future__ import annotations

import hashlib
from pathlib import Path
import re
from typing import BinaryIO, Mapping, cast

from molt.toolchain_identity import open_stable_regular_file
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES


_ARCHIVE_MAGIC = b"!<arch>\n"
_THIN_ARCHIVE_MAGIC = b"!<thin>\n"
_HEADER_SIZE = 60
_IDENTITY_SCHEMA = "molt.static-archive-semantic.v1"
_BYTE_IDENTITY_SCHEMA = "molt.artifact-bytes.v1"
_DERIVED_MEMBER_NAMES = frozenset(
    {
        "/",
        "//",
        "/SYM64/",
        "__.SYMDEF",
        "__.SYMDEF SORTED",
        "__.SYMDEF_64",
        "__.SYMDEF_64 SORTED",
        "__.LLVM_SYM_TAB",
    }
)
_MSVC_RUST_CGU_RE = re.compile(
    r"^(?P<crate>molt_[A-Za-z0-9_]+)(?:-(?P<file_id>[0-9a-f]+))?\."
    r"(?P<cgu>[0-9a-z]+)\."
    r"(?P<crate_id>[0-9a-z]+)(?P<suffix>\.rcgu\.o)$"
)
_UNIX_RUST_CGU_RE = re.compile(
    r"^(?P<library>lib)?(?P<crate>molt_[A-Za-z0-9_]+)"
    r"(?:-(?P<file_id>[0-9a-f]+))?\.(?P=crate)\."
    r"(?P<crate_id>[0-9a-z]+)(?P<suffix>-cgu\.[0-9]+\.rcgu\.o)$"
)


class StaticArchiveIdentityError(ValueError):
    """The archive is malformed or changed while its semantic identity was read."""


def validate_artifact_content_identity(value: object) -> Mapping[str, object]:
    """Admit one exact artifact receipt without Python numeric coercion."""
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise StaticArchiveIdentityError("artifact content identity must be an object")
    receipt = cast(Mapping[str, object], value)
    schema = receipt.get("schema")
    if schema == _BYTE_IDENTITY_SCHEMA:
        digest_key = "sha256"
        count_keys = ("size_bytes",)
    elif schema == _IDENTITY_SCHEMA:
        digest_key = "semantic_sha256"
        count_keys = ("member_count", "content_size_bytes")
    else:
        raise StaticArchiveIdentityError(
            "artifact content identity schema is unsupported"
        )
    if set(receipt) != {"schema", digest_key, *count_keys}:
        raise StaticArchiveIdentityError("artifact content identity shape is invalid")
    digest = receipt.get(digest_key)
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise StaticArchiveIdentityError("artifact content identity digest is invalid")
    for key in count_keys:
        count = receipt.get(key)
        if type(count) is not int or count < 0:
            raise StaticArchiveIdentityError(
                f"artifact content identity {key} must be a nonnegative integer"
            )
    return dict(receipt)


def _decimal_field(raw: bytes, *, field: str) -> int:
    try:
        value = raw.decode("ascii").strip()
        parsed = int(value) if value else 0
    except (UnicodeDecodeError, ValueError) as exc:
        raise StaticArchiveIdentityError(f"invalid archive {field} field") from exc
    if parsed < 0:
        raise StaticArchiveIdentityError(f"invalid archive {field} field")
    return parsed


def _hash_exact(stream: BinaryIO, size: int) -> str:
    digest = hashlib.sha256()
    remaining = size
    while remaining:
        block = stream.read(min(8 * 1024 * 1024, remaining))
        if not block:
            raise StaticArchiveIdentityError("truncated archive member payload")
        digest.update(block)
        remaining -= len(block)
    return digest.hexdigest()


def _read_exact(stream: BinaryIO, size: int) -> bytes:
    if size > RUNTIME_ARTIFACT_METADATA_MAX_BYTES:
        raise StaticArchiveIdentityError(
            "archive name metadata exceeds bounded input limit"
        )
    value = stream.read(size)
    if len(value) != size:
        raise StaticArchiveIdentityError("truncated archive member payload")
    return value


def _long_name(table: bytes, offset: int) -> str:
    if offset < 0 or offset >= len(table):
        raise StaticArchiveIdentityError("archive long-name offset is out of range")
    nul = table.find(b"\0", offset)
    gnu = table.find(b"/\n", offset)
    ends = [end for end in (nul, gnu) if end >= 0]
    end = min(ends) if ends else len(table)
    try:
        name = table[offset:end].decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise StaticArchiveIdentityError("archive member name is not UTF-8") from exc
    if not name:
        raise StaticArchiveIdentityError("archive member name is empty")
    return name


def _canonical_member_name(name: str) -> str:
    prefix, separator, basename = name.rpartition("/")
    preserved_prefix = f"{prefix}{separator}" if separator else ""
    msvc = _MSVC_RUST_CGU_RE.fullmatch(basename)
    if msvc is not None:
        file_id = f"-{msvc['file_id']}" if msvc["file_id"] else ""
        return (
            f"{preserved_prefix}{msvc['crate']}{file_id}.{msvc['cgu']}."
            f"<rustc-crate-id>{msvc['suffix']}"
        )
    unix = _UNIX_RUST_CGU_RE.fullmatch(basename)
    if unix is not None:
        library = unix["library"] or ""
        file_id = "-<rustc-file-id>" if unix["file_id"] else ""
        return (
            f"{preserved_prefix}{library}{unix['crate']}{file_id}."
            f"{unix['crate']}."
            f"<rustc-crate-id>{unix['suffix']}"
        )
    return name


def _static_archive_stream_identity(stream: BinaryIO) -> dict[str, object]:
    entries: list[tuple[str, int, str, bool]] = []
    long_names: bytes | None = None
    magic = stream.read(len(_ARCHIVE_MAGIC))
    if magic == _THIN_ARCHIVE_MAGIC:
        raise StaticArchiveIdentityError("thin archives are not self-contained")
    if magic != _ARCHIVE_MAGIC:
        raise StaticArchiveIdentityError("static archive magic is invalid")
    while True:
        header = stream.read(_HEADER_SIZE)
        if not header:
            break
        if len(header) != _HEADER_SIZE or header[58:60] != b"`\n":
            raise StaticArchiveIdentityError("static archive header is invalid")
        try:
            raw_name = header[:16].decode("ascii", "strict").strip()
        except UnicodeDecodeError as exc:
            raise StaticArchiveIdentityError(
                "archive member header name is not ASCII"
            ) from exc
        stored_size = _decimal_field(header[48:58], field="size")
        content_size = stored_size
        if raw_name == "//":
            long_names = _read_exact(stream, stored_size)
            content_digest = ""
        elif raw_name.startswith("#1/"):
            name_size = _decimal_field(raw_name[3:].encode("ascii"), field="name")
            if name_size > stored_size:
                raise StaticArchiveIdentityError(
                    "BSD archive member name exceeds member size"
                )
            name_bytes = _read_exact(stream, name_size).rstrip(b"\0")
            try:
                name = name_bytes.decode("utf-8", "strict")
            except UnicodeDecodeError as exc:
                raise StaticArchiveIdentityError(
                    "archive member name is not UTF-8"
                ) from exc
            content_size -= name_size
            content_digest = _hash_exact(stream, content_size)
            if name not in _DERIVED_MEMBER_NAMES:
                entries.append((name, content_size, content_digest, True))
        else:
            content_digest = _hash_exact(stream, stored_size)
            short_name = raw_name.removesuffix("/")
            if (
                raw_name not in _DERIVED_MEMBER_NAMES
                and short_name not in _DERIVED_MEMBER_NAMES
            ):
                entries.append((raw_name, content_size, content_digest, False))
        if stored_size & 1:
            if stream.read(1) != b"\n":
                raise StaticArchiveIdentityError("archive padding byte is invalid")
    if long_names is None and any(
        name.startswith("/") and not resolved for name, _, _, resolved in entries
    ):
        raise StaticArchiveIdentityError("archive long-name table is missing")
    records: list[tuple[str, int, str]] = []
    for raw_name, content_size, content_digest, resolved in entries:
        if resolved:
            name = raw_name
        elif raw_name.startswith("/"):
            try:
                offset = int(raw_name[1:])
            except ValueError as exc:
                raise StaticArchiveIdentityError(
                    f"unsupported archive member name {raw_name!r}"
                ) from exc
            assert long_names is not None
            name = _long_name(long_names, offset)
        else:
            name = raw_name.removesuffix("/")
        if not name:
            raise StaticArchiveIdentityError("archive member name is empty")
        records.append((name, content_size, content_digest))
    digest = hashlib.sha256()
    digest.update((_IDENTITY_SCHEMA + "\n").encode("ascii"))
    for ordinal, (name, size, content_digest) in enumerate(records):
        canonical_name = _canonical_member_name(name).encode("utf-8")
        digest.update(ordinal.to_bytes(8, "big"))
        digest.update(len(canonical_name).to_bytes(4, "big"))
        digest.update(canonical_name)
        digest.update(size.to_bytes(8, "big"))
        digest.update(bytes.fromhex(content_digest))
    return {
        "schema": _IDENTITY_SCHEMA,
        "semantic_sha256": digest.hexdigest(),
        "member_count": len(records),
        "content_size_bytes": sum(size for _, size, _ in records),
    }


def static_archive_identity(path: Path) -> dict[str, object]:
    """Hash ordered archive members through one stable direct-file handle."""
    try:
        with open_stable_regular_file(path, label="static archive") as opened:
            return _static_archive_stream_identity(opened.stream)
    except (OSError, ValueError) as exc:
        if isinstance(exc, StaticArchiveIdentityError):
            raise
        raise StaticArchiveIdentityError(
            f"cannot identify static archive {path}: {exc}"
        ) from exc


def artifact_content_identity(path: Path) -> dict[str, object]:
    """Read current artifact bytes once; metadata never authorizes cached content."""
    try:
        with open_stable_regular_file(path, label="runtime artifact") as opened:
            stream = opened.stream
            prefix = stream.read(len(_ARCHIVE_MAGIC))
            if path.suffix.lower() in {".a", ".lib"} and prefix in {
                _ARCHIVE_MAGIC,
                _THIN_ARCHIVE_MAGIC,
            }:
                stream.seek(0)
                return _static_archive_stream_identity(stream)
            digest = hashlib.sha256(prefix)
            size = len(prefix)
            while block := stream.read(8 * 1024 * 1024):
                digest.update(block)
                size += len(block)
            return {
                "schema": _BYTE_IDENTITY_SCHEMA,
                "sha256": digest.hexdigest(),
                "size_bytes": size,
            }
    except (OSError, ValueError) as exc:
        if isinstance(exc, StaticArchiveIdentityError):
            raise
        raise StaticArchiveIdentityError(
            f"cannot identify artifact {path}: {exc}"
        ) from exc

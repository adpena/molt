"""Deterministic content-addressed artifacts for bulky proof custody evidence."""

from __future__ import annotations

from dataclasses import dataclass
import gzip
import hashlib
import json
import os
from pathlib import Path
import stat
import tempfile
from typing import Mapping
import zlib


ARTIFACT_SCHEMA = "molt.proof-custody-artifact.v1"
REF_SCHEMA = "molt.proof-custody-artifact-ref.v1"
FILE_REF_SCHEMA = "molt.proof-custody-file-ref.v1"
JSON_GZIP_MEDIA_TYPE = "application/json+gzip"
FILE_MEDIA_TYPE = "application/octet-stream"
MAX_UNCOMPRESSED_BYTES = 512 * 1024 * 1024
# A gzip member can be slightly larger than its input. Keep the compressed
# transport bounded independently instead of trusting a declared length.
MAX_COMPRESSED_BYTES = MAX_UNCOMPRESSED_BYTES + 1024 * 1024
_READ_CHUNK_BYTES = 1024 * 1024


def _fsync_directory(path: Path) -> None:
    if os.name == "nt":
        return
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _is_link_like(path: Path) -> bool:
    if path.is_symlink():
        return True
    is_junction = getattr(path, "is_junction", None)
    return bool(is_junction is not None and is_junction())


def _durable_makedirs(path: Path) -> None:
    """Create a directory chain and durably publish every new entry on POSIX."""
    missing: list[Path] = []
    cursor = path
    while not cursor.exists():
        missing.append(cursor)
        parent = cursor.parent
        if parent == cursor:
            break
        cursor = parent
    if cursor.exists() and (not cursor.is_dir() or _is_link_like(cursor)):
        raise ValueError(f"proof custody directory is not a real directory: {cursor}")
    for directory in reversed(missing):
        try:
            directory.mkdir()
        except FileExistsError:
            pass
        if not directory.is_dir() or _is_link_like(directory):
            raise ValueError(
                f"proof custody directory is not a real directory: {directory}"
            )
        # Persist both the new directory inode and its parent's directory entry.
        _fsync_directory(directory)
        _fsync_directory(directory.parent)


def _reject_link_components(path: Path) -> None:
    anchor = Path(path.anchor)
    cursor = anchor
    for part in path.parts[1:] if path.anchor else path.parts:
        cursor /= part
        if cursor.exists() and _is_link_like(cursor):
            raise ValueError(
                f"proof custody path traverses a link or junction: {cursor}"
            )


def _canonical_root(root: Path, *, create: bool) -> Path:
    lexical = root.expanduser().absolute()
    _reject_link_components(lexical)
    if create:
        canonical = lexical.resolve()
        _durable_makedirs(canonical)
    else:
        try:
            canonical = lexical.resolve(strict=True)
        except FileNotFoundError as exc:
            raise ValueError("proof custody expected CAS root does not exist") from exc
    if not canonical.is_dir() or _is_link_like(canonical):
        raise ValueError("proof custody CAS root is not a real directory")
    _reject_link_components(lexical)
    return canonical


def _cas_directory(root: Path, *parts: str) -> Path:
    directory = root
    for part in parts:
        directory /= part
        if directory.exists() and _is_link_like(directory):
            raise ValueError(
                f"proof custody CAS namespace contains a link: {directory}"
            )
        _durable_makedirs(directory)
    return directory


def _normalized_path(path: Path) -> str:
    return os.path.normcase(os.path.normpath(os.path.abspath(path)))


def _require_canonical_path(path: str, expected: Path, root: Path) -> Path:
    supplied = Path(path)
    if not supplied.is_absolute() or _normalized_path(supplied) != _normalized_path(
        expected
    ):
        raise ValueError(
            "proof custody reference does not use the canonical CAS layout"
        )
    try:
        relative = expected.relative_to(root)
    except ValueError as exc:  # Defensive: expected paths are built from root.
        raise ValueError(
            "proof custody reference escapes the expected CAS root"
        ) from exc
    cursor = root
    for part in relative.parts:
        cursor /= part
        if _is_link_like(cursor):
            raise ValueError("proof custody reference traverses a link or junction")
    resolved = supplied.resolve(strict=True)
    if resolved != expected or not resolved.is_file():
        raise ValueError("proof custody reference escapes the expected CAS root")
    return resolved


def _blob_path(root: Path, sha256: str) -> Path:
    return root / "blobs" / "sha256" / sha256[:2] / f"{sha256[2:]}.json.gz"


def _file_path(root: Path, sha256: str, name: str, executable: bool) -> Path:
    mode = "executable" if executable else "data"
    return root / "files" / "sha256" / mode / sha256[:2] / sha256[2:] / name


def _durable_replace(source: Path, target: Path) -> None:
    """Atomically publish a file and durably commit its directory entry."""
    if os.name == "nt":
        import ctypes

        move = ctypes.windll.kernel32.MoveFileExW
        move.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint32]
        move.restype = ctypes.c_int
        # REPLACE_EXISTING | WRITE_THROUGH. Both paths are same-volume staging.
        if not move(str(source), str(target), 0x1 | 0x8):
            raise ctypes.WinError()
        return
    os.replace(source, target)
    directory = os.open(target.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def atomic_write_bytes(path: Path, payload: bytes) -> None:
    """Write one same-volume, fsync-sealed atomic terminal artifact."""
    _durable_makedirs(path.parent)
    fd, temporary_raw = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_raw)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        _durable_replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


@dataclass(frozen=True)
class ArtifactRef:
    path: str
    blob_sha256: str
    semantic_sha256: str
    compressed_bytes: int
    uncompressed_bytes: int

    def as_dict(self) -> dict[str, object]:
        return {
            "schema": REF_SCHEMA,
            "path": self.path,
            "media_type": JSON_GZIP_MEDIA_TYPE,
            "blob_sha256": self.blob_sha256,
            "semantic_sha256": self.semantic_sha256,
            "compressed_bytes": self.compressed_bytes,
            "uncompressed_bytes": self.uncompressed_bytes,
        }


@dataclass(frozen=True)
class FileRef:
    path: str
    sha256: str
    size_bytes: int
    executable: bool

    def as_dict(self) -> dict[str, object]:
        return {
            "schema": FILE_REF_SCHEMA,
            "path": self.path,
            "media_type": FILE_MEDIA_TYPE,
            "sha256": self.sha256,
            "size_bytes": self.size_bytes,
            "executable": self.executable,
        }


def _canonical_bytes(payload: Mapping[str, object]) -> bytes:
    return json.dumps(
        payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode()


def _gzip_bytes(payload: bytes) -> bytes:
    # An empty filename and zero mtime make the byte authority independent of
    # staging path and wall clock.
    import io

    output = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as stream:
        stream.write(payload)
    return output.getvalue()


def put_json(root: Path, payload: Mapping[str, object]) -> ArtifactRef:
    """Atomically publish one canonical blob, deduplicating concurrent writers."""
    canonical = _canonical_bytes(payload)
    if len(canonical) > MAX_UNCOMPRESSED_BYTES:
        raise ValueError("proof custody artifact exceeds the fail-closed size ceiling")
    semantic_sha256 = hashlib.sha256(canonical).hexdigest()
    compressed = _gzip_bytes(canonical)
    if len(compressed) > MAX_COMPRESSED_BYTES:
        raise ValueError("proof custody artifact exceeds the compressed size ceiling")
    blob_sha256 = hashlib.sha256(compressed).hexdigest()
    root = _canonical_root(root, create=True)
    directory = _cas_directory(root, "blobs", "sha256", blob_sha256[:2])
    target = _blob_path(root, blob_sha256)
    if not target.exists():
        fd, temporary_raw = tempfile.mkstemp(prefix=".custody-", dir=directory)
        temporary = Path(temporary_raw)
        try:
            with os.fdopen(fd, "wb") as stream:
                stream.write(compressed)
                stream.flush()
                os.fsync(stream.fileno())
            try:
                _durable_replace(temporary, target)
            except PermissionError:
                if not target.exists():
                    raise
        finally:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass
    reference = ArtifactRef(
        path=str(target),
        blob_sha256=blob_sha256,
        semantic_sha256=semantic_sha256,
        compressed_bytes=len(compressed),
        uncompressed_bytes=len(canonical),
    )
    verify_ref(reference.as_dict(), expected_root=root)
    return reference


def put_file(
    root: Path,
    source: Path,
    *,
    logical_name: str | None = None,
    executable: bool = False,
) -> FileRef:
    """Publish immutable file bytes under their digest, never hard-linking source."""
    source = source.resolve(strict=True)
    if not source.is_file():
        raise ValueError("proof custody source is not a regular file")
    name = logical_name or source.name
    if not name or Path(name).name != name:
        raise ValueError("proof custody file logical name must be one path component")
    digest = hashlib.sha256()
    size = 0
    with source.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
            size += len(chunk)
    sha256 = digest.hexdigest()
    root = _canonical_root(root, create=True)
    mode = "executable" if executable else "data"
    directory = _cas_directory(root, "files", "sha256", mode, sha256[:2], sha256[2:])
    target = _file_path(root, sha256, name, executable)
    if not target.exists():
        fd, temporary_raw = tempfile.mkstemp(prefix=".custody-file-", dir=directory)
        temporary = Path(temporary_raw)
        copied = hashlib.sha256()
        copied_size = 0
        try:
            with source.open("rb") as input_stream, os.fdopen(fd, "wb") as output:
                for chunk in iter(lambda: input_stream.read(1024 * 1024), b""):
                    output.write(chunk)
                    copied.update(chunk)
                    copied_size += len(chunk)
                output.flush()
                os.fsync(output.fileno())
            if copied.hexdigest() != sha256 or copied_size != size:
                raise ValueError("proof custody source changed while being published")
            if os.name != "nt":
                temporary.chmod(0o555 if executable else 0o444)
            try:
                _durable_replace(temporary, target)
            except PermissionError:
                if not target.exists():
                    raise
        finally:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass
    reference = FileRef(str(target), sha256, size, executable)
    verify_file_ref(reference.as_dict(), expected_root=root)
    return reference


def parse_ref(raw: Mapping[str, object]) -> ArtifactRef:
    if raw.get("schema") != REF_SCHEMA:
        raise ValueError("proof custody artifact reference schema mismatch")
    if raw.get("media_type") != JSON_GZIP_MEDIA_TYPE:
        raise ValueError("proof custody artifact reference media type mismatch")
    values = {
        name: raw.get(name)
        for name in (
            "path",
            "blob_sha256",
            "semantic_sha256",
            "compressed_bytes",
            "uncompressed_bytes",
        )
    }
    if not isinstance(values["path"], str):
        raise ValueError("proof custody artifact reference has no path")
    for name in ("blob_sha256", "semantic_sha256"):
        value = values[name]
        if (
            not isinstance(value, str)
            or len(value) != 64
            or value != value.lower()
            or any(character not in "0123456789abcdef" for character in value)
        ):
            raise ValueError(f"proof custody artifact reference has invalid {name}")
    for name in ("compressed_bytes", "uncompressed_bytes"):
        value = values[name]
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise ValueError(f"proof custody artifact reference has invalid {name}")
    if int(values["uncompressed_bytes"]) > MAX_UNCOMPRESSED_BYTES:
        raise ValueError("proof custody artifact reference exceeds the size ceiling")
    if int(values["compressed_bytes"]) > MAX_COMPRESSED_BYTES:
        raise ValueError(
            "proof custody artifact reference exceeds the compressed size ceiling"
        )
    return ArtifactRef(**values)  # type: ignore[arg-type]


def _read_compressed_artifact(path: Path, reference: ArtifactRef) -> tuple[bytes, str]:
    stat_size = path.stat().st_size
    if stat_size != reference.compressed_bytes:
        raise ValueError("proof custody artifact compressed size changed")
    if stat_size > MAX_COMPRESSED_BYTES:
        raise ValueError("proof custody artifact exceeds the compressed size ceiling")

    compressed_digest = hashlib.sha256()
    decompressor = zlib.decompressobj(16 + zlib.MAX_WBITS)
    output = bytearray()
    compressed_size = 0
    saw_trailing_data = False
    decompression_error: zlib.error | None = None
    with path.open("rb") as stream:
        while chunk := stream.read(_READ_CHUNK_BYTES):
            compressed_size += len(chunk)
            if compressed_size > reference.compressed_bytes:
                raise ValueError("proof custody artifact compressed size changed")
            if compressed_size > MAX_COMPRESSED_BYTES:
                raise ValueError(
                    "proof custody artifact exceeds the compressed size ceiling"
                )
            compressed_digest.update(chunk)
            if decompression_error is not None:
                continue
            if decompressor.eof:
                saw_trailing_data = True
                continue
            pending = chunk
            while pending:
                remaining = min(
                    reference.uncompressed_bytes, MAX_UNCOMPRESSED_BYTES
                ) - len(output)
                if remaining < 0:
                    raise ValueError("proof custody artifact exceeds the size ceiling")
                try:
                    produced = decompressor.decompress(pending, remaining + 1)
                except zlib.error as exc:
                    # Continue the bounded raw read so the blob authority gets a
                    # precise digest-mismatch diagnostic before codec details.
                    decompression_error = exc
                    break
                output.extend(produced)
                if len(output) > reference.uncompressed_bytes:
                    raise ValueError(
                        "proof custody artifact exceeds its declared uncompressed size"
                    )
                if len(output) > MAX_UNCOMPRESSED_BYTES:
                    raise ValueError("proof custody artifact exceeds the size ceiling")
                if decompressor.unused_data:
                    saw_trailing_data = True
                    break
                pending = decompressor.unconsumed_tail
                if not pending:
                    break
    if compressed_size != reference.compressed_bytes:
        raise ValueError("proof custody artifact compressed size changed")
    if compressed_digest.hexdigest() != reference.blob_sha256:
        raise ValueError("proof custody artifact blob digest changed")
    if decompression_error is not None:
        raise ValueError(
            "proof custody artifact is not valid deterministic gzip"
        ) from decompression_error
    if not decompressor.eof:
        raise ValueError("proof custody artifact gzip stream is truncated")
    if saw_trailing_data:
        raise ValueError("proof custody artifact has trailing gzip data")
    remaining = min(reference.uncompressed_bytes, MAX_UNCOMPRESSED_BYTES) - len(output)
    flushed = decompressor.flush(max(1, remaining + 1))
    output.extend(flushed)
    if len(output) > reference.uncompressed_bytes:
        raise ValueError(
            "proof custody artifact exceeds its declared uncompressed size"
        )
    if len(output) > MAX_UNCOMPRESSED_BYTES:
        raise ValueError("proof custody artifact exceeds the size ceiling")
    return bytes(output), compressed_digest.hexdigest()


def read_ref(raw: Mapping[str, object], *, expected_root: Path) -> dict[str, object]:
    reference = parse_ref(raw)
    root = _canonical_root(expected_root, create=False)
    expected = _blob_path(root, reference.blob_sha256)
    path = _require_canonical_path(reference.path, expected, root)
    canonical, _blob_sha256 = _read_compressed_artifact(path, reference)
    if len(canonical) != reference.uncompressed_bytes:
        raise ValueError("proof custody artifact uncompressed size changed")
    if len(canonical) > MAX_UNCOMPRESSED_BYTES:
        raise ValueError("proof custody artifact exceeds the size ceiling")
    if hashlib.sha256(canonical).hexdigest() != reference.semantic_sha256:
        raise ValueError("proof custody artifact semantic digest changed")
    try:
        payload = json.loads(canonical)
    except json.JSONDecodeError as exc:
        raise ValueError("proof custody artifact payload is invalid JSON") from exc
    if not isinstance(payload, dict) or payload.get("schema") != ARTIFACT_SCHEMA:
        raise ValueError("proof custody artifact payload schema mismatch")
    if _canonical_bytes(payload) != canonical:
        raise ValueError("proof custody artifact payload is not canonical")
    return payload


def verify_ref(raw: Mapping[str, object], *, expected_root: Path) -> None:
    read_ref(raw, expected_root=expected_root)


def parse_file_ref(raw: Mapping[str, object]) -> FileRef:
    if raw.get("schema") != FILE_REF_SCHEMA:
        raise ValueError("proof custody file reference schema mismatch")
    if raw.get("media_type") != FILE_MEDIA_TYPE:
        raise ValueError("proof custody file reference media type mismatch")
    path = raw.get("path")
    sha256 = raw.get("sha256")
    size = raw.get("size_bytes")
    executable = raw.get("executable")
    if not isinstance(path, str):
        raise ValueError("proof custody file reference has no path")
    if (
        not isinstance(sha256, str)
        or len(sha256) != 64
        or sha256 != sha256.lower()
        or any(character not in "0123456789abcdef" for character in sha256)
    ):
        raise ValueError("proof custody file reference has invalid sha256")
    if not isinstance(size, int) or isinstance(size, bool) or size < 0:
        raise ValueError("proof custody file reference has invalid size")
    if not isinstance(executable, bool):
        raise ValueError("proof custody file reference has invalid executable flag")
    return FileRef(path, sha256, size, executable)


def verify_file_ref(raw: Mapping[str, object], *, expected_root: Path) -> None:
    reference = parse_file_ref(raw)
    root = _canonical_root(expected_root, create=False)
    logical_name = Path(reference.path).name
    if not logical_name or Path(logical_name).name != logical_name:
        raise ValueError("proof custody file reference has invalid logical name")
    expected = _file_path(root, reference.sha256, logical_name, reference.executable)
    path = _require_canonical_path(reference.path, expected, root)
    file_stat = path.stat()
    if file_stat.st_size != reference.size_bytes:
        raise ValueError("proof custody file size changed")
    with path.open("rb") as stream:
        actual = hashlib.file_digest(stream, "sha256").hexdigest()
    if actual != reference.sha256:
        raise ValueError("proof custody file digest changed")
    if os.name != "nt":
        actual_mode = stat.S_IMODE(file_stat.st_mode)
        expected_mode = 0o555 if reference.executable else 0o444
        if actual_mode != expected_mode:
            kind = "executable" if reference.executable else "non-executable"
            raise ValueError(
                f"proof custody {kind} file mode changed: "
                f"expected {expected_mode:o}, got {actual_mode:o}"
            )

"""Canonical, bounded ZIP transport for release bundles and evidence.

Callers retain custody of the source tree and destination parent during the
operation. Files are read through the shared no-follow mutation authority;
private staging is validated before exclusive durable publication. Failure
never removes the public destination or an identity-replaced staging leaf.
"""

from __future__ import annotations

from dataclasses import dataclass
import datetime as dt
import hashlib
import math
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import struct
import tempfile
from typing import BinaryIO, Callable
import warnings
import zipfile

from molt.file_publication import (
    durable_publish_directory_exclusive,
    durable_publish_exclusive,
    is_link_like,
    metadata_is_link_like,
    resolve_owned_path,
)
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.toolchain_identity import StableRegularFileHandle, open_stable_regular_file

MIN_ZIP_EPOCH = 315532800
MAX_ZIP_EPOCH = 4_354_819_198
_COPY_CHUNK_SIZE = 1024 * 1024
_MAX_METADATA_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True, slots=True)
class ArchivePolicy:
    """Resource envelope applied before reading payloads or publishing output."""

    max_archive_bytes: int = 2 * 1024 * 1024 * 1024
    max_members: int = 4096
    max_file_bytes: int = 1024 * 1024 * 1024
    max_total_bytes: int = 2 * 1024 * 1024 * 1024
    max_compression_ratio: float = 200.0

    def __post_init__(self) -> None:
        if any(
            type(value) is not int or value <= 0
            for value in (
                self.max_archive_bytes,
                self.max_members,
                self.max_file_bytes,
                self.max_total_bytes,
            )
        ):
            raise ValueError("archive policy integer limits must be positive")
        ratio = self.max_compression_ratio
        if (
            isinstance(ratio, bool)
            or not isinstance(ratio, (int, float))
            or not math.isfinite(ratio)
            or ratio < 1
        ):
            raise ValueError("archive compression ratio limit must be at least one")


DEFAULT_ARCHIVE_POLICY = ArchivePolicy()


@dataclass(frozen=True, slots=True)
class _SourceFile:
    relative: PurePosixPath
    path: Path
    metadata: os.stat_result
    content_change_time_ns: int
    sha256: str
    mode: int

    def check(self, opened: StableRegularFileHandle) -> None:
        # The shared handle authority owns no-follow and in-read mutation
        # validation. Retain its identity across separate bounded reads too.
        # Reading can legitimately advance atime (including Linux relatime),
        # so comparing complete stat_result values would reject our own read.
        if (
            opened.stat.st_dev,
            opened.stat.st_ino,
            opened.stat.st_mode,
            opened.stat.st_size,
            opened.stat.st_mtime_ns,
        ) != (
            self.metadata.st_dev,
            self.metadata.st_ino,
            self.metadata.st_mode,
            self.metadata.st_size,
            self.metadata.st_mtime_ns,
        ) or opened.content_change_time_ns != self.content_change_time_ns:
            raise ValueError(f"archive source changed since inventory: {self.path}")


def _directory_identity(path: Path) -> tuple[int, int]:
    if resolve_owned_path(path) != path.absolute():
        raise ValueError(f"archive directory is indirect: {path}")
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata_is_link_like(metadata):
        raise ValueError(f"archive path is not a real directory: {path}")
    return metadata.st_dev, metadata.st_ino


def _check_directory(path: Path, identity: tuple[int, int]) -> None:
    if _directory_identity(path) != identity:
        raise ValueError(f"archive directory changed during operation: {path}")


def _inventory_regular_files(
    root: Path,
    *,
    policy: ArchivePolicy,
    mode_resolver: Callable[[PurePosixPath], int] | None = None,
) -> tuple[list[_SourceFile], dict[Path, tuple[int, int, int, int]]]:
    files: list[_SourceFile] = []
    directories: dict[Path, tuple[int, int, int, int]] = {}
    pending = [root]
    identities: set[str] = set()
    total = 0
    while pending:
        directory = pending.pop()
        device, inode = _directory_identity(directory)
        metadata = directory.lstat()
        directories[directory] = (
            device,
            inode,
            metadata.st_mtime_ns,
            metadata.st_ctime_ns,
        )
        with os.scandir(directory) as entries:
            for entry in entries:
                path = directory / entry.name
                relative = PurePosixPath(path.relative_to(root).as_posix())
                identity = portable_path_identity(relative.as_posix())
                if identity in identities:
                    raise ValueError("archive paths collide under portable identity")
                identities.add(identity)
                # Directories also consume inventory memory, even when empty.
                if len(identities) > policy.max_members:
                    raise ValueError("archive input exceeds member-count policy")
                metadata = path.lstat()
                if metadata_is_link_like(metadata):
                    raise ValueError(f"archive input contains a symbolic link: {path}")
                if stat.S_ISDIR(metadata.st_mode):
                    pending.append(path)
                    continue
                if not stat.S_ISREG(metadata.st_mode):
                    raise ValueError(f"archive input contains a special file: {path}")
                if metadata.st_size > policy.max_file_bytes:
                    raise ValueError(f"archive input file exceeds size policy: {path}")
                total += metadata.st_size
                if total > policy.max_total_bytes:
                    raise ValueError(
                        "archive input exceeds total uncompressed size policy"
                    )
                resolve_owned_path(path)
                with open_stable_regular_file(path, label="archive source") as opened:
                    if opened.stat.st_size != metadata.st_size:
                        raise ValueError(
                            f"archive source changed after inventory: {path}"
                        )
                    digest = _copy_exact(
                        opened.stream, None, expected_size=metadata.st_size
                    )
                metadata = opened.stat
                mode = (
                    mode_resolver(relative)
                    if mode_resolver is not None
                    else 0o755
                    if metadata.st_mode & 0o111 or relative.parent.name == "bin"
                    else 0o644
                )
                if type(mode) is not int or mode not in {0o644, 0o755}:
                    raise ValueError(
                        "archive mode resolver returned an unsupported mode"
                    )
                files.append(
                    _SourceFile(
                        relative,
                        path,
                        opened.stat,
                        opened.content_change_time_ns,
                        digest,
                        mode,
                    )
                )
        _check_directory(directory, (device, inode))
    files.sort(key=lambda item: item.relative.as_posix())
    return files, directories


def _verify_inventory(
    files: list[_SourceFile], directories: dict[Path, tuple[int, int, int, int]]
) -> None:
    for directory, expected in directories.items():
        device, inode = _directory_identity(directory)
        metadata = directory.lstat()
        if (device, inode, metadata.st_mtime_ns, metadata.st_ctime_ns) != expected:
            raise ValueError(f"archive source directory changed: {directory}")
    for source in files:
        resolve_owned_path(source.path)
        with open_stable_regular_file(source.path, label="archive source") as opened:
            source.check(opened)


def _zip_timestamp(epoch: int) -> tuple[int, int, int, int, int, int]:
    if type(epoch) is not int or epoch <= 0:
        raise ValueError("source date epoch must be a positive integer")
    stamp = dt.datetime.fromtimestamp(
        min(max(epoch, MIN_ZIP_EPOCH), MAX_ZIP_EPOCH), tz=dt.UTC
    )
    return (
        stamp.year,
        stamp.month,
        stamp.day,
        stamp.hour,
        stamp.minute,
        stamp.second - stamp.second % 2,
    )


def _copy_exact(source: BinaryIO, sink: BinaryIO | None, *, expected_size: int) -> str:
    digest = hashlib.sha256()
    remaining = expected_size
    while remaining:
        chunk = source.read(min(_COPY_CHUNK_SIZE, remaining))
        if not chunk:
            raise ValueError("archive source ended before its inventoried size")
        if len(chunk) > remaining:
            raise ValueError("archive source exceeded its inventoried size")
        if sink is not None and sink.write(chunk) != len(chunk):
            raise OSError("short archive write")
        digest.update(chunk)
        remaining -= len(chunk)
    if source.read(1):
        raise ValueError("archive source grew after inventory")
    return digest.hexdigest()


def _prepare_output(output: Path) -> tuple[Path, tuple[int, int]]:
    output = resolve_owned_path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    parent_identity = _directory_identity(output.parent)
    if output.exists() or is_link_like(output):
        raise ValueError(f"archive output already exists: {output}")
    return output, parent_identity


def _discard_stage(stage: Path, identity: tuple[int, int]) -> None:
    """A replacement at our random stage name belongs to somebody else."""
    try:
        if _directory_identity(stage) == identity:
            shutil.rmtree(stage)
    except (FileNotFoundError, ValueError):
        pass
    except OSError as exc:
        warnings.warn(
            f"archive retained owned staging residue {stage}: {exc}", RuntimeWarning
        )


def write_reproducible_zip(
    root: Path,
    output: Path,
    *,
    source_date_epoch: int,
    prefix: str | None = None,
    policy: ArchivePolicy = DEFAULT_ARCHIVE_POLICY,
    mode_resolver: Callable[[PurePosixPath], int] | None = None,
) -> None:
    """Publish canonical ZIP_STORED bytes, never a partial public archive."""
    date_time = _zip_timestamp(source_date_epoch)
    prefix_path = portable_relative_path(prefix) if prefix is not None else None
    if prefix_path is not None and len(prefix_path.parts) != 1:
        raise ValueError("archive prefix must be one portable path component")
    root = resolve_owned_path(root)
    output = resolve_owned_path(output)
    if output == root or root in output.parents:
        raise ValueError("archive output must be outside the source tree")
    files, directories = _inventory_regular_files(
        root, policy=policy, mode_resolver=mode_resolver
    )
    if (
        len(files) + len(directories) - 1 + (prefix_path is not None)
        > policy.max_members
    ):
        raise ValueError("archive input exceeds member-count policy including prefix")
    # ZIP_STORED has an exact, precomputable size (no ZIP64 or comments).
    names = [
        (
            item.relative if prefix_path is None else prefix_path / item.relative
        ).as_posix()
        for item in files
    ]
    metadata_size = sum(46 + len(name.encode("utf-8")) for name in names)
    expected_size = 22 + sum(
        76 + 2 * len(name.encode("utf-8")) + item.metadata.st_size
        for item, name in zip(files, names, strict=True)
    )
    if metadata_size > _MAX_METADATA_BYTES:
        raise ValueError("archive exceeds metadata-size policy")
    if expected_size > policy.max_archive_bytes:
        raise ValueError("archive output exceeds compressed-size policy")
    output, parent_identity = _prepare_output(output)
    stage = Path(tempfile.mkdtemp(prefix=".molt-archive-", dir=output.parent))
    stage_identity = _directory_identity(stage)
    staged = stage / "archive.zip"
    try:
        with zipfile.ZipFile(
            staged, "x", compression=zipfile.ZIP_STORED, allowZip64=False
        ) as archive:
            for source, name in zip(files, names, strict=True):
                info = zipfile.ZipInfo(name, date_time=date_time)
                info.create_system = 3
                info.compress_type = zipfile.ZIP_STORED
                info.external_attr = (stat.S_IFREG | source.mode) << 16
                info.file_size = source.metadata.st_size
                resolve_owned_path(source.path)
                with (
                    open_stable_regular_file(
                        source.path, label="archive source"
                    ) as opened,
                    archive.open(info, "w") as sink,
                ):
                    source.check(opened)
                    digest = _copy_exact(
                        opened.stream, sink, expected_size=info.file_size
                    )
                if digest != source.sha256:
                    raise ValueError("archive source changed while streaming")
        _verify_inventory(files, directories)
        if staged.stat().st_size != expected_size:
            raise ValueError("archive output does not have canonical size")
        _check_directory(output.parent, parent_identity)
        _check_directory(stage, stage_identity)
        durable_publish_exclusive(staged, output)
    finally:
        _discard_stage(stage, stage_identity)


def zip_member_kind(member: zipfile.ZipInfo) -> str:
    mode = stat.S_IFMT(member.external_attr >> 16)
    if member.external_attr & 0x400 or mode == stat.S_IFLNK:
        return "link"
    if mode not in {0, stat.S_IFREG, stat.S_IFDIR}:
        return "special"
    if member.filename.endswith("/"):
        return "directory" if mode in {0, stat.S_IFDIR} else "special"
    return "file" if mode in {0, stat.S_IFREG} else "special"


def _preflight_zip(stream: BinaryIO, size: int, policy: ArchivePolicy) -> None:
    """Bound central-directory allocation before ZipFile reads it into memory.

    Release policy excludes multipart/ZIP64 containers; these cannot be emitted
    by the bounded canonical writer. Deflate remains accepted for old bundles.
    """
    if size > policy.max_archive_bytes:
        raise ValueError("archive exceeds compressed-size policy")
    stream.seek(max(0, size - 65557))
    tail = stream.read(65557)
    offset = tail.rfind(b"PK\x05\x06")
    if offset < 0 or len(tail) - offset < 22:
        raise ValueError("archive has no complete ZIP end record")
    _, disk, central_disk, disk_count, count, length, start, comment = struct.unpack(
        "<4s4H2LH", tail[offset : offset + 22]
    )
    if disk or central_disk or disk_count != count or count == 0xFFFF:
        raise ValueError("multipart and ZIP64 release archives are unsupported")
    if length == 0xFFFFFFFF or start == 0xFFFFFFFF:
        raise ValueError("ZIP64 release archives are unsupported")
    if count > policy.max_members:
        raise ValueError("archive exceeds member-count policy")
    if length > _MAX_METADATA_BYTES:
        raise ValueError("archive exceeds metadata-size policy")
    if len(tail) - offset != 22 + comment or start + length != size - (22 + comment):
        raise ValueError("archive has inconsistent central-directory bounds")
    stream.seek(0)


def _validated_members(
    archive: zipfile.ZipFile, *, policy: ArchivePolicy
) -> list[tuple[zipfile.ZipInfo, PurePosixPath, str]]:
    infos = archive.infolist()
    if len(infos) > policy.max_members:
        raise ValueError("archive exceeds member-count policy")
    members: list[tuple[zipfile.ZipInfo, PurePosixPath, str]] = []
    # Include implicit parents: A/one and a/two collide even without A/ or a/.
    nodes: dict[str, tuple[str, str]] = {}
    explicit: set[str] = set()
    total = 0
    for member in infos:
        raw = member.filename.removesuffix("/")
        if member.orig_filename != member.filename:
            raise ValueError("archive member path is not portable")
        try:
            relative = portable_relative_path(raw)
        except ValueError as exc:
            if PurePosixPath(raw).is_absolute() or ".." in PurePosixPath(raw).parts:
                raise ValueError("archive member path escapes extraction root") from exc
            raise ValueError("archive member path is not portable") from exc
        kind = zip_member_kind(member)
        if kind in {"link", "special"}:
            raise ValueError("archive contains a symbolic link or special member")
        if member.flag_bits & 1:
            raise ValueError("encrypted archive members are unsupported")
        if member.compress_type not in {zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED}:
            raise ValueError("archive member compression is unsupported")
        identity = portable_path_identity(raw)
        if identity in explicit:
            raise ValueError("archive members collide under portable identity")
        explicit.add(identity)
        for index in range(1, len(relative.parts) + 1):
            node = PurePosixPath(*relative.parts[:index]).as_posix()
            node_kind = kind if index == len(relative.parts) else "directory"
            key = portable_path_identity(node)
            previous = nodes.get(key)
            if previous is not None and previous != (node, node_kind):
                raise ValueError(
                    "archive paths collide or file is also a parent directory"
                )
            nodes[key] = (node, node_kind)
            if len(nodes) > policy.max_members:
                raise ValueError(
                    "archive exceeds member-count policy including parents"
                )
        if member.file_size < 0 or member.compress_size < 0:
            raise ValueError("archive member sizes are invalid")
        if kind == "directory" and member.file_size:
            raise ValueError("archive directory contains data")
        if member.file_size > policy.max_file_bytes:
            raise ValueError("archive member exceeds size policy")
        total += member.file_size
        if total > policy.max_total_bytes:
            raise ValueError("archive exceeds total uncompressed size policy")
        if (
            member.file_size / max(member.compress_size, 1)
            > policy.max_compression_ratio
        ):
            raise ValueError("archive member exceeds compression-ratio policy")
        members.append((member, relative, kind))
    return members


def extract_zip_strict(
    archive_path: Path, output: Path, *, policy: ArchivePolicy = DEFAULT_ARCHIVE_POLICY
) -> None:
    """Validate, stream and rehash a new tree before no-replace publication."""
    archive_path = resolve_owned_path(archive_path)
    output, parent_identity = _prepare_output(output)
    stage = Path(tempfile.mkdtemp(prefix=".molt-extract-", dir=output.parent))
    stage_identity = _directory_identity(stage)
    expected: dict[str, tuple[int, str]] = {}
    expected_directories: set[PurePosixPath] = {PurePosixPath()}
    try:
        with open_stable_regular_file(archive_path, label="release archive") as opened:
            _preflight_zip(opened.stream, opened.stat.st_size, policy)
            with zipfile.ZipFile(opened.stream) as archive:
                for member, relative, kind in _validated_members(
                    archive, policy=policy
                ):
                    _check_directory(stage, stage_identity)
                    directory = relative if kind == "directory" else relative.parent
                    destination_directory = stage / directory
                    # Resolve the whole lexical chain before mkdir can follow an
                    # existing indirect parent, then validate it again before
                    # writing. Resolving every prefix separately makes deep
                    # source bundles quadratic in path depth.
                    resolve_owned_path(destination_directory)
                    destination_directory.mkdir(parents=True, exist_ok=True)
                    _directory_identity(destination_directory)
                    expected_directories.add(directory)
                    expected_directories.update(directory.parents)
                    if kind == "directory":
                        with archive.open(member, "r") as source:
                            _copy_exact(source, None, expected_size=0)
                        continue
                    destination = stage / relative
                    with (
                        archive.open(member, "r") as source,
                        destination.open("xb") as sink,
                    ):
                        digest = _copy_exact(
                            source, sink, expected_size=member.file_size
                        )
                        written = os.fstat(sink.fileno())
                    metadata = destination.lstat()
                    if metadata_is_link_like(metadata) or (
                        metadata.st_dev,
                        metadata.st_ino,
                    ) != (written.st_dev, written.st_ino):
                        raise ValueError("archive extracted file changed during write")
                    mode = (member.external_attr >> 16) & 0o111
                    destination.chmod(0o755 if mode else 0o644)
                    expected[relative.as_posix()] = (member.file_size, digest)
        # Rehash destination bytes, not only source bytes or retained CRC metadata.
        actual, directories = _inventory_regular_files(stage, policy=policy)
        if {
            source.relative.as_posix(): (source.metadata.st_size, source.sha256)
            for source in actual
        } != expected or {
            PurePosixPath(path.relative_to(stage).as_posix()) for path in directories
        } != expected_directories:
            raise ValueError("archive extracted tree changed before publication")
        _verify_inventory(actual, directories)
        _check_directory(output.parent, parent_identity)
        _check_directory(stage, stage_identity)
        durable_publish_directory_exclusive(stage, output)
    finally:
        _discard_stage(stage, stage_identity)


def same_regular_file_bytes(first: Path, second: Path) -> bool:
    """Compare stable direct-file content using bounded streaming memory."""
    first = resolve_owned_path(first)
    second = resolve_owned_path(second)
    with open_stable_regular_file(first, label="release byte comparison") as left:
        with open_stable_regular_file(second, label="release byte comparison") as right:
            if left.stat.st_size != right.stat.st_size:
                equal = False
            else:
                left_digest = _copy_exact(
                    left.stream, None, expected_size=left.stat.st_size
                )
                right_digest = _copy_exact(
                    right.stream, None, expected_size=right.stat.st_size
                )
                equal = left_digest == right_digest
    resolve_owned_path(first)
    resolve_owned_path(second)
    return equal

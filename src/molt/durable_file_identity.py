"""Filesystem-lifetime identity, separate from operation-local stat fences.

Persistent volumes use their durable authority; volatile Linux volumes use a
typed boot-scoped authority. Legacy pairs enter only through explicit migration.
"""

from __future__ import annotations

import ctypes
from dataclasses import dataclass
from enum import StrEnum
import os
from pathlib import Path
import re
import stat
import struct
import sys
from typing import Literal, assert_never


class IdentityUnavailable(ValueError):
    """The filesystem cannot establish durable identity; never adopt state."""


class IdentityUnverifiable(IdentityUnavailable):
    """A legacy record no longer proves its original filesystem object."""


class UniqueMountIdentityUnavailable(IdentityUnavailable):
    reason: Literal["unique-mount-id-unavailable"] = "unique-mount-id-unavailable"
    remedy: Literal["MOLT_SCRATCH_STORAGE=disk"] = "MOLT_SCRATCH_STORAGE=disk"

    def __init__(self, detail: str):
        super().__init__(f"{self.reason}: {detail}; use {self.remedy}")


class VolumeKind(StrEnum):
    APPLE_UUID = "apple-uuid"
    LINUX_UUID = "linux-uuid"
    WINDOWS_SERIAL = "windows-serial"
    BOOT_SCOPED = "boot-scoped"


@dataclass(frozen=True, slots=True, eq=False)
class DurableFileIdentity:
    kind: VolumeKind
    volume: str
    file_id: str

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, DurableFileIdentity):
            return NotImplemented
        kind = self.kind
        match kind:
            case (
                VolumeKind.APPLE_UUID
                | VolumeKind.LINUX_UUID
                | VolumeKind.WINDOWS_SERIAL
                | VolumeKind.BOOT_SCOPED
            ):
                return (
                    kind is other.kind
                    and self.volume == other.volume
                    and self.file_id == other.file_id
                )
            case _:
                assert_never(kind)

    def __hash__(self) -> int:
        return hash((self.kind, self.volume, self.file_id))

    def payload(self) -> dict[str, str]:
        return {"kind": self.kind.value, "volume": self.volume, "file_id": self.file_id}

    def stat_prefix(self) -> tuple[str, str]:
        return f"{self.kind.value}:{self.volume}", self.file_id

    @classmethod
    def from_stat_prefix(cls, volume: object, file_id: object) -> DurableFileIdentity:
        if not isinstance(volume, str) or ":" not in volume:
            raise IdentityUnavailable("invalid durable stat volume")
        kind, value = volume.split(":", 1)
        return cls.from_payload({"kind": kind, "volume": value, "file_id": file_id})

    @classmethod
    def from_payload(cls, value: object) -> DurableFileIdentity:
        if not isinstance(value, dict) or set(value) != {"kind", "volume", "file_id"}:
            raise IdentityUnavailable("invalid durable file identity shape")
        try:
            kind = VolumeKind(value["kind"])
        except (TypeError, ValueError) as exc:
            raise IdentityUnavailable("unknown durable volume kind") from exc
        volume, file_id = value["volume"], value["file_id"]
        if (
            not isinstance(volume, str)
            or re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", volume) is None
            or not isinstance(file_id, str)
            or re.fullmatch(r"[0-9a-f]{32}", file_id) is None
            or int(file_id, 16) == 0
        ):
            raise IdentityUnavailable("invalid durable volume or file id")
        match kind:
            case VolumeKind.APPLE_UUID:
                valid = re.fullmatch(r"[0-9a-f]{32}", volume) is not None
            case VolumeKind.WINDOWS_SERIAL:
                valid = re.fullmatch(r"[0-9a-f]{16}", volume) is not None
            case VolumeKind.BOOT_SCOPED:
                valid = (
                    re.fullmatch(r"[0-9a-f]{32}\.mnt-[0-9a-f]{16}", volume) is not None
                )
            case VolumeKind.LINUX_UUID:
                valid = True
            case _:
                assert_never(kind)
        if not valid:
            raise IdentityUnavailable("invalid durable volume identity")
        return cls(kind, volume, file_id)


@dataclass(frozen=True, slots=True)
class StatToken:
    """Class-B cache hint; its enclosing key owns the path, content owns truth."""

    inode: int
    mode: int
    size: int
    mtime_ns: int
    ctime_ns: int

    def payload(self) -> list[int]:
        return [self.inode, self.mode, self.size, self.mtime_ns, self.ctime_ns]

    @classmethod
    def from_payload(cls, value: object) -> StatToken:
        if (
            not isinstance(value, list)
            or len(value) != 5
            or any(type(n) is not int for n in value)
            or value[0] <= 0
            or value[2] < 0
        ):
            raise ValueError("invalid cache stat token")
        return cls(*value)


# Intern queried volume facts only. In particular, never cache st_dev -> UUID:
# the kernel can reuse device numbers while a process remains alive.
_VOLUMES: dict[tuple[VolumeKind, str], str] = {}


def _volume(kind: VolumeKind, value: str) -> str:
    return _VOLUMES.setdefault((kind, value), value)


class _AttrList(ctypes.Structure):
    _fields_ = [("count", ctypes.c_uint16), ("reserved", ctypes.c_uint16)] + [
        (field, ctypes.c_uint32)
        for field in ("common", "volume", "directory", "file", "fork")
    ]


@dataclass(frozen=True, slots=True)
class _AppleVolume:
    device: int
    uuid: str


@dataclass(frozen=True, slots=True)
class _AppleLeaf:
    device: int
    file_id: int


class ParentCaptureReason(StrEnum):
    UNAVAILABLE = "parent-capture-unavailable"
    CROSS_MOUNT = "cross-mount-leaf"
    CHANGED = "parent-or-leaf-changed"


class ParentCaptureUnavailable(IdentityUnavailable):
    def __init__(self, reason: ParentCaptureReason, path: Path, detail: str):
        self.reason = reason
        match reason:
            case ParentCaptureReason.UNAVAILABLE:
                self.remedy = "use a searchable parent directory on a supported volume"
            case ParentCaptureReason.CROSS_MOUNT:
                self.remedy = "select a directory below the mount root on the same volume as its parent"
            case ParentCaptureReason.CHANGED:
                self.remedy = (
                    "restore the original parent and leaf, then retry identity capture"
                )
            case _:
                assert_never(reason)
        super().__init__(f"{reason.value}: {path}: {detail}; {self.remedy}")


def _apple_attributes(
    fd: int, leaf: str | None, *, common: int, volume: int, size: int
) -> bytes:
    """XNU attr.h ABI: real device numbers are operation-local mount fences."""
    attrs = _AttrList(5, 0, common, volume, 0, 0, 0)
    buffer = ctypes.create_string_buffer(size)
    libc = ctypes.CDLL(None, use_errno=True)
    if leaf is None:
        query = libc.fgetattrlist
        query.argtypes = [
            ctypes.c_int,
            ctypes.POINTER(_AttrList),
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.c_ulong,
        ]
        arguments = (fd, ctypes.byref(attrs), buffer, size, 0x200)
    else:
        query = libc.getattrlistat
        query.argtypes = [
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.POINTER(_AttrList),
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.c_ulong,
        ]
        arguments = (fd, os.fsencode(leaf), ctypes.byref(attrs), buffer, size, 0x201)
    query.restype = ctypes.c_int
    if query(*arguments):
        code = ctypes.get_errno()
        raise IdentityUnavailable(
            f"held-parent attribute query failed: {os.strerror(code)}"
        )
    length, returned, vol, directory, file, fork = struct.unpack_from("=6I", buffer.raw)
    if (
        length != size
        or returned != common
        or vol != (volume & 0x7FFFFFFF)
        or any((directory, file, fork))
    ):
        raise IdentityUnavailable(
            "filesystem did not return the requested real-device attributes"
        )
    return buffer.raw


def _apple_volume(fd: int) -> _AppleVolume:
    # fgetattrlist holds this vnode; XNU resolves its mount root for UUID/DEVID.
    data = _apple_attributes(fd, None, common=0x80000002, volume=0x80040000, size=44)
    device = struct.unpack_from("=I", data, 24)[0]
    uuid = data[28:44].hex()
    if not int(uuid, 16):
        raise IdentityUnavailable("filesystem has no volume UUID")
    return _AppleVolume(device, uuid)


def _apple_leaf(fd: int, leaf: str) -> _AppleLeaf:
    # FILEID is the non-root leaf's va_fileid. RETURN_REALDEV avoids firmlink IDs.
    data = _apple_attributes(fd, leaf, common=0x82000002, volume=0, size=36)
    device, file_id = struct.unpack_from("=IQ", data, 24)
    if file_id <= 0:
        raise IdentityUnavailable("filesystem has no leaf file ID")
    return _AppleLeaf(device, file_id)


def _apple_identity(path: Path, expected: os.stat_result | None) -> DurableFileIdentity:
    parent, leaf = (path.parent, path.name) if path.name else (path, ".")
    search = getattr(os, "O_SEARCH", None)
    if search is None:
        raise ParentCaptureUnavailable(
            ParentCaptureReason.UNAVAILABLE, path, "O_SEARCH is unavailable"
        )
    try:
        fd = os.open(parent, search | os.O_CLOEXEC)
    except (OSError, AttributeError) as exc:
        raise ParentCaptureUnavailable(
            ParentCaptureReason.UNAVAILABLE, path, str(exc)
        ) from exc
    try:
        held = os.fstat(fd)
        before = os.stat(leaf, dir_fd=fd, follow_symlinks=False)
        if not (
            stat.S_ISDIR(before.st_mode)
            or stat.S_ISREG(before.st_mode)
            or stat.S_ISLNK(before.st_mode)
        ):
            raise IdentityUnavailable(
                "durable identity requires a direct file or directory"
            )
        if expected is not None and not os.path.samestat(before, expected):
            raise ParentCaptureUnavailable(
                ParentCaptureReason.CHANGED, path, "opening stat differs"
            )
        first = _apple_leaf(fd, leaf)
        volume = _apple_volume(fd)
        last = _apple_leaf(fd, leaf)
        after = os.stat(leaf, dir_fd=fd, follow_symlinks=False)
        if first.device != volume.device:
            raise ParentCaptureUnavailable(
                ParentCaptureReason.CROSS_MOUNT,
                path,
                "leaf and parent are on different real mounts",
            )
        if (
            first != last
            or first.file_id != before.st_ino
            or first.file_id != after.st_ino
            or not os.path.samestat(held, os.fstat(fd))
            or (before.st_mode, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
            != (after.st_mode, after.st_size, after.st_mtime_ns, after.st_ctime_ns)
        ):
            raise ParentCaptureUnavailable(
                ParentCaptureReason.CHANGED,
                path,
                "held parent or relative leaf changed",
            )
        return DurableFileIdentity(
            VolumeKind.APPLE_UUID,
            _volume(VolumeKind.APPLE_UUID, volume.uuid),
            f"{first.file_id:032x}",
        )
    except ParentCaptureUnavailable:
        raise
    except (OSError, ValueError, AttributeError) as exc:
        raise ParentCaptureUnavailable(
            ParentCaptureReason.UNAVAILABLE, path, str(exc)
        ) from exc
    finally:
        os.close(fd)


def _linux_volume(device: int, by_uuid: Path = Path("/dev/disk/by-uuid")) -> str:
    matches: set[str] = set()
    try:
        for entry in by_uuid.iterdir():
            try:
                node = entry.stat()
            except FileNotFoundError:
                continue
            if stat.S_ISBLK(node.st_mode) and node.st_rdev == device:
                matches.add(entry.name)
    except OSError as exc:
        raise IdentityUnavailable(
            "filesystem UUID directory is unavailable; provide a unique filesystem "
            "UUID mapping in /dev/disk/by-uuid on a supported volume"
        ) from exc
    if len(matches) != 1:
        raise IdentityUnavailable(
            "filesystem UUID is absent or ambiguous; provide a unique filesystem "
            "UUID mapping in /dev/disk/by-uuid on a supported volume"
        )
    return matches.pop()


def _linux_filesystem_type(fd: int) -> int:
    # Linux statfs starts with a native long f_type on all supported ABIs.
    # Reserve more than the complete kernel statfs structure, not just f_type.
    buffer = ctypes.create_string_buffer(256)
    query = ctypes.CDLL(None, use_errno=True).fstatfs
    query.argtypes = [ctypes.c_int, ctypes.c_void_p]
    query.restype = ctypes.c_int
    if query(fd, buffer):
        raise IdentityUnavailable(
            f"filesystem type query failed: {os.strerror(ctypes.get_errno())}"
        )
    return ctypes.c_long.from_buffer(buffer).value & 0xFFFFFFFF


# ABI: Linux v6.8, include/uapi/linux/stat.h and include/uapi/linux/fcntl.h.
# These UAPI layouts carry the Linux-syscall-note; no kernel implementation is copied.
STATX_MNT_ID_UNIQUE = 0x00004000
_AT_EMPTY_PATH = 0x1000


class _StatxTimestamp(ctypes.Structure):
    _fields_ = [
        ("seconds", ctypes.c_int64),
        ("nanoseconds", ctypes.c_uint32),
        ("reserved", ctypes.c_int32),
    ]


class _Statx(ctypes.Structure):
    _fields_ = [
        ("mask", ctypes.c_uint32),
        ("blksize", ctypes.c_uint32),
        ("attributes", ctypes.c_uint64),
        ("nlink", ctypes.c_uint32),
        ("uid", ctypes.c_uint32),
        ("gid", ctypes.c_uint32),
        ("mode", ctypes.c_uint16),
        ("spare0", ctypes.c_uint16),
        ("ino", ctypes.c_uint64),
        ("size", ctypes.c_uint64),
        ("blocks", ctypes.c_uint64),
        ("attributes_mask", ctypes.c_uint64),
        ("atime", _StatxTimestamp),
        ("btime", _StatxTimestamp),
        ("ctime", _StatxTimestamp),
        ("mtime", _StatxTimestamp),
        ("rdev_major", ctypes.c_uint32),
        ("rdev_minor", ctypes.c_uint32),
        ("dev_major", ctypes.c_uint32),
        ("dev_minor", ctypes.c_uint32),
        ("mnt_id", ctypes.c_uint64),
        ("dio_mem_align", ctypes.c_uint32),
        ("dio_offset_align", ctypes.c_uint32),
        ("spare3", ctypes.c_uint64 * 12),
    ]


def _linux_unique_mount_id(fd: int) -> int:
    if ctypes.sizeof(_Statx) != 256 or _Statx.mnt_id.offset != 0x90:
        raise UniqueMountIdentityUnavailable("unsupported statx ABI")
    libc = ctypes.CDLL(None, use_errno=True)
    try:
        query = libc.statx
    except AttributeError as exc:
        raise UniqueMountIdentityUnavailable("libc has no statx") from exc
    query.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_uint,
        ctypes.POINTER(_Statx),
    ]
    query.restype = ctypes.c_int
    result = _Statx()
    if query(fd, b"", _AT_EMPTY_PATH, STATX_MNT_ID_UNIQUE, ctypes.byref(result)):
        raise UniqueMountIdentityUnavailable(
            f"statx unique mount ID failed: {os.strerror(ctypes.get_errno())}"
        )
    if not result.mask & STATX_MNT_ID_UNIQUE or not result.mnt_id:
        raise UniqueMountIdentityUnavailable(
            "kernel did not return STATX_MNT_ID_UNIQUE (Linux 6.8+ required)"
        )
    return result.mnt_id


def _linux_boot_volume(fd: int) -> str:
    mount = _linux_unique_mount_id(fd)
    try:
        boot = (
            Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
        )
    except OSError as exc:
        raise IdentityUnavailable("kernel boot ID is unavailable") from exc
    if re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", boot) is None:
        raise IdentityUnavailable("kernel boot ID is invalid")
    boot = boot.replace("-", "")
    if not int(boot, 16):
        raise IdentityUnavailable("invalid boot-scoped volume identity")
    return f"{boot}.mnt-{mount:016x}"


def _windows_file_id_info(data: bytes) -> DurableFileIdentity:
    if len(data) != 24:
        raise IdentityUnavailable("invalid FILE_ID_INFO length")
    serial = int.from_bytes(data[:8], "little")
    return DurableFileIdentity.from_payload(
        {
            "kind": VolumeKind.WINDOWS_SERIAL.value,
            "volume": f"{serial:016x}",
            "file_id": data[8:].hex(),
        }
    )


def _windows_identity(fd: int) -> DurableFileIdentity:
    if sys.platform != "win32":
        raise IdentityUnavailable("Windows file identity requires Windows")
    import msvcrt

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    query = kernel.GetFileInformationByHandleEx
    query.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    query.restype = ctypes.c_int
    buffer = ctypes.create_string_buffer(24)
    if not query(msvcrt.get_osfhandle(fd), 18, buffer, 24):
        raise IdentityUnavailable(f"FileIdInfo query failed: {ctypes.get_last_error()}")
    return _windows_file_id_info(buffer.raw)


def identity_from_fd(
    fd: int, *, metadata: os.stat_result | None = None
) -> DurableFileIdentity:
    metadata = os.fstat(fd) if metadata is None else metadata
    if sys.platform == "win32":
        identity = _windows_identity(fd)
    else:
        if metadata.st_ino <= 0:
            raise IdentityUnavailable("filesystem has no file id")
        if sys.platform == "darwin":
            kind, volume = VolumeKind.APPLE_UUID, _apple_volume(fd).uuid
        elif sys.platform.startswith("linux"):
            if _linux_filesystem_type(fd) in {0x01021994, 0x858458F6}:  # tmpfs, ramfs
                kind, volume = (
                    VolumeKind.BOOT_SCOPED,
                    _linux_boot_volume(fd),
                )
            else:
                kind, volume = VolumeKind.LINUX_UUID, _linux_volume(metadata.st_dev)
        else:
            raise IdentityUnavailable(
                f"durable identity is unsupported on {sys.platform}"
            )
        identity = DurableFileIdentity.from_payload(
            {"kind": kind.value, "volume": volume, "file_id": f"{metadata.st_ino:032x}"}
        )
    return DurableFileIdentity(
        identity.kind, _volume(identity.kind, identity.volume), identity.file_id
    )


def file_identity(
    path: Path, *, expected: os.stat_result | None = None
) -> DurableFileIdentity:
    """Capture a direct node and fence its name against the held descriptor."""
    if sys.platform == "darwin":
        return _apple_identity(path, expected)
    before = path.lstat()
    if not (
        stat.S_ISDIR(before.st_mode)
        or stat.S_ISREG(before.st_mode)
        or stat.S_ISLNK(before.st_mode)
    ):
        raise IdentityUnavailable(
            f"durable identity requires a direct file or directory: {path}"
        )
    if expected is not None and not os.path.samestat(before, expected):
        raise IdentityUnavailable(
            f"file changed before durable identity capture: {path}"
        )
    flags = (
        os.O_RDONLY
        | getattr(os, "O_BINARY", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_NONBLOCK", 0)
    )
    if sys.platform.startswith("linux"):
        flags |= os.O_PATH
    if stat.S_ISLNK(before.st_mode):
        if sys.platform == "darwin":
            flags = (flags & ~getattr(os, "O_NOFOLLOW", 0)) | 0x200000  # O_SYMLINK
        elif sys.platform.startswith("linux"):
            flags |= os.O_PATH  # O_PATH plus O_NOFOLLOW.
    if os.name == "nt":
        import msvcrt

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        create = kernel.CreateFileW
        create.argtypes = [
            ctypes.c_wchar_p,
            ctypes.c_uint32,
            ctypes.c_uint32,
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.c_uint32,
            ctypes.c_void_p,
        ]
        create.restype = ctypes.c_void_p
        handle = create(str(path.absolute()), 0x80, 7, None, 3, 0x02200000, None)
        if handle == ctypes.c_void_p(-1).value:
            raise IdentityUnavailable(
                f"cannot open identity handle: {ctypes.get_last_error()}"
            )
        try:
            fd = msvcrt.open_osfhandle(handle, os.O_RDONLY)
        except BaseException:
            close = kernel.CloseHandle
            close.argtypes = [ctypes.c_void_p]
            close(handle)
            raise
    else:
        fd = os.open(path, flags)
    try:
        opened = os.fstat(fd)
        identity = identity_from_fd(fd, metadata=opened)
        if not os.path.samestat(before, opened) or not os.path.samestat(
            opened, path.lstat()
        ):
            raise IdentityUnavailable(
                f"file changed during durable identity capture: {path}"
            )
        return identity
    finally:
        os.close(fd)


def upgrade_legacy(path: Path, value: object) -> DurableFileIdentity:
    """Migration only: equality of BOTH legacy fields precedes volume capture."""
    if (
        not isinstance(value, dict)
        or set(value) != {"device", "inode"}
        or any(type(v) is not int for v in value.values())
    ):
        raise IdentityUnverifiable("identity-unverifiable: invalid legacy identity")
    metadata = path.lstat()
    if (metadata.st_dev, metadata.st_ino) != (value["device"], value["inode"]):
        raise IdentityUnverifiable(
            "identity-unverifiable: legacy filesystem object changed"
        )
    return file_identity(path, expected=metadata)

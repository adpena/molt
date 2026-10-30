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


class IdentityUnavailable(ValueError):
    """The filesystem cannot establish durable identity; never adopt state."""


class IdentityUnverifiable(IdentityUnavailable):
    """A legacy record no longer proves its original filesystem object."""


class VolumeKind(StrEnum):
    APPLE_UUID = "apple-uuid"
    LINUX_UUID = "linux-uuid"
    WINDOWS_SERIAL = "windows-serial"
    BOOT_SCOPED = "boot-scoped"


@dataclass(frozen=True, slots=True)
class DurableFileIdentity:
    kind: VolumeKind
    volume: str
    file_id: str

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
            or (
                kind is VolumeKind.APPLE_UUID
                and re.fullmatch(r"[0-9a-f]{32}", volume) is None
            )
            or (
                kind is VolumeKind.WINDOWS_SERIAL
                and re.fullmatch(r"[0-9a-f]{16}", volume) is None
            )
            or (
                kind is VolumeKind.BOOT_SCOPED
                and re.fullmatch(r"[0-9a-f]{32}\.[0-9a-f]{16}", volume) is None
            )
        ):
            raise IdentityUnavailable("invalid durable volume or file id")
        return cls(kind, volume, file_id)


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


def _apple_volume(fd: int) -> str:
    # XNU substitutes the held vnode's volume root for volume attributes.
    # RETURNED_ATTRS is first; validate support before reading UUID bytes.
    attrs = _AttrList(5, 0, 0x80000000, 0x80040000, 0, 0, 0)
    buffer = ctypes.create_string_buffer(40)
    libc = ctypes.CDLL(None, use_errno=True)
    query = libc.fgetattrlist
    query.argtypes = [
        ctypes.c_int,
        ctypes.POINTER(_AttrList),
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_ulong,
    ]
    query.restype = ctypes.c_int
    if query(fd, ctypes.byref(attrs), buffer, len(buffer), 0):
        code = ctypes.get_errno()
        raise IdentityUnavailable(f"volume UUID query failed: {os.strerror(code)}")
    length, common, volume, directory, file, fork = struct.unpack_from(
        "=6I", buffer.raw
    )
    if (
        length != 40
        or common != 0x80000000
        or not volume & 0x00040000
        or any((directory, file, fork))
    ):
        raise IdentityUnavailable("filesystem did not return a volume UUID")
    uuid = buffer.raw[24:40].hex()
    if not int(uuid, 16):
        raise IdentityUnavailable("filesystem has no volume UUID")
    return uuid


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


def _linux_boot_volume(device: int) -> str:
    try:
        boot = (
            Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
        )
    except OSError as exc:
        raise IdentityUnavailable("kernel boot ID is unavailable") from exc
    if re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", boot) is None:
        raise IdentityUnavailable("kernel boot ID is invalid")
    boot = boot.replace("-", "")
    if not int(boot, 16) or not 0 <= device < 2**64:
        raise IdentityUnavailable("invalid boot-scoped volume identity")
    return f"{boot}.{device:016x}"


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
            kind, volume = VolumeKind.APPLE_UUID, _apple_volume(fd)
        elif sys.platform.startswith("linux"):
            if _linux_filesystem_type(fd) in {0x01021994, 0x858458F6}:  # tmpfs, ramfs
                kind, volume = (
                    VolumeKind.BOOT_SCOPED,
                    _linux_boot_volume(metadata.st_dev),
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
    if stat.S_ISLNK(before.st_mode):
        if sys.platform == "darwin":
            flags = (flags & ~getattr(os, "O_NOFOLLOW", 0)) | 0x200000  # O_SYMLINK
        elif sys.platform.startswith("linux"):
            flags |= 0x200000  # O_PATH plus O_NOFOLLOW.
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


def upgrade_stat_token(path: Path, token: object) -> list[object]:
    """Re-key a legacy full token only while every recorded field matches."""
    if (
        not isinstance(token, (list, tuple))
        or len(token) < 6
        or any(type(v) is not int for v in token[:6])
    ):
        raise IdentityUnverifiable("identity-unverifiable: invalid legacy stat token")
    metadata = path.lstat()
    if (
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    ) != tuple(token[2:6]):
        raise IdentityUnverifiable(
            "identity-unverifiable: legacy file generation changed"
        )
    identity = upgrade_legacy(path, {"device": token[0], "inode": token[1]})
    return [*identity.stat_prefix(), *token[2:]]

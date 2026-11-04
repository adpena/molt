"""Fresh, process-namespace mount facts for recursive artifact deletion.

This is an admission check, not a mount lease: callers still own exclusive
artifact custody. A privileged actor can change the namespace after inspection.
No mount is detached and no payload directory is walked to gather these facts.
"""

from __future__ import annotations

import errno
import ntpath
import os
import re
import stat
import sys
from pathlib import Path
from typing import Any

from molt.typed_cache import typed_cache

_MAX_MOUNTS = 65_536
_MAX_MOUNTINFO_BYTES = 16 * 1024 * 1024
_MOUNT_ESCAPE = re.compile(rb"\\(040|011|012|134)")
_INVALID_MOUNT_ESCAPE = re.compile(rb"\\(?!040|011|012|134)")


def _unavailable(message: str) -> OSError:
    return OSError(errno.ENOTSUP, f"mount inspection unavailable: {message}")


def _linux_mount_points() -> tuple[str, ...]:
    # Kernel field 5 records each mount, including same-device bind mounts:
    # https://www.kernel.org/doc/html/latest/filesystems/proc.html
    with open("/proc/self/mountinfo", "rb") as source:
        data = source.read(_MAX_MOUNTINFO_BYTES + 1)
    return _parse_linux_mountinfo(data)


def _parse_linux_mountinfo(data: bytes) -> tuple[str, ...]:
    if not data or len(data) > _MAX_MOUNTINFO_BYTES:
        raise _unavailable("empty or oversized Linux mountinfo")
    if not data.endswith(b"\n"):
        raise _unavailable("truncated Linux mountinfo")
    points = []
    # Only ASCII space separates fields and LF separates records. Other
    # whitespace (for example carriage return or form feed) is a legal path
    # byte and must not be interpreted as a field/record boundary.
    for line in data.split(b"\n")[:-1]:
        fields = line.split(b" ")
        try:
            separator = fields.index(b"-")
        except ValueError as error:
            raise _unavailable("malformed Linux mountinfo") from error
        if separator < 6 or len(fields) < separator + 4:
            raise _unavailable("malformed Linux mountinfo")
        raw = fields[4]
        if _INVALID_MOUNT_ESCAPE.search(raw):
            raise _unavailable("invalid Linux mount escape")
        decoded = _MOUNT_ESCAPE.sub(lambda match: bytes([int(match[1], 8)]), raw)
        if not decoded.startswith(b"/") or b"\0" in decoded:
            raise _unavailable("invalid Linux mount point")
        points.append(os.fsdecode(decoded))
        if len(points) > _MAX_MOUNTS:
            raise _unavailable("too many Linux mounts")
    return tuple(points)


@typed_cache(maxsize=1)
def _darwin_api() -> tuple[Any, Any, Any]:
    import ctypes

    # Apple's public 64-bit statfs layout and getfsstat64 ABI. getmntinfo uses
    # shared static storage, which is unsuitable for concurrent cleanup callers.
    # https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/mount.h
    class Statfs64(ctypes.Structure):
        _fields_ = [
            ("bsize", ctypes.c_uint32),
            ("iosize", ctypes.c_int32),
            ("blocks", ctypes.c_uint64),
            ("bfree", ctypes.c_uint64),
            ("bavail", ctypes.c_uint64),
            ("files", ctypes.c_uint64),
            ("ffree", ctypes.c_uint64),
            ("fsid", ctypes.c_int32 * 2),
            ("owner", ctypes.c_uint32),
            ("type", ctypes.c_uint32),
            ("flags", ctypes.c_uint32),
            ("subtype", ctypes.c_uint32),
            ("fstypename", ctypes.c_char * 16),
            ("mntonname", ctypes.c_char * 1024),
            ("mntfromname", ctypes.c_char * 1024),
            ("flags_ext", ctypes.c_uint32),
            ("reserved", ctypes.c_uint32 * 7),
        ]

    if ctypes.sizeof(Statfs64) != 2168:
        raise _unavailable("unsupported Darwin statfs64 layout")
    library = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    try:
        query = library.getfsstat64
    except AttributeError as error:
        raise _unavailable("Darwin getfsstat64 is absent") from error
    query.argtypes = [ctypes.POINTER(Statfs64), ctypes.c_int, ctypes.c_int]
    query.restype = ctypes.c_int
    return ctypes, Statfs64, query


def _darwin_mount_points() -> tuple[str, ...]:
    ctypes, record, query = _darwin_api()
    # MNT_NOWAIT returns the kernel's mount information without requesting IO
    # from every filesystem, including disconnected network/external volumes.
    for _attempt in range(3):
        count = query(None, 0, 2)
        if count < 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code))
        if not 0 < count < _MAX_MOUNTS:
            raise _unavailable("invalid Darwin mount count")
        capacity = count + 1
        buffer = (record * capacity)()
        filled = query(buffer, ctypes.sizeof(buffer), 2)
        if filled < 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code))
        if filled >= capacity:
            # A full buffer may have been truncated by namespace growth. Only
            # the native size-query protocol retries; deletion never retries.
            continue
        if filled <= 0:
            raise _unavailable("empty Darwin mount result")
        points = tuple(os.fsdecode(buffer[index].mntonname) for index in range(filled))
        if any(
            not point.startswith("/") or len(os.fsencode(point)) >= 1024
            for point in points
        ):
            raise _unavailable("invalid or truncated Darwin mount point")
        return points
    raise _unavailable("Darwin mount table changed during inspection")


def _windows_mount_points() -> tuple[str, ...]:
    # Python 3.12+ owns the Win32 volume/path APIs, including mounted folders.
    # https://docs.python.org/3/library/os.html#os.listmounts
    if os.name != "nt":
        raise _unavailable("Windows volume APIs require Windows")
    points = []
    for volume in os.listvolumes():
        points.extend(os.listmounts(volume))
        if len(points) > _MAX_MOUNTS:
            raise _unavailable("too many Windows mounts")
    if not points:
        raise _unavailable("empty Windows mount result")
    if any(not isinstance(point, str) or not ntpath.isabs(point) for point in points):
        raise _unavailable("invalid Windows mount point")
    return tuple(points)


def _mount_points() -> tuple[str, ...]:
    if sys.platform.startswith("linux"):
        return _linux_mount_points()
    if sys.platform == "darwin":
        return _darwin_mount_points()
    if os.name == "nt":
        return _windows_mount_points()
    raise _unavailable(f"unsupported host {sys.platform}")


def _directory_spellings(path: Path, metadata: os.stat_result) -> tuple[str, ...]:
    if os.name == "nt":
        return (str(path.resolve(strict=True)),)
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        if not os.path.samestat(metadata, os.fstat(descriptor)):
            raise OSError("deletion directory changed before mount inspection")
        if sys.platform.startswith("linux"):
            return (os.readlink(f"/proc/self/fd/{descriptor}"),)
        if sys.platform == "darwin":
            import fcntl

            # Both spellings are observed from this fd, not reconstructed by
            # stripping a guessed /System/Volumes/Data prefix. The second
            # command is F_GETPATH_NOFIRMLINK from Apple's public fcntl.h.
            return tuple(
                os.fsdecode(
                    fcntl.fcntl(descriptor, command, bytes(1024)).split(b"\0", 1)[0]
                )
                for command in (fcntl.F_GETPATH, 102)
            )
        raise _unavailable(f"unsupported host {sys.platform}")
    finally:
        os.close(descriptor)


def _windows_key(path: str) -> str:
    value = ntpath.normcase(ntpath.normpath(path))
    if value.startswith("\\\\?\\unc\\"):
        return "\\\\" + value[8:]
    if value.startswith("\\\\?\\"):
        return value[4:]
    return value


def require_unmounted_directory(path: Path, metadata: os.stat_result) -> None:
    """Refuse a currently mounted root/descendant before any tree mutation.

    Acquire fresh facts for each recursive deletion; a cached snapshot cannot
    authorize a later operation. Leaf symlinks/reparse points never call here.
    """
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValueError("mount inspection requires a directory")
    spellings = _directory_spellings(path, metadata)
    if not spellings or any(
        not os.path.isabs(value) or "\0" in value for value in spellings
    ):
        raise _unavailable("invalid deletion directory spelling")
    points = _mount_points()
    if os.name == "nt":
        roots = tuple(_windows_key(value).rstrip("\\") for value in spellings)
        for point in points:
            candidate = _windows_key(point).rstrip("\\")
            if any(
                candidate == root or candidate.startswith(root + "\\") for root in roots
            ):
                raise OSError(
                    errno.EBUSY, "deletion tree contains a mounted filesystem", point
                )
    else:
        roots = tuple(Path(value) for value in spellings)
        for point in points:
            candidate = Path(point)
            if any(
                candidate == root or candidate.is_relative_to(root) for root in roots
            ):
                raise OSError(
                    errno.EBUSY, "deletion tree contains a mounted filesystem", point
                )
    if not os.path.samestat(metadata, path.lstat()):
        raise OSError("deletion directory changed during mount inspection")

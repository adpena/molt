# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.utime(dir_fd=...) via times= and ns=."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_utime_")
target = os.path.join(root, "payload.txt")
dir_fd = -1

try:
    with open(target, "wb") as handle:
        handle.write(b"timed")

    dir_fd = os.open(root, os.O_RDONLY)

    # times= path: whole-second values round-trip exactly.
    os.utime("payload.txt", times=(1_000_000_000, 1_111_111_111), dir_fd=dir_fd)
    st = os.stat(target)
    print("times_atime", int(st.st_atime))
    print("times_mtime", int(st.st_mtime))

    # ns= path: nanosecond resolution.
    os.utime(
        "payload.txt",
        ns=(1_234_000_000_000, 5_678_000_000_000),
        dir_fd=dir_fd,
    )
    st2 = os.stat(target)
    print("ns_atime_sec", int(st2.st_atime))
    print("ns_mtime_sec", int(st2.st_mtime))

    # No times (UTIME_NOW): mtime jumps forward from the stale 5678-second value.
    os.utime("payload.txt", dir_fd=dir_fd)
    st3 = os.stat(target)
    print("now_advanced", st3.st_mtime > 1_000_000_000)
finally:
    if dir_fd != -1:
        os.close(dir_fd)
    try:
        os.unlink(target)
    except Exception:
        pass
    try:
        os.rmdir(root)
    except Exception:
        pass

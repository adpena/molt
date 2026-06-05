# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.stat(dir_fd=-1) -> EBADF OSError."""

from __future__ import annotations

import errno
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_badf_")
target = os.path.join(root, "payload.txt")

try:
    with open(target, "wb") as handle:
        handle.write(b"x")

    # -1 is never a valid open directory fd -> EBADF.
    try:
        os.stat("payload.txt", dir_fd=-1)
    except OSError as exc:
        print("stat_badfd", type(exc).__name__, exc.errno == errno.EBADF)

    try:
        os.lstat("payload.txt", dir_fd=-1)
    except OSError as exc:
        print("lstat_badfd", type(exc).__name__, exc.errno == errno.EBADF)
finally:
    try:
        os.unlink(target)
    except Exception:
        pass
    try:
        os.rmdir(root)
    except Exception:
        pass

# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.stat(dir_fd=<file fd>) -> ENOTDIR."""

from __future__ import annotations

import errno
import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_notdir_")
target = os.path.join(root, "payload.txt")
file_fd = -1

try:
    with open(target, "wb") as handle:
        handle.write(b"x")

    # Open a regular file (not a directory) and use its fd as dir_fd -> ENOTDIR.
    file_fd = os.open(target, os.O_RDONLY)
    try:
        os.stat("payload.txt", dir_fd=file_fd)
    except OSError as exc:
        print("stat_notdir", type(exc).__name__, exc.errno == errno.ENOTDIR)
finally:
    if file_fd != -1:
        os.close(file_fd)
    try:
        os.unlink(target)
    except Exception:
        pass
    try:
        os.rmdir(root)
    except Exception:
        pass

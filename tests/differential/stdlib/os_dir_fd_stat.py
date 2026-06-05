# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.stat(dir_fd=...)."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_stat_")
target = os.path.join(root, "payload.txt")
dir_fd = -1

try:
    with open(target, "wb") as handle:
        handle.write(b"0123456789")

    full = os.stat(target)
    dir_fd = os.open(root, os.O_RDONLY)
    rel = os.stat("payload.txt", dir_fd=dir_fd)

    print("type", type(rel).__name__)
    print("shape", len(rel))
    print("size_match", rel.st_size == full.st_size, rel.st_size)
    print("mode_match", rel.st_mode == full.st_mode)
    print("ino_match", rel.st_ino == full.st_ino)
    print("is_int", isinstance(rel.st_mode, int), isinstance(rel.st_size, int))
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

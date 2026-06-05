# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.readlink(dir_fd=...)."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_readlink_")
real = os.path.join(root, "real.txt")
link = os.path.join(root, "link.txt")
dir_fd = -1

try:
    with open(real, "wb") as handle:
        handle.write(b"x")
    os.symlink("real.txt", link)

    dir_fd = os.open(root, os.O_RDONLY)
    target = os.readlink("link.txt", dir_fd=dir_fd)

    print("type", type(target).__name__)
    print("target", target)
finally:
    if dir_fd != -1:
        os.close(dir_fd)
    for path in (link, real):
        try:
            os.unlink(path)
        except Exception:
            pass
    try:
        os.rmdir(root)
    except Exception:
        pass

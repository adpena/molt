# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.symlink(dir_fd=...)."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_symlink_")
real = os.path.join(root, "real.txt")
link = os.path.join(root, "link.txt")
dir_fd = -1

try:
    with open(real, "wb") as handle:
        handle.write(b"payload")

    dir_fd = os.open(root, os.O_RDONLY)
    # Create the symlink relative to the open directory fd.
    os.symlink("real.txt", "link.txt", dir_fd=dir_fd)

    print("link_exists", os.path.islink(link))
    print("readlink", os.readlink(link))
    with open(link, "rb") as handle:
        print("contents", handle.read())
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

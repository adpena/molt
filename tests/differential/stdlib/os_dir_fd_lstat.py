# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.lstat(dir_fd=...) on a symlink."""

from __future__ import annotations

import os
import stat
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_lstat_")
real = os.path.join(root, "real.txt")
link = os.path.join(root, "link.txt")
dir_fd = -1

try:
    with open(real, "wb") as handle:
        handle.write(b"hello")
    os.symlink("real.txt", link)

    dir_fd = os.open(root, os.O_RDONLY)
    link_lstat = os.lstat("link.txt", dir_fd=dir_fd)
    real_lstat = os.lstat("real.txt", dir_fd=dir_fd)

    print("type", type(link_lstat).__name__)
    print("link_is_symlink", stat.S_ISLNK(link_lstat.st_mode))
    print("real_is_reg", stat.S_ISREG(real_lstat.st_mode))
    print("real_size", real_lstat.st_size)
    print("ino_differs", link_lstat.st_ino != real_lstat.st_ino)
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

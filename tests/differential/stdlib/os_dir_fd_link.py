# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.link(src_dir_fd=..., dst_dir_fd=...)."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_link_")
src = os.path.join(root, "src.txt")
dst = os.path.join(root, "hard.txt")
dir_fd = -1

try:
    with open(src, "wb") as handle:
        handle.write(b"shared")

    dir_fd = os.open(root, os.O_RDONLY)
    os.link("src.txt", "hard.txt", src_dir_fd=dir_fd, dst_dir_fd=dir_fd)

    src_stat = os.stat(src)
    dst_stat = os.stat(dst)
    print("same_inode", src_stat.st_ino == dst_stat.st_ino)
    print("nlink", src_stat.st_nlink)
    with open(dst, "rb") as handle:
        print("payload", handle.read())
finally:
    if dir_fd != -1:
        os.close(dir_fd)
    for path in (dst, src):
        try:
            os.unlink(path)
        except Exception:
            pass
    try:
        os.rmdir(root)
    except Exception:
        pass

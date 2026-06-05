# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: os.stat(dir_fd=..., follow_symlinks=False) matches lstat on a link."""

from __future__ import annotations

import os
import stat
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_follow_")
real = os.path.join(root, "real.txt")
link = os.path.join(root, "link.txt")
dir_fd = -1

try:
    with open(real, "wb") as handle:
        handle.write(b"hello-world")
    os.symlink("real.txt", link)

    dir_fd = os.open(root, os.O_RDONLY)

    followed = os.stat("link.txt", dir_fd=dir_fd, follow_symlinks=True)
    not_followed = os.stat("link.txt", dir_fd=dir_fd, follow_symlinks=False)

    print("followed_is_reg", stat.S_ISREG(followed.st_mode))
    print("followed_size", followed.st_size)
    print("not_followed_is_link", stat.S_ISLNK(not_followed.st_mode))
    print("inode_differs", followed.st_ino != not_followed.st_ino)
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

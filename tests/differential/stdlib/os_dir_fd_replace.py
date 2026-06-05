# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.replace(src_dir_fd=..., dst_dir_fd=...)."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_dir_fd_replace_")
src = os.path.join(root, "src.txt")
dst = os.path.join(root, "dst.txt")
dir_fd = -1

try:
    with open(src, "wb") as handle:
        handle.write(b"new-data")
    with open(dst, "wb") as handle:
        handle.write(b"old-data")

    dir_fd = os.open(root, os.O_RDONLY)
    # replace atomically overwrites an existing destination.
    os.replace("src.txt", "dst.txt", src_dir_fd=dir_fd, dst_dir_fd=dir_fd)

    print("src_gone", os.path.exists(src))
    with open(dst, "rb") as handle:
        print("payload", handle.read())
    # os.replace is functional with dir_fd but is NOT in supports_dir_fd.
    print("replace_in_set", os.replace in os.supports_dir_fd)
finally:
    if dir_fd != -1:
        os.close(dir_fd)
    for path in (src, dst):
        try:
            os.unlink(path)
        except Exception:
            pass
    try:
        os.rmdir(root)
    except Exception:
        pass

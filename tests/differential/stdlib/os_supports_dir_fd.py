# MOLT_ENV: MOLT_CAPABILITIES=fs.read
"""Purpose: os.supports_dir_fd / supports_follow_symlinks membership parity.

Only the functions this arc implements are checked, so the printed booleans are
byte-identical between molt and CPython even though CPython additionally
registers chown/mkfifo/mknod (which molt does not implement).
"""

from __future__ import annotations

import os


print("supports_dir_fd_is_set", isinstance(os.supports_dir_fd, set))
print("stat", os.stat in os.supports_dir_fd)
print("lstat", os.lstat in os.supports_dir_fd)
print("rename", os.rename in os.supports_dir_fd)
print("replace", os.replace in os.supports_dir_fd)
print("link", os.link in os.supports_dir_fd)
print("symlink", os.symlink in os.supports_dir_fd)
print("readlink", os.readlink in os.supports_dir_fd)
print("utime", os.utime in os.supports_dir_fd)

print("follow_is_set", isinstance(os.supports_follow_symlinks, set))
print("follow_stat", os.stat in os.supports_follow_symlinks)
print("follow_link", os.link in os.supports_follow_symlinks)
print("follow_utime", os.utime in os.supports_follow_symlinks)
print("follow_lstat", os.lstat in os.supports_follow_symlinks)

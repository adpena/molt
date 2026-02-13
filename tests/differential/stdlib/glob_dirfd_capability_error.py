# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Differential coverage for glob dir_fd semantics (native + wasm host split)."""

from __future__ import annotations

import glob
import os
import tempfile


with tempfile.TemporaryDirectory(prefix="molt_glob_dirfd_") as root:
    with open(os.path.join(root, "a.txt"), "w", encoding="utf-8") as handle:
        handle.write("a")

    abs_pattern = os.path.join(root, "*.txt")
    bad_fd = 1_000_000

    # Absolute paths/roots should still work even when dir_fd is unusable.
    print(
        "dir_fd_abs_bad",
        sorted(os.path.basename(match) for match in glob.glob(abs_pattern, dir_fd=bad_fd)),
    )
    print("dir_fd_root_abs_bad", glob.glob("*.txt", root_dir=root, dir_fd=bad_fd))
    print(
        "dir_fd_root_abs_bad_bytes",
        glob.glob(b"*.txt", root_dir=root.encode(), dir_fd=bad_fd),
    )

    # Relative dir_fd globbing diverges by host kind:
    # - native hosts: swallowed bad-fd errors -> []
    # - wasm browser-like hosts: explicit NotImplementedError
    try:
        print("dir_fd_rel_bad", glob.glob("*.txt", dir_fd=bad_fd))
    except Exception as exc:
        print("dir_fd_rel_bad_exc", type(exc).__name__, str(exc))

    try:
        glob.glob("*.txt", dir_fd=10**20)
    except Exception as exc:
        print("dir_fd_overflow", type(exc).__name__, str(exc))

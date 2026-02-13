# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Differential coverage for glob dir_fd edge/corner semantics."""

from __future__ import annotations

import glob
import os
import tempfile


with tempfile.TemporaryDirectory(prefix="molt_glob_dirfd_edges_") as root:
    with open(os.path.join(root, "a.txt"), "w", encoding="utf-8") as handle:
        handle.write("a")
    os.mkdir(os.path.join(root, "x1"))
    with open(os.path.join(root, "x1", "b.txt"), "w", encoding="utf-8") as handle:
        handle.write("b")
    cases = [
        (
            "star_dirfd_str_root_abs",
            {"pathname": "*.txt", "root_dir": root, "dir_fd": "."},
        ),
        (
            "subdir_dirfd_str_root_abs",
            {"pathname": "x1/*.txt", "root_dir": root, "dir_fd": "."},
        ),
        (
            "star_dirfd_bytes_root_abs",
            {"pathname": b"*.txt", "root_dir": root.encode(), "dir_fd": b"."},
        ),
        (
            "subdir_dirfd_bytes_root_abs",
            {"pathname": b"x1/*.txt", "root_dir": root.encode(), "dir_fd": b"."},
        ),
        (
            "star_dirfd_float_root_abs",
            {"pathname": "*.txt", "root_dir": root, "dir_fd": 1.25},
        ),
        (
            "subdir_dirfd_float_root_abs",
            {"pathname": "x1/*.txt", "root_dir": root, "dir_fd": 1.25},
        ),
        (
            "star_dirfd_badfd_root_abs",
            {"pathname": "*.txt", "root_dir": root, "dir_fd": 1_000_000},
        ),
        (
            "subdir_dirfd_badfd_root_abs",
            {"pathname": "x1/*.txt", "root_dir": root, "dir_fd": 1_000_000},
        ),
        (
            "star_dirfd_minus1_root_abs",
            {"pathname": "*.txt", "root_dir": root, "dir_fd": -1},
        ),
        (
            "subdir_dirfd_minus1_root_abs",
            {"pathname": "x1/*.txt", "root_dir": root, "dir_fd": -1},
        ),
    ]

    for label, kwargs in cases:
        try:
            out = glob.glob(**kwargs)
            if isinstance(out, list):
                try:
                    out = sorted(out)
                except Exception:
                    pass
            print(label, out)
        except Exception as exc:
            print(label, type(exc).__name__, str(exc))

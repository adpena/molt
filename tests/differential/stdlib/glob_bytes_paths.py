# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Differential coverage for glob bytes-path semantics."""

from __future__ import annotations

import glob
import os
import tempfile


with tempfile.TemporaryDirectory(prefix="molt_glob_bytes_") as root:
    os.makedirs(os.path.join(root, "pkg"), exist_ok=True)
    with open(os.path.join(root, "a.txt"), "w", encoding="utf-8") as handle:
        handle.write("a")
    with open(os.path.join(root, ".h.txt"), "w", encoding="utf-8") as handle:
        handle.write("h")
    with open(os.path.join(root, "pkg", "b.txt"), "w", encoding="utf-8") as handle:
        handle.write("b")

    broot = root.encode()
    abs_str_pattern = os.path.join(root, "*.txt")
    abs_bytes_pattern = abs_str_pattern.encode()

    print("has_magic_bytes", glob.has_magic(b"*.txt"))
    print(
        "bytes_root",
        sorted(glob.glob(b"*.txt", root_dir=broot)),
    )
    print(
        "bytes_include_hidden",
        sorted(glob.glob(b"*.txt", root_dir=broot, include_hidden=True)),
    )
    print(
        "bytes_recursive",
        sorted(glob.glob(b"**/*.txt", root_dir=broot, recursive=True)),
    )
    print(
        "bytes_recursive_include_hidden",
        sorted(
            glob.glob(
                b"**/*.txt",
                root_dir=broot,
                recursive=True,
                include_hidden=True,
            )
        ),
    )
    print(
        "iglob_item_type",
        type(next(iter(glob.iglob(b"*.txt", root_dir=broot)), b"")).__name__,
    )

    for label, kwargs in [
        ("str_plus_bytes_root", {"pathname": "*.txt", "root_dir": broot}),
        ("bytes_plus_str_root", {"pathname": b"*.txt", "root_dir": root}),
        (
            "str_abs_plus_bytes_root",
            {"pathname": abs_str_pattern, "root_dir": broot},
        ),
        (
            "bytes_abs_plus_str_root",
            {"pathname": abs_bytes_pattern, "root_dir": root},
        ),
    ]:
        try:
            glob.glob(**kwargs)
        except Exception as exc:
            print(label, type(exc).__name__, str(exc))

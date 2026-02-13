# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Differential coverage for glob option and edge semantics."""

from __future__ import annotations

import glob
import os
import tempfile
from pathlib import Path


def _touch(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(path)


with tempfile.TemporaryDirectory(prefix="molt_glob_opts_") as root:
    _touch(os.path.join(root, "top.txt"))
    _touch(os.path.join(root, "top.log"))
    _touch(os.path.join(root, ".top_hidden.txt"))
    _touch(os.path.join(root, "pkg", "module.py"))
    _touch(os.path.join(root, "pkg", "sub", "data.txt"))
    _touch(os.path.join(root, "pkg", ".dotpkg", "inner.txt"))
    _touch(os.path.join(root, ".hidden", "inner", "secret.txt"))

    print("has_magic", glob.has_magic("*.txt"), glob.has_magic("plain.txt"))
    print("root_star", sorted(glob.glob("*", root_dir=root)))
    print(
        "root_star_include_hidden",
        sorted(glob.glob("*", root_dir=root, include_hidden=True)),
    )
    print("explicit_dot", sorted(glob.glob(".*", root_dir=root)))
    print(
        "recursive_false",
        sorted(glob.glob("**/*.txt", root_dir=root, recursive=False)),
    )
    print(
        "recursive_true",
        sorted(glob.glob("**/*.txt", root_dir=root, recursive=True)),
    )
    print(
        "recursive_true_include_hidden",
        sorted(
            glob.glob(
                "**/*.txt",
                root_dir=root,
                recursive=True,
                include_hidden=True,
            )
        ),
    )
    print(
        "root_dir_pathlike",
        sorted(glob.glob("*.txt", root_dir=Path(root))),
    )
    abs_matches = sorted(
        glob.glob(
            os.path.join(root, "*.txt"),
            root_dir=os.path.join(root, "pkg"),
        )
    )
    print(
        "absolute_pattern_ignores_root_dir",
        sorted(os.path.basename(match) for match in abs_matches),
    )
    print("trailing_dir", glob.glob("pkg/", root_dir=root))
    print("trailing_file", glob.glob("top.txt/", root_dir=root))
    print("dot_prefix", sorted(glob.glob("./*.txt", root_dir=root)))
    print(
        "double_star_recursive",
        sorted(glob.glob("**", root_dir=root, recursive=True)),
    )

# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Differential coverage for complex, real-world glob workloads."""

from __future__ import annotations

import glob
import os
import tempfile
from pathlib import Path


def _touch(path: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("ok")


with tempfile.TemporaryDirectory(prefix="molt_glob_realworld_") as root:
    paths = [
        "src/app/main.py",
        "src/app/tests/test_main.py",
        "src/app/tests/test_cli.py",
        "src/app/static/logo.svg",
        "src/.cache/state.json",
        "data/2026/01/report.csv",
        "data/2026/02/report.csv",
        "data/2026/02/.draft.csv",
        "build/output.bin",
        ".venv/bin/python",
    ]
    for rel in paths:
        _touch(os.path.join(root, rel))

    print(
        "py_recursive",
        sorted(glob.glob("src/**/*.py", root_dir=root, recursive=True)),
    )
    print(
        "py_recursive_pathlike_root",
        sorted(glob.glob("src/**/*.py", root_dir=Path(root), recursive=True)),
    )
    print(
        "tests_only",
        sorted(glob.glob("**/test_*.py", root_dir=root, recursive=True)),
    )
    print(
        "csv_recursive_default",
        sorted(glob.glob("**/*.csv", root_dir=root, recursive=True)),
    )
    print(
        "csv_recursive_include_hidden",
        sorted(
            glob.glob(
                "**/*.csv",
                root_dir=root,
                recursive=True,
                include_hidden=True,
            )
        ),
    )
    print(
        "calendar_reports",
        sorted(glob.glob("data/2026/[0-1][0-9]/report.*", root_dir=root)),
    )
    print(
        "starstar_literal_mode",
        sorted(glob.glob("src/**/test_*.py", root_dir=root, recursive=False)),
    )
    print(
        "double_sep_pattern",
        sorted(glob.glob("src//**//*.py", root_dir=root, recursive=True)),
    )

"""Executable custody fixtures; tool behavior belongs to explicit subprocess mocks."""

from __future__ import annotations

import os
from pathlib import Path
import stat

from tests.process_guard_common import run_guarded_test_process


def write_mock_executable(path: Path, content: bytes) -> Path:
    """Write mocked tool bytes with the host's executable permission contract."""
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def native_executable_name(stem: str) -> str:
    """Return the host spelling of an executable file name."""
    return f"{stem}.exe" if os.name == "nt" else stem


def build_native_executable(path: Path, rust_source: str) -> Path:
    """Compile a tiny native executable from Rust source with the pinned rustc.

    Tool identity admits only native executables (ELF, Mach-O, PE). A test that
    needs a fake tool with real process behavior, such as answering differently
    by `argv[0]`, compiles one instead of writing a script.
    """
    source = path.with_name(f"{path.name}.rs")
    source.write_text(rust_source, encoding="utf-8")
    run_guarded_test_process(
        ["rustc", "--edition=2021", "-C", "opt-level=0", "-o", str(path), str(source)],
        cwd=path.parent,
        check=True,
    )
    return path

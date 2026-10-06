"""Executable custody fixtures; tool behavior belongs to explicit subprocess mocks."""

from __future__ import annotations

from pathlib import Path
import stat


def write_mock_executable(path: Path, content: bytes) -> Path:
    """Write mocked tool bytes with the host's executable permission contract."""
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path

from __future__ import annotations

import os
from pathlib import Path

from molt.toolchain_identity import resolve_executable
from tests.executable_test_support import custody_spelling, write_mock_executable


def test_mock_executable_preserves_bytes_and_admits_host_execution_permission(
    tmp_path: Path,
) -> None:
    path = tmp_path / "fake-compiler"
    path.write_bytes(b"before")
    path.chmod(0o600)
    assert write_mock_executable(path, b"mocked-native-image") == path
    assert path.read_bytes() == b"mocked-native-image"
    assert os.access(path, os.X_OK)
    assert resolve_executable(str(path), environment={}, label="test compiler") == path


def test_custody_spelling_lowercases_only_the_windows_anchor(tmp_path: Path) -> None:
    if os.name != "nt":
        entry = tmp_path / "Mixed Case" / "Tool"
        assert custody_spelling(entry) == str(entry)
        return
    assert custody_spelling(Path("D:/Work Dir/Tool.EXE")) == "d:\\Work Dir\\Tool.EXE"
    assert (
        custody_spelling(Path("//Build-Host/Share/Dir/Tool.exe"))
        == "\\\\build-host\\share\\Dir\\Tool.exe"
    )

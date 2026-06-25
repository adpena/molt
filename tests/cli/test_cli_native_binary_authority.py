from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import native_binary
import pytest

_NATIVE_BINARY_NAMES = (
    "_NativeBinaryInvalid",
    "_assert_native_binary_valid",
    "_darwin_binary_imports_validation_error",
    "_darwin_binary_magic_error",
    "_expected_binary_format_for_target",
    "_smoke_probe_native_binary",
    "_target_is_host_executable",
    "_validate_native_binary_format",
)

_NATIVE_BINARY_DEFINITIONS = (
    "class _NativeBinaryInvalid",
    "def _assert_native_binary_valid(",
    "def _darwin_binary_imports_validation_error(",
    "def _darwin_binary_magic_error(",
    "def _expected_binary_format_for_target(",
    "def _smoke_probe_native_binary(",
    "def _target_is_host_executable(",
    "def _validate_native_binary_format(",
)


def test_cli_native_binary_authority_is_single_home() -> None:
    for name in _NATIVE_BINARY_NAMES:
        assert getattr(cli, name) is getattr(native_binary, name)

    cli_source = inspect.getsource(cli)
    for marker in _NATIVE_BINARY_DEFINITIONS:
        assert marker not in cli_source


def _write_binary(tmp_path, name: str, header: bytes) -> None:
    (tmp_path / name).write_bytes(header + b"\x00" * 16)


@pytest.mark.parametrize(
    ("name", "header", "target"),
    [
        ("app.macho", bytes((0xCF, 0xFA, 0xED, 0xFE)), "aarch64-apple-darwin"),
        ("app.elf", b"\x7fELF", "x86_64-unknown-linux-gnu"),
        ("app.exe", b"MZ\x00\x00", "x86_64-pc-windows-msvc"),
    ],
)
def test_native_binary_validation_accepts_target_object_magic(
    tmp_path, name: str, header: bytes, target: str
) -> None:
    _write_binary(tmp_path, name, header)

    native_binary._validate_native_binary_format(tmp_path / name, target)


def test_native_binary_validation_rejects_wrong_target_object_magic(tmp_path) -> None:
    _write_binary(tmp_path, "not-windows.exe", b"\x7fELF")

    with pytest.raises(native_binary._NativeBinaryInvalid, match="PE/COFF"):
        native_binary._validate_native_binary_format(
            tmp_path / "not-windows.exe",
            "x86_64-pc-windows-msvc",
        )


def test_native_binary_validation_rejects_truncated_outputs(tmp_path) -> None:
    binary = tmp_path / "truncated"
    binary.write_bytes(b"MZ")

    with pytest.raises(native_binary._NativeBinaryInvalid, match="truncated"):
        native_binary._validate_native_binary_format(
            binary,
            "x86_64-pc-windows-msvc",
        )


def test_native_binary_validation_identifies_32_bit_macho_corruption(tmp_path) -> None:
    _write_binary(tmp_path, "corrupt-macho", bytes((0xCE, 0xFA, 0xED, 0xFE)))

    with pytest.raises(native_binary._NativeBinaryInvalid, match="32-bit magic"):
        native_binary._validate_native_binary_format(
            tmp_path / "corrupt-macho",
            "aarch64-apple-darwin",
        )


def test_expected_binary_format_for_explicit_targets() -> None:
    assert native_binary._expected_binary_format_for_target("aarch64-apple-darwin") == "macho"
    assert native_binary._expected_binary_format_for_target("x86_64-unknown-linux-gnu") == "elf"
    assert native_binary._expected_binary_format_for_target("x86_64-pc-windows-msvc") == "pe"
    assert native_binary._target_is_host_executable("wasm32-wasi") is False

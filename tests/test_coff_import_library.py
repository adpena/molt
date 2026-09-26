from __future__ import annotations

from pathlib import Path
import struct

import pytest

from molt.coff_import_library import validate_coff_import_library


_AMD64 = 0x8664
_ARM64 = 0xAA64


def _archive_member(name: str, payload: bytes) -> bytes:
    header = b"".join(
        (
            name.encode("ascii").ljust(16),
            b"0".ljust(12),
            b"0".ljust(6),
            b"0".ljust(6),
            b"100644".ljust(8),
            str(len(payload)).encode("ascii").ljust(10),
            b"`\n",
        )
    )
    assert len(header) == 60
    return header + payload + (b"\n" if len(payload) & 1 else b"")


def _archive(path: Path, dll: str, *payloads: bytes) -> Path:
    # The linker index is derived metadata, not a provider-bearing content member.
    path.write_bytes(
        b"!<arch>\n"
        + _archive_member("/", b"index")
        + b"".join(_archive_member(f"{dll}/", payload) for payload in payloads)
    )
    return path


def _short_import(
    dll: str = "python312.dll",
    *,
    machine: int = _AMD64,
    symbol: str = "PyLong_FromLong",
    import_type: int = 0,
    name_type: int = 1,
) -> bytes:
    names = symbol.encode("ascii") + b"\0" + dll.encode("ascii") + b"\0"
    if name_type == 4:
        names += b"exported_name\0"
    return (
        struct.pack(
            "<HHHHIIHH",
            0,
            0xFFFF,
            0,
            machine,
            0,
            len(names),
            0,
            import_type | (name_type << 2),
        )
        + names
    )


def _support_object(
    role: str,
    dll: str = "python312.dll",
    *,
    machine: int = _AMD64,
    symbol: str | None = None,
    extra_section: tuple[str, bytes] | None = None,
) -> bytes:
    stem = dll[:-4]
    definitions = {
        "descriptor": (
            f"__IMPORT_DESCRIPTOR_{stem}",
            [(".idata$2", bytes(20)), (".idata$6", dll.encode("ascii") + b"\0")],
        ),
        "null-descriptor": ("__NULL_IMPORT_DESCRIPTOR", [(".idata$3", bytes(20))]),
        "null-thunk": (
            f"\x7f{stem}_NULL_THUNK_DATA",
            [(".idata$5", bytes(8)), (".idata$4", bytes(8))],
        ),
    }
    defined, sections = definitions[role]
    if symbol is not None:
        defined = symbol
    if extra_section is not None:
        sections.append(extra_section)
    section_start = 20
    data_start = section_start + 40 * len(sections)
    rows = []
    data = bytearray()
    for name, payload in sections:
        assert len(name.encode("ascii")) <= 8
        rows.append(
            name.encode("ascii").ljust(8, b"\0")
            + struct.pack(
                "<IIIIIIHHI",
                0,
                0,
                len(payload),
                data_start + len(data),
                0,
                0,
                0,
                0,
                0xC0000040,
            )
        )
        data.extend(payload)
    symbol_offset = data_start + len(data)
    symbol_bytes = defined.encode("ascii")
    if len(symbol_bytes) > 8:
        name_field = bytes(4) + struct.pack("<I", 4)
        string_table = (
            struct.pack("<I", 4 + len(symbol_bytes) + 1) + symbol_bytes + b"\0"
        )
    else:
        name_field = symbol_bytes.ljust(8, b"\0")
        string_table = struct.pack("<I", 4)
    symbol_row = name_field + struct.pack("<IhHBB", 0, 1, 0, 2, 0)
    fixed = struct.pack("<HHIIIHH", machine, len(sections), 0, symbol_offset, 1, 0, 0)
    return fixed + b"".join(rows) + data + symbol_row + string_table


def test_accepts_short_imports_and_bound_descriptor_triad(tmp_path: Path) -> None:
    path = _archive(
        tmp_path / "python312.lib",
        "python312.dll",
        _support_object("descriptor"),
        _support_object("null-descriptor"),
        _support_object("null-thunk"),
        _short_import(),
        _short_import(symbol="PyExc_TypeError", import_type=1),
    )
    assert (
        validate_coff_import_library(
            path, dll_name="python312.dll", architecture="x86_64"
        )
        is None
    )


def test_pure_short_import_archive_is_supported(tmp_path: Path) -> None:
    path = _archive(tmp_path / "pure.lib", "python312.dll", _short_import())
    validate_coff_import_library(path, dll_name="PYTHON312.DLL", architecture="amd64")


@pytest.mark.parametrize(
    ("payload", "message"),
    [
        (_short_import(dll="other.dll"), "different DLL"),
        (_short_import(machine=_ARM64), "machine"),
        (_short_import()[:-1], "extent"),
        (_short_import(name_type=7), "encoding"),
        (_short_import() + b"junk", "extent"),
    ],
)
def test_rejects_short_import_provider_or_encoding_mutations(
    tmp_path: Path, payload: bytes, message: str
) -> None:
    path = _archive(tmp_path / "bad.lib", "python312.dll", payload)
    with pytest.raises(ValueError, match=message):
        validate_coff_import_library(
            path, dll_name="python312.dll", architecture="x86_64"
        )


@pytest.mark.parametrize(
    ("payload", "message"),
    [
        (_support_object("descriptor", symbol="unrelated"), "non-import support"),
        (
            _support_object(
                "descriptor", dll="other.dll", symbol="__IMPORT_DESCRIPTOR_python312"
            ),
            "different DLL",
        ),
        (_support_object("descriptor", machine=_ARM64), "machine"),
        (
            _support_object("descriptor", extra_section=(".text", b"code")),
            "unexpected section",
        ),
    ],
)
def test_rejects_foreign_or_non_import_regular_coff_member(
    tmp_path: Path, payload: bytes, message: str
) -> None:
    path = _archive(tmp_path / "bad.lib", "python312.dll", payload, _short_import())
    with pytest.raises(ValueError, match=message):
        validate_coff_import_library(
            path, dll_name="python312.dll", architecture="x86_64"
        )


def test_rejects_missing_or_repeated_support_role(tmp_path: Path) -> None:
    path = _archive(
        tmp_path / "missing.lib",
        "python312.dll",
        _support_object("descriptor"),
        _short_import(),
    )
    with pytest.raises(ValueError, match="triad is incomplete"):
        validate_coff_import_library(
            path, dll_name="python312.dll", architecture="x86_64"
        )
    path = _archive(
        tmp_path / "repeated.lib",
        "python312.dll",
        _support_object("descriptor"),
        _support_object("descriptor"),
        _short_import(),
    )
    with pytest.raises(ValueError, match="duplicated"):
        validate_coff_import_library(
            path, dll_name="python312.dll", architecture="x86_64"
        )


def test_rejects_static_archive_without_short_import_records(tmp_path: Path) -> None:
    path = _archive(
        tmp_path / "static.lib", "python312.dll", _support_object("descriptor")
    )
    with pytest.raises(ValueError, match="no short import records"):
        validate_coff_import_library(
            path, dll_name="python312.dll", architecture="x86_64"
        )


def test_rejects_member_name_mismatch_even_if_payload_matches(tmp_path: Path) -> None:
    path = _archive(tmp_path / "bad.lib", "other.dll", _short_import())
    with pytest.raises(ValueError, match="member names a different DLL"):
        validate_coff_import_library(
            path, dll_name="python312.dll", architecture="x86_64"
        )

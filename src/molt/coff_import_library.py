"""Bind a COFF import archive to one DLL and target machine.

This recognizes the MSVC/LLVM short-import format and its optional import
descriptor, null descriptor, and null thunk COFF objects. It deliberately does
not admit arbitrary COFF objects merely because they share an archive name.
The caller owns the separate live file-node/content custody check.
"""

from __future__ import annotations

from pathlib import Path
import re
import struct
from typing import BinaryIO

from molt.cli.static_archive_identity import (
    StaticArchiveMember,
    visit_static_archive_members,
)
from molt.native_artifact_header import (
    NativeFileKind,
    NativeReader,
    decode_native_artifact,
)
from molt.native_target_shape import NativeObjectFormat, native_artifact_shape


_SHORT_IMPORT_SIGNATURE = b"\0\0\xff\xff"
_MAX_MEMBER_BYTES = 16 * 1024 * 1024
_IMAGE_SCN_MEM_EXECUTE = 0x20000000
_IMAGE_SCN_CNT_CODE = 0x20


def _ascii(raw: bytes, label: str) -> str:
    try:
        return raw.decode("ascii", "strict")
    except UnicodeDecodeError as exc:
        raise ValueError(f"COFF import library {label} is not ASCII") from exc


def _short_import(
    reader: NativeReader, dll_name: str, machines: tuple[int, ...]
) -> None:
    header = reader.read(0, 20, "COFF short import header")
    signature1, signature2, version, machine, _stamp, size, _hint, flags = (
        struct.unpack("<HHHHIIHH", header)
    )
    if (signature1, signature2) != (0, 0xFFFF) or version != 0:
        raise ValueError("COFF short import signature/version is invalid")
    if machine not in machines:
        raise ValueError("COFF short import machine disagrees with target architecture")
    if size != reader.size - 20 or not size or size > _MAX_MEMBER_BYTES:
        raise ValueError("COFF short import data extent is invalid")
    import_type, name_type = flags & 3, (flags >> 2) & 7
    if flags >> 5 or import_type == 3 or name_type > 4:
        raise ValueError("COFF short import type/name encoding is invalid")
    strings = reader.read(20, size, "COFF short import names").split(b"\0")
    expected = 3 if name_type == 4 else 2  # IMPORT_NAME_EXPORTAS has a third name.
    if len(strings) != expected + 1 or strings[-1] or any(not s for s in strings[:-1]):
        raise ValueError("COFF short import names are malformed")
    _ascii(strings[0], "symbol name")
    actual_dll = _ascii(strings[1], "DLL name")
    if actual_dll.casefold() != dll_name.casefold():
        raise ValueError("COFF short import targets a different DLL")
    if expected == 3:
        _ascii(strings[2], "export name")


def _coff_symbol_name(raw: bytes, string_table: bytes) -> str:
    if raw[:4] == bytes(4):
        offset = struct.unpack_from("<I", raw, 4)[0]
        if offset < 4 or offset >= len(string_table):
            raise ValueError("COFF import support symbol string offset is invalid")
        end = string_table.find(b"\0", offset)
        if end < 0:
            raise ValueError("COFF import support symbol is unterminated")
        name = _ascii(string_table[offset:end], "support symbol")
    else:
        name = _ascii(raw.rstrip(b"\0"), "support symbol")
    if not name:
        raise ValueError("COFF import support symbol name is empty")
    return name


def _support_object(
    reader: NativeReader, dll_name: str, machines: tuple[int, ...]
) -> str:
    artifact = decode_native_artifact(reader)
    if len(artifact.headers) != 1:
        raise ValueError("COFF import support member must be one object")
    native = artifact.headers[0]
    if (
        native.object_format is not NativeObjectFormat.COFF
        or native.kind is not NativeFileKind.OBJECT
        or native.machine not in machines
    ):
        raise ValueError("COFF import support member has a different target machine")
    fixed = reader.read(0, 20, "COFF import support header")
    _machine, section_count, _stamp, symbol_offset, symbol_count, _optional, _flags = (
        struct.unpack("<HHIIIHH", fixed)
    )
    if not symbol_count:
        raise ValueError("COFF import support member has no defining symbol")
    symbol_end = symbol_offset + symbol_count * 18
    table_size = struct.unpack(
        "<I", reader.read(symbol_end, 4, "COFF string table size")
    )[0]
    if table_size < 4 or table_size > _MAX_MEMBER_BYTES:
        raise ValueError("COFF import support string table size is invalid")
    string_table = reader.read(symbol_end, table_size, "COFF string table")
    if symbol_end + table_size != reader.size:
        raise ValueError("COFF import support member has trailing payload")

    sections: dict[str, bytes] = {}
    section_names: list[str] = []
    header_end = 20 + section_count * 40
    for index in range(section_count):
        row = reader.read(20 + index * 40, 40, "COFF import support section")
        name = _ascii(row[:8].rstrip(b"\0"), "support section name")
        size, offset, reloc_offset, _line_offset, reloc_count, line_count, flags = (
            struct.unpack_from("<IIIIHHI", row, 16)
        )
        if name in sections or name not in {
            ".debug$S",
            ".idata$2",
            ".idata$3",
            ".idata$4",
            ".idata$5",
            ".idata$6",
        }:
            raise ValueError("COFF import support has an unexpected section")
        if flags & (_IMAGE_SCN_MEM_EXECUTE | _IMAGE_SCN_CNT_CODE) or line_count:
            raise ValueError(
                "COFF import support contains executable or line-number data"
            )
        if not size or offset < header_end or offset + size > symbol_offset:
            raise ValueError("COFF import support section extent is invalid")
        if reloc_count:
            if (
                reloc_offset < header_end
                or reloc_offset + reloc_count * 10 > symbol_offset
            ):
                raise ValueError("COFF import support relocation extent is invalid")
            reader.extent(
                reloc_offset, reloc_count * 10, "COFF import support relocations"
            )
        section_names.append(name)
        sections[name] = reader.read(offset, size, "COFF import support section data")

    defined: list[tuple[str, str]] = []
    ordinal = 0
    while ordinal < symbol_count:
        row = reader.read(
            symbol_offset + ordinal * 18, 18, "COFF import support symbol"
        )
        name = _coff_symbol_name(row[:8], string_table)
        section = struct.unpack_from("<h", row, 12)[0]
        storage, aux_count = row[16], row[17]
        if storage == 2 and section > 0:
            if section > section_count:
                raise ValueError("COFF import support symbol section is invalid")
            defined.append((name, section_names[section - 1]))
        if aux_count >= symbol_count - ordinal:
            raise ValueError("COFF import support auxiliary symbol count is invalid")
        ordinal += 1 + aux_count

    stem = dll_name[:-4]
    roles = {
        f"__IMPORT_DESCRIPTOR_{stem}".casefold(): ("descriptor", ".idata$2"),
        "__NULL_IMPORT_DESCRIPTOR".casefold(): ("null-descriptor", ".idata$3"),
        f"\x7f{stem}_NULL_THUNK_DATA".casefold(): ("null-thunk", ".idata$5"),
    }
    if len(defined) != 1 or defined[0][0].casefold() not in roles:
        raise ValueError("COFF archive contains a non-import support object")
    role, defining_section = roles[defined[0][0].casefold()]
    if defined[0][1] != defining_section:
        raise ValueError("COFF import support symbol has the wrong defining section")
    expected_sections = {
        "descriptor": {".idata$2", ".idata$6"},
        "null-descriptor": {".idata$3"},
        "null-thunk": {".idata$4", ".idata$5"},
    }[role]
    if set(sections) - {".debug$S"} != expected_sections:
        raise ValueError("COFF import support sections disagree with their role")
    if role == "descriptor":
        if sections[".idata$2"] != bytes(20):
            raise ValueError("COFF import descriptor has an invalid extent")
        dll_data = sections[".idata$6"]
        if (
            not dll_data.endswith(b"\0")
            or dll_data.rstrip(b"\0").lower() != dll_name.encode("ascii").lower()
        ):
            raise ValueError("COFF import descriptor names a different DLL")
    elif role == "null-descriptor":
        if sections[".idata$3"] != bytes(20):
            raise ValueError("COFF null import descriptor is malformed")
    else:
        pointer_bytes = native.bits // 8
        if any(sections[name] != bytes(pointer_bytes) for name in expected_sections):
            raise ValueError("COFF null import thunk is malformed")
    return role


def validate_coff_import_library(
    path: Path, *, dll_name: str, architecture: str
) -> None:
    """Reject a non-import, mixed-provider, malformed, or wrong-machine archive.

    The check is structural; call it only after the owning runtime-file node's
    live identity has been verified. It does not verify imported symbol coverage
    or that the target DLL exports every symbol in this archive.
    """
    if (
        not isinstance(dll_name, str)
        or re.fullmatch(r"[A-Za-z0-9_.-]+\.dll", dll_name, re.I) is None
    ):
        raise ValueError("COFF import DLL name must be a bare ASCII .dll name")
    try:
        machines = native_artifact_shape(
            architecture, object_format=NativeObjectFormat.COFF
        ).coff_machines
    except RuntimeError as exc:
        raise ValueError(str(exc)) from exc
    short_count = 0
    roles: set[str] = set()

    def visit(member: StaticArchiveMember, stream: BinaryIO) -> None:
        nonlocal short_count
        if member.name.casefold() != dll_name.casefold():
            raise ValueError("COFF import archive member names a different DLL")
        if not 20 <= member.size <= _MAX_MEMBER_BYTES:
            raise ValueError("COFF import archive member extent is invalid")

        def read_at(offset: int, size: int) -> bytes:
            stream.seek(member.content_offset + offset)
            return stream.read(size)

        reader = NativeReader(member.size, read_at)
        if reader.read(0, 4, "COFF import member magic") == _SHORT_IMPORT_SIGNATURE:
            _short_import(reader, dll_name, machines)
            short_count += 1
        else:
            role = _support_object(reader, dll_name, machines)
            if role in roles:
                raise ValueError("COFF import support role is duplicated")
            roles.add(role)

    count = visit_static_archive_members(path, visit_member=visit)
    if not count or not short_count:
        raise ValueError("COFF archive contains no short import records")
    if roles and roles != {"descriptor", "null-descriptor", "null-thunk"}:
        raise ValueError("COFF import support object triad is incomplete")

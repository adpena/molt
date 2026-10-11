"""In-process global symbol tables for native and WebAssembly objects.

This module reads the symbol table of one ELF, Mach-O (thin or universal),
COFF, PE, COFF short-import or WebAssembly relocatable object. It reports each
global symbol with the type letter that ``llvm-nm -g --no-llvm-bc`` prints for
it, so the facts keep one meaning for the in-process reader and for the
``llvm-nm`` reader that LLVM bitcode still needs. The letters follow LLVM's
classification, including its format quirks: Mach-O weak symbols keep their
section letter, and a weak ELF symbol that is not ``STT_OBJECT`` is ``W``.

Every read goes through ``NativeReader``, so no read leaves the mapped extent.
Truncated, overlapping or inconsistent tables raise ``NativeSymbolTableError``.
The reader never runs a subprocess.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
import struct
from typing import Iterable

from molt.native_artifact_header import (
    ElfHeader,
    MachOHeader,
    NativeArtifactError,
    NativeHeader,
    NativeReader,
    decode_native_artifact,
)
from molt.native_target_shape import NativeArtifactShape, coff_machine_bits


class NativeSymbolTableError(NativeArtifactError):
    """The input has no readable global symbol table in a format this reader owns."""


@dataclass(frozen=True, slots=True)
class NativeSymbolRow:
    """One global symbol as ``llvm-nm -g`` reports it.

    ``kind`` is the ``llvm-nm`` type letter. ``indirect`` names the target of a
    Mach-O ``N_INDR`` symbol, which ``llvm-nm`` prints as ``(indirect for X)``.
    """

    kind: str
    name: str
    indirect: str | None = None


class SymbolInputFormat(str, Enum):
    ELF = "elf"
    MACHO = "macho"
    MACHO_UNIVERSAL = "macho-universal"
    COFF = "coff"
    COFF_SHORT_IMPORT = "coff-short-import"
    PE = "pe"
    WASM = "wasm"
    LLVM_BITCODE = "llvm-bitcode"
    ARCHIVE = "archive"


_MACHO_MAGICS = frozenset(
    {b"\xce\xfa\xed\xfe", b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xce", b"\xfe\xed\xfa\xcf"}
)
_FAT_MAGICS = frozenset(
    {b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca", b"\xca\xfe\xba\xbf", b"\xbf\xba\xfe\xca"}
)
# Raw bitcode starts with "BC" 0xC0DE; the wrapper header is 0x0B17C0DE (LE).
_LLVM_BITCODE_MAGICS = (b"BC\xc0\xde", b"\xde\xc0\x17\x0b")
_COFF_SHORT_SIGNATURE = b"\0\0\xff\xff"
_WASM_MAGIC = b"\0asm"


def is_llvm_bitcode(prefix: bytes) -> bool:
    """Raw or wrapped LLVM bitcode, which only ``llvm-nm`` reads."""
    return prefix[:4] in _LLVM_BITCODE_MAGICS


def symbol_input_format(prefix: bytes) -> SymbolInputFormat:
    """Classify an input by its leading bytes; unknown bytes raise.

    COFF objects have no magic number. Their first two bytes are a machine
    value, which must name a COFF machine that the target-shape table knows.
    """
    magic = prefix[:4]
    if prefix[:8] in (b"!<arch>\n", b"!<thin>\n"):
        return SymbolInputFormat.ARCHIVE
    if magic in _LLVM_BITCODE_MAGICS:
        return SymbolInputFormat.LLVM_BITCODE
    if magic == b"\x7fELF":
        return SymbolInputFormat.ELF
    if magic in _MACHO_MAGICS:
        return SymbolInputFormat.MACHO
    if magic in _FAT_MAGICS:
        return SymbolInputFormat.MACHO_UNIVERSAL
    if magic == _WASM_MAGIC:
        return SymbolInputFormat.WASM
    if magic == _COFF_SHORT_SIGNATURE:
        # A bigobj header shares this signature; its version field is 2.
        if len(prefix) >= 6 and struct.unpack_from("<H", prefix, 4)[0] == 0:
            return SymbolInputFormat.COFF_SHORT_IMPORT
        return SymbolInputFormat.COFF
    if magic[:2] == b"MZ":
        return SymbolInputFormat.PE
    if len(prefix) >= 2:
        try:
            coff_machine_bits(struct.unpack_from("<H", prefix)[0])
        except RuntimeError:
            pass
        else:
            return SymbolInputFormat.COFF
    raise NativeSymbolTableError(
        f"unrecognized object format (leading bytes {prefix[:8].hex() or 'none'})"
    )


def read_symbol_rows(
    reader: NativeReader,
    *,
    macho_shape: NativeArtifactShape | None,
) -> tuple[NativeSymbolRow, ...]:
    """Read the global symbols of one object, image or short-import member.

    ``macho_shape`` selects the slice of a universal Mach-O file. Archives and
    LLVM bitcode are not objects this function reads: the caller frames
    archive members, and only ``llvm-nm`` reads bitcode.
    """
    if reader.size < 4:
        raise NativeSymbolTableError(
            f"object is too short for any known format ({reader.size} bytes)"
        )
    input_format = symbol_input_format(
        reader.read(0, 8 if reader.size >= 8 else 4, "object magic")
    )
    if input_format is SymbolInputFormat.ARCHIVE:
        raise NativeSymbolTableError("a nested archive is not an object")
    if input_format is SymbolInputFormat.LLVM_BITCODE:
        raise NativeSymbolTableError("LLVM bitcode has no native symbol table")
    if input_format is SymbolInputFormat.COFF_SHORT_IMPORT:
        return _short_import_rows(reader)
    if input_format is SymbolInputFormat.WASM:
        return _wasm_rows(reader.read(0, reader.size, "WebAssembly object"))
    if input_format is SymbolInputFormat.PE:
        return _pe_rows(reader)
    artifact = decode_native_artifact(reader)
    if input_format is SymbolInputFormat.MACHO_UNIVERSAL:
        header = _universal_slice(artifact.headers, macho_shape)
        return _macho_rows(
            NativeReader(header.size, reader.read_at, header.offset), header
        )
    header = artifact.headers[0]
    if input_format is SymbolInputFormat.ELF:
        return _elf_rows(reader, header)
    if input_format is SymbolInputFormat.MACHO:
        return _macho_rows(reader, header)
    return _coff_object_rows(reader)


def _c_string(table: bytes, offset: int, label: str) -> str:
    if offset >= len(table):
        raise NativeSymbolTableError(
            f"{label} offset {offset} is outside its string table ({len(table)} bytes)"
        )
    end = table.find(b"\0", offset)
    if end < 0:
        raise NativeSymbolTableError(f"{label} at offset {offset} has no terminator")
    try:
        return table[offset:end].decode("utf-8")
    except UnicodeDecodeError as error:
        raise NativeSymbolTableError(f"{label} is not UTF-8: {error}") from error


def _reject_overlaps(regions: Iterable[tuple[str, int, int]]) -> None:
    occupied = sorted(
        (offset, offset + size, label) for label, offset, size in regions if size
    )
    for (_, end, label), (start, _, other) in zip(occupied, occupied[1:]):
        if start < end:
            raise NativeSymbolTableError(f"{label} overlaps {other}")


# ELF ---------------------------------------------------------------------

_SHT_SYMTAB = 2
_SHT_STRTAB = 3
_SHT_NOBITS = 8
_SHT_SYMTAB_SHNDX = 18
_SHF_WRITE = 0x1
_SHF_ALLOC = 0x2
_SHF_EXECINSTR = 0x4
_SHN_UNDEF = 0
_SHN_LORESERVE = 0xFF00
_SHN_ABS = 0xFFF1
_SHN_COMMON = 0xFFF2
_SHN_XINDEX = 0xFFFF
_STB_LOCAL = 0
_STB_GLOBAL = 1
_STB_WEAK = 2
_STB_GNU_UNIQUE = 10
_STT_OBJECT = 1
_STT_SECTION = 3
_STT_FILE = 4
_STT_COMMON = 5
_STT_GNU_IFUNC = 10
# LLVM marks these mapping symbols format-specific; llvm-nm hides them.
_ELF_MAPPING_PREFIXES = {
    40: ("$a", "$d", "$t"),  # EM_ARM
    183: ("$d", "$x"),  # EM_AARCH64
    243: ("$d", "$x"),  # EM_RISCV
}


def _elf_rows(
    reader: NativeReader, header: NativeHeader
) -> tuple[NativeSymbolRow, ...]:
    meta = header.metadata
    assert isinstance(meta, ElfHeader)
    if not meta.section_count:
        return ()
    endian = header.endian
    wide = header.bits == 64
    section_struct = struct.Struct(endian + ("IIQQQQIIQQ" if wide else "IIIIIIIIII"))
    table = reader.read(
        meta.section_offset,
        meta.section_count * meta.section_entry_size,
        "ELF section headers",
    )
    # name, type, flags, addr, offset, size, link, info, addralign, entsize
    sections = [
        section_struct.unpack_from(table, index * meta.section_entry_size)
        for index in range(meta.section_count)
    ]
    symtabs = [index for index, row in enumerate(sections) if row[1] == _SHT_SYMTAB]
    if not symtabs:
        return ()
    if len(symtabs) > 1:
        raise NativeSymbolTableError("ELF has more than one SHT_SYMTAB section")
    symtab_index = symtabs[0]
    _, _, _, _, symtab_offset, symtab_size, string_index, _, _, entry_size = sections[
        symtab_index
    ]
    expected_entry = 24 if wide else 16
    if entry_size != expected_entry or symtab_size % expected_entry:
        raise NativeSymbolTableError(
            f"ELF symbol table entry size {entry_size} or size {symtab_size} is invalid"
        )
    if not 0 < string_index < meta.section_count:
        raise NativeSymbolTableError("ELF symbol table links no string table")
    string_row = sections[string_index]
    if string_row[1] != _SHT_STRTAB:
        raise NativeSymbolTableError("ELF symbol table links a non-SHT_STRTAB section")
    string_offset, string_size = string_row[4], string_row[5]
    count = symtab_size // expected_entry
    shndx_rows = [
        row
        for row in sections
        if row[1] == _SHT_SYMTAB_SHNDX and row[6] == symtab_index
    ]
    if len(shndx_rows) > 1:
        raise NativeSymbolTableError("ELF has more than one SHT_SYMTAB_SHNDX table")
    regions = [
        ("ELF header", 0, 64 if wide else 52),
        (
            "ELF section headers",
            meta.section_offset,
            meta.section_count * meta.section_entry_size,
        ),
        ("ELF symbol table", symtab_offset, symtab_size),
        ("ELF string table", string_offset, string_size),
    ]
    extended: tuple[int, ...] | None = None
    if shndx_rows:
        shndx_offset, shndx_size = shndx_rows[0][4], shndx_rows[0][5]
        if shndx_size != 4 * count:
            raise NativeSymbolTableError(
                "ELF SHT_SYMTAB_SHNDX size disagrees with its symbol table"
            )
        regions.append(("ELF extended section index table", shndx_offset, shndx_size))
        extended = struct.unpack(
            f"{endian}{count}I",
            reader.read(shndx_offset, shndx_size, "ELF extended section indexes"),
        )
    _reject_overlaps(regions)
    symbols = reader.read(symtab_offset, symtab_size, "ELF symbol table")
    strings = reader.read(string_offset, string_size, "ELF string table")
    if strings and strings[-1] != 0:
        raise NativeSymbolTableError("ELF string table is not NUL-terminated")
    mapping_prefixes = _ELF_MAPPING_PREFIXES.get(header.machine, ())
    symbol_format = endian + ("IBBHQQ" if wide else "IIIBBH")
    rows: list[NativeSymbolRow] = []
    for index, fields in enumerate(struct.iter_unpack(symbol_format, symbols)):
        if wide:
            name_offset, info, _other, shndx, _value, _size = fields
        else:
            name_offset, _value, _size, info, _other, shndx = fields
        binding, symbol_type = info >> 4, info & 0xF
        if (
            index == 0
            or binding == _STB_LOCAL
            or symbol_type in (_STT_SECTION, _STT_FILE)
        ):
            continue
        name = _c_string(strings, name_offset, "ELF symbol name")
        if mapping_prefixes and name.startswith(mapping_prefixes):
            continue
        if shndx == _SHN_XINDEX:
            if extended is None:
                raise NativeSymbolTableError(
                    f"ELF symbol {name!r} uses SHN_XINDEX without SHT_SYMTAB_SHNDX"
                )
            section_index = extended[index]
        else:
            section_index = shndx
        rows.append(
            NativeSymbolRow(
                _elf_kind(
                    name,
                    binding=binding,
                    symbol_type=symbol_type,
                    shndx=shndx,
                    section_index=section_index,
                    sections=sections,
                ),
                name,
            )
        )
    return tuple(rows)


def _elf_kind(
    name: str,
    *,
    binding: int,
    symbol_type: int,
    shndx: int,
    section_index: int,
    sections: list[tuple[int, ...]],
) -> str:
    undefined = shndx == _SHN_UNDEF
    if binding == _STB_WEAK:
        kind = "v" if symbol_type == _STT_OBJECT else "w"
        return kind if undefined else kind.upper()
    if undefined:
        return "U"
    if symbol_type == _STT_COMMON or shndx == _SHN_COMMON:
        return "C"
    if shndx == _SHN_ABS:
        return "A"
    if binding == _STB_GNU_UNIQUE:
        return "u"
    if binding != _STB_GLOBAL:
        raise NativeSymbolTableError(
            f"ELF symbol {name!r} has unsupported binding {binding}"
        )
    if symbol_type == _STT_GNU_IFUNC:
        return "i"
    if shndx >= _SHN_LORESERVE and shndx != _SHN_XINDEX:
        raise NativeSymbolTableError(
            f"ELF symbol {name!r} names reserved section index {shndx:#x}"
        )
    if not 0 < section_index < len(sections):
        raise NativeSymbolTableError(
            f"ELF symbol {name!r} names missing section {section_index}"
        )
    section_type, flags = sections[section_index][1], sections[section_index][2]
    if flags & _SHF_EXECINSTR:
        return "T"
    if section_type == _SHT_NOBITS:
        return "B"
    if flags & _SHF_ALLOC:
        return "D" if flags & _SHF_WRITE else "R"
    raise NativeSymbolTableError(
        f"ELF global symbol {name!r} is defined in a non-allocated section"
    )


# Mach-O -------------------------------------------------------------------

_LC_SEGMENT = 0x1
_LC_SYMTAB = 0x2
_LC_SEGMENT_64 = 0x19
_MH_KEXT_BUNDLE = 0xB
_N_STAB = 0xE0
_N_TYPE = 0x0E
_N_EXT = 0x01
_N_UNDF = 0x0
_N_ABS = 0x2
_N_INDR = 0xA
_N_SECT = 0xE


def _fixed_name(raw: bytes) -> str:
    return raw.split(b"\0", 1)[0].decode("utf-8", "replace")


def _macho_rows(
    reader: NativeReader, header: NativeHeader
) -> tuple[NativeSymbolRow, ...]:
    meta = header.metadata
    assert isinstance(meta, MachOHeader)
    endian = header.endian
    wide = header.bits == 64
    filetype = struct.unpack(endian + "I", reader.read(12, 4, "Mach-O file type"))[0]
    commands = reader.read(meta.header_size, meta.command_bytes, "Mach-O load commands")
    alignment = 8 if wide else 4
    sections: list[tuple[str, str]] = []
    symtab: tuple[int, int, int, int] | None = None
    cursor = 0
    for _ in range(meta.command_count):
        if cursor + 8 > len(commands):
            raise NativeSymbolTableError("Mach-O load command header is truncated")
        command, size = struct.unpack_from(endian + "II", commands, cursor)
        if size < 8 or size % alignment or cursor + size > len(commands):
            raise NativeSymbolTableError(
                f"Mach-O load command {command:#x} has invalid size {size}"
            )
        if command in (_LC_SEGMENT, _LC_SEGMENT_64):
            segment_size, section_size, count_offset = (
                (72, 80, 64) if command == _LC_SEGMENT_64 else (56, 68, 48)
            )
            if size < segment_size:
                raise NativeSymbolTableError("Mach-O segment command is truncated")
            count = struct.unpack_from(endian + "I", commands, cursor + count_offset)[0]
            if segment_size + count * section_size > size:
                raise NativeSymbolTableError(
                    "Mach-O segment sections exceed their load command"
                )
            for index in range(count):
                start = cursor + segment_size + index * section_size
                sections.append(
                    (
                        _fixed_name(commands[start + 16 : start + 32]),
                        _fixed_name(commands[start : start + 16]),
                    )
                )
        elif command == _LC_SYMTAB:
            if symtab is not None:
                raise NativeSymbolTableError("Mach-O has more than one LC_SYMTAB")
            if size != 24:
                raise NativeSymbolTableError("Mach-O LC_SYMTAB has an invalid size")
            symtab = struct.unpack_from(endian + "IIII", commands, cursor + 8)
        cursor += size
    if symtab is None:
        return ()
    symbol_offset, symbol_count, string_offset, string_size = symtab
    if not symbol_count:
        return ()
    entry = 16 if wide else 12
    _reject_overlaps(
        (
            (
                "Mach-O header and load commands",
                0,
                meta.header_size + meta.command_bytes,
            ),
            ("Mach-O symbol table", symbol_offset, symbol_count * entry),
            ("Mach-O string table", string_offset, string_size),
        )
    )
    symbols = reader.read(symbol_offset, symbol_count * entry, "Mach-O symbol table")
    strings = reader.read(string_offset, string_size, "Mach-O string table")
    rows: list[NativeSymbolRow] = []
    for name_offset, n_type, n_sect, _desc, value in struct.iter_unpack(
        endian + ("IBBHQ" if wide else "IBBHI"), symbols
    ):
        if n_type & _N_STAB or not n_type & _N_EXT:
            continue
        name = _c_string(strings, name_offset, "Mach-O symbol name")
        kind_bits = n_type & _N_TYPE
        indirect: str | None = None
        if kind_bits == _N_UNDF:
            kind = "C" if value else "U"
        elif kind_bits == _N_ABS:
            kind = "A"
        elif kind_bits == _N_INDR:
            kind = "I"
            indirect = _c_string(strings, value, "Mach-O indirect symbol name")
        elif kind_bits == _N_SECT:
            if not 0 < n_sect <= len(sections):
                raise NativeSymbolTableError(
                    f"Mach-O symbol {name!r} names missing section {n_sect}"
                )
            segment, section = sections[n_sect - 1]
            if section == "__text" and (
                segment == "__TEXT"
                or (wide and filetype == _MH_KEXT_BUNDLE and segment == "__TEXT_EXEC")
            ):
                kind = "T"
            elif segment == "__DATA" and section == "__data":
                kind = "D"
            elif segment == "__DATA" and section == "__bss":
                kind = "B"
            else:
                kind = "S"
        else:
            raise NativeSymbolTableError(
                f"Mach-O symbol {name!r} has unsupported type {kind_bits:#x}"
            )
        rows.append(NativeSymbolRow(kind, name, indirect))
    return tuple(rows)


def _universal_slice(
    headers: tuple[NativeHeader, ...], shape: NativeArtifactShape | None
) -> NativeHeader:
    """Select the slice ``llvm-nm --arch`` names for the target architecture.

    The CPU type and the base subtype must both match. mach/machine.h keeps
    capability bits in the high byte of the subtype, so they do not take part.
    An arm64 target never reads an arm64e slice, and x86_64 never reads x86_64h.
    """
    if shape is None or shape.macho_cpu is None or shape.macho_subtype is None:
        raise NativeSymbolTableError(
            "a universal Mach-O file needs a Mach-O target architecture to select a slice"
        )
    matches = [
        header
        for header in headers
        if header.machine == shape.macho_cpu
        and isinstance(header.metadata, MachOHeader)
        and header.metadata.subtype & 0x00FFFFFF == shape.macho_subtype
    ]
    if len(matches) != 1:
        raise NativeSymbolTableError(
            f"universal Mach-O file has {len(matches)} slices for "
            f"{shape.architecture} (cpu {shape.macho_cpu:#x}, "
            f"subtype {shape.macho_subtype})"
        )
    return matches[0]


# COFF and PE ------------------------------------------------------------------

_IMAGE_SYM_CLASS_EXTERNAL = 2
_IMAGE_SYM_CLASS_WEAK_EXTERNAL = 105
_IMAGE_SYM_ABSOLUTE = -1
_IMAGE_WEAK_EXTERN_SEARCH_ALIAS = 3
_IMAGE_SCN_CNT_CODE = 0x20
_IMAGE_SCN_CNT_INITIALIZED_DATA = 0x40
_IMAGE_SCN_CNT_UNINITIALIZED_DATA = 0x80
_IMAGE_SCN_LNK_INFO = 0x200
_IMAGE_SCN_MEM_WRITE = 0x80000000
_BASE64_DIGITS = {
    character: value
    for value, character in enumerate(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    )
}


@dataclass(frozen=True, slots=True)
class _CoffLayout:
    section_offset: int
    section_count: int
    symbol_offset: int
    symbol_count: int
    symbol_size: int
    headers_end: int


def _coff_object_rows(reader: NativeReader) -> tuple[NativeSymbolRow, ...]:
    signature = reader.read(0, 4, "COFF header")
    if signature == _COFF_SHORT_SIGNATURE:
        fixed = reader.read(0, 56, "COFF bigobj header")
        count, symbol_offset, symbol_count = struct.unpack_from("<III", fixed, 44)
        layout = _CoffLayout(
            56, count, symbol_offset, symbol_count, 20, 56 + 40 * count
        )
    else:
        fixed = reader.read(0, 20, "COFF header")
        _machine, count, _stamp, symbol_offset, symbol_count, optional, _flags = (
            struct.unpack("<HHIIIHH", fixed)
        )
        start = 20 + optional
        layout = _CoffLayout(
            start, count, symbol_offset, symbol_count, 18, start + 40 * count
        )
    return _coff_rows(reader, layout)


def _pe_rows(reader: NativeReader) -> tuple[NativeSymbolRow, ...]:
    decode_native_artifact(reader)  # Validates the DOS, PE and section headers.
    pe_offset = struct.unpack("<I", reader.read(60, 4, "PE header offset"))[0]
    fixed = reader.read(pe_offset + 4, 20, "PE COFF header")
    _machine, count, _stamp, symbol_offset, symbol_count, optional, _flags = (
        struct.unpack("<HHIIIHH", fixed)
    )
    start = pe_offset + 24 + optional
    layout = _CoffLayout(
        start, count, symbol_offset, symbol_count, 18, start + 40 * count
    )
    return _coff_rows(reader, layout)


def _coff_rows(
    reader: NativeReader, layout: _CoffLayout
) -> tuple[NativeSymbolRow, ...]:
    if not layout.symbol_offset or not layout.symbol_count:
        return ()
    symbol_bytes = layout.symbol_count * layout.symbol_size
    if layout.symbol_offset < layout.headers_end:
        raise NativeSymbolTableError("COFF symbol table overlaps the section headers")
    symbols = reader.read(layout.symbol_offset, symbol_bytes, "COFF symbol table")
    string_start = layout.symbol_offset + symbol_bytes
    # The size field counts itself. LLVM reads a size below four as empty.
    declared = struct.unpack(
        "<I", reader.read(string_start, 4, "COFF string table size")
    )[0]
    strings = (
        reader.read(string_start, declared, "COFF string table")
        if declared > 4
        else b"\0\0\0\0"
    )
    if declared > 4 and strings[-1] != 0:
        raise NativeSymbolTableError("COFF string table is not NUL-terminated")
    section_table = reader.read(
        layout.section_offset, 40 * layout.section_count, "COFF section headers"
    )
    big = layout.symbol_size == 20
    record = struct.Struct("<8sIiHBB" if big else "<8sIhHBB")
    rows: list[NativeSymbolRow] = []
    index = 0
    while index < layout.symbol_count:
        raw_name, value, section, _type, storage, aux = record.unpack_from(
            symbols, index * layout.symbol_size
        )
        if aux >= layout.symbol_count - index:
            raise NativeSymbolTableError(
                f"COFF symbol {index} auxiliary records run past the symbol table"
            )
        if storage in (_IMAGE_SYM_CLASS_EXTERNAL, _IMAGE_SYM_CLASS_WEAK_EXTERNAL):
            name = _coff_symbol_name(raw_name, strings)
            weak_alias: bool | None = None
            if storage == _IMAGE_SYM_CLASS_WEAK_EXTERNAL and aux:
                characteristics = struct.unpack_from(
                    "<I", symbols, (index + 1) * layout.symbol_size + 4
                )[0]
                weak_alias = characteristics == _IMAGE_WEAK_EXTERN_SEARCH_ALIAS
            rows.append(
                NativeSymbolRow(
                    _coff_kind(
                        name,
                        value=value,
                        section=section,
                        external=storage == _IMAGE_SYM_CLASS_EXTERNAL,
                        weak_alias=weak_alias,
                        section_table=section_table,
                        strings=strings,
                    ),
                    name,
                )
            )
        index += 1 + aux
    return tuple(rows)


def _coff_symbol_name(raw: bytes, strings: bytes) -> str:
    if raw[:4] == b"\0\0\0\0":
        offset = struct.unpack_from("<I", raw, 4)[0]
        if offset < 4:
            raise NativeSymbolTableError(
                f"COFF symbol name offset {offset} points into the size field"
            )
        return _c_string(strings, offset, "COFF symbol name")
    try:
        return raw.split(b"\0", 1)[0].decode("utf-8")
    except UnicodeDecodeError as error:
        raise NativeSymbolTableError(
            f"COFF symbol name is not UTF-8: {error}"
        ) from error


def _coff_section_name(raw: bytes, strings: bytes) -> str:
    text = raw.split(b"\0", 1)[0]
    if not text.startswith(b"/"):
        return text.decode("utf-8", "replace")
    digits = text[2:] if text.startswith(b"//") else text[1:]
    try:
        if text.startswith(b"//"):
            if len(digits) > 6:
                raise ValueError("base64 section name offset is too long")
            offset = 0
            for character in digits.decode("ascii"):
                offset = offset * 64 + _BASE64_DIGITS[character]
        else:
            offset = int(digits.decode("ascii"), 10)
    except (KeyError, UnicodeDecodeError, ValueError) as error:
        raise NativeSymbolTableError(
            f"COFF long section name {text!r} is invalid"
        ) from error
    return _c_string(strings, offset, "COFF section name")


def _coff_kind(
    name: str,
    *,
    value: int,
    section: int,
    external: bool,
    weak_alias: bool | None,
    section_table: bytes,
    strings: bytes,
) -> str:
    if weak_alias is not None:
        return "W" if weak_alias else "w"
    if external and section == 0:
        return "C" if value else "U"
    if section == _IMAGE_SYM_ABSOLUTE:
        return "A"
    if name.startswith((".debug", ".sxdata")):
        raise NativeSymbolTableError(f"COFF global symbol {name!r} is a debug symbol")
    if section <= 0:  # IMAGE_SYM_DEBUG and other reserved numbers
        raise NativeSymbolTableError(
            f"COFF global symbol {name!r} has reserved section number {section}"
        )
    if section * 40 > len(section_table):
        raise NativeSymbolTableError(
            f"COFF symbol {name!r} names missing section {section}"
        )
    row = section_table[(section - 1) * 40 : section * 40]
    if _coff_section_name(row[:8], strings).startswith(".idata"):
        return "I"
    characteristics = struct.unpack_from("<I", row, 36)[0]
    if characteristics & _IMAGE_SCN_CNT_CODE:
        return "T"
    if characteristics & _IMAGE_SCN_CNT_INITIALIZED_DATA:
        return "D" if characteristics & _IMAGE_SCN_MEM_WRITE else "R"
    if characteristics & _IMAGE_SCN_CNT_UNINITIALIZED_DATA:
        return "B"
    if characteristics & _IMAGE_SCN_LNK_INFO:
        return "I"
    raise NativeSymbolTableError(
        f"COFF global symbol {name!r} is in a section with no content type"
    )


def _short_import_rows(reader: NativeReader) -> tuple[NativeSymbolRow, ...]:
    from molt.coff_import_library import decode_coff_short_import

    try:
        record = decode_coff_short_import(reader)
    except ValueError as error:
        if isinstance(error, NativeArtifactError):
            raise
        raise NativeSymbolTableError(str(error)) from error
    if record.machine in (0xA641, 0xA64E):
        raise NativeSymbolTableError(
            "ARM64EC and ARM64X short imports are not supported"
        )
    try:
        name = record.symbol_name.decode("utf-8")
    except UnicodeDecodeError as error:
        raise NativeSymbolTableError(
            f"COFF short import symbol is not UTF-8: {error}"
        ) from error
    # llvm-nm types every symbol of one import by the import type.
    kind = {0: "T", 1: "D", 2: "R"}[record.import_type]
    if record.import_type == 1:
        return (NativeSymbolRow(kind, "__imp_" + name),)
    return (NativeSymbolRow(kind, "__imp_" + name), NativeSymbolRow(kind, name))


# WebAssembly --------------------------------------------------------------------


def _wasm_rows(data: bytes) -> tuple[NativeSymbolRow, ...]:
    from molt.wasm_artifact import parse_wasm_import_section, parse_wasm_section_spans
    from molt.wasm_linking_symbols import (
        FLAG_BINDING_LOCAL,
        FLAG_BINDING_WEAK,
        SYMBOL_BINDING_MASK,
        parse_wasm_linking_symbols,
    )

    try:
        spans = parse_wasm_section_spans(data)
        if not any(span.id == 0 and span.custom_name == "linking" for span in spans):
            raise NativeSymbolTableError(
                "WebAssembly module has no linking section; only relocatable "
                "objects carry a symbol table"
            )
        imports = tuple(
            item
            for span in spans
            if span.id == 2
            for item in parse_wasm_import_section(
                data[span.offset : span.offset + span.size]
            )
        )
        table = parse_wasm_linking_symbols(
            data, wasm_imports=imports, section_spans=spans
        )
    except NativeSymbolTableError:
        raise
    except (IndexError, UnicodeDecodeError, ValueError) as error:
        raise NativeSymbolTableError(
            f"WebAssembly object is malformed: {error}"
        ) from error
    rows: list[NativeSymbolRow] = []
    for symbol in table.symbols:
        binding = symbol.flags & SYMBOL_BINDING_MASK
        if binding == FLAG_BINDING_LOCAL:
            continue
        if binding == FLAG_BINDING_WEAK:
            kind = "W" if symbol.is_defined else "w"
        elif not symbol.is_defined:
            kind = "U"
        else:
            kind = "T" if symbol.kind == "function" else "D"
        rows.append(NativeSymbolRow(kind, symbol.name))
    return tuple(rows)

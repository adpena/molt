"""Differential and malformed-input proofs for the in-process symbol reader.

Two independent oracles check ``molt.native_symbol_table``:

* Objects built here byte by byte, with the expected ``llvm-nm`` letters
  written next to each symbol. They cover formats a host cannot produce.
* ``llvm-nm -g --no-llvm-bc`` itself, on those objects and on objects that a
  host clang compiles. A case skips only when its oracle tool is absent.

The reader must never fail with anything but ``NativeArtifactError`` and must
never read past the bytes it was given.
"""

from __future__ import annotations

import functools
import random
import struct
from pathlib import Path

import pytest

from molt.cli.llvm_wasi_tools import _tool_version, llvm_tool_candidates
from molt.native_artifact_header import NativeArtifactError, NativeReader
from molt.native_symbol_table import (
    NativeSymbolRow,
    NativeSymbolTableError,
    SymbolInputFormat,
    read_symbol_rows,
    symbol_input_format,
)
from molt.native_target_shape import NativeObjectFormat, native_artifact_shape
from tests.native_artifact_fixtures import native_relocatable_object
from tools.command_execution import CommandExecutor


_COMMANDS = CommandExecutor.for_file(__file__)


def _rows(data: bytes, *, architecture: str | None = None) -> list[tuple]:
    shape = (
        None
        if architecture is None
        else native_artifact_shape(architecture, object_format=NativeObjectFormat.MACHO)
    )
    reader = NativeReader(len(data), lambda offset, size: data[offset : offset + size])
    return sorted(
        (row.kind, row.name, row.indirect)
        for row in read_symbol_rows(reader, macho_shape=shape)
    )


# Oracle tools ----------------------------------------------------------------


@functools.cache
def _llvm_nm() -> str | None:
    for candidate in llvm_tool_candidates("nm"):
        banner = _tool_version(candidate)
        if banner is not None and banner.strip().startswith("llvm-nm"):
            return str(candidate)
    return None


def _require_llvm_nm() -> str:
    tool = _llvm_nm()
    if tool is None:
        pytest.skip("no llvm-nm on this host; the llvm-nm oracle is absent")
    return tool


def _llvm_nm_rows(path: Path, *extra: str) -> list[tuple]:
    result = _COMMANDS.run(
        [_require_llvm_nm(), "-g", "--no-llvm-bc", *extra, str(path)],
        capture_output=True,
        text=True,
        timeout=60,
        encoding="utf-8",
    )
    assert result.returncode == 0, result.stderr
    rows = []
    for line in result.stdout.splitlines():
        if not line.strip() or line.endswith(":"):
            continue
        indirect = None
        if " (indirect for " in line:
            line, indirect = line.split(" (indirect for ")
            indirect = indirect.removesuffix(")")
        parts = line.split()
        rows.append((parts[-2], parts[-1], indirect))
    return sorted(rows)


def _compile(tmp_path: Path, target: str, source: str, suffix: str, *flags: str):
    """Compile with the first clang that targets ``target``, or skip."""
    source_path = tmp_path / f"input{suffix}"
    source_path.write_text(source, encoding="utf-8")
    output = tmp_path / f"{target}.o"
    errors = []
    for compiler in llvm_tool_candidates("cc"):
        result = _COMMANDS.run(
            [str(compiler), f"--target={target}", *flags, "-c", str(source_path)]
            + ["-o", str(output)],
            capture_output=True,
            text=True,
            timeout=60,
            encoding="utf-8",
        )
        if result.returncode == 0:
            return output
        errors.append(f"{compiler}: {result.stderr.strip()[:200]}")
    pytest.skip(f"no clang on this host targets {target}: {errors or 'none found'}")


# Byte-built objects -------------------------------------------------------------

_STB_LOCAL, _STB_GLOBAL, _STB_WEAK, _STB_GNU_UNIQUE = 0, 1, 2, 10
_STT_NOTYPE, _STT_OBJECT, _STT_FUNC, _STT_SECTION, _STT_FILE = 0, 1, 2, 3, 4
_STT_GNU_IFUNC = 10
_SHN_ABS, _SHN_COMMON, _SHN_XINDEX = 0xFFF1, 0xFFF2, 0xFFFF

# name, binding, type, st_shndx, extended index, expected llvm-nm letter.
_ELF_SYMBOLS = (
    ("file.c", _STB_LOCAL, _STT_FILE, _SHN_ABS, 0, None),
    ("", _STB_LOCAL, _STT_SECTION, 1, 0, None),
    ("local_function", _STB_LOCAL, _STT_FUNC, 1, 0, None),
    ("text_function", _STB_GLOBAL, _STT_FUNC, 1, 0, "T"),
    ("data_object", _STB_GLOBAL, _STT_OBJECT, 2, 0, "D"),
    ("rodata_object", _STB_GLOBAL, _STT_OBJECT, 3, 0, "R"),
    ("bss_object", _STB_GLOBAL, _STT_OBJECT, 4, 0, "B"),
    ("weak_function", _STB_WEAK, _STT_FUNC, 1, 0, "W"),
    ("weak_object", _STB_WEAK, _STT_OBJECT, 2, 0, "V"),
    ("weak_reference", _STB_WEAK, _STT_NOTYPE, 0, 0, "w"),
    ("weak_object_reference", _STB_WEAK, _STT_OBJECT, 0, 0, "v"),
    ("required", _STB_GLOBAL, _STT_NOTYPE, 0, 0, "U"),
    ("common_object", _STB_GLOBAL, _STT_OBJECT, _SHN_COMMON, 0, "C"),
    ("absolute_value", _STB_GLOBAL, _STT_NOTYPE, _SHN_ABS, 0, "A"),
    ("unique_object", _STB_GNU_UNIQUE, _STT_OBJECT, 2, 0, "u"),
    ("resolver", _STB_GLOBAL, _STT_GNU_IFUNC, 1, 0, "i"),
    ("extended_index_function", _STB_GLOBAL, _STT_FUNC, _SHN_XINDEX, 1, "T"),
)
_ELF_MACHINES = {(64, "<"): 62, (64, ">"): 21, (32, "<"): 3, (32, ">"): 20}


def _elf_object(
    bits: int,
    endian: str,
    symbols=_ELF_SYMBOLS,
    *,
    symtab_entry_size: int | None = None,
    symtab_link: int = 7,
    terminate_strings: bool = True,
    with_extended_indexes: bool = True,
) -> bytes:
    """A complete relocatable ELF object with the given global symbols."""
    wide = bits == 64
    header_size = 64 if wide else 52
    symbol_size = 24 if wide else 16
    section_size = 64 if wide else 40
    strings = bytearray(b"\0")
    symbol_rows = bytearray()
    extended = bytearray()
    for name, binding, symbol_type, shndx, index, _letter in (
        ("", _STB_LOCAL, _STT_NOTYPE, 0, 0, None),
        *symbols,
    ):
        name_offset = 0
        if name:
            name_offset = len(strings)
            strings += name.encode("ascii") + b"\0"
        info = binding << 4 | symbol_type
        size = 8 if symbol_type == _STT_OBJECT else 0
        if wide:
            row = struct.pack(endian + "IBBHQQ", name_offset, info, 0, shndx, 0, size)
        else:
            row = struct.pack(endian + "IIIBBH", name_offset, 0, size, info, 0, shndx)
        symbol_rows += row
        extended += struct.pack(endian + "I", index)
    if not terminate_strings:
        strings += b"unterminated"
    names = bytearray(b"\0")
    name_offsets = {}
    for name in (
        ".text",
        ".data",
        ".rodata",
        ".bss",
        ".note.molt",
        ".symtab",
        ".strtab",
        ".symtab_shndx",
        ".shstrtab",
    ):
        name_offsets[name] = len(names)
        names += name.encode("ascii") + b"\0"
    image = bytearray(header_size)
    placed = {}
    for label, payload in (
        (".text", b"\xc3" * 4),
        (".data", bytes(8)),
        (".rodata", bytes(8)),
        (".note.molt", bytes(4)),
        (".symtab", bytes(symbol_rows)),
        (".strtab", bytes(strings)),
        (".symtab_shndx", bytes(extended)),
        (".shstrtab", bytes(names)),
    ):
        image += bytes(-len(image) % 8)
        placed[label] = (len(image), len(payload))
        image += payload
    image += bytes(-len(image) % 8)
    section_offset = len(image)
    local_count = 1 + sum(1 for row in symbols if row[1] == _STB_LOCAL)
    # name, type, flags, addr, offset, size, link, info, align, entsize
    rows = [
        (0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
        (name_offsets[".text"], 1, 0x6, 0, *placed[".text"], 0, 0, 4, 0),
        (name_offsets[".data"], 1, 0x3, 0, *placed[".data"], 0, 0, 8, 0),
        (name_offsets[".rodata"], 1, 0x2, 0, *placed[".rodata"], 0, 0, 8, 0),
        (name_offsets[".bss"], 8, 0x3, 0, placed[".note.molt"][0], 8, 0, 0, 8, 0),
        (name_offsets[".note.molt"], 7, 0, 0, *placed[".note.molt"], 0, 0, 4, 0),
        (
            name_offsets[".symtab"],
            2,
            0,
            0,
            *placed[".symtab"],
            symtab_link,
            local_count,
            8,
            symbol_size if symtab_entry_size is None else symtab_entry_size,
        ),
        (name_offsets[".strtab"], 3, 0, 0, *placed[".strtab"], 0, 0, 1, 0),
        (name_offsets[".symtab_shndx"], 18, 0, 0, *placed[".symtab_shndx"])
        + (6, 0, 4, 4),
        (name_offsets[".shstrtab"], 3, 0, 0, *placed[".shstrtab"], 0, 0, 1, 0),
    ]
    if not with_extended_indexes:
        rows[8] = (
            (name_offsets[".symtab_shndx"], 1, 0, 0)
            + placed[".symtab_shndx"]
            + (0, 0, 4, 0)
        )
    row_format = endian + ("IIQQQQIIQQ" if wide else "IIIIIIIIII")
    for row in rows:
        image += struct.pack(row_format, *row)
    ident = b"\x7fELF" + bytes((2 if wide else 1, 1 if endian == "<" else 2, 1))
    ident = ident.ljust(16, b"\0")
    fixed = struct.pack(
        endian + ("HHIQQQIHHHHHH" if wide else "HHIIIIIHHHHHH"),
        1,
        _ELF_MACHINES[(bits, endian)],
        1,
        0,
        0,
        section_offset,
        0,
        header_size,
        0,
        0,
        section_size,
        len(rows),
        len(rows) - 1,
    )
    image[:header_size] = ident + fixed
    return bytes(image)


def _elf_expected(symbols=_ELF_SYMBOLS) -> list[tuple]:
    return sorted(
        (letter, name, None) for name, *_rest, letter in symbols if letter is not None
    )


_SECTION_CODE = 0x60500020
_SECTION_DATA = 0xC0300040
_SECTION_RODATA = 0x40300040
_SECTION_BSS = 0xC0300080
_CLASS_EXTERNAL, _CLASS_STATIC, _CLASS_FILE, _CLASS_WEAK_EXTERNAL = 2, 3, 103, 105


_BIGOBJ_CLASS_ID = bytes.fromhex("c7a1bad1eebaa94baf20faf66aa4dcb8")


def _coff_object(
    *,
    file_aux_count: int = 1,
    long_name_offset: int | None = None,
    bigobj: bool = False,
) -> tuple[bytes, list]:
    """A complete x86_64 COFF (or bigobj) object and its expected llvm-nm rows."""
    long_section = b".rdata$molt_long_section"
    long_symbol = b"a_symbol_name_longer_than_eight_bytes"
    strings = bytearray(4)
    section_name_offset = len(strings)
    strings += long_section + b"\0"
    symbol_name_offset = len(strings)
    strings += long_symbol + b"\0"
    struct.pack_into("<I", strings, 0, len(strings))
    sections = (
        (b".text", _SECTION_CODE),
        (b".data", _SECTION_DATA),
        (b".rdata", _SECTION_RODATA),
        (b".bss", _SECTION_BSS),
        (b".idata$5", _SECTION_DATA),
        (f"/{section_name_offset}".encode("ascii"), _SECTION_RODATA),
    )
    record_size = 20 if bigobj else 18
    record_format = "<8sIiHBB" if bigobj else "<8sIhHBB"

    def symbol(name, value, section, storage, aux=0, symbol_type=0):
        raw = (
            struct.pack("<II", 0, name)
            if isinstance(name, int)
            else name.ljust(8, b"\0")
        )
        return struct.pack(
            record_format, raw, value, section, symbol_type, storage, aux
        )

    def aux(payload: bytes) -> bytes:
        return payload.ljust(record_size, b"\0")

    records = [
        symbol(b".file", 0, -2, _CLASS_FILE, file_aux_count),
        aux(b"input.c"),
        symbol(b".text", 0, 1, _CLASS_STATIC, 1),
        aux(b""),
        symbol(b"textfn", 0, 1, _CLASS_EXTERNAL, symbol_type=0x20),
        symbol(b"data", 0, 2, _CLASS_EXTERNAL),
        symbol(b"rodata", 0, 3, _CLASS_EXTERNAL),
        symbol(b"bss", 0, 4, _CLASS_EXTERNAL),
        symbol(b"__imp_x", 0, 5, _CLASS_EXTERNAL),
        symbol(
            symbol_name_offset if long_name_offset is None else long_name_offset,
            0,
            6,
            _CLASS_EXTERNAL,
        ),
        symbol(b"required", 0, 0, _CLASS_EXTERNAL),
        symbol(b"common", 8, 0, _CLASS_EXTERNAL),
        symbol(b"absolute", 16, -1, _CLASS_EXTERNAL),
        symbol(b"weakdef", 0, 0, _CLASS_WEAK_EXTERNAL, 1),
        aux(struct.pack("<II", 4, 3)),  # SEARCH_ALIAS to textfn: defined
        symbol(b"weakref", 0, 0, _CLASS_WEAK_EXTERNAL, 1),
        aux(struct.pack("<II", 4, 2)),  # SEARCH_LIBRARY: a weak reference
        symbol(b"static", 0, 1, _CLASS_STATIC),
    ]
    table = b"".join(records)
    count = len(table) // record_size
    header_size = (56 if bigobj else 20) + 40 * len(sections)
    if bigobj:
        image = bytearray(
            struct.pack(
                "<HHHHI16sIIIIIII",
                0,
                0xFFFF,
                2,
                0x8664,
                0,
                _BIGOBJ_CLASS_ID,
                0,
                0,
                0,
                0,
                len(sections),
                header_size,
                count,
            )
        )
    else:
        image = bytearray(
            struct.pack("<HHIIIHH", 0x8664, len(sections), 0, header_size, count, 0, 0)
        )
    for name, characteristics in sections:
        image += struct.pack(
            "<8sIIIIIIHHI", name, 0, 0, 0, 0, 0, 0, 0, 0, characteristics
        )
    image += table + strings
    expected = [
        ("T", "textfn", None),
        ("D", "data", None),
        ("R", "rodata", None),
        ("B", "bss", None),
        ("I", "__imp_x", None),
        ("R", long_symbol.decode("ascii"), None),
        ("U", "required", None),
        ("C", "common", None),
        ("A", "absolute", None),
        ("W", "weakdef", None),
        ("w", "weakref", None),
    ]
    return bytes(image), sorted(expected)


def _pe_image() -> tuple[bytes, list]:
    """A PE32+ executable that keeps a COFF symbol table, as MinGW links do."""
    optional_size = 240
    section_offset = 64 + 24 + optional_size
    text_offset = section_offset + 2 * 40
    data_offset = text_offset + 16
    symbol_offset = data_offset + 16
    strings = struct.pack("<I", 4)

    def symbol(name, section, storage=_CLASS_EXTERNAL):
        return struct.pack("<8sIhHBB", name.ljust(8, b"\0"), 0, section, 0, storage, 0)

    table = symbol(b"main", 1) + symbol(b"state", 2) + symbol(b"local", 1, 3)
    image = bytearray(64)
    image[:2] = b"MZ"
    struct.pack_into("<I", image, 60, 64)
    image += b"PE\0\0" + struct.pack(
        "<HHIIIHH", 0x8664, 2, 0, symbol_offset, 3, optional_size, 0x22
    )
    optional = bytearray(optional_size)
    struct.pack_into("<H", optional, 0, 0x20B)
    struct.pack_into("<Q", optional, 24, 0x140000000)
    struct.pack_into("<I", optional, 108, 16)
    image += optional
    for name, raw_offset, characteristics in (
        (b".text", text_offset, _SECTION_CODE),
        (b".data", data_offset, _SECTION_DATA),
    ):
        image += struct.pack(
            "<8sIIIIIIHHI",
            name,
            16,
            0x1000 + raw_offset,
            16,
            raw_offset,
            0,
            0,
            0,
            0,
            characteristics,
        )
    image += bytes(32) + table + strings
    return bytes(image), [("D", "state", None), ("T", "main", None)]


def _short_import(name: bytes, import_type: int, machine: int = 0x8664) -> bytes:
    payload = name + b"\0" + b"provider.dll\0"
    return (
        struct.pack(
            "<HHHHIIHH",
            0,
            0xFFFF,
            0,
            machine,
            0,
            len(payload),
            0,
            import_type | (1 << 2),
        )
        + payload
    )


def _universal(*slices: tuple[int, int, bytes]) -> bytes:
    alignment = 4
    offset = (8 + 20 * len(slices) + 15) & ~15
    table = bytearray(struct.pack(">II", 0xCAFEBABE, len(slices)))
    body = bytearray()
    for cpu, subtype, data in slices:
        table += struct.pack(">IIIII", cpu, subtype, offset + len(body), len(data), 4)
        body += data + bytes(-len(data) % (1 << alignment))
    return bytes(table.ljust(offset, b"\0") + body)


def _leb(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def _wasm_name(text: str) -> bytes:
    return _leb(len(text)) + text.encode("utf-8")


def _wasm_section(section_id: int, payload: bytes) -> bytes:
    return bytes((section_id,)) + _leb(len(payload)) + payload


def _wasm_object() -> tuple[bytes, list]:
    """A relocatable WebAssembly object with a linking symbol table."""
    weak, local, undefined = 0x1, 0x2, 0x10
    symbols = [
        b"\x00" + _leb(0) + _leb(1) + _wasm_name("defined_function"),
        b"\x00" + _leb(weak) + _leb(2) + _wasm_name("weak_function"),
        b"\x00" + _leb(undefined) + _leb(0),  # named by its import
        b"\x01" + _leb(0) + _wasm_name("data_object") + _leb(0) + _leb(0) + _leb(4),
        b"\x01" + _leb(undefined) + _wasm_name("external_data"),
        b"\x01" + _leb(weak | undefined) + _wasm_name("weak_data_reference"),
        b"\x00" + _leb(local) + _leb(1) + _wasm_name("local_alias"),
    ]
    symbol_table = _leb(len(symbols)) + b"".join(symbols)
    linking = _wasm_name("linking") + _leb(2) + b"\x08"
    linking += _leb(len(symbol_table)) + symbol_table
    data = b"\0asm\x01\0\0\0"
    data += _wasm_section(1, b"\x01\x60\x00\x00")
    imports = b"\x01" + _wasm_name("env") + _wasm_name("imported") + b"\x00\x00"
    data += _wasm_section(2, imports)
    data += _wasm_section(3, b"\x02\x00\x00")
    data += _wasm_section(10, b"\x02\x02\x00\x0b\x02\x00\x0b")
    data += _wasm_section(0, linking)
    expected = [
        ("T", "defined_function", None),
        ("W", "weak_function", None),
        ("U", "imported", None),
        ("D", "data_object", None),
        ("U", "external_data", None),
        ("w", "weak_data_reference", None),
    ]
    return data, sorted(expected)


# Byte-built differential proofs ---------------------------------------------------


@pytest.mark.parametrize("bits,endian", sorted(_ELF_MACHINES))
def test_elf_reader_matches_independent_letters_and_llvm_nm(tmp_path, bits, endian):
    data = _elf_object(bits, endian)
    assert symbol_input_format(data) is SymbolInputFormat.ELF
    assert _rows(data) == _elf_expected()
    if _llvm_nm() is not None:
        path = tmp_path / "input.o"
        path.write_bytes(data)
        assert _llvm_nm_rows(path) == _elf_expected()


@pytest.mark.parametrize("bigobj", [False, True])
def test_coff_reader_matches_independent_letters_and_llvm_nm(tmp_path, bigobj):
    data, expected = _coff_object(bigobj=bigobj)
    assert symbol_input_format(data) is SymbolInputFormat.COFF
    assert _rows(data) == expected
    if _llvm_nm() is not None:
        path = tmp_path / "input.obj"
        path.write_bytes(data)
        assert _llvm_nm_rows(path) == expected


def test_pe_image_reads_its_coff_symbol_table(tmp_path):
    data, expected = _pe_image()
    assert symbol_input_format(data) is SymbolInputFormat.PE
    assert _rows(data) == expected
    if _llvm_nm() is not None:
        path = tmp_path / "image.exe"
        path.write_bytes(data)
        assert _llvm_nm_rows(path) == expected


@pytest.mark.parametrize(
    "import_type,expected",
    [
        (0, [("T", "__imp_provide", None), ("T", "provide", None)]),
        (1, [("D", "__imp_provide", None)]),
        (2, [("R", "__imp_provide", None), ("R", "provide", None)]),
    ],
)
def test_short_import_rows_follow_the_import_type(import_type, expected):
    data = _short_import(b"provide", import_type)
    assert symbol_input_format(data) is SymbolInputFormat.COFF_SHORT_IMPORT
    assert _rows(data) == expected


def test_wasm_reader_matches_independent_letters():
    data, expected = _wasm_object()
    assert _rows(data) == expected


@pytest.mark.parametrize("target", ["x86_64-pc-windows-msvc", "aarch64-apple-darwin"])
def test_fixture_writer_objects_match_independent_letters(tmp_path, target):
    data = native_relocatable_object(
        target_triple=target,
        symbols=("function_symbol",),
        data_symbols=("data_symbol",),
        undefined_symbols=("required_symbol",),
    )
    prefix = "_" if target.endswith("darwin") else ""
    expected = sorted(
        [
            ("T", f"{prefix}function_symbol", None),
            ("D", f"{prefix}data_symbol", None),
            ("U", f"{prefix}required_symbol", None),
        ]
    )
    assert _rows(data) == expected
    if _llvm_nm() is not None:
        path = tmp_path / "input.o"
        path.write_bytes(data)
        assert _llvm_nm_rows(path) == expected


def test_universal_mach_o_reads_exactly_the_target_slice():
    x86 = native_relocatable_object(
        target_triple="x86_64-apple-darwin", symbols=("x86_only",)
    )
    arm = native_relocatable_object(
        target_triple="aarch64-apple-darwin", symbols=("arm_only",)
    )
    data = _universal((0x01000007, 3, x86), (0x0100000C, 0, arm))
    assert symbol_input_format(data) is SymbolInputFormat.MACHO_UNIVERSAL
    assert _rows(data, architecture="aarch64") == [("T", "_arm_only", None)]
    assert _rows(data, architecture="x86_64") == [("T", "_x86_only", None)]
    # arm64e is a distinct slice identity; it never reads the arm64 slice.
    with pytest.raises(NativeSymbolTableError, match="0 slices for arm64e"):
        _rows(data, architecture="arm64e")
    with pytest.raises(NativeSymbolTableError, match="needs a Mach-O target"):
        _rows(data)


# Compiler-built differential proofs ------------------------------------------------

_C_SOURCE = """
extern int external_function(void);
extern int weak_reference(void) __attribute__((weak));
__attribute__((weak)) int weak_definition(void) { return 1; }
int initialized_data = 3;
const int read_only_data = 4;
int zero_data;
static int local_function(void) { return 0; }
int defined_function(void) {
  return external_function() + (weak_reference ? weak_reference() : 0)
      + initialized_data + read_only_data + zero_data + local_function();
}
"""
_C_COMMON = "int common_data __attribute__((common));\n"
_ELF_ASSEMBLY = """
.text
.globl gfun
.type gfun,@function
gfun: ret
.globl ifn
.type ifn,@gnu_indirect_function
ifn: ret
.data
.weak wobj
.type wobj,@object
wobj: .long 1
.globl uniq
.type uniq,@gnu_unique_object
uniq: .long 2
.globl absym
.set absym, 0x1234
.weak wundef
.weak wundefobj
.type wundefobj,@object
.section .tbss,"awT",@nobits
.globl tlsv
.type tlsv,@object
tlsv: .zero 4
.comm cmn,8,8
.text
call needed
call wundef
lea wundefobj(%rip), %rax
"""
_MACHO_ASSEMBLY = """
.text
.globl _f
_f: ret
.weak_definition _wd
.globl _wd
_wd: ret
.weak_reference _wr
.globl _abs
_abs = 0x10
.zerofill __DATA,__bss,_zb,4
.globl _zb
.section __DATA,__const
.globl _cst
_cst: .long 1
.comm _cm,8
.globl _alias
_alias = _target
.text
bl _wr
"""


@pytest.mark.parametrize(
    "target,flags",
    [
        ("x86_64-unknown-linux-gnu", ()),
        ("aarch64-unknown-linux-gnu", ()),
        ("i686-unknown-linux-gnu", ()),
        ("powerpc64-unknown-linux-gnu", ()),
        ("x86_64-pc-windows-msvc", ()),
        ("aarch64-pc-windows-msvc", ()),
        ("i686-pc-windows-msvc", ()),
        ("arm64-apple-darwin", ()),
        ("x86_64-apple-darwin", ()),
        ("arm64-apple-darwin", ("-fembed-bitcode",)),
        ("wasm32-unknown-unknown", ()),
    ],
)
def test_compiled_objects_match_llvm_nm(tmp_path, target, flags):
    _require_llvm_nm()
    source = _C_SOURCE if target.startswith("wasm") else _C_SOURCE + _C_COMMON
    path = _compile(tmp_path, target, source, ".c", *flags)
    assert _rows(path.read_bytes()) == _llvm_nm_rows(path)


@pytest.mark.parametrize(
    "target,source",
    [
        ("x86_64-unknown-linux-gnu", _ELF_ASSEMBLY),
        ("arm64-apple-darwin", _MACHO_ASSEMBLY),
    ],
)
def test_assembled_symbol_kinds_match_llvm_nm(tmp_path, target, source):
    _require_llvm_nm()
    path = _compile(tmp_path, target, source, ".s")
    rows = _rows(path.read_bytes())
    assert rows == _llvm_nm_rows(path)
    # The assembly reaches the rare letters, not only T/D/U.
    kinds = {row[0] for row in rows}
    assert kinds >= (
        {"i", "u", "V", "w", "v", "A", "C"}
        if "linux" in target
        else {"I", "A", "C", "S"}
    )


def _paired_bitcode_tools() -> tuple[Path, Path]:
    """A clang and the llvm-nm beside it: the reader must be as new as clang."""
    for compiler in llvm_tool_candidates("cc"):
        for name in ("llvm-nm", "llvm-nm.exe"):
            reader = compiler.parent / name
            banner = _tool_version(reader) if reader.is_file() else None
            if banner is not None and banner.strip().startswith("llvm-nm"):
                return compiler, reader
    pytest.skip("no clang with a sibling llvm-nm on this host; no bitcode oracle")


def test_bitcode_members_are_read_by_the_admitted_llvm_nm(tmp_path, monkeypatch):
    from molt.cli import native_symbol_inspection
    from molt.cli.native_link_plan import resolve_native_target_spec
    from tests.cli.native_link_test_support import static_archive_bytes

    compiler, reader = _paired_bitcode_tools()
    target = resolve_native_target_spec(None).triple
    source = tmp_path / "input.c"
    source.write_text(_C_SOURCE, encoding="utf-8")
    bitcode = tmp_path / "input.bc.o"
    result = _COMMANDS.run(
        [str(compiler), f"--target={target}", "-flto", "-c", str(source)]
        + ["-o", str(bitcode)],
        capture_output=True,
        text=True,
        timeout=60,
        encoding="utf-8",
    )
    if result.returncode != 0:
        pytest.skip(f"{compiler} cannot emit {target} bitcode: {result.stderr[:200]}")
    assert symbol_input_format(bitcode.read_bytes()) is SymbolInputFormat.LLVM_BITCODE
    native = native_relocatable_object(target_triple=target, symbols=("native_root",))
    archive = tmp_path / "mixed.a"
    archive.write_bytes(
        static_archive_bytes(native) + static_archive_bytes(bitcode.read_bytes())[8:]
    )
    monkeypatch.setattr(
        native_symbol_inspection, "_default_molt_cache", lambda: tmp_path / "cache"
    )
    facts = native_symbol_inspection._native_archive_global_symbol_facts(
        archive, nm_command=(str(reader),), target_triple=target
    )
    assert facts.members is not None
    assert facts.members[0].symbols.defined_functions == {"native_root"}
    module = facts.members[1].symbols
    assert {"defined_function", "weak_definition"} <= module.defined_functions
    assert {"initialized_data", "read_only_data"} <= module.defined
    assert "external_function" in module.undefined
    assert "weak_reference" in module.weak_undefined
    single = native_symbol_inspection._native_object_global_symbol_facts(
        bitcode, nm_command=(str(reader),), target_triple=target
    )
    assert single.defined == module.defined
    assert single.undefined == module.undefined


# Malformed inputs -------------------------------------------------------------------


def _expect_error(data: bytes, match: str, **kwargs) -> None:
    with pytest.raises(NativeArtifactError, match=match):
        _rows(data, **kwargs)


def test_elf_tables_are_validated():
    _expect_error(_elf_object(64, "<", symtab_entry_size=16), "entry size")
    _expect_error(_elf_object(64, "<", symtab_link=1), "non-SHT_STRTAB")
    _expect_error(_elf_object(64, "<", terminate_strings=False), "NUL-terminated")
    _expect_error(
        _elf_object(64, "<", with_extended_indexes=False),
        "SHN_XINDEX without SHT_SYMTAB_SHNDX",
    )
    unsupported = _ELF_SYMBOLS[:3] + (
        ("note_symbol", _STB_GLOBAL, _STT_OBJECT, 5, 0, None),
    )
    _expect_error(_elf_object(64, "<", unsupported), "non-allocated section")
    data = bytearray(_elf_object(32, ">"))
    # Point the symbol table at the ELF header itself.
    section_offset = struct.unpack_from(">I", data, 32)[0]
    struct.pack_into(">I", data, section_offset + 6 * 40 + 16, 0)
    _expect_error(bytes(data), "overlaps")


def _macho_symtab_offset(data: bytes) -> int:
    count = struct.unpack_from("<I", data, 16)[0]
    cursor = 32
    for _ in range(count):
        command, size = struct.unpack_from("<II", data, cursor)
        if command == 0x2:
            return cursor
        cursor += size
    raise AssertionError("fixture has no LC_SYMTAB")


def test_mach_o_tables_are_validated():
    base = native_relocatable_object(
        target_triple="aarch64-apple-darwin", symbols=("function_symbol",)
    )
    symtab = _macho_symtab_offset(base)
    symbol_offset = struct.unpack_from("<I", base, symtab + 8)[0]
    cases = {
        "overlaps": (symtab + 8, 0),
        "overlaps|truncated": (symtab + 12, 0x10000),
        "outside its string table": (symbol_offset, 0x7FFF),
        "invalid size": (symtab + 4, 16),
    }
    for match, (offset, value) in cases.items():
        data = bytearray(base)
        struct.pack_into("<I", data, offset, value)
        _expect_error(bytes(data), match)
    data = bytearray(base)
    data[symbol_offset + 5] = 9  # n_sect beyond the section list
    _expect_error(bytes(data), "missing section 9")


def test_coff_tables_are_validated():
    for bigobj in (False, True):
        data, _expected = _coff_object(file_aux_count=200, bigobj=bigobj)
        _expect_error(data, "auxiliary records run past")
    data, _expected = _coff_object(long_name_offset=2)
    _expect_error(data, "size field")
    data, _expected = _coff_object()
    _expect_error(data[:-6], "truncated")
    damaged = bytearray(data)
    # textfn's section number names a section the object does not have.
    textfn = damaged.index(b"textfn")
    struct.pack_into("<h", damaged, textfn + 12, 40)
    _expect_error(bytes(damaged), "missing section 40")


def test_unsupported_inputs_fail_with_typed_errors():
    _expect_error(b"\x01\x02", "too short")
    _expect_error(b"plain text, not an object", "unrecognized object format")
    _expect_error(b"!<arch>\n", "nested archive")
    _expect_error(b"BC\xc0\xde\x35\x14\x00\x00", "LLVM bitcode")
    _expect_error(_short_import(b"provide", 0, machine=0xA641), "ARM64EC")
    _expect_error(_short_import(b"provide", 3), "type/name encoding")
    module = b"\0asm\x01\0\0\0" + _wasm_section(1, b"\x01\x60\x00\x00")
    _expect_error(module, "no linking section")


def _valid_inputs() -> list[tuple[bytes, str | None]]:
    inputs: list[tuple[bytes, str | None]] = [
        (_elf_object(bits, endian), None) for bits, endian in sorted(_ELF_MACHINES)
    ]
    inputs.append((_coff_object()[0], None))
    inputs.append((_coff_object(bigobj=True)[0], None))
    inputs.append((_short_import(b"provide", 0), None))
    inputs.append((_wasm_object()[0], None))
    inputs.append(
        (
            native_relocatable_object(
                target_triple="aarch64-apple-darwin",
                symbols=("function_symbol",),
                undefined_symbols=("required_symbol",),
            ),
            None,
        )
    )
    inputs.append(
        (
            _universal(
                (
                    0x0100000C,
                    0,
                    native_relocatable_object(
                        target_triple="aarch64-apple-darwin", symbols=("arm",)
                    ),
                ),
            ),
            "aarch64",
        )
    )
    return inputs


@pytest.mark.parametrize("index", range(10))
def test_damaged_inputs_raise_only_typed_artifact_errors(index):
    data, architecture = _valid_inputs()[index]
    _rows(data, architecture=architecture)  # The undamaged input reads.
    generator = random.Random(0x5EED + index)
    reads: list[tuple[int, int]] = []
    for trial in range(400):
        damaged = bytearray(data)
        if trial % 4 == 0:
            damaged = damaged[: generator.randrange(len(damaged))]
        else:
            for _ in range(generator.randint(1, 4)):
                position = generator.randrange(len(damaged))
                damaged[position] = generator.randrange(256)
        damaged_bytes = bytes(damaged)

        def read_at(offset: int, size: int, image=damaged_bytes) -> bytes:
            reads.append((offset, size))
            return image[offset : offset + size]

        shape = (
            None
            if architecture is None
            else native_artifact_shape(
                architecture, object_format=NativeObjectFormat.MACHO
            )
        )
        try:
            rows = read_symbol_rows(
                NativeReader(len(damaged_bytes), read_at), macho_shape=shape
            )
        except NativeArtifactError:
            pass
        else:
            assert all(isinstance(row, NativeSymbolRow) for row in rows)
        assert all(
            offset >= 0 and offset + size <= len(damaged_bytes)
            for offset, size in reads
        )
        reads.clear()


def test_every_valid_input_is_listed_in_the_damage_proof():
    assert len(_valid_inputs()) == 10

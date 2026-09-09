"""Bounded native header decoding shared by compiler output and runtime custody.

This validates fixed headers and declared metadata extents, not instruction
safety, relocation semantics, dynamic loader acceptance or a support matrix.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
import os
from pathlib import Path
import stat
import struct
from typing import BinaryIO, Callable

from molt.native_target_shape import (
    ByteOrder,
    NativeArtifactShape,
    NativeObjectFormat,
    coff_machine_bits,
)


class NativeArtifactError(ValueError):
    """Malformed or mismatched native artifact; never a successful cache miss."""


class NativeFileKind(str, Enum):
    OBJECT = "object"
    EXECUTABLE = "executable"
    SHARED_LIBRARY = "shared-library"
    DYNAMIC_IMAGE = "dynamic-image"  # ELF ET_DYN: shared object or PIE.
    OTHER = "other"


@dataclass(frozen=True, slots=True)
class NativeReader:
    size: int
    read_at: Callable[[int, int], bytes]
    origin: int = 0

    def read(self, offset: int, size: int, label: str) -> bytes:
        self.extent(offset, size, label)
        data = self.read_at(self.origin + offset, size)
        if len(data) != size:
            raise NativeArtifactError(
                f"{label} is truncated at offset {offset}: expected {size} bytes, got {len(data)}"
            )
        return data

    def extent(self, offset: int, size: int, label: str) -> None:
        if offset < 0 or size < 0 or offset > self.size or size > self.size - offset:
            raise NativeArtifactError(
                f"{label} is truncated: offset={offset}, size={size}, image_size={self.size}"
            )

    def slice(self, offset: int, size: int, label: str) -> NativeReader:
        self.extent(offset, size, label)
        return NativeReader(size, self.read_at, self.origin + offset)


@dataclass(frozen=True, slots=True)
class ElfHeader:
    flags: int
    program_offset: int
    program_entry_size: int
    program_count: int
    section_offset: int
    section_entry_size: int
    section_count: int


@dataclass(frozen=True, slots=True)
class PeHeader:
    image_base: int
    directory_offset: int
    directory_count: int
    # virtual_address, virtual_size, raw_offset, raw_size
    sections: tuple[tuple[int, int, int, int], ...]


@dataclass(frozen=True, slots=True)
class CoffHeader:
    bigobj: bool
    section_count: int


@dataclass(frozen=True, slots=True)
class MachOHeader:
    subtype: int
    header_size: int
    command_count: int
    command_bytes: int


@dataclass(frozen=True, slots=True)
class NativeHeader:
    object_format: NativeObjectFormat
    machine: int
    bits: int
    byte_order: ByteOrder
    kind: NativeFileKind
    offset: int
    size: int
    metadata: ElfHeader | PeHeader | CoffHeader | MachOHeader

    @property
    def endian(self) -> str:
        return "<" if self.byte_order == "little" else ">"

    def matches(self, shape: NativeArtifactShape, *, exact_target: bool) -> bool:
        if (
            self.machine not in shape.machines(self.object_format)
            or self.bits != shape.header_bits
            or self.byte_order != shape.byte_order
        ):
            return False
        if isinstance(self.metadata, ElfHeader):
            return self.metadata.flags & shape.elf_flags_mask == shape.elf_flags_value
        if not isinstance(self.metadata, MachOHeader):
            return True
        if not exact_target and shape.macho_runtime_family:
            return (
                True  # Observed loaded-image family, not an ISA/ABI capability claim.
            )
        if shape.macho_subtype is None:
            return False
        if exact_target:
            return self.metadata.subtype == shape.macho_subtype
        # mach/machine.h reserves the high byte for CPU capability bits; family
        # admission compares the base subtype without erasing its low 24 bits.
        return self.metadata.subtype & 0x00FFFFFF == shape.macho_subtype


@dataclass(frozen=True, slots=True)
class NativeArtifact:
    headers: tuple[NativeHeader, ...]
    universal: bool = False

    def validate_format_and_kind(
        self, *, object_format: NativeObjectFormat, kinds: frozenset[NativeFileKind]
    ) -> None:
        for header in self.headers:
            if header.object_format is not object_format or header.kind not in kinds:
                raise NativeArtifactError(
                    f"expected {object_format.value} "
                    f"{'/'.join(sorted(kind.value for kind in kinds))}, "
                    f"got {header.object_format.value}/{header.kind.value}"
                )

    def admit(
        self,
        *,
        object_format: NativeObjectFormat,
        kinds: frozenset[NativeFileKind],
        shape: NativeArtifactShape | None = None,
        exact_target: bool = False,
        loaded_macho_identity: tuple[int, int] | None = None,
    ) -> NativeHeader:
        if loaded_macho_identity is not None and (
            object_format is not NativeObjectFormat.MACHO
            or shape is None
            or exact_target
        ):
            raise NativeArtifactError(
                "loaded Mach-O identity requires runtime-family Mach-O admission"
            )
        if exact_target and self.universal:
            raise NativeArtifactError(
                "exact target admission requires a thin artifact, not a universal container"
            )
        if exact_target and shape is None:
            raise NativeArtifactError(
                "exact target admission requires an explicit shape"
            )
        if self.universal and NativeFileKind.OBJECT in kinds:
            raise NativeArtifactError(
                "a relocatable object cannot be a universal container"
            )
        self.validate_format_and_kind(object_format=object_format, kinds=kinds)
        if shape is None:
            if self.universal:
                raise NativeArtifactError(
                    "Mach-O universal images require an explicit runtime architecture"
                )
            return self.headers[0]
        if loaded_macho_identity is not None:
            loaded_machine, loaded_subtype = loaded_macho_identity
            matches = tuple(
                header
                for header in self.headers
                if isinstance(header.metadata, MachOHeader)
                and header.machine == loaded_machine
                and header.metadata.subtype == loaded_subtype
                and header.matches(shape, exact_target=False)
            )
        else:
            matches = tuple(
                header
                for header in self.headers
                if header.matches(shape, exact_target=exact_target)
            )
        if len(matches) != 1:
            actual = ", ".join(
                f"machine=0x{header.machine:x}/{header.bits}/{header.byte_order}"
                + (
                    f"/subtype=0x{header.metadata.subtype:x}"
                    if isinstance(header.metadata, MachOHeader)
                    else f"/flags=0x{header.metadata.flags:x}"
                    if isinstance(header.metadata, ElfHeader)
                    else ""
                )
                for header in self.headers
            )
            raise NativeArtifactError(
                "native image does not match runtime architecture / requested exact target "
                f"{shape.architecture}/header{shape.header_bits}/pointer{shape.pointer_bits}/"
                f"{shape.byte_order}/subtype={shape.macho_subtype!r}/"
                f"flags={shape.elf_flags_value:#x}&{shape.elf_flags_mask:#x}; "
                f"found [{actual}], matching slices={len(matches)}"
            )
        return matches[0]


LINKED_IMAGE_KINDS = frozenset(
    {NativeFileKind.EXECUTABLE, NativeFileKind.DYNAMIC_IMAGE}
)
LOADED_IMAGE_KINDS = LINKED_IMAGE_KINDS | {NativeFileKind.SHARED_LIBRARY}
OBJECT_KINDS = frozenset({NativeFileKind.OBJECT})

_MACHO_MAGICS = {
    b"\xce\xfa\xed\xfe": ("<", 32),
    b"\xcf\xfa\xed\xfe": ("<", 64),
    b"\xfe\xed\xfa\xce": (">", 32),
    b"\xfe\xed\xfa\xcf": (">", 64),
}
_FAT_MAGICS = {
    b"\xca\xfe\xba\xbe": (">", False),
    b"\xbe\xba\xfe\xca": ("<", False),
    b"\xca\xfe\xba\xbf": (">", True),
    b"\xbf\xba\xfe\xca": ("<", True),
}
_BIGOBJ_CLASS_ID = bytes.fromhex("c7a1bad1eebaa94baf20faf66aa4dcb8")


def _elf(reader: NativeReader) -> NativeHeader:
    ident = reader.read(0, 16, "ELF identification")
    if ident[4] not in (1, 2) or ident[5] not in (1, 2) or ident[6] != 1:
        raise NativeArtifactError("ELF class, byte order, or version is unsupported")
    bits = 64 if ident[4] == 2 else 32
    endian = "<" if ident[5] == 1 else ">"
    fmt = endian + ("HHIQQQIHHHHHH" if bits == 64 else "HHIIIIIHHHHHH")
    fixed_size = 64 if bits == 64 else 52
    fields = struct.unpack(fmt, reader.read(16, fixed_size - 16, "ELF header"))
    (
        kind,
        machine,
        version,
        _entry,
        phoff,
        shoff,
        flags,
        ehsize,
        phsize,
        phnum,
        shsize,
        shnum,
        shstr,
    ) = fields
    if version != 1 or ehsize != fixed_size:
        raise NativeArtifactError(
            "ELF header version or encoded header size is invalid"
        )
    section_min = 64 if bits == 64 else 40
    if phnum == 0xFFFF or (shnum == 0 and shoff) or shstr == 0xFFFF:
        if shoff < fixed_size or shsize < section_min:
            raise NativeArtifactError("ELF extended numbering lacks section zero")
        zero = reader.read(shoff, section_min, "ELF extended-numbering section zero")
        section_fields = struct.unpack(
            endian + ("IIQQQQIIQQ" if bits == 64 else "IIIIIIIIII"), zero
        )
        if section_fields[1] != 0:
            raise NativeArtifactError(
                "ELF extended numbering section zero is not SHT_NULL"
            )
        if shnum == 0:
            shnum = section_fields[5]
        if phnum == 0xFFFF:
            phnum = section_fields[7]
        if shstr == 0xFFFF:
            shstr = section_fields[6]
    for offset, count, entry_size, minimum, label in (
        (phoff, phnum, phsize, 56 if bits == 64 else 32, "ELF program-header extent"),
        (shoff, shnum, shsize, section_min, "ELF section-header extent"),
    ):
        if count:
            if offset < fixed_size or entry_size < minimum:
                raise NativeArtifactError(f"{label} is invalid")
            reader.extent(offset, count * entry_size, label)
        elif offset:
            raise NativeArtifactError(f"{label} has an offset but no entries")
    if shstr and shstr >= shnum:
        raise NativeArtifactError("ELF section-name string table index is invalid")
    return NativeHeader(
        NativeObjectFormat.ELF,
        machine,
        bits,
        "little" if endian == "<" else "big",
        {
            1: NativeFileKind.OBJECT,
            2: NativeFileKind.EXECUTABLE,
            3: NativeFileKind.DYNAMIC_IMAGE,
        }.get(kind, NativeFileKind.OTHER),
        reader.origin,
        reader.size,
        ElfHeader(flags, phoff, phsize, phnum, shoff, shsize, shnum),
    )


def _pe(reader: NativeReader) -> NativeHeader:
    dos = reader.read(0, 64, "DOS/PE header")
    pe_offset = struct.unpack_from("<I", dos, 60)[0]
    if pe_offset < 64:
        raise NativeArtifactError("PE header offset overlaps the DOS header")
    coff = reader.read(pe_offset, 24, "PE header")
    if coff[:4] != b"PE\0\0":
        raise NativeArtifactError("PE signature is invalid")
    machine, count, _stamp, _symbols, _symbol_count, optional_size, flags = (
        struct.unpack_from("<HHIIIHH", coff, 4)
    )
    optional = pe_offset + 24
    if optional_size < 2:
        raise NativeArtifactError("truncated PE header: missing optional image header")
    reader.extent(optional, optional_size, "PE optional header")
    magic = struct.unpack("<H", reader.read(optional, 2, "PE optional magic"))[0]
    if magic not in (0x10B, 0x20B):
        raise NativeArtifactError(f"unsupported PE optional-header magic 0x{magic:x}")
    bits, directory_start = (64, 112) if magic == 0x20B else (32, 96)
    if optional_size < directory_start:
        raise NativeArtifactError("truncated PE header: fixed optional fields")
    fixed = reader.read(optional, directory_start, "PE fixed optional header")
    directories = struct.unpack_from("<I", fixed, directory_start - 4)[0]
    if directories > (optional_size - directory_start) // 8:
        raise NativeArtifactError("PE data directories are truncated")
    section_start = optional + optional_size
    reader.extent(section_start, count * 40, "PE sections")
    sections = []
    for index in range(count):
        section = reader.read(section_start + index * 40, 40, "PE section")
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", section, 8
        )
        reader.extent(raw_offset, raw_size, "PE section raw data")
        sections.append((virtual_address, virtual_size, raw_offset, raw_size))
    if bits != coff_machine_bits(machine):
        raise NativeArtifactError("PE optional bitness disagrees with COFF machine")
    kind = (
        NativeFileKind.SHARED_LIBRARY
        if flags & 0x2000
        else NativeFileKind.EXECUTABLE
        if flags & 2
        else NativeFileKind.OTHER
    )
    if not flags & 2:
        kind = NativeFileKind.OTHER
    image_base = struct.unpack_from(
        "<Q" if bits == 64 else "<I", fixed, 24 if bits == 64 else 28
    )[0]
    return NativeHeader(
        NativeObjectFormat.COFF,
        machine,
        bits,
        "little",
        kind,
        reader.origin,
        reader.size,
        PeHeader(image_base, optional + directory_start, directories, tuple(sections)),
    )


def _coff(reader: NativeReader, magic: bytes) -> NativeHeader:
    bigobj = magic == b"\0\0\xff\xff"
    if bigobj:
        fixed = reader.read(0, 56, "COFF bigobj header")
        version, machine = struct.unpack_from("<HH", fixed, 4)
        if version != 2 or fixed[12:28] != _BIGOBJ_CLASS_ID:
            raise NativeArtifactError("unsupported COFF bigobj/import-object header")
        count, symbol_offset, symbol_count = struct.unpack_from("<III", fixed, 44)
        header_size, symbol_size = 56, 20
    else:
        fixed = reader.read(0, 20, "COFF object header")
        machine, count, _stamp, symbol_offset, symbol_count, optional_size, flags = (
            struct.unpack("<HHIIIHH", fixed)
        )
        if optional_size or flags & 0x2002:
            raise NativeArtifactError(
                "COFF object has image flags or an optional header"
            )
        header_size, symbol_size = 20, 18
    bits = coff_machine_bits(machine)
    if not count:
        raise NativeArtifactError("COFF object has no sections")
    reader.extent(header_size, count * 40, "COFF section table")
    if symbol_count:
        if symbol_offset < header_size + count * 40:
            raise NativeArtifactError("COFF symbol table overlaps headers")
        reader.extent(symbol_offset, symbol_count * symbol_size, "COFF symbol table")
    return NativeHeader(
        NativeObjectFormat.COFF,
        machine,
        bits,
        "little",
        NativeFileKind.OBJECT,
        reader.origin,
        reader.size,
        CoffHeader(bigobj, count),
    )


def _macho(reader: NativeReader, magic: bytes) -> NativeHeader:
    try:
        endian, bits = _MACHO_MAGICS[magic]
    except KeyError as exc:
        raise NativeArtifactError(
            "Mach-O universal slice is not a thin Mach-O image"
        ) from exc
    size = 32 if bits == 64 else 28
    fixed = reader.read(0, size, "Mach-O header")
    machine, subtype, filetype, count, commands = struct.unpack_from(
        endian + "IIIII", fixed, 4
    )
    cpu_abi = machine & 0xFF000000
    if cpu_abi not in (0, 0x01000000, 0x02000000):
        raise NativeArtifactError(f"unsupported Mach-O CPU ABI encoding 0x{cpu_abi:x}")
    expected_header_bits = 64 if cpu_abi else 32
    if bits != expected_header_bits:
        raise NativeArtifactError("Mach-O CPU ABI and fixed-header width disagree")
    if bits == 64 and fixed[28:32] != bytes(4):
        raise NativeArtifactError("Mach-O 64-bit header reserved field is nonzero")
    if commands % (8 if bits == 64 else 4):
        raise NativeArtifactError("Mach-O load-command extent is misaligned")
    reader.extent(size, commands, "Mach-O load commands")
    if bool(count) != bool(commands) or count > commands // 8:
        raise NativeArtifactError("Mach-O load-command count/extent disagree")
    return NativeHeader(
        NativeObjectFormat.MACHO,
        machine,
        bits,
        "little" if endian == "<" else "big",
        {
            1: NativeFileKind.OBJECT,
            2: NativeFileKind.EXECUTABLE,
            6: NativeFileKind.SHARED_LIBRARY,
            7: NativeFileKind.SHARED_LIBRARY,
            8: NativeFileKind.SHARED_LIBRARY,
        }.get(filetype, NativeFileKind.OTHER),
        reader.origin,
        reader.size,
        MachOHeader(subtype, size, count, commands),
    )


def _universal(reader: NativeReader, magic: bytes) -> NativeArtifact:
    endian, fat64 = _FAT_MAGICS[magic]
    count = struct.unpack(endian + "I", reader.read(4, 4, "Mach-O universal count"))[0]
    fmt = endian + ("IIQQII" if fat64 else "IIIII")
    row_size = struct.calcsize(fmt)
    # Resource-admission bound, not an architecture support claim.
    if not 1 <= count <= 4096:
        raise NativeArtifactError(
            f"Mach-O universal slice count {count} exceeds bounded admission (1..4096)"
        )
    reader.extent(8, count * row_size, "Mach-O universal slice table")
    table_end = 8 + count * row_size
    rows = []
    for index in range(count):
        row = struct.unpack(
            fmt,
            reader.read(8 + index * row_size, row_size, "Mach-O universal slice row"),
        )
        cpu, subtype, offset, size, alignment = row[:5]
        if (
            not size
            or offset < table_end
            or alignment > (63 if fat64 else 31)
            or offset % (1 << alignment)
            or (fat64 and row[5] != 0)
        ):
            raise NativeArtifactError(
                "Mach-O universal slice extent/alignment is invalid"
            )
        reader.extent(offset, size, "Mach-O universal slice extent")
        rows.append((cpu, subtype, offset, size))
    ordered = sorted(rows, key=lambda row: row[2])
    if any(a[2] + a[3] > b[2] for a, b in zip(ordered, ordered[1:])):
        raise NativeArtifactError(
            "Mach-O universal slice extent overlaps another slice"
        )
    headers = []
    identities: set[tuple[int, int]] = set()
    for cpu, subtype, offset, size in rows:
        image = reader.slice(offset, size, "Mach-O universal slice extent")
        header = _macho(image, image.read(0, 4, "Mach-O slice magic"))
        assert isinstance(header.metadata, MachOHeader)
        if cpu != header.machine or subtype != header.metadata.subtype:
            raise NativeArtifactError(
                "Mach-O universal table/slice identity disagreement; "
                "image does not match runtime architecture"
            )
        identity = (cpu, subtype)
        if identity in identities:
            raise NativeArtifactError("Mach-O universal slice identity is duplicated")
        identities.add(identity)
        headers.append(header)
    return NativeArtifact(tuple(headers), universal=True)


def decode_native_artifact(reader: NativeReader) -> NativeArtifact:
    magic = reader.read(0, 4, "native artifact magic")
    try:
        if magic in _FAT_MAGICS:
            return _universal(reader, magic)
        if magic in _MACHO_MAGICS:
            header = _macho(reader, magic)
        elif magic == b"\x7fELF":
            header = _elf(reader)
        elif magic[:2] == b"MZ":
            header = _pe(reader)
        elif magic in (b"!<ar", b"!<th"):
            raise NativeArtifactError("an archive is not a native object or image")
        else:
            header = _coff(reader, magic)
    except RuntimeError as exc:
        raise NativeArtifactError(str(exc)) from exc
    return NativeArtifact((header,))


def native_artifact_from_bytes(data: bytes) -> NativeArtifact:
    return decode_native_artifact(
        NativeReader(len(data), lambda offset, size: data[offset : offset + size])
    )


def native_artifact_from_file(stream: BinaryIO) -> NativeArtifact:
    metadata = os.fstat(stream.fileno())
    if not stat.S_ISREG(metadata.st_mode):
        raise NativeArtifactError("native artifact is not a regular file")

    def read_at(offset: int, size: int) -> bytes:
        stream.seek(offset)
        return stream.read(size)

    return decode_native_artifact(NativeReader(metadata.st_size, read_at))


def read_native_artifact(path: Path) -> NativeArtifact:
    with path.open("rb") as stream:
        return native_artifact_from_file(stream)

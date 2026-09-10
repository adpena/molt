"""Complete synthetic native headers for cross-target admission tests.

These encode test inputs independently of production parsing. They are not
executable loader fixtures and make no target execution/support claim.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass


@dataclass(frozen=True)
class NativeSymbolFixture:
    """Independent object-symbol inputs, never inferred from a manifest."""

    functions: tuple[str, ...] = ()
    data: tuple[str, ...] = ()

    @property
    def defined(self) -> tuple[str, ...]:
        return tuple(sorted((*self.functions, *self.data)))


def native_relocatable_object(
    *,
    target_triple: str | None = None,
    symbols: tuple[str, ...] = (),
    data_symbols: tuple[str, ...] = (),
) -> bytes:
    """Real readable object sections; no executable-body or support claim.

    Function and data symbols occupy distinct sections, so callable-admission
    fixtures cannot accidentally encode a function while asserting it is data.
    """
    from molt.cli.native_link_plan import resolve_native_target_spec
    from molt.native_target_shape import NativeObjectFormat, native_artifact_shape

    target = resolve_native_target_spec(target_triple)
    shape = native_artifact_shape(
        target.arch, target_triple=target.triple, object_format=target.object_format
    )
    if (
        shape.header_bits != 64
        or shape.byte_order != "little"
        or shape.architecture not in {"x86_64", "aarch64"}
    ):
        raise ValueError(f"No relocatable symbol fixture layout for {target.triple}")
    all_symbols = (*symbols, *data_symbols)
    if len(set(all_symbols)) != len(all_symbols) or any(
        not symbol or "\0" in symbol for symbol in all_symbols
    ):
        raise ValueError("Fixture symbols must be unique, nonempty, and NUL-free")
    names = tuple(symbol.encode("ascii") for symbol in all_symbols)
    function_count = len(symbols)
    body = bytes(4 * max(1, function_count))
    data = bytes(4 * len(data_symbols))
    has_data = bool(data_symbols)
    if target.object_format is NativeObjectFormat.COFF:
        image = coff_header(machine=shape.coff_machines[0])
        if has_data:
            image.extend(bytes(40))
            struct.pack_into("<H", image, 2, 2)
        text_offset = len(image)
        image.extend(body)
        data_offset = len(image)
        image.extend(data)
        symbol_offset = len(image)
        strings = bytearray(4)
        for index, name in enumerate(names):
            if len(name) <= 8:
                encoded_name = name.ljust(8, b"\0")
            else:
                encoded_name = struct.pack("<II", 0, len(strings))
                strings.extend(name + b"\0")
            function = index < function_count
            value = 4 * (index if function else index - function_count)
            image.extend(
                struct.pack(
                    "<8sIhHBB",
                    encoded_name,
                    value,
                    1 if function else 2,
                    0x20 if function else 0,
                    2,
                    0,
                )
            )
        struct.pack_into("<I", strings, 0, len(strings))
        image.extend(strings)
        struct.pack_into("<II", image, 8, symbol_offset, len(names))
        for index, (name, offset, size, flags) in enumerate(
            [(b".text", text_offset, len(body), 0x60300020)]
            + ([(b".data", data_offset, len(data), 0xC0300040)] if has_data else [])
        ):
            struct.pack_into(
                "<8sIIIIIIHHI",
                image,
                20 + 40 * index,
                name,
                0,
                0,
                size,
                offset,
                0,
                0,
                0,
                0,
                flags,
            )
        return bytes(image)
    if target.object_format is NativeObjectFormat.ELF:
        assert shape.elf_machine is not None
        image = elf_header(machine=shape.elf_machine, kind=1)
        text_offset = len(image)
        image.extend(body)
        data_offset = len(image)
        image.extend(data)
        image.extend(bytes(-len(image) % 8))
        symbol_offset = len(image)
        image.extend(bytes(24))  # STN_UNDEF
        strings = bytearray(b"\0")
        for index, name in enumerate(names):
            function = index < function_count
            value = 4 * (index if function else index - function_count)
            image.extend(
                struct.pack(
                    "<IBBHQQ",
                    len(strings),
                    0x12 if function else 0x11,
                    0,
                    1 if function else 2,
                    value,
                    4,
                )
            )
            strings.extend(name + b"\0")
        symbol_size = len(image) - symbol_offset
        string_offset = len(image)
        image.extend(strings)
        section_names = bytearray(b"\0")
        offsets: dict[str, int] = {}
        for name in (
            ".text",
            *((".data",) if has_data else ()),
            ".symtab",
            ".strtab",
            ".shstrtab",
        ):
            offsets[name] = len(section_names)
            section_names.extend(name.encode("ascii") + b"\0")
        section_name_offset = len(image)
        image.extend(section_names)
        image.extend(bytes(-len(image) % 8))
        section_offset = len(image)
        image.extend(bytes(64))  # SHT_NULL
        string_section = 4 if has_data else 3
        rows = [(offsets[".text"], 1, 6, 0, text_offset, len(body), 0, 0, 4, 0)]
        if has_data:
            rows.append((offsets[".data"], 1, 3, 0, data_offset, len(data), 0, 0, 4, 0))
        rows.extend(
            [
                (
                    offsets[".symtab"],
                    2,
                    0,
                    0,
                    symbol_offset,
                    symbol_size,
                    string_section,
                    1,
                    8,
                    24,
                ),
                (offsets[".strtab"], 3, 0, 0, string_offset, len(strings), 0, 0, 1, 0),
                (
                    offsets[".shstrtab"],
                    3,
                    0,
                    0,
                    section_name_offset,
                    len(section_names),
                    0,
                    0,
                    1,
                    0,
                ),
            ]
        )
        for row in rows:
            image.extend(struct.pack("<IIQQQQIIQQ", *row))
        struct.pack_into("<Q", image, 40, section_offset)
        struct.pack_into("<HHH", image, 58, 64, len(rows) + 1, len(rows))
        return bytes(image)
    assert target.object_format is NativeObjectFormat.MACHO
    assert shape.macho_cpu is not None
    segment_size = 152 + (80 if has_data else 0)
    image = macho_header(
        cpu=shape.macho_cpu,
        subtype=shape.macho_subtype,
        kind=1,
        command_count=2,
        command_bytes=segment_size + 24,
    )
    text_offset = len(image)
    image.extend(body)
    data_offset = len(image)
    image.extend(data)
    image.extend(bytes(-len(image) % 8))
    symbol_offset = len(image)
    strings = bytearray(b"\0")
    for index, name in enumerate(names):
        function = index < function_count
        value = 4 * index if function else len(body) + 4 * (index - function_count)
        image.extend(
            struct.pack("<IBBHQ", len(strings), 0x0F, 1 if function else 2, 0, value)
        )
        strings.extend(b"_" + name + b"\0")
    string_offset = len(image)
    image.extend(strings)
    struct.pack_into(
        "<II16sQQQQiiII",
        image,
        32,
        0x19,
        segment_size,
        b"",
        0,
        len(body) + len(data),
        text_offset,
        len(body) + len(data),
        7,
        7,
        2 if has_data else 1,
        0,
    )
    sections = [(b"__text", b"__TEXT", 0, len(body), text_offset, 0x80000400)]
    if has_data:
        sections.append((b"__data", b"__DATA", len(body), len(data), data_offset, 0))
    for index, (name, segment, address, size, offset, flags) in enumerate(sections):
        struct.pack_into(
            "<16s16sQQIIIIIIII",
            image,
            104 + 80 * index,
            name,
            segment,
            address,
            size,
            offset,
            2,
            0,
            0,
            flags,
            0,
            0,
            0,
        )
    struct.pack_into(
        "<IIIIII",
        image,
        32 + segment_size,
        2,
        24,
        symbol_offset,
        len(names),
        string_offset,
        len(strings),
    )
    return bytes(image)


def elf_header(
    *,
    machine: int = 62,
    bits: int = 64,
    endian: str = "<",
    kind: int = 2,
    image_size: int | None = None,
) -> bytearray:
    size = 64 if bits == 64 else 52
    image = bytearray(image_size or size)
    image[:7] = b"\x7fELF" + bytes(
        (2 if bits == 64 else 1, 1 if endian == "<" else 2, 1)
    )
    struct.pack_into(
        endian + ("HHIQQQIHHHHHH" if bits == 64 else "HHIIIIIHHHHHH"),
        image,
        16,
        kind,
        machine,
        1,
        0,
        0,
        0,
        0,
        size,
        0,
        0,
        0,
        0,
        0,
    )
    return image


def pe_header(
    *,
    machine: int = 0x8664,
    bits: int = 64,
    dll: bool = False,
    image_size: int = 0x600,
) -> bytearray:
    image = bytearray(image_size)
    image[:2] = b"MZ"
    struct.pack_into("<I", image, 0x3C, 0x80)
    image[0x80:0x84] = b"PE\0\0"
    optional_size = 240 if bits == 64 else 224
    struct.pack_into(
        "<HHIIIHH",
        image,
        0x84,
        machine,
        1,
        0,
        0,
        0,
        optional_size,
        2 | (0x2000 if dll else 0),
    )
    struct.pack_into("<H", image, 0x98, 0x20B if bits == 64 else 0x10B)
    struct.pack_into(
        "<Q" if bits == 64 else "<I", image, 0x98 + (24 if bits == 64 else 28), 0x400000
    )
    struct.pack_into("<I", image, 0x98 + (108 if bits == 64 else 92), 16)
    return image


def coff_header(*, machine: int = 0x8664, bigobj: bool = False) -> bytearray:
    image = bytearray(96 if bigobj else 60)
    if bigobj:
        struct.pack_into("<HHHHI", image, 0, 0, 0xFFFF, 2, machine, 0)
        image[12:28] = bytes.fromhex("c7a1bad1eebaa94baf20faf66aa4dcb8")
        struct.pack_into("<I", image, 44, 1)
    else:
        struct.pack_into("<HHIIIHH", image, 0, machine, 1, 0, 0, 0, 0, 0)
    return image


def macho_header(
    *,
    cpu: int = 0x01000007,
    subtype: int | None = None,
    bits: int = 64,
    endian: str = "<",
    kind: int = 2,
    command_count: int = 0,
    command_bytes: int = 0,
) -> bytearray:
    if subtype is None:
        subtype = 3 if cpu in (7, 0x01000007) else 1 if cpu == 0x0200000C else 0
    size = 32 if bits == 64 else 28
    image = bytearray(size + command_bytes)
    struct.pack_into(
        endian + "IIIIIII",
        image,
        0,
        0xFEEDFACF if bits == 64 else 0xFEEDFACE,
        cpu,
        subtype,
        kind,
        command_count,
        command_bytes,
        0,
    )
    return image


def fat_macho(
    slices: tuple[bytes | bytearray, ...],
    *,
    endian: str = ">",
    fat64: bool = False,
) -> bytearray:
    row_format = endian + ("IIQQII" if fat64 else "IIIII")
    row_size = struct.calcsize(row_format)
    cursor = (8 + len(slices) * row_size + 255) & ~255
    image = bytearray(cursor)
    struct.pack_into(
        endian + "II", image, 0, 0xCAFEBABF if fat64 else 0xCAFEBABE, len(slices)
    )
    for index, thin in enumerate(slices):
        thin_endian = (
            "<" if thin[:4] in (b"\xce\xfa\xed\xfe", b"\xcf\xfa\xed\xfe") else ">"
        )
        cpu, subtype = struct.unpack_from(thin_endian + "II", thin, 4)
        struct.pack_into(
            row_format,
            image,
            8 + index * row_size,
            cpu,
            subtype,
            cursor,
            len(thin),
            8,
            *((0,) if fat64 else ()),
        )
        image.extend(thin)
        cursor = (len(image) + 255) & ~255
        image.extend(bytes(cursor - len(image)))
    return image

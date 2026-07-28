from __future__ import annotations

import mmap
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

WasmLinkingSymbolKind = Literal["function", "data"]
_Buffer = bytes | mmap.mmap

_WASM_HEADER = b"\0asm\x01\0\0\0"
_LINKING_SECTION_NAME = "linking"
_LINKING_METADATA_VERSION = 2
_SYMBOL_TABLE_SUBSECTION_ID = 8
_SYMBOL_KIND_FUNCTION = 0
_SYMBOL_KIND_DATA = 1
_INDEXED_SYMBOL_KINDS = frozenset({2, 4, 5})
_SYMBOL_KIND_SECTION = 3
_SYMBOL_BINDING_MASK = 0x3
_SYMBOL_BINDING_GLOBAL = 0
_SYMBOL_BINDING_WEAK = 1
_SYMBOL_BINDING_LOCAL = 2
_SYMBOL_UNDEFINED = 0x10
_SYMBOL_EXPLICIT_NAME = 0x40


@dataclass(frozen=True, slots=True)
class WasmLinkingSymbol:
    name: str
    kind: WasmLinkingSymbolKind
    flags: int
    index: int | None = None
    segment_index: int | None = None
    data_offset: int | None = None
    size: int | None = None

    @property
    def is_defined(self) -> bool:
        return not bool(self.flags & _SYMBOL_UNDEFINED)

    @property
    def is_externally_linkable(self) -> bool:
        binding = self.flags & _SYMBOL_BINDING_MASK
        return self.is_defined and binding in {
            _SYMBOL_BINDING_GLOBAL,
            _SYMBOL_BINDING_WEAK,
        }


@dataclass(frozen=True, slots=True)
class WasmLinkingSymbolTable:
    symbols: tuple[WasmLinkingSymbol, ...]

    @property
    def function_symbols(self) -> tuple[WasmLinkingSymbol, ...]:
        return tuple(symbol for symbol in self.symbols if symbol.kind == "function")

    @property
    def data_symbols(self) -> tuple[WasmLinkingSymbol, ...]:
        return tuple(symbol for symbol in self.symbols if symbol.kind == "data")

    @property
    def defined_functions(self) -> frozenset[str]:
        return frozenset(
            symbol.name
            for symbol in self.symbols
            if symbol.kind == "function"
            and symbol.name
            and symbol.is_externally_linkable
        )

    @property
    def defined_data(self) -> frozenset[str]:
        return frozenset(
            symbol.name
            for symbol in self.symbols
            if symbol.kind == "data" and symbol.name and symbol.is_externally_linkable
        )

    @property
    def undefined_functions(self) -> frozenset[str]:
        return frozenset(
            symbol.name
            for symbol in self.symbols
            if symbol.kind == "function" and symbol.name and not symbol.is_defined
        )

    @property
    def undefined_data(self) -> frozenset[str]:
        return frozenset(
            symbol.name
            for symbol in self.symbols
            if symbol.kind == "data" and symbol.name and not symbol.is_defined
        )

    @property
    def defined_names(self) -> frozenset[str]:
        return frozenset(
            symbol.name
            for symbol in self.symbols
            if symbol.name and symbol.is_externally_linkable
        )

    def defined_names_for_kinds(
        self, expected_symbol_kinds: Mapping[str, str]
    ) -> frozenset[str]:
        expected_by_kind: dict[WasmLinkingSymbolKind, set[str]] = {
            "function": set(),
            "data": set(),
        }
        for name, kind in _validated_expected_symbol_kinds(expected_symbol_kinds):
            expected_by_kind[kind].add(name)
        available: set[str] = set()
        for symbol in self.symbols:
            if (
                symbol.name in expected_by_kind[symbol.kind]
                and symbol.is_externally_linkable
            ):
                available.add(symbol.name)
        return frozenset(available)


def _validated_expected_symbol_kinds(
    expected_symbol_kinds: Mapping[str, str],
) -> Iterator[tuple[str, WasmLinkingSymbolKind]]:
    for name, kind in expected_symbol_kinds.items():
        if kind == "function":
            yield name, "function"
        elif kind == "data":
            yield name, "data"
        else:
            raise ValueError(f"unsupported WebAssembly linking symbol kind {kind!r}")


def _read_varuint(data: _Buffer, offset: int, limit: int) -> tuple[int, int]:
    if offset >= limit:
        raise ValueError("Unexpected EOF while reading wasm varuint")
    byte = data[offset]
    offset += 1
    if byte < 0x80:
        return byte, offset
    result = byte & 0x7F
    shift = 7
    while byte & 0x80:
        if offset >= limit:
            raise ValueError("Unexpected EOF while reading wasm varuint")
        if shift >= 70:
            raise ValueError("wasm varuint exceeds u64")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        shift += 7
    return result, offset


def _read_section_varuint(data: _Buffer, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading wasm varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 63:
            raise ValueError("wasm varuint is too large")


def _skip_varuint(data: _Buffer, offset: int, limit: int) -> int:
    if offset >= limit:
        raise ValueError("Unexpected EOF while reading wasm varuint")
    byte = data[offset]
    offset += 1
    if byte < 0x80:
        return offset
    shift = 7
    while byte & 0x80:
        if offset >= limit:
            raise ValueError("Unexpected EOF while reading wasm varuint")
        if shift >= 70:
            raise ValueError("wasm varuint exceeds u64")
        byte = data[offset]
        offset += 1
        shift += 7
    return offset


def _read_string_bounds(data: _Buffer, offset: int, limit: int) -> tuple[int, int]:
    if offset >= limit:
        raise ValueError("Unexpected EOF while reading wasm varuint")
    length = data[offset]
    offset += 1
    if length >= 0x80:
        length, offset = _read_varuint(data, offset - 1, limit)
    end = offset + length
    if end > limit:
        raise ValueError("Unexpected EOF while reading wasm string")
    return offset, end


def _read_string(data: _Buffer, offset: int, limit: int) -> tuple[str, int]:
    start, end = _read_string_bounds(data, offset, limit)
    return data[start:end].decode("utf-8"), end


def _is_externally_linkable(flags: int) -> bool:
    return not flags & _SYMBOL_UNDEFINED and flags & _SYMBOL_BINDING_MASK in {
        _SYMBOL_BINDING_GLOBAL,
        _SYMBOL_BINDING_WEAK,
    }


def _indexed_symbol(
    data: _Buffer, offset: int, limit: int, flags: int
) -> tuple[int, str, int]:
    index, offset = _read_varuint(data, offset, limit)
    name = ""
    if not flags & _SYMBOL_UNDEFINED or flags & _SYMBOL_EXPLICIT_NAME:
        name, offset = _read_string(data, offset, limit)
    return index, name, offset


def _symbol_table(
    data: _Buffer,
    offset: int,
    limit: int,
    symbols: list[WasmLinkingSymbol],
) -> None:
    count, offset = _read_varuint(data, offset, limit)
    for _ in range(count):
        if offset >= limit:
            raise ValueError("Unexpected EOF while reading linking symbols")
        kind = data[offset]
        offset += 1
        if offset >= limit:
            raise ValueError("Unexpected EOF while reading wasm varuint")
        flags = data[offset]
        offset += 1
        if flags >= 0x80:
            flags, offset = _read_varuint(data, offset - 1, limit)
        if kind == _SYMBOL_KIND_FUNCTION:
            index, name, offset = _indexed_symbol(data, offset, limit, flags)
            symbols.append(WasmLinkingSymbol(name, "function", flags, index=index))
            continue
        if kind == _SYMBOL_KIND_DATA:
            name, offset = _read_string(data, offset, limit)
            if flags & _SYMBOL_UNDEFINED:
                symbols.append(WasmLinkingSymbol(name, "data", flags))
                continue
            segment_index, offset = _read_varuint(data, offset, limit)
            data_offset, offset = _read_varuint(data, offset, limit)
            size, offset = _read_varuint(data, offset, limit)
            symbols.append(
                WasmLinkingSymbol(
                    name,
                    "data",
                    flags,
                    segment_index=segment_index,
                    data_offset=data_offset,
                    size=size,
                )
            )
            continue
        if kind in _INDEXED_SYMBOL_KINDS:
            _, _, offset = _indexed_symbol(data, offset, limit, flags)
            continue
        if kind == _SYMBOL_KIND_SECTION:
            _, offset = _read_varuint(data, offset, limit)
            continue
        raise ValueError(f"unknown WebAssembly linking symbol kind {kind}")
    if offset != limit:
        raise ValueError("trailing bytes in WebAssembly linking symbol table")


def _defined_names_symbol_table(
    data: _Buffer,
    offset: int,
    limit: int,
    expected_by_kind_and_length: dict[
        tuple[WasmLinkingSymbolKind, int], dict[bytes, str]
    ],
    available: set[str],
) -> None:
    count, offset = _read_varuint(data, offset, limit)
    for _ in range(count):
        if offset >= limit:
            raise ValueError("Unexpected EOF while reading linking symbols")
        kind = data[offset]
        offset += 1
        if offset >= limit:
            raise ValueError("Unexpected EOF while reading wasm varuint")
        flags = data[offset]
        offset += 1
        if flags >= 0x80:
            flags, offset = _read_varuint(data, offset - 1, limit)
        if kind == _SYMBOL_KIND_FUNCTION:
            offset = _skip_varuint(data, offset, limit)
            has_name = not flags & _SYMBOL_UNDEFINED or flags & _SYMBOL_EXPLICIT_NAME
            if has_name:
                name_start, name_end = _read_string_bounds(data, offset, limit)
                offset = name_end
            if (
                has_name
                and not flags & _SYMBOL_UNDEFINED
                and (flags & _SYMBOL_BINDING_MASK) < _SYMBOL_BINDING_LOCAL
            ):
                candidates = expected_by_kind_and_length.get(
                    ("function", name_end - name_start)
                )
                if candidates is not None:
                    matched = candidates.get(data[name_start:name_end])
                    if matched is not None:
                        available.add(matched)
            continue
        if kind == _SYMBOL_KIND_DATA:
            name_start, name_end = _read_string_bounds(data, offset, limit)
            offset = name_end
            if flags & _SYMBOL_UNDEFINED:
                continue
            offset = _skip_varuint(data, offset, limit)
            offset = _skip_varuint(data, offset, limit)
            offset = _skip_varuint(data, offset, limit)
            if flags & _SYMBOL_BINDING_MASK < _SYMBOL_BINDING_LOCAL:
                candidates = expected_by_kind_and_length.get(
                    ("data", name_end - name_start)
                )
                if candidates is not None:
                    matched = candidates.get(data[name_start:name_end])
                    if matched is not None:
                        available.add(matched)
            continue
        if kind in _INDEXED_SYMBOL_KINDS:
            offset = _skip_varuint(data, offset, limit)
            if not flags & _SYMBOL_UNDEFINED or flags & _SYMBOL_EXPLICIT_NAME:
                _, offset = _read_string_bounds(data, offset, limit)
            continue
        if kind == _SYMBOL_KIND_SECTION:
            offset = _skip_varuint(data, offset, limit)
            continue
        raise ValueError(f"unknown WebAssembly linking symbol kind {kind}")
    if offset != limit:
        raise ValueError("trailing bytes in WebAssembly linking symbol table")


def _parse_wasm_linking_symbols(
    data: _Buffer,
    expected_by_kind_and_length: dict[
        tuple[WasmLinkingSymbolKind, int], dict[bytes, str]
    ]
    | None = None,
) -> WasmLinkingSymbolTable | frozenset[str]:
    if len(data) < len(_WASM_HEADER) or data[: len(_WASM_HEADER)] != _WASM_HEADER:
        raise ValueError("Invalid wasm binary")
    symbols: list[WasmLinkingSymbol] | None = (
        [] if expected_by_kind_and_length is None else None
    )
    available: set[str] | None = (
        set() if expected_by_kind_and_length is not None else None
    )
    symbol_table_seen = False
    offset = len(_WASM_HEADER)
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_section_varuint(data, offset)
        section_end = offset + section_size
        if section_end > len(data):
            raise ValueError("Invalid wasm section length")
        if section_id != 0:
            offset = section_end
            continue
        section_name, payload_offset = _read_string(data, offset, section_end)
        if section_name != _LINKING_SECTION_NAME:
            offset = section_end
            continue
        version, payload_offset = _read_varuint(data, payload_offset, section_end)
        if version != _LINKING_METADATA_VERSION:
            raise ValueError(
                f"unsupported WebAssembly linking metadata version {version}"
            )
        while payload_offset < section_end:
            subsection_id = data[payload_offset]
            payload_offset += 1
            subsection_size, payload_offset = _read_varuint(
                data, payload_offset, section_end
            )
            subsection_end = payload_offset + subsection_size
            if subsection_end > section_end:
                raise ValueError("Unexpected EOF while reading linking subsection")
            if subsection_id == _SYMBOL_TABLE_SUBSECTION_ID:
                if symbol_table_seen:
                    raise ValueError("duplicate WebAssembly linking symbol table")
                symbol_table_seen = True
                if symbols is not None:
                    _symbol_table(data, payload_offset, subsection_end, symbols)
                else:
                    assert expected_by_kind_and_length is not None
                    assert available is not None
                    _defined_names_symbol_table(
                        data,
                        payload_offset,
                        subsection_end,
                        expected_by_kind_and_length,
                        available,
                    )
            payload_offset = subsection_end
        offset = section_end
    if symbols is not None:
        return WasmLinkingSymbolTable(tuple(symbols))
    assert available is not None
    return frozenset(available)


def parse_wasm_linking_symbols(data: bytes) -> WasmLinkingSymbolTable:
    table = _parse_wasm_linking_symbols(data)
    assert isinstance(table, WasmLinkingSymbolTable)
    return table


def _read_mapped(
    path: Path, *, expected_symbol_kinds: Mapping[str, str] | None
) -> WasmLinkingSymbolTable | frozenset[str]:
    expected_by_kind_and_length: (
        dict[tuple[WasmLinkingSymbolKind, int], dict[bytes, str]] | None
    ) = None
    if expected_symbol_kinds is not None:
        expected_by_kind_and_length = {}
        for name, kind in _validated_expected_symbol_kinds(expected_symbol_kinds):
            encoded = name.encode("utf-8")
            expected_by_kind_and_length.setdefault((kind, len(encoded)), {})[
                encoded
            ] = name
    with path.open("rb") as stream:
        if stream.seek(0, 2) == 0:
            return _parse_wasm_linking_symbols(b"", expected_by_kind_and_length)
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            return _parse_wasm_linking_symbols(data, expected_by_kind_and_length)


def read_wasm_linking_symbols(path: Path) -> WasmLinkingSymbolTable:
    table = _read_mapped(path, expected_symbol_kinds=None)
    assert isinstance(table, WasmLinkingSymbolTable)
    return table


def wasm_linking_defined_names(
    path: Path, expected_symbol_kinds: Mapping[str, str]
) -> frozenset[str]:
    if not expected_symbol_kinds:
        return frozenset()
    names = _read_mapped(path, expected_symbol_kinds=expected_symbol_kinds)
    assert isinstance(names, frozenset)
    return names

#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    pass

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"


from molt import _wasm_abi_generated as _WASM_ABI  # noqa: E402
from molt import _wasm_runtime_exports as _runtime_exports  # noqa: E402
from molt.wasm_artifact import (  # noqa: E402
    WasmImport,
    parse_wasm_import_section,
    parse_wasm_sections,
)

WASM_MAGIC = b"\x00asm"

WASM_VERSION = b"\x01\x00\x00\x00"


WASM_EXTERNAL_NATIVE_LINK_IMPORTS = tuple(_WASM_ABI.WASM_EXTERNAL_NATIVE_LINK_IMPORTS)

WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES = dict(
    _WASM_ABI.WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES
)


def wasm_runtime_import_name(name: str) -> str | None:
    return _WASM_ABI.wasm_runtime_import_name(name)


def wasm_runtime_export_name(name: str) -> str | None:
    return _runtime_exports.wasm_runtime_export_name_for_import(name)


def _physical_essential_export(name: str) -> str:
    for logical in ("molt_main", "molt_host_init", "molt_isolate_import"):
        physical = _WASM_ABI.compiled_execution_symbol(logical)
        if name in (logical, physical):
            return physical
    if name in {"memory", "molt_memory", "molt_table", "__indirect_function_table"}:
        return name
    execution = _WASM_ABI.runtime_execution_abi(name)
    if execution is None:
        raise ValueError(f"unknown essential WASM runtime export {name}")
    return execution.physical_symbol


_ESSENTIAL_EXPORTS = frozenset(
    _physical_essential_export(name) for name in _WASM_ABI.WASM_ESSENTIAL_EXPORTS
)


def _read_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varuint")
        if shift >= 70:  # 10 * 7 = 70 bits, covers u64
            raise ValueError("varuint overflow: more than 10 bytes")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            break
        shift += 7
    return result, offset


def _write_varuint(value: int) -> bytes:
    parts: list[int] = []
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            parts.append(byte | 0x80)
        else:
            parts.append(byte)
            break
    return bytes(parts)


def _read_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_varuint(data, offset)
    end = offset + length
    if end > len(data):
        raise ValueError("Unexpected EOF while reading string")
    return data[offset:end].decode("utf-8"), end


def _write_string(value: str) -> bytes:
    raw = value.encode("utf-8")
    return _write_varuint(len(raw)) + raw


_parse_sections = parse_wasm_sections


def _build_sections(sections: Sequence[tuple[int, bytes | memoryview]]) -> bytes:
    output = bytearray()
    output.extend(WASM_MAGIC)
    output.extend(WASM_VERSION)
    for section_id, payload in sections:
        output.append(section_id)
        output.extend(_write_varuint(len(payload)))
        output.extend(payload)
    return bytes(output)


def _collect_imports(data: bytes) -> list[WasmImport]:
    for section_id, payload in _parse_sections(data):
        if section_id == 2:
            return parse_wasm_import_section(payload)
    return []

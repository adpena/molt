#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import re
import sys
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))
_WASM_ABI_GENERATED = REPO_ROOT / "src/molt/_wasm_abi_generated.py"
_WASM_ABI_SPEC = importlib.util.spec_from_file_location(
    "molt_tools_wasm_abi_generated", _WASM_ABI_GENERATED
)
if _WASM_ABI_SPEC is None or _WASM_ABI_SPEC.loader is None:
    raise RuntimeError(f"cannot load generated WASM ABI data: {_WASM_ABI_GENERATED}")
_WASM_ABI = importlib.util.module_from_spec(_WASM_ABI_SPEC)
_WASM_ABI_SPEC.loader.exec_module(_WASM_ABI)

from molt.wasm_linking_symbols import parse_wasm_linking_symbols  # noqa: E402

WASM_MAGIC = b"\x00asm"

WASM_VERSION = b"\x01\x00\x00\x00"

_STANDARD_SECTION_ORDER = {
    1: 1,
    2: 2,
    3: 3,
    4: 4,
    5: 5,
    13: 6,
    6: 7,
    7: 8,
    8: 9,
    9: 10,
    12: 11,
    10: 12,
    11: 13,
}

SYMTAB_SUBSECTION_ID = 8

SYMBOL_KIND_FUNCTION = 0

FLAG_BINDING_GLOBAL = 0x1

FLAG_UNDEFINED = 0x10

FLAG_EXPORTED = 0x20

FLAG_EXPLICIT_NAME = 0x40

FLAG_NO_STRIP = 0x80

FLAG_TOKEN_BITS = {
    "BINDING_LOCAL": 0x0,
    "BINDING_GLOBAL": FLAG_BINDING_GLOBAL,
    "BINDING_WEAK": 0x2,
    "VISIBILITY_HIDDEN": 0x4,
    "UNDEFINED": FLAG_UNDEFINED,
    "EXPORTED": FLAG_EXPORTED,
    "EXPLICIT_NAME": FLAG_EXPLICIT_NAME,
    "NO_STRIP": FLAG_NO_STRIP,
}

SYMBOL_DUMP_RE = re.compile(
    r'Func\s+\{\s+flags:\s+SymbolFlags\(([^)]*)\),\s+index:\s+(\d+),\s+name:\s+Some\("([^"]+)"\)'
)

CALL_INDIRECT_RE = re.compile(r"molt_call_indirect(\d+)")

CALL_INDIRECT_MANGLED_RE = re.compile(r"molt_call_indirect(\d+)(?=\d{2}h[0-9a-fA-F]+E)")

WASM_CALL_INDIRECT_IMPORTS = tuple(_WASM_ABI.WASM_CALL_INDIRECT_IMPORTS)

WASM_EXTERNAL_NATIVE_LINK_IMPORTS = tuple(_WASM_ABI.WASM_EXTERNAL_NATIVE_LINK_IMPORTS)

WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES = dict(
    _WASM_ABI.WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES
)

@dataclass(frozen=True, slots=True)
class CallableTableLayout:
    fixed_prefix_base: int
    fixed_prefix_len: int
    finalized_app_base: int
    app_entry_count: int

    def validate(self) -> None:
        values = (
            self.fixed_prefix_base,
            self.fixed_prefix_len,
            self.finalized_app_base,
            self.app_entry_count,
        )
        if any(value < 0 or value > 0xFFFF_FFFF for value in values):
            raise ValueError("callable-table layout values must fit u32")
        if self.fixed_prefix_len == 0 and self.fixed_prefix_base != 0:
            raise ValueError("empty callable-table fixed prefix must have base zero")
        fixed_end = self.fixed_prefix_base + self.fixed_prefix_len
        if fixed_end > 0xFFFF_FFFF:
            raise ValueError("callable-table fixed prefix boundary overflows u32")
        if self.fixed_prefix_len and fixed_end > self.finalized_app_base:
            raise ValueError(
                "callable-table fixed runtime prefix overlaps finalized app base"
            )
        app_end = self.finalized_app_base + self.app_entry_count
        if app_end > 0xFFFF_FFFF:
            raise ValueError("callable-table finalized app boundary overflows u32")
def wasm_runtime_import_name(name: str) -> str | None:
    return _WASM_ABI.wasm_runtime_import_name(name)


def wasm_runtime_export_name(name: str) -> str | None:
    return _WASM_ABI.wasm_runtime_export_name(name)


_CALL_INDIRECT_IMPORT_BY_ARITY = {
    int(name.removeprefix("molt_call_indirect")): name
    for name in WASM_CALL_INDIRECT_IMPORTS
}

_CALL_INDIRECT_IMPORT_SET = frozenset(WASM_CALL_INDIRECT_IMPORTS)


def call_indirect_import_name_for_arity(arity_text: str) -> str | None:
    if not arity_text.isdecimal():
        return None
    arity = int(arity_text)
    if str(arity) != arity_text:
        return None
    return _CALL_INDIRECT_IMPORT_BY_ARITY.get(arity)


def is_call_indirect_import_name(name: str) -> bool:
    return name in _CALL_INDIRECT_IMPORT_SET


_OUTPUT_RUNTIME_EXPORT_ALIASES = _WASM_ABI.WASM_OUTPUT_RUNTIME_EXPORT_ALIASES

_OUTPUT_EXPORT_ALIAS_PREFIX = _WASM_ABI.WASM_OUTPUT_EXPORT_ALIAS_PREFIX

_INTERNAL_OUTPUT_EXPORT_PREFIXES = _WASM_ABI.WASM_INTERNAL_OUTPUT_EXPORT_PREFIXES

_EMPTY_FUNC_BODY = bytes([0x00, 0x0B])

_ESSENTIAL_EXPORTS = _WASM_ABI.WASM_ESSENTIAL_EXPORTS

_TRAP_STUB_BODY = bytes([0x00, 0x00, 0x0B])


@dataclass(frozen=True, slots=True)
class WasmModuleFacts:
    imports: tuple[tuple[str, str, int, bytes], ...]
    exports: frozenset[str]
    function_exports: Mapping[str, int]
    export_kinds: Mapping[str, tuple[int, int]]
    custom_names: tuple[str, ...]
    module_imports: Mapping[str, frozenset[str]]
    table_import_mins: Mapping[tuple[str, str], int]
    memory_import_mins: Mapping[tuple[str, str], int]
    element_validation_error: str | None


def _is_wasm_binary(data: bytes) -> bool:
    return len(data) >= 8 and data[:4] == WASM_MAGIC and data[4:8] == WASM_VERSION


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


def _parse_sections(data: bytes) -> list[tuple[int, bytes]]:
    if len(data) < 8 or data[:4] != WASM_MAGIC or data[4:8] != WASM_VERSION:
        raise ValueError("Invalid wasm header")
    offset = 8
    sections: list[tuple[int, bytes]] = []
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        size, offset = _read_varuint(data, offset)
        end = offset + size
        if end > len(data):
            raise ValueError("Unexpected EOF while reading section")
        sections.append((section_id, data[offset:end]))
        offset = end
    return sections


def _build_sections(sections: list[tuple[int, bytes]]) -> bytes:
    output = bytearray()
    output.extend(WASM_MAGIC)
    output.extend(WASM_VERSION)
    for section_id, payload in sections:
        output.append(section_id)
        output.extend(_write_varuint(len(payload)))
        output.extend(payload)
    return bytes(output)


def _flatten_rec_groups(data: bytes) -> bytes | None:
    """Rewrite the type section so a recursive type group (`0x4E`) of plain
    function types is re-emitted as a run of standalone function types.

    wasm-ld 22 / LLD 22 (LLVM 22 toolchain drift) emits the merged type section
    as a single GC-proposal *recursive type group* even when every member is an
    ordinary MVP `func` type with no actual recursion. The rec-group encoding
    (`0x4E`) is only valid under the GC proposal, so a pre-GC parser — molt's own
    wasmtime-based host runner, Cloudflare Workers' V8, and `wasm-opt` without
    `--all-features` — rejects the module with "rec group usage requires `gc`
    proposal to be enabled". Flattening the group back to standalone types is a
    pure *encoding* canonicalization: a singleton-or-flat run of `func` types is
    semantically identical to one rec group of the same types, and because the
    members keep their exact sequential order, every existing type index (in the
    function section, `call_indirect`, etc.) stays valid with no renumbering.

    Returns the rewritten module, or ``None`` when there is no type section or no
    rec group to flatten. Fails closed (raises ``ValueError``) if a rec group
    contains anything other than plain `func` types, since collapsing real
    subtype/recursive structure would change the module's meaning.

    Function parameter/result value types are walked with full awareness of the
    multi-byte typed-reference encodings (`0x64 (ref ht)` / `0x63 (ref null ht)`,
    each followed by a heap-type LEB128) that LLD 22 introduces alongside the rec
    group, so the byte spans are skipped exactly — a single-byte value-type
    assumption would desynchronize the walk.
    """
    REC_GROUP = 0x4E
    FUNC_FORM = 0x60
    REF_FORMS = (0x63, 0x64)  # (ref null ht), (ref ht): prefix + heaptype LEB128

    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    type_section_index = -1
    payload = b""
    for idx, (sid, sec_payload) in enumerate(sections):
        if sid == 1:
            type_section_index = idx
            payload = sec_payload
            break
    if type_section_index < 0:
        return None

    offset = 0
    group_count, offset = _read_varuint(payload, offset)

    def _skip_value_type(buf: bytes, pos: int) -> int:
        # Numtypes / vectype / abstract heap-type reftypes are a single byte;
        # the concrete-reference forms (0x63/0x64) carry a trailing heap-type
        # LEB128 whose byte-length is sign-agnostic, so an unsigned-LEB skip
        # advances past it correctly.
        form = buf[pos]
        pos += 1
        if form in REF_FORMS:
            _heap_type, pos = _read_varuint(buf, pos)
        return pos

    def _read_func_type(buf: bytes, pos: int) -> tuple[bytes, int]:
        # buf[pos] is the 0x60 func form byte. Returns (encoded_func, new_pos).
        start = pos
        if buf[pos] != FUNC_FORM:
            raise ValueError(
                f"type section: expected func form 0x60, found {hex(buf[pos])}"
            )
        pos += 1
        param_count, pos = _read_varuint(buf, pos)
        for _ in range(param_count):
            pos = _skip_value_type(buf, pos)
        result_count, pos = _read_varuint(buf, pos)
        for _ in range(result_count):
            pos = _skip_value_type(buf, pos)
        return buf[start:pos], pos

    flat_types: list[bytes] = []
    saw_rec_group = False
    for _ in range(group_count):
        form = payload[offset]
        if form == REC_GROUP:
            saw_rec_group = True
            offset += 1
            member_count, offset = _read_varuint(payload, offset)
            for _member in range(member_count):
                if payload[offset] != FUNC_FORM:
                    raise ValueError(
                        "rec group flatten: group member is not a plain func "
                        f"type (form {hex(payload[offset])}); cannot flatten a "
                        "real recursive/subtype group without changing semantics"
                    )
                encoded, offset = _read_func_type(payload, offset)
                flat_types.append(encoded)
        elif form == FUNC_FORM:
            encoded, offset = _read_func_type(payload, offset)
            flat_types.append(encoded)
        else:
            raise ValueError(
                "rec group flatten: unsupported type form "
                f"{hex(form)} in type section; expected func (0x60) or rec "
                "group (0x4E) of func types"
            )

    if not saw_rec_group:
        return None
    if offset != len(payload):
        raise ValueError(
            "rec group flatten: trailing bytes after type section "
            f"({offset} != {len(payload)})"
        )

    new_payload = bytearray()
    new_payload.extend(_write_varuint(len(flat_types)))
    for encoded in flat_types:
        new_payload.extend(encoded)

    new_sections = list(sections)
    new_sections[type_section_index] = (1, bytes(new_payload))
    return _build_sections(new_sections)


def _parse_custom_section(payload: bytes) -> tuple[str, bytes]:
    name_len, offset = _read_varuint(payload, 0)
    end = offset + name_len
    if end > len(payload):
        raise ValueError("Unexpected EOF while reading custom section name")
    name = payload[offset:end].decode("utf-8")
    return name, payload[end:]


def _build_custom_section(name: str, payload: bytes) -> bytes:
    return _write_string(name) + payload


def _parse_linking_payload(payload: bytes) -> tuple[int, list[tuple[int, bytes]]]:
    version, offset = _read_varuint(payload, 0)
    subsections: list[tuple[int, bytes]] = []
    while offset < len(payload):
        sub_id = payload[offset]
        offset += 1
        sub_size, offset = _read_varuint(payload, offset)
        end = offset + sub_size
        if end > len(payload):
            raise ValueError("Unexpected EOF while reading linking subsection")
        subsections.append((sub_id, payload[offset:end]))
        offset = end
    return version, subsections


def _build_linking_payload(version: int, subsections: list[tuple[int, bytes]]) -> bytes:
    output = bytearray()
    output.extend(_write_varuint(version))
    for sub_id, payload in subsections:
        output.append(sub_id)
        output.extend(_write_varuint(len(payload)))
        output.extend(payload)
    return bytes(output)


def _parse_symbol_flags(flags_text: str) -> int:
    flags_text = flags_text.strip()
    if not flags_text or flags_text == "0x0":
        return 0
    flags = 0
    for token in (part.strip() for part in flags_text.split("|")):
        if not token:
            continue
        bit = FLAG_TOKEN_BITS.get(token)
        if bit is None:
            print(f"Unknown symbol flag token: {token}", file=sys.stderr)
            continue
        flags |= bit
    return flags


def _parse_indexed_symbol(
    payload: bytes, offset: int, flags: int
) -> tuple[int, str, int]:
    index, offset = _read_varuint(payload, offset)
    name = ""
    if (flags & FLAG_UNDEFINED) == 0 or (flags & FLAG_EXPLICIT_NAME):
        name, offset = _read_string(payload, offset)
    return index, name, offset


def _encode_function_symbol_entry(*, flags: int, index: int, name: str) -> bytes:
    entry = bytearray()
    entry.append(SYMBOL_KIND_FUNCTION)
    entry.extend(_write_varuint(flags))
    entry.extend(_write_varuint(index))
    if (flags & FLAG_UNDEFINED) == 0 or (flags & FLAG_EXPLICIT_NAME):
        entry.extend(_write_string(name))
    return bytes(entry)


def _require_encodable_function_symbol(
    *,
    name: str,
    index: int,
    flags: int,
    func_import_count: int,
    total_func_count: int,
) -> None:
    """Fail closed on function symbols the object format cannot express.

    LLVM's WASM object reader rejects a symbol table where a function
    symbol's defined/undefined flag disagrees with its index range
    (``invalid function symbol index``): a symbol without
    ``WASM_SYM_UNDEFINED`` must reference a defined function
    (``index >= func_import_count``) and an undefined symbol must
    reference a function import. Catch that here, at the stage that
    writes the symbol, instead of at wasm-ld.
    """
    if index >= total_func_count:
        raise ValueError(
            f"function symbol {name!r} references function index {index} "
            f"outside the module function index space "
            f"(total functions: {total_func_count})"
        )
    defined = (flags & FLAG_UNDEFINED) == 0
    if defined and index < func_import_count:
        raise ValueError(
            f"function symbol {name!r} is flagged defined but references "
            f"function import index {index} (function imports: "
            f"{func_import_count}); a defined symbol cannot alias an "
            "imported function"
        )
    if not defined and index >= func_import_count:
        raise ValueError(
            f"function symbol {name!r} is flagged undefined but references "
            f"defined function index {index} (function imports: "
            f"{func_import_count})"
        )


def _append_linking_function_symbols(
    data: bytes, entries: list[tuple[str, int, int]]
) -> bytes | None:
    if not entries:
        return None
    existing_names = {
        symbol.name
        for symbol in parse_wasm_linking_symbols(data).function_symbols
        if symbol.name
    }
    sections = _parse_sections(data)
    func_import_count = _count_func_imports(sections)
    total_func_count = _get_total_func_count(data)
    pending = []
    for name, index, flags in entries:
        if name in existing_names:
            continue
        _require_encodable_function_symbol(
            name=name,
            index=index,
            flags=flags,
            func_import_count=func_import_count,
            total_func_count=total_func_count,
        )
        pending.append(
            _encode_function_symbol_entry(flags=flags, index=index, name=name)
        )
    if not pending:
        return None

    sections = _parse_sections(data)
    new_sections: list[tuple[int, bytes]] = []
    modified = False
    linking_found = False
    for section_id, payload in sections:
        if section_id != 0:
            new_sections.append((section_id, payload))
            continue
        name, custom_payload = _parse_custom_section(payload)
        if name != "linking":
            new_sections.append((section_id, payload))
            continue
        linking_found = True
        version, subsections = _parse_linking_payload(custom_payload)
        new_subsections: list[tuple[int, bytes]] = []
        symtab_found = False
        for sub_id, sub_payload in subsections:
            if sub_id != SYMTAB_SUBSECTION_ID:
                new_subsections.append((sub_id, sub_payload))
                continue
            symtab_found = True
            count, offset = _read_varuint(sub_payload, 0)
            updated_payload = bytearray()
            updated_payload.extend(_write_varuint(count + len(pending)))
            updated_payload.extend(sub_payload[offset:])
            for entry in pending:
                updated_payload.extend(entry)
            new_subsections.append((sub_id, bytes(updated_payload)))
            modified = True
        if not symtab_found:
            payload_bytes = bytearray()
            payload_bytes.extend(_write_varuint(len(pending)))
            for entry in pending:
                payload_bytes.extend(entry)
            new_subsections.append((SYMTAB_SUBSECTION_ID, bytes(payload_bytes)))
            modified = True
        new_sections.append(
            (
                section_id,
                _build_custom_section(
                    name, _build_linking_payload(version, new_subsections)
                ),
            )
        )
    if not linking_found:
        payload_bytes = bytearray()
        payload_bytes.extend(_write_varuint(len(pending)))
        for entry in pending:
            payload_bytes.extend(entry)
        new_sections = list(sections)
        new_sections.append(
            (
                0,
                _build_custom_section(
                    "linking",
                    _build_linking_payload(
                        2, [(SYMTAB_SUBSECTION_ID, bytes(payload_bytes))]
                    ),
                ),
            )
        )
        modified = True
    if not modified:
        return None
    return _build_sections(new_sections)


def _collect_func_names(data: bytes) -> dict[int, str]:
    names: dict[int, str] = {}
    for section_id, payload in _parse_sections(data):
        if section_id != 0:
            continue
        name, custom_payload = _parse_custom_section(payload)
        if name != "name":
            continue
        offset = 0
        while offset < len(custom_payload):
            sub_id = custom_payload[offset]
            offset += 1
            sub_size, offset = _read_varuint(custom_payload, offset)
            sub_end = offset + sub_size
            if sub_end > len(custom_payload):
                break
            if sub_id == 1:
                sub_offset = offset
                try:
                    count, sub_offset = _read_varuint(custom_payload, sub_offset)
                except ValueError:
                    # Ignore malformed function-name payloads and continue
                    # scanning other subsections.
                    offset = sub_end
                    continue
                for _ in range(count):
                    if sub_offset >= sub_end:
                        break
                    try:
                        func_idx, sub_offset = _read_varuint(custom_payload, sub_offset)
                        name_len, name_start = _read_varuint(custom_payload, sub_offset)
                    except ValueError:
                        break
                    if name_start > sub_end:
                        break
                    name_end = name_start + name_len
                    if name_end > sub_end:
                        break
                    name_bytes = custom_payload[name_start:name_end]
                    sub_offset = name_end
                    try:
                        func_name = name_bytes.decode("utf-8")
                    except UnicodeDecodeError:
                        # Linked artifacts can contain malformed UTF-8 function
                        # names in the optional name section; skip those entries.
                        continue
                    names[func_idx] = func_name
            offset = sub_end
        break
    return names


def _parse_export_payload(
    payload: bytes,
) -> tuple[set[str], dict[str, int], dict[str, tuple[int, int]]]:
    exports: set[str] = set()
    function_exports: dict[str, int] = {}
    export_kinds: dict[str, tuple[int, int]] = {}
    offset = 0
    count, offset = _read_varuint(payload, offset)
    for _ in range(count):
        name, offset = _read_string(payload, offset)
        if offset >= len(payload):
            raise ValueError("Unexpected EOF while reading export kind")
        kind = payload[offset]
        offset += 1
        index, offset = _read_varuint(payload, offset)
        exports.add(name)
        export_kinds[name] = (kind, index)
        if kind == 0:
            function_exports[name] = index
    return exports, function_exports, export_kinds


def _collect_function_exports(data: bytes) -> dict[str, int]:
    for section_id, payload in _parse_sections(data):
        if section_id == 7:
            _, exports, _ = _parse_export_payload(payload)
            return exports
    return {}


def _read_varsint(data: bytes, offset: int) -> tuple[int, int]:
    """Read a signed LEB128 integer."""
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varsint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if byte & 0x80 == 0:
            if byte & 0x40:
                result -= 1 << shift
            break
    return result, offset


def _read_init_expr_refs(data: bytes, offset: int) -> tuple[int, tuple[int, ...]]:
    ref_funcs: list[int] = []
    while offset < len(data):
        opcode = data[offset]
        offset += 1
        if opcode == 0x0B:
            return offset, tuple(ref_funcs)
        if opcode == 0x41 or opcode == 0x42:
            _, offset = _read_varuint(data, offset)
            continue
        if opcode == 0x43 or opcode == 0x44:
            offset += 4 if opcode == 0x43 else 8
            continue
        if opcode == 0x23:  # global.get
            _, offset = _read_varuint(data, offset)
            continue
        if opcode == 0xD0:  # ref.null
            if offset >= len(data):
                raise ValueError("Unexpected EOF while reading ref.null")
            offset += 1
            continue
        if opcode == 0xD2:  # ref.func
            func_idx, offset = _read_varuint(data, offset)
            ref_funcs.append(func_idx)
            continue
        raise ValueError(f"Unsupported init expr opcode 0x{opcode:02x}")
    raise ValueError("Unexpected EOF while reading init expr")


def _skip_init_expr(data: bytes, offset: int) -> int:
    offset, _ = _read_init_expr_refs(data, offset)
    return offset


def _parse_element_payload(payload: bytes) -> tuple[set[int], str | None]:
    declared: set[int] = set()
    validation_error: str | None = None

    def remember_error(message: str) -> None:
        nonlocal validation_error
        if validation_error is None:
            validation_error = message

    offset = 0
    count, offset = _read_varuint(payload, offset)
    for _ in range(count):
        flags, offset = _read_varuint(payload, offset)
        if flags in (0x02, 0x06):
            table_index, offset = _read_varuint(payload, offset)
            if table_index != 0:
                remember_error(f"element segment targets table {table_index}")
            offset = _skip_init_expr(payload, offset)
        elif flags in (0x00, 0x04):
            offset = _skip_init_expr(payload, offset)
        elif flags in (0x01, 0x03, 0x05, 0x07):
            pass
        else:
            return declared, f"unsupported element segment flags 0x{flags:x}"

        if flags in (0x00, 0x01, 0x02, 0x03):
            if offset >= len(payload):
                remember_error("unexpected EOF reading elemkind")
                return declared, validation_error
            if flags in (0x01, 0x02, 0x03) and payload[offset] == 0x00:
                offset += 1
            elem_count, offset = _read_varuint(payload, offset)
            for _ in range(elem_count):
                func_idx, offset = _read_varuint(payload, offset)
                declared.add(func_idx)
            continue

        if offset >= len(payload):
            remember_error("unexpected EOF reading elemtype")
            return declared, validation_error
        offset += 1
        expr_count, offset = _read_varuint(payload, offset)
        for _ in range(expr_count):
            offset, refs = _read_init_expr_refs(payload, offset)
            declared.update(refs)

    return declared, validation_error


def _count_func_imports(sections: list[tuple[int, bytes]]) -> int:
    """Return the number of function imports in the import section."""
    for sid, payload in sections:
        if sid == 2:
            _, func_imports, _, _, _ = _parse_import_payload(payload)
            return func_imports
    return 0


def _get_total_func_count(data: bytes) -> int:
    """Return the total number of functions (imports + defined) in the module."""
    sections = _parse_sections(data)
    import_count = _count_func_imports(sections)
    defined_count = 0
    for sid, payload in sections:
        if sid == 3:  # function section
            offset = 0
            defined_count, _ = _read_varuint(payload, offset)
            break
    return import_count + defined_count


def _parse_limits(data: bytes, offset: int) -> int:
    flags, offset = _read_varuint(data, offset)
    _, offset = _read_varuint(data, offset)
    if flags & 0x01:
        _, offset = _read_varuint(data, offset)
    return offset


def _read_limits(data: bytes, offset: int) -> tuple[int, int, int | None, int]:
    flags, offset = _read_varuint(data, offset)
    minimum, offset = _read_varuint(data, offset)
    maximum = None
    if flags & 0x01:
        maximum, offset = _read_varuint(data, offset)
    return flags, minimum, maximum, offset


def _write_limits(flags: int, minimum: int, maximum: int | None) -> bytes:
    output = bytearray()
    output.extend(_write_varuint(flags))
    output.extend(_write_varuint(minimum))
    if flags & 0x01:
        if maximum is None:
            maximum = minimum
        output.extend(_write_varuint(maximum))
    return bytes(output)


def _parse_import_desc(data: bytes, offset: int, kind: int) -> int:
    if kind == 0:
        _, offset = _read_varuint(data, offset)
        return offset
    if kind == 1:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading table import")
        offset += 1
        return _parse_limits(data, offset)
    if kind == 2:
        return _parse_limits(data, offset)
    if kind == 3:
        if offset + 2 > len(data):
            raise ValueError("Unexpected EOF while reading global import")
        return offset + 2
    if kind == 4:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading tag import")
        offset += 1
        _, offset = _read_varuint(data, offset)
        return offset
    raise ValueError(f"Unknown import kind: {kind}")


def _import_limits_min(kind: int, desc: bytes) -> int | None:
    if not desc:
        return None
    if kind == 1:
        _, minimum, _, _ = _read_limits(desc, 1)
        return minimum
    if kind == 2:
        _, minimum, _, _ = _read_limits(desc, 0)
        return minimum
    return None


def _parse_import_payload(
    payload: bytes,
) -> tuple[
    list[tuple[str, str, int, bytes]],
    int,
    dict[str, set[str]],
    dict[tuple[str, str], int],
    dict[tuple[str, str], int],
]:
    imports: list[tuple[str, str, int, bytes]] = []
    module_imports: dict[str, set[str]] = {}
    table_import_mins: dict[tuple[str, str], int] = {}
    memory_import_mins: dict[tuple[str, str], int] = {}
    func_imports = 0
    offset = 0
    count, offset = _read_varuint(payload, offset)
    for _ in range(count):
        module, offset = _read_string(payload, offset)
        name, offset = _read_string(payload, offset)
        if offset >= len(payload):
            raise ValueError("Unexpected EOF while reading import kind")
        kind = payload[offset]
        offset += 1
        desc_start = offset
        offset = _parse_import_desc(payload, offset, kind)
        desc = payload[desc_start:offset]
        imports.append((module, name, kind, desc))
        module_imports.setdefault(module, set()).add(name)
        if kind == 0:
            func_imports += 1
        elif kind == 1:
            minimum = _import_limits_min(kind, desc)
            if minimum is not None:
                table_import_mins[(module, name)] = minimum
        elif kind == 2:
            minimum = _import_limits_min(kind, desc)
            if minimum is not None:
                memory_import_mins[(module, name)] = minimum
    return (
        imports,
        func_imports,
        module_imports,
        table_import_mins,
        memory_import_mins,
    )


def _collect_exports(data: bytes) -> set[str]:
    for section_id, payload in _parse_sections(data):
        if section_id == 7:
            exports, _, _ = _parse_export_payload(payload)
            return exports
    return set()


def _collect_imports(data: bytes) -> list[tuple[str, str, int, bytes]]:
    for section_id, payload in _parse_sections(data):
        if section_id == 2:
            imports, _, _, _, _ = _parse_import_payload(payload)
            return imports
    return []


def _has_table(data: bytes) -> bool:
    for module, name, kind, _ in _collect_imports(data):
        if kind == 1 and name == "__indirect_function_table":
            return True
    for section_id, _ in _parse_sections(data):
        if section_id == 4:
            return True
    return False


def _validate_linked_table_import_contract(
    imports: list[tuple[str, str, int, bytes]],
) -> tuple[bool, str | None]:
    table_imports = [
        (module, name, desc) for module, name, kind, desc in imports if kind == 1
    ]
    if not table_imports:
        return True, None
    if len(table_imports) > 1:
        table_names = ", ".join(
            f"{module}::{name}" for module, name, _ in table_imports
        )
        return (
            False,
            "Linked wasm imports multiple tables "
            f"({table_names}); only env::__indirect_function_table is supported.",
        )
    module, name, desc = table_imports[0]
    if module != "env" or name != "__indirect_function_table":
        return (
            False,
            "Linked wasm imports unsupported table "
            f"{module}::{name}; expected env::__indirect_function_table.",
        )
    if not desc:
        return False, "Linked wasm table import is missing its limits descriptor."
    return True, None


def _ensure_table_export(data: bytes, export_name: str = "molt_table") -> bytes | None:
    if not _has_table(data):
        return None
    sections = _parse_sections(data)
    new_sections: list[tuple[int, bytes]] = []
    modified = False
    saw_export = False
    for section_id, payload in sections:
        if section_id != 7:
            new_sections.append((section_id, payload))
            continue
        saw_export = True
        offset = 0
        count, offset = _read_varuint(payload, offset)
        entries_offset = offset
        has_table_export = False
        while offset < len(payload):
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                break
            kind = payload[offset]
            offset += 1
            _, offset = _read_varuint(payload, offset)
            if kind == 1 and name in (export_name, "__indirect_function_table"):
                has_table_export = True
                break
        if has_table_export:
            new_sections.append((section_id, payload))
            continue
        entry = _write_string(export_name) + bytes([1]) + _write_varuint(0)
        new_payload = _write_varuint(count + 1) + payload[entries_offset:] + entry
        new_sections.append((section_id, new_payload))
        modified = True
    if not saw_export:
        entry = _write_string(export_name) + bytes([1]) + _write_varuint(0)
        export_payload = _write_varuint(1) + entry
        inserted = False
        for idx, (section_id, payload) in enumerate(new_sections):
            if section_id > 7:
                new_sections.insert(idx, (7, export_payload))
                inserted = True
                break
        if not inserted:
            new_sections.append((7, export_payload))
        modified = True
    if not modified:
        return None
    return _build_sections(new_sections)


def _find_func_import_index(
    data: bytes, module_name: str, import_name: str
) -> int | None:
    func_index = 0
    for module, name, kind, _desc in _collect_imports(data):
        if kind != 0:
            continue
        if module == module_name and name == import_name:
            return func_index
        func_index += 1
    return None


def _collect_custom_names(data: bytes) -> list[str]:
    names: list[str] = []
    for section_id, payload in _parse_sections(data):
        if section_id != 0:
            continue
        try:
            name, _ = _parse_custom_section(payload)
        except ValueError:
            continue
        names.append(name)
    return names


def _validate_elements(data: bytes) -> tuple[bool, str | None]:
    for section_id, payload in _parse_sections(data):
        if section_id == 9:
            _, error = _parse_element_payload(payload)
            return error is None, error
    return True, None


def _collect_module_imports(wasm_data: bytes, module_name: str) -> set[str]:
    """Parse a WASM module and return the set of import names from *module_name*.

    For example, if the app module imports ``(import "molt_runtime" "print_obj" ...)``,
    calling ``_collect_module_imports(app_data, "molt_runtime")`` returns ``{"print_obj"}``.
    """
    for section_id, payload in _parse_sections(wasm_data):
        if section_id == 2:
            _, _, module_imports, _, _ = _parse_import_payload(payload)
            return set(module_imports.get(module_name, ()))
    return set()


def parse_wasm_module_facts(data: bytes) -> WasmModuleFacts:
    sections = _parse_sections(data)
    imports: list[tuple[str, str, int, bytes]] = []
    exports: set[str] = set()
    function_exports: dict[str, int] = {}
    export_kinds: dict[str, tuple[int, int]] = {}
    custom_names: list[str] = []
    module_imports: dict[str, set[str]] = {}
    table_import_mins: dict[tuple[str, str], int] = {}
    memory_import_mins: dict[tuple[str, str], int] = {}
    element_validation_error: str | None = None
    saw_imports = False
    saw_exports = False
    saw_elements = False

    for section_id, payload in sections:
        if section_id == 0:
            try:
                name, _ = _parse_custom_section(payload)
            except ValueError:
                continue
            custom_names.append(name)
            continue
        if section_id == 2 and not saw_imports:
            (
                imports,
                _func_import_count,
                module_imports,
                table_import_mins,
                memory_import_mins,
            ) = _parse_import_payload(payload)
            saw_imports = True
            continue
        if section_id == 7 and not saw_exports:
            exports, function_exports, export_kinds = _parse_export_payload(payload)
            saw_exports = True
            continue
        if section_id == 9 and not saw_elements:
            _, element_validation_error = _parse_element_payload(payload)
            saw_elements = True
            continue

    frozen_module_imports = {
        module: frozenset(names) for module, names in module_imports.items()
    }
    return WasmModuleFacts(
        imports=tuple(imports),
        exports=frozenset(exports),
        function_exports=MappingProxyType(dict(function_exports)),
        export_kinds=MappingProxyType(dict(export_kinds)),
        custom_names=tuple(custom_names),
        module_imports=MappingProxyType(frozen_module_imports),
        table_import_mins=MappingProxyType(dict(table_import_mins)),
        memory_import_mins=MappingProxyType(dict(memory_import_mins)),
        element_validation_error=element_validation_error,
    )


def _parse_type_section(
    sections: list[tuple[int, bytes]],
) -> list[tuple[tuple[int, ...], tuple[int, ...]]]:
    """Parse the type section and return a list of (param_types, result_types)."""
    for sid, payload in sections:
        if sid == 1:
            offset = 0
            type_count, offset = _read_varuint(payload, offset)
            types: list[tuple[tuple[int, ...], tuple[int, ...]]] = []
            for _ in range(type_count):
                _form = payload[offset]
                offset += 1
                pc, offset = _read_varuint(payload, offset)
                params = tuple(payload[offset + j] for j in range(pc))
                offset += pc
                rc, offset = _read_varuint(payload, offset)
                results = tuple(payload[offset + j] for j in range(rc))
                offset += rc
                types.append((params, results))
            return types
    return []


def _parse_func_type_indices(
    sections: list[tuple[int, bytes]],
) -> tuple[int, list[int]]:
    """Parse the function section. Returns (section_list_index, type_indices)."""
    for idx, (sid, payload) in enumerate(sections):
        if sid == 3:
            offset = 0
            fc, offset = _read_varuint(payload, offset)
            type_indices: list[int] = []
            for _ in range(fc):
                ti, offset = _read_varuint(payload, offset)
                type_indices.append(ti)
            return idx, type_indices
    return -1, []

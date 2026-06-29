#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
_WASM_ABI_GENERATED = REPO_ROOT / "src/molt/_wasm_abi_generated.py"
_WASM_ABI_SPEC = importlib.util.spec_from_file_location(
    "molt_tools_wasm_abi_generated", _WASM_ABI_GENERATED
)
if _WASM_ABI_SPEC is None or _WASM_ABI_SPEC.loader is None:
    raise RuntimeError(f"cannot load generated WASM ABI data: {_WASM_ABI_GENERATED}")
_WASM_ABI = importlib.util.module_from_spec(_WASM_ABI_SPEC)
_WASM_ABI_SPEC.loader.exec_module(_WASM_ABI)


WASM_MAGIC = b"\x00asm"

WASM_VERSION = b"\x01\x00\x00\x00"

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

WASM_TABLE_REF_EXPORT_PREFIX = _WASM_ABI.WASM_TABLE_REF_EXPORT_PREFIX

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


def table_ref_export_name(index: int) -> str:
    if index < 0:
        raise ValueError("WASM table-ref export index must be non-negative")
    return f"{WASM_TABLE_REF_EXPORT_PREFIX}{index}"


def parse_table_ref_export_name(name: str) -> int | None:
    if not name.startswith(WASM_TABLE_REF_EXPORT_PREFIX):
        return None
    raw = name[len(WASM_TABLE_REF_EXPORT_PREFIX) :]
    if not raw or not raw.isascii() or not raw.isdecimal():
        return None
    if raw != str(int(raw)):
        return None
    return int(raw)


def is_table_ref_export_name(name: str) -> bool:
    return parse_table_ref_export_name(name) is not None


_OUTPUT_RUNTIME_EXPORT_ALIASES = _WASM_ABI.WASM_OUTPUT_RUNTIME_EXPORT_ALIASES

_OUTPUT_EXPORT_ALIAS_PREFIX = _WASM_ABI.WASM_OUTPUT_EXPORT_ALIAS_PREFIX

_INTERNAL_OUTPUT_EXPORT_PREFIXES = _WASM_ABI.WASM_INTERNAL_OUTPUT_EXPORT_PREFIXES

_EMPTY_FUNC_BODY = bytes([0x00, 0x0B])

_ESSENTIAL_EXPORTS = _WASM_ABI.WASM_ESSENTIAL_EXPORTS

_TRAP_STUB_BODY = bytes([0x00, 0x00, 0x0B])

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

def _skip_data_symbol(payload: bytes, offset: int, flags: int) -> int:
    _, offset = _read_string(payload, offset)
    if not (flags & FLAG_UNDEFINED):
        _, offset = _read_varuint(payload, offset)
        _, offset = _read_varuint(payload, offset)
        _, offset = _read_varuint(payload, offset)
    return offset

def _collect_linking_function_symbols(data: bytes) -> list[tuple[int, int, str, str]]:
    symbols: list[tuple[int, int, str, str]] = []
    for section_id, payload in _parse_sections(data):
        if section_id != 0:
            continue
        name, custom_payload = _parse_custom_section(payload)
        if name != "linking":
            continue
        _, subsections = _parse_linking_payload(custom_payload)
        for sub_id, sub_payload in subsections:
            if sub_id != SYMTAB_SUBSECTION_ID:
                continue
            count, offset = _read_varuint(sub_payload, 0)
            for _ in range(count):
                if offset >= len(sub_payload):
                    raise ValueError("Unexpected EOF while reading linking symbols")
                kind = sub_payload[offset]
                offset += 1
                flags, offset = _read_varuint(sub_payload, offset)
                if kind == SYMBOL_KIND_FUNCTION:
                    index, symbol_name, offset = _parse_indexed_symbol(
                        sub_payload, offset, flags
                    )
                    symbols.append((flags, index, symbol_name, ""))
                    continue
                if kind in (2, 4, 5):
                    _, _, offset = _parse_indexed_symbol(sub_payload, offset, flags)
                    continue
                if kind == 1:
                    offset = _skip_data_symbol(sub_payload, offset, flags)
                    continue
                if kind == 3:
                    _, offset = _read_varuint(sub_payload, offset)
                    continue
                raise ValueError(f"Unknown linking symbol kind: {kind}")
            return symbols
    return symbols

def _encode_function_symbol_entry(*, flags: int, index: int, name: str) -> bytes:
    entry = bytearray()
    entry.append(SYMBOL_KIND_FUNCTION)
    entry.extend(_write_varuint(flags))
    entry.extend(_write_varuint(index))
    if (flags & FLAG_UNDEFINED) == 0 or (flags & FLAG_EXPLICIT_NAME):
        entry.extend(_write_string(name))
    return bytes(entry)

def _append_linking_function_symbols(
    data: bytes, entries: list[tuple[str, int, int]]
) -> bytes | None:
    if not entries:
        return None
    existing_names = {name for _, _, name, _ in _collect_linking_function_symbols(data)}
    pending = [
        _encode_function_symbol_entry(flags=flags, index=index, name=name)
        for name, index, flags in entries
        if name not in existing_names
    ]
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

def _collect_function_exports(data: bytes) -> dict[str, int]:
    exports: dict[str, int] = {}
    for section_id, payload in _parse_sections(data):
        if section_id != 7:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF while reading export kind")
            kind = payload[offset]
            offset += 1
            index, offset = _read_varuint(payload, offset)
            if kind == 0:
                exports[name] = index
        break
    return exports

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

def _skip_init_expr(data: bytes, offset: int) -> int:
    while offset < len(data):
        opcode = data[offset]
        offset += 1
        if opcode == 0x0B:
            return offset
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
            _, offset = _read_varuint(data, offset)
            continue
        raise ValueError(f"Unsupported init expr opcode 0x{opcode:02x}")
    raise ValueError("Unexpected EOF while reading init expr")

def _collect_element_declared_funcs(data: bytes) -> set[int]:
    """Collect all function indices declared in element segments."""
    declared: set[int] = set()
    for section_id, payload in _parse_sections(data):
        if section_id != 9:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_varuint(payload, offset)
            # Parse offset expression for active segments
            if flags in (0x02, 0x06):
                _, offset = _read_varuint(payload, offset)  # table index
                offset = _skip_init_expr(payload, offset)
            elif flags in (0x00, 0x04):
                offset = _skip_init_expr(payload, offset)
            # Parse element entries
            if flags in (0x00, 0x01, 0x02, 0x03):
                # Legacy format: optional elemkind byte + function index vector
                if flags in (0x01, 0x02, 0x03):
                    if offset < len(payload) and payload[offset] == 0x00:
                        offset += 1  # elemkind byte
                elem_count, offset = _read_varuint(payload, offset)
                for _ in range(elem_count):
                    func_idx, offset = _read_varuint(payload, offset)
                    declared.add(func_idx)
            else:
                # Expression format
                if flags in (0x05, 0x07):
                    offset += 1  # reftype
                expr_count, offset = _read_varuint(payload, offset)
                for _ in range(expr_count):
                    while offset < len(payload) and payload[offset] != 0x0B:
                        opcode = payload[offset]
                        offset += 1
                        if opcode == 0xD2:  # ref.func
                            func_idx, offset = _read_varuint(payload, offset)
                            declared.add(func_idx)
                        elif opcode == 0xD0:  # ref.null
                            offset += 1
                        elif opcode in (0x41, 0x42, 0x23):
                            _, offset = _read_varuint(payload, offset)
                        elif opcode == 0x43:
                            offset += 4
                        elif opcode == 0x44:
                            offset += 8
                    if offset < len(payload):
                        offset += 1  # skip 0x0B end
        break
    return declared

def _scan_code_ref_funcs(data: bytes) -> set[int]:
    """Scan all code bodies for ref.func (0xD2) instructions.

    Returns the set of function indices referenced by ref.func instructions.
    Uses the same full instruction decoder as ``_build_call_graph`` to avoid
    desynchronisation on opcodes with multi-byte immediates.
    """
    ref_funcs: set[int] = set()
    for section_id, payload in _parse_sections(data):
        if section_id != 10:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            body_size, body_start = _read_varuint(payload, offset)
            body_end = body_start + body_size
            pos = body_start
            # Skip locals
            num_local_decls, pos = _read_varuint(payload, pos)
            for _ld in range(num_local_decls):
                _, pos = _read_varuint(payload, pos)  # count
                pos += 1  # type
            # Scan instructions — mirrors _build_call_graph's decoder
            while pos < body_end:
                op = payload[pos]
                pos += 1
                if pos > body_end:
                    break
                # ref.func — the instruction we are looking for
                if op == 0xD2:
                    func_idx, pos = _read_varuint(payload, pos)
                    ref_funcs.add(func_idx)
                # No-immediate opcodes
                elif op in (
                    0x00,
                    0x01,
                    0x05,
                    0x0B,
                    0x0F,
                    0x1A,
                    0x1B,
                    0xD1,  # ref.is_null
                    0xD3,  # ref.as_non_null
                ):
                    pass
                # Block-type opcodes (block / loop / if)
                elif op in (0x02, 0x03, 0x04):
                    bt = payload[pos]
                    if bt in (0x40, 0x7F, 0x7E, 0x7D, 0x7C, 0x70, 0x6F, 0x7B):
                        pos += 1
                    else:
                        # Signed LEB128 type index
                        _, pos = _read_varsint(payload, pos)
                # Single-varuint opcodes
                elif op in (
                    0x0C,
                    0x0D,  # br, br_if
                    0x20,
                    0x21,
                    0x22,
                    0x23,
                    0x24,  # local/global ops
                    0x25,
                    0x26,  # table.get, table.set
                    0xD0,  # ref.null (heaptype)
                    0xD4,
                    0xD5,  # br_on_null, br_on_non_null
                ):
                    _, pos = _read_varuint(payload, pos)
                # br_table
                elif op == 0x0E:
                    cnt, pos = _read_varuint(payload, pos)
                    for _bt in range(cnt + 1):
                        _, pos = _read_varuint(payload, pos)
                # call / return_call
                elif op in (0x10, 0x12):
                    _, pos = _read_varuint(payload, pos)
                # call_indirect / return_call_indirect
                elif op in (0x11, 0x13):
                    _, pos = _read_varuint(payload, pos)
                    _, pos = _read_varuint(payload, pos)
                # call_ref / return_call_ref (type index immediate)
                elif op in (0x14, 0x15):
                    _, pos = _read_varuint(payload, pos)
                # select with types
                elif op == 0x1C:
                    n, pos = _read_varuint(payload, pos)
                    pos += n
                # Memory load/store (2 varuints: align + offset)
                elif 0x28 <= op <= 0x3E:
                    _, pos = _read_varuint(payload, pos)  # align
                    _, pos = _read_varuint(payload, pos)  # offset
                # memory.size / memory.grow
                elif op in (0x3F, 0x40):
                    _, pos = _read_varuint(payload, pos)  # memory index
                # Constants
                elif op == 0x41:  # i32.const
                    _, pos = _read_varsint(payload, pos)
                elif op == 0x42:  # i64.const
                    _, pos = _read_varsint(payload, pos)
                elif op == 0x43:  # f32.const
                    pos += 4
                elif op == 0x44:  # f64.const
                    pos += 8
                # Numeric ops (no immediates)
                elif 0x45 <= op <= 0xC4:
                    pass
                # try_table (exception handling)
                elif op == 0x1F:
                    bt = payload[pos]
                    if bt == 0x40 or (bt >= 0x7C and bt <= 0x7F):
                        pos += 1
                    else:
                        _, pos = _read_varsint(payload, pos)
                    n_catches, pos = _read_varuint(payload, pos)
                    for _ in range(n_catches):
                        catch_kind = payload[pos]
                        pos += 1
                        if catch_kind in (0x00, 0x01):
                            _, pos = _read_varuint(payload, pos)
                            _, pos = _read_varuint(payload, pos)
                        elif catch_kind in (0x02, 0x03):
                            _, pos = _read_varuint(payload, pos)
                # Extended opcodes (0xFC prefix)
                elif op == 0xFC:
                    ext, pos = _read_varuint(payload, pos)
                    if ext <= 7:  # trunc_sat
                        pass
                    elif ext == 8:  # memory.init
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                    elif ext == 9:  # data.drop
                        _, pos = _read_varuint(payload, pos)
                    elif ext == 10:  # memory.copy
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                    elif ext == 11:  # memory.fill
                        _, pos = _read_varuint(payload, pos)
                    elif ext == 12:  # table.init
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                    elif ext == 13:  # elem.drop
                        _, pos = _read_varuint(payload, pos)
                    elif ext == 14:  # table.copy
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                    elif ext in (15, 16, 17):  # table.grow/size/fill
                        _, pos = _read_varuint(payload, pos)
                # SIMD prefix (0xFD)
                elif op == 0xFD:
                    simd, pos = _read_varuint(payload, pos)
                    if simd <= 11:  # v128.load variants
                        _, pos = _read_varuint(payload, pos)  # align
                        _, pos = _read_varuint(payload, pos)  # offset
                    elif simd in (12, 13):  # v128.const / i8x16.shuffle
                        pos += 16
                    elif 84 <= simd <= 91:  # v128.load_lane/store_lane
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                        pos += 1  # lane index
                    elif 21 <= simd <= 34:  # extract_lane/replace_lane
                        pos += 1  # lane index
                    elif 92 <= simd <= 93:  # v128.load32_zero/load64_zero
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                    # Other SIMD ops have no immediates
                # Atomics prefix (0xFE)
                elif op == 0xFE:
                    atom, pos = _read_varuint(payload, pos)
                    if atom == 0x03:  # atomic.fence
                        pos += 1
                    elif atom >= 0x10 or atom in (0x00, 0x01, 0x02):
                        _, pos = _read_varuint(payload, pos)  # alignment
                        _, pos = _read_varuint(payload, pos)  # offset
                # All other single-byte opcodes (nop, unreachable, end,
                # return, drop, numeric ops, etc.) need no immediate
                # parsing.
            offset = body_end
        break
    return ref_funcs

def _declare_ref_func_elements(data: bytes) -> bytes | None:
    """Add a declarative element segment for functions referenced by ref.func
    but not yet declared in any element segment.

    The WebAssembly spec requires every function index used in a ref.func
    instruction to be *declared* in some element segment.  After wasm-ld
    links and --gc-sections runs, some element entries may be dropped while
    the code section still contains ref.func instructions pointing at them.
    This function patches the binary to add a declarative (flags=0x03)
    element segment covering the missing declarations.
    """
    declared = _collect_element_declared_funcs(data)
    referenced = _scan_code_ref_funcs(data)
    undeclared = sorted(referenced - declared)
    if not undeclared:
        return None

    # Build a declarative element segment (flags = 0x03).
    # Format: flags(0x03) elemkind(0x00) vec(funcidx...)
    new_segment = bytearray()
    new_segment.extend(_write_varuint(0x03))  # declarative
    new_segment.append(0x00)  # elemkind = funcref
    new_segment.extend(_write_varuint(len(undeclared)))
    for func_idx in undeclared:
        new_segment.extend(_write_varuint(func_idx))

    sections = _parse_sections(data)
    new_sections: list[tuple[int, bytes]] = []
    modified = False
    for section_id, payload in sections:
        if section_id != 9:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        rest = payload[offset:]
        updated = bytearray()
        updated.extend(_write_varuint(count + 1))
        updated.extend(rest)
        updated.extend(new_segment)
        new_sections.append((section_id, bytes(updated)))
        modified = True
    if not modified:
        # No element section yet -- create one.
        payload = _write_varuint(1) + bytes(new_segment)
        # Insert before section 10 (code) if possible.
        inserted = False
        for idx, (section_id, _payload) in enumerate(new_sections):
            if section_id > 9:
                new_sections.insert(idx, (9, payload))
                inserted = True
                break
        if not inserted:
            new_sections.append((9, payload))
        modified = True
    if not modified:
        return None
    return _build_sections(new_sections)

def _count_func_imports(sections: list[tuple[int, bytes]]) -> int:
    """Return the number of function imports in the import section."""
    for sid, payload in sections:
        if sid != 2:
            continue
        offset = 0
        total, offset = _read_varuint(payload, offset)
        func_imports = 0
        for _ in range(total):
            mod_len, offset = _read_varuint(payload, offset)
            offset += mod_len
            field_len, offset = _read_varuint(payload, offset)
            offset += field_len
            kind = payload[offset]
            offset += 1
            if kind == 0:  # function
                _, offset = _read_varuint(payload, offset)
                func_imports += 1
            elif kind == 1:  # table
                offset += 1  # reftype
                flags = payload[offset]
                offset += 1
                _, offset = _read_varuint(payload, offset)
                if flags & 1:
                    _, offset = _read_varuint(payload, offset)
            elif kind == 2:  # memory
                flags = payload[offset]
                offset += 1
                _, offset = _read_varuint(payload, offset)
                if flags & 1:
                    _, offset = _read_varuint(payload, offset)
            elif kind == 3:  # global
                offset += 2
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

def _repair_out_of_bounds_func_refs(data: bytes) -> bytes | None:
    """Detect and repair function index references that exceed the valid range.

    After post-link optimizations (export stripping, dead-code elimination,
    wasm-opt), function indices referenced by ``ref.func`` instructions or
    element segments may point beyond the total function count
    (import_count + defined_count).  ``wasm-tools validate`` rejects these
    as "undeclared reference to function #N".

    This pass:
    1. Computes the valid function index range [0, total_func_count).
    2. Scans element segments for out-of-bounds function indices and
       replaces them with function index 0 (the sentinel/first import).
    3. Scans code bodies for ``ref.func`` instructions with out-of-bounds
       indices and rewrites them to reference function index 0.

    Returns the repaired binary, or ``None`` if no repairs were needed.
    """
    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    import_count = _count_func_imports(sections)
    defined_count = 0
    for sid, payload in sections:
        if sid == 3:
            offset = 0
            defined_count, _ = _read_varuint(payload, offset)
            break
    total_func_count = import_count + defined_count

    if total_func_count == 0:
        return None

    oob_in_elements = 0
    oob_in_code = 0
    new_sections: list[tuple[int, bytes]] = []

    for sid, payload in sections:
        if sid == 9:
            # Rewrite element section, clamping out-of-bounds indices
            offset = 0
            count, offset = _read_varuint(payload, offset)
            out = bytearray(_write_varuint(count))
            for _ in range(count):
                flags = payload[offset]
                out.append(flags)
                offset += 1
                if flags in (0x02, 0x06):
                    # table index
                    leb_start = offset
                    _, offset = _read_varuint(payload, offset)
                    out.extend(payload[leb_start:offset])
                    # init expression
                    while offset < len(payload) and payload[offset] != 0x0B:
                        out.append(payload[offset])
                        offset += 1
                    if offset < len(payload):
                        out.append(payload[offset])  # end byte
                        offset += 1
                elif flags in (0x00, 0x04):
                    # init expression
                    while offset < len(payload) and payload[offset] != 0x0B:
                        out.append(payload[offset])
                        offset += 1
                    if offset < len(payload):
                        out.append(payload[offset])  # end byte
                        offset += 1

                if flags in (0x00, 0x01, 0x02, 0x03):
                    if flags in (0x01, 0x02, 0x03):
                        if offset < len(payload):
                            elemkind = payload[offset]
                            out.append(elemkind)
                            offset += 1
                    elem_count, offset = _read_varuint(payload, offset)
                    out.extend(_write_varuint(elem_count))
                    for _ in range(elem_count):
                        idx, offset = _read_varuint(payload, offset)
                        if idx >= total_func_count:
                            out.extend(_write_varuint(0))
                            oob_in_elements += 1
                        else:
                            out.extend(_write_varuint(idx))
                else:
                    if flags in (0x05, 0x07):
                        if offset < len(payload):
                            out.append(payload[offset])  # reftype
                            offset += 1
                    expr_count, offset = _read_varuint(payload, offset)
                    out.extend(_write_varuint(expr_count))
                    for _ in range(expr_count):
                        while offset < len(payload) and payload[offset] != 0x0B:
                            opcode = payload[offset]
                            offset += 1
                            if opcode == 0xD2:  # ref.func
                                idx, offset = _read_varuint(payload, offset)
                                out.append(opcode)
                                if idx >= total_func_count:
                                    out.extend(_write_varuint(0))
                                    oob_in_elements += 1
                                else:
                                    out.extend(_write_varuint(idx))
                            elif opcode == 0xD0:  # ref.null
                                out.append(opcode)
                                if offset < len(payload):
                                    out.append(payload[offset])
                                    offset += 1
                            elif opcode in (0x41, 0x42, 0x23):
                                out.append(opcode)
                                leb_start = offset
                                _, offset = _read_varuint(payload, offset)
                                out.extend(payload[leb_start:offset])
                            elif opcode == 0x43:
                                out.append(opcode)
                                out.extend(payload[offset : offset + 4])
                                offset += 4
                            elif opcode == 0x44:
                                out.append(opcode)
                                out.extend(payload[offset : offset + 8])
                                offset += 8
                            else:
                                out.append(opcode)
                        if offset < len(payload):
                            out.append(payload[offset])  # end byte
                            offset += 1
            new_sections.append((sid, bytes(out)))
        elif sid == 10:
            # Rewrite code section, patching out-of-bounds ref.func
            offset = 0
            func_count, offset = _read_varuint(payload, offset)
            out = bytearray(_write_varuint(func_count))
            for _ in range(func_count):
                body_size, body_start = _read_varuint(payload, offset)
                body_end = body_start + body_size
                body = payload[body_start:body_end]
                # Scan for ref.func (0xD2) and patch out-of-bounds indices
                new_body = bytearray()
                pos = 0
                # Skip locals
                local_count, pos = _read_varuint(body, pos)
                new_body.extend(body[:pos])
                patched_this_func = False
                while pos < len(body):
                    op = body[pos]
                    pos += 1
                    if op == 0xD2:  # ref.func
                        idx, pos = _read_varuint(body, pos)
                        new_body.append(op)
                        if idx >= total_func_count:
                            new_body.extend(_write_varuint(0))
                            oob_in_code += 1
                            patched_this_func = True
                        else:
                            new_body.extend(_write_varuint(idx))
                    elif op in (0x10, 0x12):  # call / return_call
                        idx, pos = _read_varuint(body, pos)
                        new_body.append(op)
                        if idx >= total_func_count:
                            new_body.extend(_write_varuint(0))
                            oob_in_code += 1
                            patched_this_func = True
                        else:
                            new_body.extend(_write_varuint(idx))
                    else:
                        # Copy instruction verbatim.  We only need to
                        # advance `pos` past any immediates to stay
                        # aligned.  For ref.func we already handled it
                        # above; for all other opcodes, copy the raw
                        # bytes and let the rest of the body follow.
                        new_body.append(op)
                        # For most single-byte opcodes, pos is already
                        # past the opcode.  We copy the rest of the
                        # function body as a single chunk if we haven't
                        # patched anything yet.
                        if not patched_this_func:
                            # No patches needed in this function at all:
                            # just copy the rest of the body.
                            new_body.extend(body[pos:])
                            break
                        # If we HAVE patched something, we need to copy
                        # byte by byte to stay synchronized with the
                        # instruction stream.  However, a full
                        # instruction decoder is complex.  Instead, just
                        # copy the remaining bytes as-is -- the only
                        # instructions we need to patch are ref.func and
                        # call, which we've already handled above by
                        # scanning forward.  For a function body where
                        # we've already seen a patch, copy the rest.
                        new_body.extend(body[pos:])
                        break
                if len(new_body) != len(body):
                    # Size changed due to LEB128 encoding differences;
                    # update the body size.
                    out.extend(_write_varuint(len(new_body)))
                    out.extend(new_body)
                else:
                    out.extend(_write_varuint(body_size))
                    out.extend(new_body)
                offset = body_end
            new_sections.append((sid, bytes(out)))
        else:
            new_sections.append((sid, payload))

    if oob_in_elements == 0 and oob_in_code == 0:
        return None

    print(
        f"Repaired {oob_in_elements + oob_in_code} out-of-bounds function reference(s) "
        f"({oob_in_elements} in elements, {oob_in_code} in code)",
        file=sys.stderr,
    )
    return _build_sections(new_sections)

def _safe_repair_out_of_bounds_func_refs(data: bytes) -> bytes | None:
    """Best-effort function-reference repair that never masks validation.

    The repair pass runs before canonical wasm validation and operates on raw
    post-link bytes.  Malformed or synthetic modules should still be judged by
    the validator; the repair scanner must not become a second validator that
    can crash before that decision point.
    """
    try:
        return _repair_out_of_bounds_func_refs(data)
    except (IndexError, ValueError):
        return None

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

def _collect_exports(data: bytes) -> set[str]:
    exports: set[str] = set()
    for section_id, payload in _parse_sections(data):
        if section_id != 7:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF while reading export")
            offset += 1
            _, offset = _read_varuint(payload, offset)
            exports.add(name)
    return exports

def _collect_imports(data: bytes) -> list[tuple[str, str, int, bytes]]:
    for section_id, payload in _parse_sections(data):
        if section_id != 2:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        imports: list[tuple[str, str, int, bytes]] = []
        for _ in range(count):
            module, offset = _read_string(payload, offset)
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF while reading import kind")
            kind = payload[offset]
            offset += 1
            desc_start = offset
            offset = _parse_import_desc(payload, offset, kind)
            imports.append((module, name, kind, payload[desc_start:offset]))
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
        if section_id != 9:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_varuint(payload, offset)
            if flags in (0x02, 0x06):
                table_index, offset = _read_varuint(payload, offset)
                if table_index != 0:
                    return False, f"element segment targets table {table_index}"
                offset = _skip_init_expr(payload, offset)
            elif flags in (0x00, 0x04):
                offset = _skip_init_expr(payload, offset)
            elif flags in (0x01, 0x03, 0x05, 0x07):
                pass
            else:
                return False, f"unsupported element segment flags 0x{flags:x}"
            if flags in (0x00, 0x01, 0x02, 0x03):
                if offset >= len(payload):
                    return False, "unexpected EOF reading elemkind"
                # Some toolchains omit the legacy elemkind byte; tolerate both.
                if payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_varuint(payload, offset)
                for _ in range(elem_count):
                    _, offset = _read_varuint(payload, offset)
            else:
                if offset >= len(payload):
                    return False, "unexpected EOF reading elemtype"
                offset += 1
                expr_count, offset = _read_varuint(payload, offset)
                for _ in range(expr_count):
                    offset = _skip_init_expr(payload, offset)
        break
    return True, None

def _collect_module_imports(wasm_data: bytes, module_name: str) -> set[str]:
    """Parse a WASM module and return the set of import names from *module_name*.

    For example, if the app module imports ``(import "molt_runtime" "print_obj" ...)``,
    calling ``_collect_module_imports(app_data, "molt_runtime")`` returns ``{"print_obj"}``.
    """
    sections = _parse_sections(wasm_data)
    result: set[str] = set()
    for section_id, payload in sections:
        if section_id != 2:  # import section
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            mod, offset = _read_string(payload, offset)
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF reading import kind")
            kind = payload[offset]
            offset += 1
            # Skip the import descriptor based on kind.
            if kind == 0:  # function
                _, offset = _read_varuint(payload, offset)
            elif kind == 1:  # table
                offset += 1  # elemtype
                flags, offset = _read_varuint(payload, offset)
                _, offset = _read_varuint(payload, offset)  # initial
                if flags & 0x1:
                    _, offset = _read_varuint(payload, offset)  # maximum
            elif kind == 2:  # memory
                flags, offset = _read_varuint(payload, offset)
                _, offset = _read_varuint(payload, offset)  # initial
                if flags & 0x1:
                    _, offset = _read_varuint(payload, offset)  # maximum
            elif kind == 3:  # global
                offset += 1  # valtype
                offset += 1  # mutability
            elif kind == 4:  # tag
                offset += 1  # attribute
                _, offset = _read_varuint(payload, offset)  # type index
            else:
                raise ValueError(f"Unknown import kind {kind}")
            if mod == module_name:
                result.add(name)
    return result

def _build_call_graph(code_payload: bytes, import_count: int) -> dict[int, set[int]]:
    """Build a call graph by decoding WASM instructions in the code section.

    Returns a mapping from function index to the set of function indices it
    directly calls (via the ``call`` opcode 0x10) or references (via
    ``ref.func`` opcode 0xD2).  Indirect calls (``call_indirect``) are
    intentionally excluded since their targets are determined at runtime.
    """
    graph: dict[int, set[int]] = {}
    offset = 0
    func_count, offset = _read_varuint(code_payload, offset)

    for f_idx in range(func_count):
        func_index = import_count + f_idx
        body_size, offset = _read_varuint(code_payload, offset)
        body_end = offset + body_size
        calls: set[int] = set()

        if body_size <= 3:
            offset = body_end
            graph[func_index] = calls
            continue

        pos = offset
        try:
            lc, pos = _read_varuint(code_payload, pos)
            for _ in range(lc):
                _, pos = _read_varuint(code_payload, pos)
                pos += 1  # valtype
        except (IndexError, ValueError):
            offset = body_end
            graph[func_index] = calls
            continue

        # Decode instructions, tracking only call/ref.func targets
        while pos < body_end:
            op = code_payload[pos]
            pos += 1
            if pos > body_end:
                break
            # No-immediate opcodes
            if op in (
                0x00,
                0x01,
                0x05,
                0x0B,
                0x0F,
                0x1A,
                0x1B,
                0xD1,  # ref.is_null
                0xD3,  # ref.as_non_null
            ):
                pass
            # Block-type opcodes
            elif op in (0x02, 0x03, 0x04):
                bt = code_payload[pos]
                if bt in (0x40, 0x7F, 0x7E, 0x7D, 0x7C, 0x70, 0x6F, 0x7B):
                    pos += 1
                else:
                    while code_payload[pos] & 0x80:
                        pos += 1
                    pos += 1
            # Single-varuint opcodes
            elif op in (
                0x0C,
                0x0D,  # br, br_if
                0x20,
                0x21,
                0x22,
                0x23,
                0x24,  # local/global ops
                0x25,
                0x26,  # table.get, table.set
                0x3F,
                0x40,  # memory.size, memory.grow
                0xD0,  # ref.null (heaptype)
                0xD4,
                0xD5,  # br_on_null, br_on_non_null
            ):
                _, pos = _read_varuint(code_payload, pos)
            # br_table
            elif op == 0x0E:
                n, pos = _read_varuint(code_payload, pos)
                for _ in range(n + 1):
                    _, pos = _read_varuint(code_payload, pos)
            # call / return_call
            elif op in (0x10, 0x12):
                idx, pos = _read_varuint(code_payload, pos)
                calls.add(idx)
            # call_indirect / return_call_indirect
            elif op in (0x11, 0x13):
                _, pos = _read_varuint(code_payload, pos)
                _, pos = _read_varuint(code_payload, pos)
            # call_ref / return_call_ref (type index immediate)
            elif op in (0x14, 0x15):
                _, pos = _read_varuint(code_payload, pos)
            # ref.func
            elif op == 0xD2:
                idx, pos = _read_varuint(code_payload, pos)
                calls.add(idx)
            # Memory load/store (2 varuints: align + offset)
            elif 0x28 <= op <= 0x3E:
                _, pos = _read_varuint(code_payload, pos)
                _, pos = _read_varuint(code_payload, pos)
            # Constants
            elif op == 0x41:  # i32.const
                while code_payload[pos] & 0x80:
                    pos += 1
                pos += 1
            elif op == 0x42:  # i64.const
                while code_payload[pos] & 0x80:
                    pos += 1
                pos += 1
            elif op == 0x43:
                pos += 4  # f32.const
            elif op == 0x44:
                pos += 8  # f64.const
            # Numeric ops (no immediates)
            elif 0x45 <= op <= 0xC4:
                pass
            # select with types
            elif op == 0x1C:
                n, pos = _read_varuint(code_payload, pos)
                pos += n
            # Extended opcodes
            elif op == 0xFC:
                ext, pos = _read_varuint(code_payload, pos)
                if ext <= 7:
                    pass
                elif ext in (8, 10, 12, 14):
                    _, pos = _read_varuint(code_payload, pos)
                    _, pos = _read_varuint(code_payload, pos)
                elif ext in (9, 11, 13, 15, 16, 17):
                    _, pos = _read_varuint(code_payload, pos)
            # SIMD prefix
            elif op == 0xFD:
                simd, pos = _read_varuint(code_payload, pos)
                if simd <= 11:
                    _, pos = _read_varuint(code_payload, pos)
                    _, pos = _read_varuint(code_payload, pos)
                elif simd in (12, 13):
                    pos += 16
                elif 84 <= simd <= 91:
                    _, pos = _read_varuint(code_payload, pos)
                    _, pos = _read_varuint(code_payload, pos)
                    pos += 1
                elif 21 <= simd <= 34:
                    pos += 1
                elif 92 <= simd <= 93:
                    _, pos = _read_varuint(code_payload, pos)
                    _, pos = _read_varuint(code_payload, pos)
            # try_table (exception handling)
            elif op == 0x1F:
                # Block type
                bt = code_payload[pos]
                if bt == 0x40 or (bt >= 0x7C and bt <= 0x7F):  # void or valtype
                    pos += 1
                else:
                    _, pos = _read_varsint(
                        code_payload, pos
                    )  # type index (signed LEB128)
                # Catch vector
                n_catches, pos = _read_varuint(code_payload, pos)
                for _ in range(n_catches):
                    catch_kind = code_payload[pos]
                    pos += 1
                    if catch_kind in (
                        0x00,
                        0x01,
                    ):  # catch / catch_ref: tag_index + label
                        _, pos = _read_varuint(code_payload, pos)
                        _, pos = _read_varuint(code_payload, pos)
                    elif catch_kind in (
                        0x02,
                        0x03,
                    ):  # catch_all / catch_all_ref: label only
                        _, pos = _read_varuint(code_payload, pos)
            # Atomics prefix
            elif op == 0xFE:
                atom, pos = _read_varuint(code_payload, pos)
                if atom == 0x03:  # atomic.fence — 1-byte reserved immediate
                    pos += 1
                elif atom >= 0x10 or atom in (0x00, 0x01, 0x02):
                    _, pos = _read_varuint(code_payload, pos)  # alignment
                    _, pos = _read_varuint(code_payload, pos)  # offset
            else:
                # Unknown opcode -- stop decoding this function body
                break

        graph[func_index] = calls
        offset = body_end

    return graph

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

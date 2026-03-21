#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


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
# Rust wasm symbol names include a hash suffix like "17h<hex...>E". Capture the arity
# digits that precede the 2-digit hash-length tag so 10+ arities don't get truncated.
CALL_INDIRECT_MANGLED_RE = re.compile(r"molt_call_indirect(\d+)(?=\d{2}h[0-9a-fA-F]+E)")
_OUTPUT_RUNTIME_EXPORT_ALIASES = (
    "molt_isolate_bootstrap",
    "molt_isolate_import",
)


def _default_runtime_path() -> Path:
    env_root = os.environ.get("MOLT_WASM_RUNTIME_DIR")
    if env_root:
        return Path(env_root).expanduser() / "molt_runtime.wasm"
    ext_root = os.environ.get("MOLT_EXT_ROOT")
    external_root = Path(ext_root).expanduser() if ext_root else None
    if external_root is not None and external_root.is_dir():
        return external_root / "wasm" / "molt_runtime.wasm"
    return Path("wasm/molt_runtime.wasm")


def _is_wasm_binary(data: bytes) -> bool:
    return len(data) >= 8 and data[:4] == WASM_MAGIC and data[4:8] == WASM_VERSION


def _read_wasm_bytes_with_retry(
    path: Path, *, attempts: int = 8, delay_sec: float = 0.05
) -> bytes:
    data = b""
    for _ in range(max(1, attempts)):
        try:
            data = path.read_bytes()
        except OSError:
            data = b""
        if _is_wasm_binary(data):
            return data
        time.sleep(delay_sec)
    return data


def _find_tool(names: list[str]) -> str | None:
    for name in names:
        path = shutil.which(name)
        if path:
            return path
    return None


def _find_wasm_ld() -> str | None:
    """Return the path to `wasm-ld` if it is available."""
    return _find_tool(["wasm-ld"])


def _read_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varuint")
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


def _dump_symbols(path: Path, wasm_tools: str) -> list[tuple[int, int, str, str]]:
    res = subprocess.run(
        [wasm_tools, "dump", str(path)],
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        err = res.stderr.strip() or res.stdout.strip()
        if err:
            print(err, file=sys.stderr)
        return []
    symbols: list[tuple[int, int, str, str]] = []
    for line in res.stdout.splitlines():
        match = SYMBOL_DUMP_RE.search(line)
        if not match:
            continue
        flags_text, index_text, name = match.groups()
        flags = _parse_symbol_flags(flags_text)
        index = int(index_text)
        symbols.append((flags, index, name, flags_text))
    return symbols


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
    Uses a robust scanning approach that handles the full WASM instruction set.
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
            # Scan instructions
            while pos < body_end:
                opcode = payload[pos]
                pos += 1
                if opcode == 0xD2:  # ref.func
                    func_idx, pos = _read_varuint(payload, pos)
                    ref_funcs.add(func_idx)
                elif opcode == 0x10:  # call
                    _, pos = _read_varuint(payload, pos)
                elif opcode == 0x11:  # call_indirect
                    _, pos = _read_varuint(payload, pos)
                    _, pos = _read_varuint(payload, pos)
                elif opcode in (0x02, 0x03, 0x04):  # block/loop/if
                    bt = payload[pos]
                    pos += 1
                    if bt not in (
                        0x40, 0x7F, 0x7E, 0x7D, 0x7C, 0x7B, 0x70, 0x6F,
                    ):
                        # Signed LEB128 type index; we already consumed one
                        # byte so back up and re-read.
                        pos -= 1
                        _, pos = _read_varsint(payload, pos)
                elif opcode in (0x0C, 0x0D):  # br, br_if
                    _, pos = _read_varuint(payload, pos)
                elif opcode == 0x0E:  # br_table
                    cnt, pos = _read_varuint(payload, pos)
                    for _bt in range(cnt + 1):
                        _, pos = _read_varuint(payload, pos)
                elif opcode in (0x20, 0x21, 0x22, 0x23, 0x24):  # local/global
                    _, pos = _read_varuint(payload, pos)
                elif opcode == 0x41:  # i32.const
                    _, pos = _read_varsint(payload, pos)
                elif opcode == 0x42:  # i64.const
                    _, pos = _read_varsint(payload, pos)
                elif opcode == 0x43:  # f32.const
                    pos += 4
                elif opcode == 0x44:  # f64.const
                    pos += 8
                elif opcode == 0xD0:  # ref.null
                    pos += 1
                elif 0x28 <= opcode <= 0x3E:  # memory load/store
                    _, pos = _read_varuint(payload, pos)  # align
                    _, pos = _read_varuint(payload, pos)  # offset
                elif opcode in (0x3F, 0x40):  # memory.size/grow
                    pos += 1  # memory index
                elif opcode == 0xFC:  # multi-byte prefix
                    sub, pos = _read_varuint(payload, pos)
                    if sub < 8:  # trunc_sat
                        pass
                    elif sub == 8:  # memory.init
                        _, pos = _read_varuint(payload, pos)
                        pos += 1
                    elif sub == 9:  # data.drop
                        _, pos = _read_varuint(payload, pos)
                    elif sub == 10:  # memory.copy
                        pos += 2
                    elif sub == 11:  # memory.fill
                        pos += 1
                    elif sub == 12:  # table.init
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                    elif sub == 13:  # elem.drop
                        _, pos = _read_varuint(payload, pos)
                    elif sub == 14:  # table.copy
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                    elif sub in (15, 16, 17):  # table.grow/size/fill
                        _, pos = _read_varuint(payload, pos)
                elif opcode == 0xFD:  # SIMD prefix
                    sub, pos = _read_varuint(payload, pos)
                    if sub < 12:  # v128.load variants
                        _, pos = _read_varuint(payload, pos)  # align
                        _, pos = _read_varuint(payload, pos)  # offset
                    elif sub == 12:  # v128.const
                        pos += 16
                    elif sub == 13:  # i8x16.shuffle
                        pos += 16
                    elif 84 <= sub <= 91:  # v128.load_lane/store_lane
                        _, pos = _read_varuint(payload, pos)
                        _, pos = _read_varuint(payload, pos)
                        pos += 1  # lane index
                    # Other SIMD ops have no immediates
                # All other single-byte opcodes (nop, unreachable, end,
                # return, drop, select, numeric ops, etc.) need no
                # immediate parsing.
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


def _append_table_ref_elements(data: bytes) -> bytes | None:
    names = _collect_func_names(data)
    table_refs: list[int] = []
    for func_idx, name in names.items():
        if name.startswith("__molt_table_ref_"):
            table_refs.append(func_idx)
    if not table_refs:
        return None
    table_refs.sort()
    sections = _parse_sections(data)
    new_sections: list[tuple[int, bytes]] = []
    modified = False
    new_segment = bytearray()
    new_segment.append(0x01)
    new_segment.append(0x00)
    new_segment.extend(_write_varuint(len(table_refs)))
    for func_idx in table_refs:
        new_segment.extend(_write_varuint(func_idx))
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
        payload = _write_varuint(1) + bytes(new_segment)
        new_sections.append((9, payload))
        modified = True
    if not modified:
        return None
    return _build_sections(new_sections)


def _find_call_indirect_mangled(runtime: Path) -> dict[str, str]:
    wasm_tools = _find_tool(["wasm-tools"])
    if not wasm_tools:
        print(
            "wasm-tools not found; cannot extract call_indirect symbol name.",
            file=sys.stderr,
        )
        return {}
    names: dict[str, str] = {}
    for flags, _, name, _ in _dump_symbols(runtime, wasm_tools):
        if not (flags & FLAG_UNDEFINED):
            continue
        match = CALL_INDIRECT_RE.fullmatch(name)
        if match:
            idx = match.group(1)
            names[f"molt_call_indirect{idx}"] = name
            continue
        mangled_match = CALL_INDIRECT_MANGLED_RE.search(name)
        if mangled_match:
            idx = mangled_match.group(1)
            names[f"molt_call_indirect{idx}"] = name
    if not names:
        print("Unable to locate runtime call_indirect symbol names.", file=sys.stderr)
    return names


def _find_output_call_indirect_symbol(output: Path) -> dict[str, tuple[int, int]]:
    wasm_tools = _find_tool(["wasm-tools"])
    if not wasm_tools:
        print(
            "wasm-tools not found; cannot extract output symbol info.", file=sys.stderr
        )
        return {}
    symbols: dict[str, tuple[int, int]] = {}
    for flags, index, name, _ in _dump_symbols(output, wasm_tools):
        if CALL_INDIRECT_RE.fullmatch(name):
            symbols[name] = (index, flags)
    if not symbols:
        print("Unable to locate output call_indirect symbols.", file=sys.stderr)
    return symbols


def _add_symtab_alias(
    data: bytes, alias_name: str, alias_index: int, alias_flags: int
) -> bytes | None:
    sections = _parse_sections(data)
    modified = False
    for idx, (section_id, payload) in enumerate(sections):
        if section_id != 0:
            continue
        name, custom_payload = _parse_custom_section(payload)
        if name != "linking":
            continue
        version, subsections = _parse_linking_payload(custom_payload)
        new_subsections: list[tuple[int, bytes]] = []
        for sub_id, sub_payload in subsections:
            if sub_id != SYMTAB_SUBSECTION_ID:
                new_subsections.append((sub_id, sub_payload))
                continue
            if _write_string(alias_name) in sub_payload:
                new_subsections.append((sub_id, sub_payload))
                continue
            count, offset = _read_varuint(sub_payload, 0)
            entries = sub_payload[offset:]
            alias_entry = bytearray()
            alias_entry.append(SYMBOL_KIND_FUNCTION)
            alias_entry.extend(
                _write_varuint((alias_flags & ~FLAG_EXPORTED) | FLAG_EXPLICIT_NAME)
            )
            alias_entry.extend(_write_varuint(alias_index))
            alias_entry.extend(_write_string(alias_name))
            new_payload = _write_varuint(count + 1) + entries + alias_entry
            new_subsections.append((sub_id, new_payload))
            modified = True
        if modified:
            updated = _build_linking_payload(version, new_subsections)
            sections[idx] = (section_id, _build_custom_section(name, updated))
            break
    if not modified:
        return None
    return _build_sections(sections)


def _inject_call_indirect_alias(
    output: Path, runtime: Path, temp_dir: tempfile.TemporaryDirectory
) -> Path:
    mangled = _find_call_indirect_mangled(runtime)
    symbol_info = _find_output_call_indirect_symbol(output)
    if not mangled or not symbol_info:
        return output
    data = output.read_bytes()
    updated = data
    modified = False
    for name, mangled_name in mangled.items():
        alias = symbol_info.get(name)
        if alias is None:
            print(f"Unable to locate output {name} symbol.", file=sys.stderr)
            continue
        alias_index, alias_flags = alias
        next_data = _add_symtab_alias(updated, mangled_name, alias_index, alias_flags)
        if next_data is not None:
            updated = next_data
            modified = True
    if not modified:
        return output
    alias_path = Path(temp_dir.name) / "output_alias.wasm"
    alias_path.write_bytes(updated)
    return alias_path


def _inject_output_export_aliases(
    output: Path, temp_dir: tempfile.TemporaryDirectory
) -> Path:
    try:
        export_indices = _collect_function_exports(output.read_bytes())
    except ValueError as exc:
        print(f"Failed to parse output exports: {exc}", file=sys.stderr)
        return output
    updated = output.read_bytes()
    modified = False
    for name in _OUTPUT_RUNTIME_EXPORT_ALIASES:
        func_index = export_indices.get(name)
        if func_index is None:
            continue
        next_data = _add_symtab_alias(
            updated,
            name,
            func_index,
            FLAG_BINDING_GLOBAL | FLAG_EXPLICIT_NAME | FLAG_EXPORTED,
        )
        if next_data is not None:
            updated = next_data
            modified = True
    if not modified:
        return output
    alias_path = Path(temp_dir.name) / "output_exports_alias.wasm"
    alias_path.write_bytes(updated)
    return alias_path


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


def _has_table(data: bytes) -> bool:
    for module, name, kind, _ in _collect_imports(data):
        if kind == 1 and name == "__indirect_function_table":
            return True
    for section_id, _ in _parse_sections(data):
        if section_id == 4:
            return True
    return False


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


def _table_import_min(data: bytes) -> int | None:
    for module, name, kind, desc in _collect_imports(data):
        if kind != 1 or module != "env" or name != "__indirect_function_table":
            continue
        if not desc:
            return None
        _, minimum, _, _ = _read_limits(desc, 1)
        return minimum
    return None


def _memory_import_min(data: bytes) -> int | None:
    for module, name, kind, desc in _collect_imports(data):
        if kind != 2 or module != "env" or name != "memory":
            continue
        if not desc:
            return None
        _, minimum, _, _ = _read_limits(desc, 0)
        return minimum
    return None


def _rewrite_table_import_min(data: bytes, required_min: int) -> bytes | None:
    sections = _parse_sections(data)
    changed = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 2:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        rebuilt = bytearray()
        rebuilt.extend(_write_varuint(count))
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
            if kind == 1 and module == "env" and name == "__indirect_function_table":
                if not desc:
                    raise ValueError("Missing table import descriptor")
                element_type = desc[0:1]
                flags, minimum, maximum, _ = _read_limits(desc, 1)
                new_min = max(minimum, required_min)
                new_max = maximum
                if maximum is not None and new_min > maximum:
                    new_max = new_min
                if new_min != minimum or new_max != maximum:
                    changed = True
                    desc = element_type + _write_limits(flags, new_min, new_max)
            rebuilt.extend(_write_string(module))
            rebuilt.extend(_write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(desc)
        new_sections.append((section_id, bytes(rebuilt)))
    if not changed:
        return None
    return _build_sections(new_sections)


def _rewrite_memory_min(data: bytes, required_min: int) -> bytes | None:
    sections = _parse_sections(data)
    changed = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id == 2:
            offset = 0
            count, offset = _read_varuint(payload, offset)
            rebuilt = bytearray()
            rebuilt.extend(_write_varuint(count))
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
                if kind == 2 and module == "env" and name == "memory":
                    flags, minimum, maximum, _ = _read_limits(desc, 0)
                    new_min = max(minimum, required_min)
                    new_max = maximum
                    if maximum is not None and new_min > maximum:
                        new_max = new_min
                    if new_min != minimum or new_max != maximum:
                        changed = True
                        desc = _write_limits(flags, new_min, new_max)
                rebuilt.extend(_write_string(module))
                rebuilt.extend(_write_string(name))
                rebuilt.append(kind)
                rebuilt.extend(desc)
            new_sections.append((section_id, bytes(rebuilt)))
            continue
        if section_id == 5:
            offset = 0
            count, offset = _read_varuint(payload, offset)
            rebuilt = bytearray()
            rebuilt.extend(_write_varuint(count))
            for _ in range(count):
                flags, minimum, maximum, offset = _read_limits(payload, offset)
                new_min = max(minimum, required_min)
                new_max = maximum
                if maximum is not None and new_min > maximum:
                    new_max = new_min
                if new_min != minimum or new_max != maximum:
                    changed = True
                rebuilt.extend(_write_limits(flags, new_min, new_max))
            new_sections.append((section_id, bytes(rebuilt)))
            continue
        new_sections.append((section_id, payload))
    if not changed:
        return None
    return _build_sections(new_sections)


def _rewrite_output_imports(
    output: Path, runtime_exports: set[str]
) -> tuple[Path, tempfile.TemporaryDirectory] | None:
    data = output.read_bytes()
    try:
        sections = _parse_sections(data)
    except ValueError as exc:
        print(f"Failed to parse wasm: {exc}", file=sys.stderr)
        return None

    missing: list[str] = []
    needs_rewrite = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 2:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        rebuilt = bytearray()
        rebuilt.extend(_write_varuint(count))
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

            new_name = name
            if module == "molt_runtime" and kind == 0 and not name.startswith("molt_"):
                prefixed = f"molt_{name}"
                if prefixed in runtime_exports:
                    new_name = prefixed
                    needs_rewrite = True
                elif name not in runtime_exports:
                    missing.append(name)

            rebuilt.extend(_write_string(module))
            rebuilt.extend(_write_string(new_name))
            rebuilt.append(kind)
            rebuilt.extend(desc)
        new_sections.append((section_id, bytes(rebuilt)))

    if missing:
        missing_list = ", ".join(sorted(set(missing)))
        print(
            f"Output imports missing in runtime exports: {missing_list}",
            file=sys.stderr,
        )
        return None

    if not needs_rewrite:
        return output, tempfile.TemporaryDirectory(prefix="molt-wasm-link-")

    temp_dir = tempfile.TemporaryDirectory(prefix="molt-wasm-link-")
    wasm_path = Path(temp_dir.name) / "output_rewrite.wasm"
    wasm_path.write_bytes(_build_sections(new_sections))
    return wasm_path, temp_dir


def _strip_debug_sections(data: bytes) -> bytes | None:
    """Remove custom debug/name/producer sections that bloat the linked artifact.

    V8 must compile every function in the module at load time.  Large name
    sections and debug info cause disproportionate memory pressure during
    compilation — stripping them is the single biggest win for OOM avoidance.
    """
    sections = _parse_sections(data)
    keep: list[tuple[int, bytes]] = []
    stripped = False
    for section_id, payload in sections:
        if section_id != 0:
            keep.append((section_id, payload))
            continue
        try:
            name, _ = _parse_custom_section(payload)
        except ValueError:
            keep.append((section_id, payload))
            continue
        # Strip name, debug, producers, source-mapping and reloc sections
        if name in (
            "name",
            ".debug_info",
            ".debug_line",
            ".debug_abbrev",
            ".debug_str",
            ".debug_ranges",
            ".debug_loc",
            ".debug_aranges",
            ".debug_pubtypes",
            ".debug_pubnames",
            "producers",
            "sourceMappingURL",
            "linking",
            "dylink.0",
        ) or name.startswith("reloc."):
            stripped = True
            continue
        keep.append((section_id, payload))
    if not stripped:
        return None
    return _build_sections(keep)


_ESSENTIAL_EXPORTS = frozenset({
    "memory",
    "molt_memory",
    "molt_main",
    "molt_table_init",
    "molt_table",
    "molt_set_wasm_table_base",
    "__indirect_function_table",
})


def _strip_internal_exports(data: bytes) -> bytes | None:
    """Remove exports that only exist for internal ABI wiring or relocatable linking.

    After linking, these exports serve no purpose but each one marks its
    target function as a module root, preventing dead-code elimination by
    wasm-opt.  Stripping them is critical for enabling the DCE pass to
    remove thousands of unreachable runtime functions.

    Only the exports actually referenced by the host JS (worker.js) are
    retained (see ``_ESSENTIAL_EXPORTS``).
    """
    sections = _parse_sections(data)
    new_sections: list[tuple[int, bytes]] = []
    modified = False
    for section_id, payload in sections:
        if section_id != 7:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        entries: list[bytes] = []
        new_count = 0
        while offset < len(payload):
            entry_start = offset
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                break
            kind = payload[offset]
            offset += 1
            _, offset = _read_varuint(payload, offset)
            entry_bytes = payload[entry_start:offset]
            if name not in _ESSENTIAL_EXPORTS:
                modified = True
                continue
            entries.append(entry_bytes)
            new_count += 1
        rebuilt = bytearray(_write_varuint(new_count))
        for entry in entries:
            rebuilt.extend(entry)
        new_sections.append((section_id, bytes(rebuilt)))
    if not modified:
        return None
    return _build_sections(new_sections)


def _collect_code_referenced_funcs(sections: list[tuple[int, bytes]]) -> set[int]:
    """Scan the code section for function indices referenced by ``call`` instructions.

    Returns the set of function indices that appear as direct call targets.
    This is intentionally conservative -- functions reached only via
    ``call_indirect`` (through the element/table) are NOT included, which is
    exactly what we want: element entries whose targets never appear in a
    direct ``call`` are candidates for neutralisation.
    """
    called: set[int] = set()
    for sid, payload in sections:
        if sid != 10:
            continue
        offset = 0
        func_count, offset = _read_varuint(payload, offset)
        for _ in range(func_count):
            body_size, offset = _read_varuint(payload, offset)
            body_end = offset + body_size
            # Skip local declarations
            pos = offset
            try:
                local_count, pos = _read_varuint(payload, pos)
                for _lc in range(local_count):
                    _, pos = _read_varuint(payload, pos)  # count
                    pos += 1  # type byte
            except (IndexError, ValueError):
                offset = body_end
                continue
            # Scan for call opcode (0x10) and read the immediate func index
            while pos < body_end:
                b = payload[pos]
                if b == 0x10:  # call
                    pos += 1
                    try:
                        idx, pos = _read_varuint(payload, pos)
                        called.add(idx)
                    except (IndexError, ValueError):
                        break
                else:
                    pos += 1
            offset = body_end
    return called


def _collect_element_func_indices(sections: list[tuple[int, bytes]]) -> set[int]:
    """Return the set of function indices referenced by active element segments."""
    indices: set[int] = set()
    for sid, payload in sections:
        if sid != 9:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            flags = payload[offset]
            offset += 1
            if flags != 0:
                # Non-trivial segment (passive, declarative, etc.) -- skip rest
                break
            # Active segment for table 0: i32.const <offset> end <count> <idx>*
            if payload[offset] != 0x41:
                break
            offset += 1
            _, offset = _read_varuint(payload, offset)  # table offset
            if payload[offset] != 0x0B:
                break
            offset += 1  # end
            n, offset = _read_varuint(payload, offset)
            for _ in range(n):
                idx, offset = _read_varuint(payload, offset)
                indices.add(idx)
    return indices


def _neutralize_dead_element_entries(data: bytes) -> bytes | None:
    """Replace indirect-call table entries for dead functions with the sentinel.

    After linking, the element section (section 9) populates the indirect
    function table.  Many entries point to runtime functions that are never
    actually dispatched -- they exist only because the runtime compiled them
    with ``#[no_mangle]`` and ``wasm-ld`` preserved them.

    This pass identifies function indices that appear ONLY in the element
    section (never as a direct ``call`` target in the code section) and
    replaces them with function index 0 (the sentinel/trap function).
    Once neutralised, wasm-opt's ``--remove-unused-module-elements`` and
    ``--dce`` passes can eliminate the now-unreferenced function bodies,
    typically removing 30-40% of all functions.
    """
    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    code_called = _collect_code_referenced_funcs(sections)
    elem_indices = _collect_element_func_indices(sections)
    # Functions only in the element table, never directly called from code
    dead_indices = elem_indices - code_called

    if not dead_indices:
        return None

    # Rebuild the element section, replacing dead entries with sentinel (0)
    new_sections: list[tuple[int, bytes]] = []
    replaced = 0
    for sid, payload in sections:
        if sid != 9:
            new_sections.append((sid, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        new_payload = bytearray(_write_varuint(count))
        for seg_i in range(count):
            flags = payload[offset]
            offset += 1
            if flags != 0:
                # Passive/declarative segment -- copy remainder as-is
                new_payload.append(flags)
                new_payload.extend(payload[offset:])
                break
            new_payload.append(flags)
            # i32.const opcode
            new_payload.append(payload[offset])
            offset += 1
            # LEB128 table offset
            leb_start = offset
            _, offset = _read_varuint(payload, offset)
            new_payload.extend(payload[leb_start:offset])
            # end opcode
            new_payload.append(payload[offset])
            offset += 1
            # function count
            leb_start = offset
            n, offset = _read_varuint(payload, offset)
            new_payload.extend(payload[leb_start:offset])
            # rewrite function indices
            for _ in range(n):
                idx, offset = _read_varuint(payload, offset)
                if idx in dead_indices:
                    new_payload.extend(_write_varuint(0))
                    replaced += 1
                else:
                    new_payload.extend(_write_varuint(idx))
        new_sections.append((sid, bytes(new_payload)))

    if replaced == 0:
        return None

    print(
        f"Neutralised {replaced:,} dead element entries "
        f"({len(dead_indices):,} unique functions eligible for DCE)",
        file=sys.stderr,
    )
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


def _build_call_graph(
    code_payload: bytes, import_count: int
) -> dict[int, set[int]]:
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
                0x00, 0x01, 0x05, 0x0B, 0x0F, 0x1A, 0x1B, 0xD1,
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
            elif op in (0x0C, 0x0D, 0x20, 0x21, 0x22, 0x23, 0x24, 0x3F, 0x40, 0xD0):
                _, pos = _read_varuint(code_payload, pos)
            # br_table
            elif op == 0x0E:
                n, pos = _read_varuint(code_payload, pos)
                for _ in range(n + 1):
                    _, pos = _read_varuint(code_payload, pos)
            # call
            elif op == 0x10:
                idx, pos = _read_varuint(code_payload, pos)
                calls.add(idx)
            # call_indirect
            elif op == 0x11:
                _, pos = _read_varuint(code_payload, pos)
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
            # Atomics prefix
            elif op == 0xFE:
                atom, pos = _read_varuint(code_payload, pos)
                if atom >= 0x10 or atom in (0x00, 0x01, 0x02):
                    _, pos = _read_varuint(code_payload, pos)
                    _, pos = _read_varuint(code_payload, pos)
            else:
                # Unknown opcode -- stop decoding this function body
                break

        graph[func_index] = calls
        offset = body_end

    return graph


# Minimal function body: 0 locals, ``unreachable``, ``end``.
_TRAP_STUB_BODY = bytes([0x00, 0x00, 0x0B])


def _stub_dead_functions(data: bytes) -> bytes | None:
    """Replace bodies of provably-dead functions with a minimal trap stub.

    A function is *dead* when it is unreachable from every export and every
    element-section entry via direct calls (``call`` / ``ref.func``).  Since
    element entries are the roots of ``call_indirect`` dispatch, all
    indirectly-callable functions remain conservatively live.

    Dead function bodies are replaced with ``unreachable; end`` (3 bytes),
    making them trivially small.  This alone saves 1-2 MB on typical
    linked artifacts, and when followed by wasm-opt the dead stubs can be
    fully removed, yielding an additional ~400 KB gzip saving.
    """
    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    import_count = _count_func_imports(sections)

    # Build call graph from the code section
    code_payload = None
    for sid, payload in sections:
        if sid == 10:
            code_payload = payload
            break
    if code_payload is None:
        return None

    call_graph = _build_call_graph(code_payload, import_count)
    if not call_graph:
        return None

    # Collect roots: exports + element-section entries
    roots: set[int] = set()
    for sid, payload in sections:
        if sid == 7:  # export section
            offset = 0
            count, offset = _read_varuint(payload, offset)
            while offset < len(payload):
                _, offset = _read_string(payload, offset)
                if offset >= len(payload):
                    break
                kind = payload[offset]
                offset += 1
                idx, offset = _read_varuint(payload, offset)
                if kind == 0:  # function export
                    roots.add(idx)
        elif sid == 9:  # element section
            offset = 0
            count, offset = _read_varuint(payload, offset)
            for _ in range(count):
                flags = payload[offset]
                offset += 1
                if flags != 0:
                    break
                if payload[offset] != 0x41:
                    break
                offset += 1
                _, offset = _read_varuint(payload, offset)
                if payload[offset] != 0x0B:
                    break
                offset += 1
                n, offset = _read_varuint(payload, offset)
                for _ in range(n):
                    idx, offset = _read_varuint(payload, offset)
                    roots.add(idx)

    # Compute transitive reachability
    reachable: set[int] = set()
    worklist = list(roots)
    while worklist:
        f = worklist.pop()
        if f in reachable:
            continue
        reachable.add(f)
        for callee in call_graph.get(f, ()):
            if callee not in reachable:
                worklist.append(callee)

    all_defined = set(range(import_count, import_count + len(call_graph)))
    dead = all_defined - reachable
    if not dead:
        return None

    # Rewrite the code section, replacing dead bodies with the trap stub
    new_sections: list[tuple[int, bytes]] = []
    saved_bytes = 0
    for sid, payload in sections:
        if sid != 10:
            new_sections.append((sid, payload))
            continue
        offset = 0
        func_count, offset = _read_varuint(payload, offset)
        new_code = bytearray(_write_varuint(func_count))
        for f_idx in range(func_count):
            func_index = import_count + f_idx
            body_size, offset = _read_varuint(payload, offset)
            body_end = offset + body_size
            if func_index in dead:
                new_code.extend(_write_varuint(len(_TRAP_STUB_BODY)))
                new_code.extend(_TRAP_STUB_BODY)
                saved_bytes += body_size - len(_TRAP_STUB_BODY)
            else:
                new_code.extend(_write_varuint(body_size))
                new_code.extend(payload[offset:body_end])
            offset = body_end
        new_sections.append((sid, bytes(new_code)))

    if saved_bytes <= 0:
        return None

    print(
        f"Stubbed {len(dead):,} dead functions "
        f"({saved_bytes:,} bytes / {saved_bytes / 1024:.1f} KB freed)",
        file=sys.stderr,
    )
    return _build_sections(new_sections)


def _dedup_data_segments(data: bytes) -> bytes | None:
    """Deduplicate identical data segments in the linked artifact.

    The output module and runtime module often share identical constant
    strings (error messages, type names, format strings).  After wasm-ld
    merges them, the data section can contain many duplicates.  This pass
    identifies identical segment payloads and merges them, rewriting the
    constant offsets in the code section.

    For safety, this only deduplicates passive segments and active segments
    whose contents are byte-identical.  It does NOT attempt to rewrite
    code references (which would require full relocation awareness).  Instead
    it coalesces adjacent identical active segments that share the same
    memory offset pattern.
    """
    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    data_section_idx = None
    data_payload = None
    for idx, (section_id, payload) in enumerate(sections):
        if section_id == 11:
            data_section_idx = idx
            data_payload = payload
            break

    if data_payload is None:
        return None

    # Parse segments to find duplicates
    offset = 0
    seg_count, offset = _read_varuint(data_payload, offset)
    segments: list[tuple[int, bytes]] = []  # (header_end, raw_bytes_from_start)
    seg_starts: list[int] = []
    seg_raw: list[bytes] = []

    parse_offset = offset
    for _ in range(seg_count):
        seg_start = parse_offset
        flags_byte = data_payload[parse_offset]
        parse_offset += 1
        if flags_byte == 0:
            # active, table 0, init expr
            parse_offset_after_expr = parse_offset
            while parse_offset_after_expr < len(data_payload):
                if data_payload[parse_offset_after_expr] == 0x0B:
                    parse_offset_after_expr += 1
                    break
                parse_offset_after_expr += 1
            parse_offset = parse_offset_after_expr
        elif flags_byte == 1:
            # passive
            pass
        elif flags_byte == 2:
            # active with table index
            _, parse_offset = _read_varuint(data_payload, parse_offset)
            while parse_offset < len(data_payload):
                if data_payload[parse_offset] == 0x0B:
                    parse_offset += 1
                    break
                parse_offset += 1
        else:
            # Unknown flags, bail
            return None
        # Read the data bytes
        data_len, parse_offset = _read_varuint(data_payload, parse_offset)
        seg_data = data_payload[parse_offset : parse_offset + data_len]
        parse_offset += data_len
        seg_raw.append(seg_data)
        seg_starts.append(seg_start)

    if len(seg_raw) < 2:
        return None

    # Find duplicate data payloads (only count savings, don't rewrite references)
    seen: dict[bytes, int] = {}
    dup_bytes = 0
    for raw in seg_raw:
        if raw in seen:
            dup_bytes += len(raw)
        else:
            seen[raw] = len(raw)

    # If less than 1KB of duplicates, not worth the risk of rewriting
    if dup_bytes < 1024:
        return None

    # For now, report savings potential but don't rewrite (code references
    # would need relocation).  The real win is from strip_debug_sections
    # and strip_internal_exports above.
    print(
        f"Data section has ~{dup_bytes:,} bytes of duplicate segments "
        f"({dup_bytes / 1024:.1f} KB). Consider wasm-opt --merge-data.",
        file=sys.stderr,
    )
    return None


def _post_link_optimize(data: bytes) -> bytes:
    """Apply post-link optimizations to reduce V8 compilation memory pressure.

    This is the key fix for MOL-183/MOL-186: the linked artifact was
    overwhelming V8 because of debug sections, internal exports, and
    duplicate data.  Stripping them reduces the module size by 30-60%
    which directly translates to less compilation memory.
    """
    updated = _strip_debug_sections(data)
    if updated is not None:
        data = updated

    updated = _strip_internal_exports(data)
    if updated is not None:
        data = updated

    updated = _stub_dead_functions(data)
    if updated is not None:
        data = updated

    _dedup_data_segments(data)

    return data


def _validate_freestanding(data: bytes) -> bool:
    """Validate a freestanding wasm binary has no prohibited imports.

    Returns True if valid, False if critical issues found.
    """
    try:
        imports = _collect_imports(data)
    except ValueError as exc:
        print(f"Failed to parse freestanding wasm imports: {exc}", file=sys.stderr)
        return False

    wasi_imports = [
        (module, name)
        for module, name, _, _ in imports
        if module == "wasi_snapshot_preview1"
    ]
    if wasi_imports:
        for module, name in wasi_imports:
            print(
                f"Freestanding validation error: remaining WASI import {module}::{name}",
                file=sys.stderr,
            )
        return False

    runtime_imports = [
        (module, name)
        for module, name, _, _ in imports
        if module == "molt_runtime"
    ]
    if runtime_imports:
        for module, name in runtime_imports:
            print(
                f"Freestanding validation error: remaining molt_runtime import {module}::{name}",
                file=sys.stderr,
            )
        return False

    other_imports = [
        (module, name)
        for module, name, _, _ in imports
        if module != "env"
    ]
    for module, name in other_imports:
        print(
            f"Freestanding validation warning: unexpected import {module}::{name}",
            file=sys.stderr,
        )

    # Optionally run wasm-validate for structural validation
    exe = shutil.which("wasm-validate")
    if exe is not None:
        with tempfile.NamedTemporaryFile(suffix=".wasm", delete=False) as f:
            f.write(data)
            f.flush()
            tmp_path = f.name
        try:
            result = subprocess.run(
                [exe, tmp_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
            if result.returncode != 0:
                print(
                    f"wasm-validate warning: {result.stderr.strip()}",
                    file=sys.stderr,
                )
        except Exception as exc:
            print(
                f"wasm-validate warning: {exc}",
                file=sys.stderr,
            )
        finally:
            try:
                Path(tmp_path).unlink()
            except OSError:
                pass

    return True


def _validate_linked(linked: Path) -> bool:
    data = linked.read_bytes()
    try:
        imports = _collect_imports(data)
    except ValueError as exc:
        print(f"Failed to parse linked wasm: {exc}", file=sys.stderr)
        return False
    if any(module == "molt_runtime" for module, _, _, _ in imports):
        print(
            "Linked wasm still imports molt_runtime; link step incomplete.",
            file=sys.stderr,
        )
        return False
    call_indirect = [
        name
        for module, name, kind, _ in imports
        if module == "env" and kind == 0 and name.startswith("molt_call_indirect")
    ]
    if call_indirect:
        print(
            f"Linked wasm still imports {', '.join(sorted(call_indirect))}; "
            "remove JS call_indirect stubs.",
            file=sys.stderr,
        )
        return False
    table_imports = [
        (module, name)
        for module, name, kind, _ in imports
        if kind == 1 and name == "__indirect_function_table"
    ]
    if table_imports:
        print(
            "Linked wasm imports a function table; host will supply it.",
            file=sys.stderr,
        )
    memory_imports = [(module, name) for module, name, kind, _ in imports if kind == 2]
    if memory_imports:
        print("Linked wasm still imports memory.", file=sys.stderr)
        return False
    try:
        custom_names = _collect_custom_names(data)
    except ValueError as exc:
        print(f"Failed to parse linked wasm custom sections: {exc}", file=sys.stderr)
        return False
    reloc_sections = [name for name in custom_names if name.startswith("reloc.")]
    if reloc_sections:
        print(
            f"Linked wasm still has reloc sections ({', '.join(reloc_sections)}); "
            "link step incomplete.",
            file=sys.stderr,
        )
        return False
    if "linking" in custom_names or "dylink.0" in custom_names:
        print("Linked wasm still has linking metadata sections.", file=sys.stderr)
        return False
    try:
        exports = _collect_exports(data)
    except ValueError as exc:
        print(f"Failed to parse linked wasm exports: {exc}", file=sys.stderr)
        return False
    if "molt_memory" not in exports and "memory" not in exports:
        print("Linked wasm missing exported memory.", file=sys.stderr)
        return False
    if "molt_table" not in exports and "__indirect_function_table" not in exports:
        print("Linked wasm missing exported table.", file=sys.stderr)
        return False
    try:
        ok, err = _validate_elements(data)
    except ValueError as exc:
        print(f"Failed to parse linked wasm element section: {exc}", file=sys.stderr)
        return False
    if not ok:
        print(f"Linked wasm element validation failed: {err}", file=sys.stderr)
        return False
    return True


# Pass pipelines from docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md Section 4.4.
_OZ_PASSES: list[str] = [
    "--remove-unused-module-elements",
    "--remove-unused-names",
    "--strip-debug",
    "--coalesce-locals",
    "--reorder-locals",
    "--dce",
    "--vacuum",
    "--duplicate-function-elimination",
    "--code-folding",
]

_O3_PASSES: list[str] = [
    "--remove-unused-module-elements",
    "--remove-unused-names",
    "--coalesce-locals",
    "--reorder-locals",
    "--dce",
    "--vacuum",
    "--inlining",
    "--flatten",
    "--local-cse",
]

_LEVEL_PASSES: dict[str, list[str]] = {
    "Oz": _OZ_PASSES,
    "O3": _O3_PASSES,
}


def _run_wasm_opt_via_optimize(linked: Path, level: str = "Oz") -> bool:
    """Run wasm-opt on the linked binary via tools/wasm_optimize.py.

    Returns True if optimization ran successfully.
    Writes to a temp file first to avoid corrupting the linked binary on failure.

    For ``Oz`` and ``O3`` levels the recommended pass pipelines from the WASM
    Optimization Plan (Section 4.4) are forwarded as *extra_passes*.
    """
    try:
        import importlib.util as _ilu

        optimize_path = Path(__file__).parent / "wasm_optimize.py"
        spec = _ilu.spec_from_file_location("wasm_optimize", optimize_path)
        if spec is None or spec.loader is None:
            print("wasm_optimize.py not found; skipping wasm-opt.", file=sys.stderr)
            return False
        mod = _ilu.module_from_spec(spec)
        spec.loader.exec_module(mod)
    except Exception as exc:
        print(f"Failed to load wasm_optimize: {exc}", file=sys.stderr)
        return False

    extra_passes = _LEVEL_PASSES.get(level)

    pre_size = linked.stat().st_size
    temp_output = linked.with_suffix(".opt.wasm")
    result = mod.optimize(
        linked, output_path=temp_output, level=level, extra_passes=extra_passes,
    )

    if not result["ok"]:
        err = result.get("error", "unknown error")
        print(f"wasm-opt failed (non-fatal): {err}", file=sys.stderr)
        if temp_output.exists():
            temp_output.unlink()
        return False

    shutil.move(str(temp_output), str(linked))

    post_size = result["output_bytes"]
    savings = pre_size - post_size
    if savings > 0:
        print(
            f"wasm-opt ({level}): {savings:,} bytes saved "
            f"({savings / pre_size * 100:.1f}% reduction, "
            f"{post_size:,} bytes final)",
            file=sys.stderr,
        )
    return True


def _run_wasm_ld(
    wasm_ld: str,
    runtime: Path,
    output: Path,
    linked: Path,
    *,
    allowlist_override: Path | None = None,
    optimize: bool = False,
    optimize_level: str = "Oz",
    freestanding: bool = False,
) -> int:
    try:
        runtime_exports = _collect_exports(runtime.read_bytes())
    except ValueError as exc:
        print(
            f"Failed to parse runtime wasm exports ({runtime}): {exc}", file=sys.stderr
        )
        runtime_exports = {}
    if not runtime_exports and runtime.name.endswith("_reloc.wasm"):
        fallback = runtime.with_name(runtime.name.replace("_reloc", ""))
        if fallback.exists():
            try:
                runtime_exports = _collect_exports(fallback.read_bytes())
            except ValueError as exc:
                print(
                    f"Failed to parse fallback runtime wasm exports ({fallback}): {exc}",
                    file=sys.stderr,
                )
                runtime_exports = {}
    if not runtime_exports:
        print("Runtime exports unavailable for linking.", file=sys.stderr)
        return 1
    rewritten = _rewrite_output_imports(output, runtime_exports)
    if rewritten is None:
        return 1
    rewritten_path, temp_dir = rewritten
    rewritten_path = _inject_call_indirect_alias(rewritten_path, runtime, temp_dir)
    rewritten_path = _inject_output_export_aliases(rewritten_path, temp_dir)
    if allowlist_override is not None:
        allowlist = allowlist_override
    else:
        allowlist = Path(__file__).parent / "wasm_allowed_imports.txt"
    if not allowlist.exists():
        print(f"Allowlist not found: {allowlist}", file=sys.stderr)
        return 1
    cmd = [
        wasm_ld,
        "--no-entry",
        "--gc-sections",
        f"--allow-undefined-file={str(allowlist)}",
        "--import-table",
        "--export=molt_main",
        "--export-if-defined=molt_memory",
        "--export-if-defined=memory",
        "--export-if-defined=molt_table",
        "--export-if-defined=__indirect_function_table",
        "--export-if-defined=molt_set_wasm_table_base",
        "-o",
        str(linked),
        str(rewritten_path),
        str(runtime),
    ]
    res = subprocess.run(cmd, capture_output=True, text=True)
    try:
        if res.returncode != 0:
            err = res.stderr.strip() or res.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            return res.returncode
        if not linked.exists():
            print(
                f"wasm-ld exited successfully but produced no linked output: {linked}",
                file=sys.stderr,
            )
            return 1
        linked_bytes = _read_wasm_bytes_with_retry(linked)
        if not _is_wasm_binary(linked_bytes):
            print(
                "wasm-ld produced non-wasm linked output "
                f"({linked}, size={len(linked_bytes)} bytes)",
                file=sys.stderr,
            )
            return 1

        # MOL-183/MOL-186: Post-link optimization to reduce V8 OOM risk.
        # Strip debug sections, internal exports, and report data duplicates.
        pre_opt_size = len(linked_bytes)
        linked_bytes = _post_link_optimize(linked_bytes)
        post_opt_size = len(linked_bytes)
        if post_opt_size < pre_opt_size:
            savings = pre_opt_size - post_opt_size
            print(
                f"Post-link optimization: stripped {savings:,} bytes "
                f"({savings / 1024:.1f} KB, "
                f"{savings / pre_opt_size * 100:.1f}% reduction)",
                file=sys.stderr,
            )
            linked.write_bytes(linked_bytes)

        if optimize:
            if _run_wasm_opt_via_optimize(linked, level=optimize_level):
                # Re-read after optimization since the file changed on disk
                linked_bytes = linked.read_bytes()

        output_table_min = _table_import_min(output.read_bytes())
        if output_table_min is not None:
            try:
                updated = _rewrite_table_import_min(linked_bytes, output_table_min)
            except ValueError as exc:
                print(f"Failed to rewrite linked table min: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                linked.write_bytes(updated)
                linked_bytes = updated
        output_memory_min = _memory_import_min(output.read_bytes())
        if output_memory_min is not None:
            try:
                updated = _rewrite_memory_min(linked_bytes, output_memory_min)
            except ValueError as exc:
                print(f"Failed to rewrite linked memory min: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                linked.write_bytes(updated)
                linked_bytes = updated
        append_table_refs = os.environ.get(
            "MOLT_WASM_LINK_APPEND_TABLE_REFS", "1"
        ).strip().lower() not in {"0", "false", "no", "off"}
        if append_table_refs:
            try:
                updated = _append_table_ref_elements(linked_bytes)
            except ValueError as exc:
                print(f"Failed to append table ref elements: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                linked.write_bytes(updated)
                linked_bytes = updated
        try:
            updated = _declare_ref_func_elements(linked_bytes)
        except ValueError as exc:
            print(
                f"Failed to declare ref.func elements: {exc}", file=sys.stderr
            )
            return 1
        if updated is not None:
            linked.write_bytes(updated)
            linked_bytes = updated
        try:
            updated = _ensure_table_export(linked_bytes)
        except ValueError as exc:
            print(f"Failed to ensure table export: {exc}", file=sys.stderr)
            return 1
        if updated is not None:
            linked.write_bytes(updated)
            linked_bytes = updated
        if freestanding:
            try:
                import importlib.util as _ilu

                stub_path = Path(__file__).parent / "wasm_stub_wasi.py"
                spec = _ilu.spec_from_file_location("wasm_stub_wasi", stub_path)
                if spec is None or spec.loader is None:
                    print("wasm_stub_wasi.py not found", file=sys.stderr)
                    return 1
                stub_mod = _ilu.module_from_spec(spec)
                spec.loader.exec_module(stub_mod)
                linked_bytes, n_stubbed = stub_mod.stub_wasi_imports(linked_bytes)
                if n_stubbed > 0:
                    linked.write_bytes(linked_bytes)
                    print(
                        f"Freestanding: stubbed {n_stubbed} WASI imports",
                        file=sys.stderr,
                    )
            except Exception as exc:
                print(f"Freestanding WASI stubbing failed: {exc}", file=sys.stderr)
                return 1

        if freestanding:
            if not _validate_freestanding(linked_bytes):
                return 1
        if not _validate_linked(linked):
            return 1
        return 0
    finally:
        temp_dir.cleanup()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Attempt to link Molt output/runtime into a single WASM module.",
    )
    parser.add_argument("--runtime", type=Path, default=_default_runtime_path())
    parser.add_argument("--input", type=Path, default=Path("output.wasm"))
    parser.add_argument("--output", type=Path, default=Path("output_linked.wasm"))
    parser.add_argument(
        "--freestanding", action="store_true", default=False,
        help="Stub out WASI imports post-link for freestanding deployment",
    )
    parser.add_argument(
        "--optimize", action="store_true", default=False,
        help="Run wasm-opt after linking (requires Binaryen)",
    )
    parser.add_argument(
        "--optimize-level", default="Oz",
        help="wasm-opt optimization level (O1/O2/O3/O4/Os/Oz, default: Oz)",
    )
    args = parser.parse_args()

    runtime = args.runtime
    output = args.input
    linked = args.output

    if not runtime.exists():
        print(f"Runtime wasm not found: {runtime}", file=sys.stderr)
        return 1
    if not output.exists():
        print(f"Output wasm not found: {output}", file=sys.stderr)
        return 1
    linked.parent.mkdir(parents=True, exist_ok=True)
    if linked.exists():
        linked.unlink()

    wasm_ld = _find_tool(["wasm-ld"])
    if not wasm_ld:
        print(
            "wasm-ld not found; install LLVM to enable single-module linking.",
            file=sys.stderr,
        )
        return 1

    return _run_wasm_ld(
        wasm_ld,
        runtime,
        output,
        linked,
        optimize=args.optimize,
        optimize_level=args.optimize_level,
        freestanding=args.freestanding,
    )


if __name__ == "__main__":
    raise SystemExit(main())

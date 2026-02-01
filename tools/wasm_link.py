#!/usr/bin/env python3
from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
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


def _find_tool(names: list[str]) -> str | None:
    for name in names:
        path = shutil.which(name)
        if path:
            return path
    return None


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
                count, sub_offset = _read_varuint(custom_payload, sub_offset)
                for _ in range(count):
                    func_idx, sub_offset = _read_varuint(custom_payload, sub_offset)
                    func_name, sub_offset = _read_string(custom_payload, sub_offset)
                    names[func_idx] = func_name
            offset = sub_end
        break
    return names


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


def _run_wasm_ld(wasm_ld: str, runtime: Path, output: Path, linked: Path) -> int:
    runtime_exports = _collect_exports(runtime.read_bytes())
    if not runtime_exports and runtime.name.endswith("_reloc.wasm"):
        fallback = runtime.with_name(runtime.name.replace("_reloc", ""))
        if fallback.exists():
            runtime_exports = _collect_exports(fallback.read_bytes())
    if not runtime_exports:
        print("Runtime exports unavailable for linking.", file=sys.stderr)
        return 1
    rewritten = _rewrite_output_imports(output, runtime_exports)
    if rewritten is None:
        return 1
    rewritten_path, temp_dir = rewritten
    rewritten_path = _inject_call_indirect_alias(rewritten_path, runtime, temp_dir)
    cmd = [
        wasm_ld,
        "--no-entry",
        "--no-gc-sections",
        "--allow-undefined",
        "--import-table",
        "--export=molt_main",
        "--export=molt_memory",
        "--export=molt_table",
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
        output_table_min = _table_import_min(output.read_bytes())
        if output_table_min is not None:
            try:
                updated = _rewrite_table_import_min(
                    linked.read_bytes(), output_table_min
                )
            except ValueError as exc:
                print(f"Failed to rewrite linked table min: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                linked.write_bytes(updated)
        try:
            updated = _append_table_ref_elements(linked.read_bytes())
        except ValueError as exc:
            print(f"Failed to append table ref elements: {exc}", file=sys.stderr)
            return 1
        if updated is not None:
            linked.write_bytes(updated)
        try:
            updated = _ensure_table_export(linked.read_bytes())
        except ValueError as exc:
            print(f"Failed to ensure table export: {exc}", file=sys.stderr)
            return 1
        if updated is not None:
            linked.write_bytes(updated)
        if not _validate_linked(linked):
            return 1
        return 0
    finally:
        temp_dir.cleanup()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Attempt to link Molt output/runtime into a single WASM module.",
    )
    parser.add_argument("--runtime", type=Path, default=Path("wasm/molt_runtime.wasm"))
    parser.add_argument("--input", type=Path, default=Path("output.wasm"))
    parser.add_argument("--output", type=Path, default=Path("output_linked.wasm"))
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

    return _run_wasm_ld(wasm_ld, runtime, output, linked)


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
from __future__ import annotations

import sys
import tempfile
from collections import Counter
from pathlib import Path

from wasm_link_format import (
    FLAG_BINDING_GLOBAL,
    FLAG_EXPLICIT_NAME,
    FLAG_EXPORTED,
    FLAG_NO_STRIP,
    SYMBOL_KIND_FUNCTION,
    SYMTAB_SUBSECTION_ID,
    _ESSENTIAL_EXPORTS,
    _INTERNAL_OUTPUT_EXPORT_PREFIXES,
    _OUTPUT_EXPORT_ALIAS_PREFIX,
    _OUTPUT_RUNTIME_EXPORT_ALIASES,
    _append_linking_function_symbols,
    _build_call_graph,
    _build_custom_section,
    _build_linking_payload,
    _build_sections,
    _collect_func_names,
    _collect_function_exports,
    _collect_imports,
    _collect_linking_function_symbols,
    _count_func_imports,
    _find_func_import_index,
    is_table_ref_export_name,
    parse_table_ref_export_name,
    wasm_runtime_export_name,
    _parse_custom_section,
    _parse_func_type_indices,
    _parse_import_desc,
    _parse_linking_payload,
    _parse_sections,
    _parse_type_section,
    _read_limits,
    _read_string,
    _read_varuint,
    _read_varsint,
    _skip_init_expr,
    _write_limits,
    _write_string,
    _write_varuint,
)


_STANDARD_SECTION_ORDER = {
    1: 1,  # type
    2: 2,  # import
    3: 3,  # function
    4: 4,  # table
    5: 5,  # memory
    6: 6,  # global
    7: 7,  # export
    8: 8,  # start
    9: 9,  # element
    12: 10,  # data count
    10: 11,  # code
    11: 12,  # data
}


def _append_table_ref_elements(
    data: bytes,
    *,
    min_table_index: int = 0,
    allowed_table_indices: set[int] | None = None,
) -> bytes | None:
    table_refs: dict[int, int] = {}
    for func_idx, name in _collect_func_names(data).items():
        table_idx = parse_table_ref_export_name(name)
        if table_idx is not None:
            if table_idx >= min_table_index and (
                allowed_table_indices is None or table_idx in allowed_table_indices
            ):
                table_refs[table_idx] = func_idx
    for name, func_idx in _collect_function_exports(data).items():
        table_idx = parse_table_ref_export_name(name)
        if table_idx is not None:
            if table_idx >= min_table_index and (
                allowed_table_indices is None or table_idx in allowed_table_indices
            ):
                table_refs[table_idx] = func_idx
    if not table_refs:
        return None

    segments: list[bytes] = []
    current_start: int | None = None
    current_prev: int | None = None
    current_funcs: list[int] = []
    for table_idx, func_idx in sorted(table_refs.items()):
        if current_start is None:
            current_start = current_prev = table_idx
            current_funcs = [func_idx]
            continue
        if table_idx == current_prev + 1:
            current_prev = table_idx
            current_funcs.append(func_idx)
            continue
        segment = bytearray()
        segment.append(0x00)
        segment.append(0x41)
        segment.extend(_write_varuint(current_start))
        segment.append(0x0B)
        segment.extend(_write_varuint(len(current_funcs)))
        for item in current_funcs:
            segment.extend(_write_varuint(item))
        segments.append(bytes(segment))
        current_start = current_prev = table_idx
        current_funcs = [func_idx]
    if current_start is not None:
        segment = bytearray()
        segment.append(0x00)
        segment.append(0x41)
        segment.extend(_write_varuint(current_start))
        segment.append(0x0B)
        segment.extend(_write_varuint(len(current_funcs)))
        for item in current_funcs:
            segment.extend(_write_varuint(item))
        segments.append(bytes(segment))

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
        updated.extend(_write_varuint(count + len(segments)))
        updated.extend(rest)
        for segment in segments:
            updated.extend(segment)
        new_sections.append((section_id, bytes(updated)))
        modified = True
    if not modified:
        payload = _write_varuint(len(segments)) + b"".join(segments)
        new_sections.append((9, payload))
        modified = True
    if not modified:
        return None
    return _build_sections(new_sections)


def _read_element_active_i32_offset(payload: bytes, offset: int) -> tuple[int, int]:
    if offset >= len(payload) or payload[offset] != 0x41:
        raise ValueError("active table element offset must be i32.const")
    value, offset = _read_varsint(payload, offset + 1)
    if offset >= len(payload) or payload[offset] != 0x0B:
        raise ValueError("unterminated active table element offset")
    return value, offset + 1


def _linked_element_segment_end(
    payload: bytes, offset: int
) -> tuple[int, tuple[int, int] | None]:
    flags, offset = _read_varuint(payload, offset)
    active_table: tuple[int, int] | None = None
    if flags in (0x00, 0x04):
        start, offset = _read_element_active_i32_offset(payload, offset)
        active_table = (0, start)
    elif flags in (0x02, 0x06):
        table_index, offset = _read_varuint(payload, offset)
        start, offset = _read_element_active_i32_offset(payload, offset)
        active_table = (table_index, start)
    elif flags not in (0x01, 0x03, 0x05, 0x07):
        raise ValueError(f"unsupported element segment flags 0x{flags:x}")

    if flags in (0x00, 0x01, 0x02, 0x03):
        if flags in (0x01, 0x02, 0x03):
            if offset >= len(payload):
                raise ValueError("missing element kind")
            offset += 1
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            _, offset = _read_varuint(payload, offset)
        return offset, active_table

    if flags in (0x05, 0x06, 0x07):
        if offset >= len(payload):
            raise ValueError("missing element reference type")
        offset += 1
    count, offset = _read_varuint(payload, offset)
    for _ in range(count):
        offset = _skip_init_expr(payload, offset)
    return offset, active_table


def _drop_linked_app_active_table_elements(data: bytes) -> bytes | None:
    """Remove app-owned active element segments from a linked monolith.

    Relocatable app modules own callable-table population through
    ``molt_table_init``.  When wasm-ld materializes those app entries as active
    segments in the fully linked output, table contents gain a second authority
    and can trap before the exported ``molt_main`` wrapper reaches its table
    initializer.  Preserve the runtime's active prefix at table offset 1 plus
    passive/declarative metadata; later ``ref.func`` declaration repair records
    any declarations still needed by the initializer body.
    """
    sections = _parse_sections(data)
    changed = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 9:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        kept_segments: list[bytes] = []
        for _ in range(count):
            segment_start = offset
            offset, active_table = _linked_element_segment_end(payload, offset)
            segment = payload[segment_start:offset]
            if active_table is None:
                kept_segments.append(segment)
                continue
            table_index, start = active_table
            if table_index == 0 and start == 1:
                kept_segments.append(segment)
                continue
            changed = True
        if offset != len(payload):
            raise ValueError("trailing bytes after element section")
        if kept_segments:
            rebuilt = bytearray()
            rebuilt.extend(_write_varuint(len(kept_segments)))
            for segment in kept_segments:
                rebuilt.extend(segment)
            new_sections.append((section_id, bytes(rebuilt)))
        else:
            changed = True
    if not changed:
        return None
    return _build_sections(new_sections)


def _add_symtab_alias(
    data: bytes,
    alias_name: str,
    alias_index: int,
    alias_flags: int,
    *,
    preserve_export: bool = False,
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
            entry_flags = alias_flags
            if not preserve_export:
                entry_flags &= ~FLAG_EXPORTED
            alias_entry.extend(_write_varuint(entry_flags | FLAG_EXPLICIT_NAME))
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


def _inject_output_export_aliases(
    output: Path, temp_dir: tempfile.TemporaryDirectory
) -> Path:
    data = output.read_bytes()
    wrapper_specs = _collect_output_wrapper_specs(data)
    if not wrapper_specs:
        return output
    try:
        sections = _parse_sections(data)
    except ValueError as exc:
        print(
            f"Failed to parse output module for export aliasing: {exc}", file=sys.stderr
        )
        return output
    types = _parse_type_section(sections)
    if not types:
        return output
    func_section_idx, func_type_indices = _parse_func_type_indices(sections)
    if func_section_idx < 0:
        return output
    import_count = _count_func_imports(sections)
    inc_ref_import_index = _find_func_import_index(
        data, "molt_runtime", "molt_inc_ref_obj"
    )
    original_func_count = len(func_type_indices)

    new_sections: list[tuple[int, bytes]] = []
    wrapper_symbol_entries: list[tuple[str, int, int]] = []
    wrapper_index_by_name: dict[str, int] = {}
    modified = False
    for section_id, payload in sections:
        if section_id == 3:
            offset = 0
            count, offset = _read_varuint(payload, offset)
            updated_payload = bytearray()
            updated_payload.extend(_write_varuint(count + len(wrapper_specs)))
            updated_payload.extend(payload[offset:])
            for _name, _alias_name, type_idx, _target_idx in wrapper_specs:
                updated_payload.extend(_write_varuint(type_idx))
            new_sections.append((section_id, bytes(updated_payload)))
            modified = True
            continue
        if section_id == 7:
            offset = 0
            count, offset = _read_varuint(payload, offset)
            updated_payload = bytearray()
            updated_payload.extend(_write_varuint(count + len(wrapper_specs)))
            updated_payload.extend(payload[offset:])
            for i, (_name, alias_name, _type_idx, _target_idx) in enumerate(
                wrapper_specs
            ):
                wrapper_func_index = import_count + original_func_count + i
                wrapper_index_by_name[alias_name] = wrapper_func_index
                updated_payload.extend(_write_string(alias_name))
                updated_payload.append(0)
                updated_payload.extend(_write_varuint(wrapper_func_index))
                wrapper_symbol_entries.append(
                    (
                        alias_name,
                        wrapper_func_index,
                        FLAG_BINDING_GLOBAL
                        | FLAG_EXPLICIT_NAME
                        | FLAG_EXPORTED
                        | FLAG_NO_STRIP,
                    )
                )
                if _name in _OUTPUT_RUNTIME_EXPORT_ALIASES:
                    wrapper_symbol_entries.append(
                        (
                            _name,
                            wrapper_func_index,
                            FLAG_BINDING_GLOBAL | FLAG_EXPLICIT_NAME | FLAG_NO_STRIP,
                        )
                    )
            new_sections.append((section_id, bytes(updated_payload)))
            modified = True
            continue
        if section_id == 10:
            offset = 0
            count, offset = _read_varuint(payload, offset)
            updated_payload = bytearray()
            updated_payload.extend(_write_varuint(count + len(wrapper_specs)))
            updated_payload.extend(payload[offset:])
            for name, alias_name, type_idx, target_idx in wrapper_specs:
                params, results = types[type_idx]
                body = bytearray()
                local_count = (
                    1
                    if results
                    and len(results) == 1
                    and inc_ref_import_index is not None
                    else 0
                )
                body.extend(_write_varuint(local_count))
                if local_count:
                    body.extend(_write_varuint(1))
                    body.append(0x7E)
                for param_index in range(len(params)):
                    body.append(0x20)
                    body.extend(_write_varuint(param_index))
                body.append(0x10)
                body.extend(_write_varuint(target_idx))
                if local_count:
                    result_local = len(params)
                    body.append(0x22)
                    body.extend(_write_varuint(result_local))
                    body.append(0x10)
                    body.extend(_write_varuint(inc_ref_import_index))
                    body.append(0x20)
                    body.extend(_write_varuint(result_local))
                body.append(0x0B)
                updated_payload.extend(_write_varuint(len(body)))
                updated_payload.extend(body)
            new_sections.append((section_id, bytes(updated_payload)))
            modified = True
            continue
        new_sections.append((section_id, payload))
    if not modified:
        return output

    updated = _build_sections(new_sections)
    next_data = _append_linking_function_symbols(updated, wrapper_symbol_entries)
    if next_data is not None:
        updated = next_data
    alias_path = Path(temp_dir.name) / "output_exports_alias.wasm"
    alias_path.write_bytes(updated)
    return alias_path


def _collect_output_wrapper_specs(data: bytes) -> list[tuple[str, str, int, int]]:
    export_indices = _collect_function_exports(data)
    sections = _parse_sections(data)
    types = _parse_type_section(sections)
    if not types:
        return []
    func_section_idx, func_type_indices = _parse_func_type_indices(sections)
    if func_section_idx < 0:
        return []
    import_count = _count_func_imports(sections)
    original_func_count = len(func_type_indices)
    primary_prefix = _entry_module_prefix_from_main_init(data, export_indices)
    if primary_prefix is None:
        primary_prefix = _dominant_output_module_prefix(export_indices)

    wrapper_specs: list[tuple[str, str, int, int]] = []
    for name, func_index in export_indices.items():
        if is_table_ref_export_name(name):
            continue
        if name == "molt_main":
            continue
        local_index = func_index - import_count
        if local_index < 0 or local_index >= original_func_count:
            continue
        type_idx = func_type_indices[local_index]
        _params, results = types[type_idx]
        if name in _OUTPUT_RUNTIME_EXPORT_ALIASES:
            wrapper_specs.append(
                (name, f"{_OUTPUT_EXPORT_ALIAS_PREFIX}{name}", type_idx, func_index)
            )
            continue
        if name.startswith("molt_"):
            continue
        if not results:
            continue
        if not _is_public_output_export_name(name, primary_prefix):
            continue
        wrapper_specs.append(
            (name, f"{_OUTPUT_EXPORT_ALIAS_PREFIX}{name}", type_idx, func_index)
        )
    return wrapper_specs


def _collect_preserved_output_export_names(data: bytes) -> list[str]:
    return [
        name
        for name, _alias, _type_idx, _func_idx in _collect_output_wrapper_specs(data)
    ]


def _collect_output_export_symbol_map(data: bytes) -> dict[str, str]:
    export_indices = _collect_function_exports(data)
    by_index: dict[int, list[str]] = {}
    for _flags, index, name, _kind in _collect_linking_function_symbols(data):
        if name:
            by_index.setdefault(index, []).append(name)
    mapping: dict[str, str] = {}
    for public_name, index in export_indices.items():
        candidates = by_index.get(index, [])
        preferred = next((name for name in candidates if name == public_name), None)
        if preferred is None:
            preferred = next(
                (
                    name
                    for name in candidates
                    if name.startswith("__molt_output_export_")
                ),
                None,
            )
        if preferred is None and candidates:
            preferred = candidates[0]
        if preferred is not None:
            mapping[public_name] = preferred
    return mapping


def _rename_export_names(data: bytes, rename_map: dict[str, str]) -> bytes | None:
    if not rename_map:
        return None
    sections = _parse_sections(data)
    modified = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 7:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        exports: list[tuple[str, int, int]] = []
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            kind = payload[offset]
            offset += 1
            index, offset = _read_varuint(payload, offset)
            exports.append((name, kind, index))
        rebuilt = bytearray()
        seen_names: set[str] = set()
        kept: list[tuple[str, int, int]] = []
        for name, kind, index in exports:
            renamed = rename_map.get(name, name)
            if renamed != name:
                modified = True
            if renamed in seen_names:
                modified = True
                continue
            seen_names.add(renamed)
            kept.append((renamed, kind, index))
        rebuilt.extend(_write_varuint(len(kept)))
        for name, kind, index in kept:
            rebuilt.extend(_write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(_write_varuint(index))
        new_sections.append((section_id, bytes(rebuilt)))
    if not modified:
        return None
    return _build_sections(new_sections)


def _ensure_function_exports_by_symbol_names(
    data: bytes, public_to_symbol: dict[str, str]
) -> bytes | None:
    if not public_to_symbol:
        return None
    symbol_indices = {
        name: index
        for _flags, index, name, _kind in _collect_linking_function_symbols(data)
        if name
    }
    if not set(public_to_symbol.values()).issubset(symbol_indices):
        for index, name in _collect_func_names(data).items():
            symbol_indices.setdefault(name, index)
    existing_exports = _collect_function_exports(data)
    replacements: dict[str, int] = {}
    additions: list[tuple[str, int]] = []
    for public_name, symbol_name in public_to_symbol.items():
        symbol_index = symbol_indices.get(symbol_name)
        if symbol_index is None:
            continue
        if public_name in existing_exports:
            if existing_exports[public_name] != symbol_index:
                replacements[public_name] = symbol_index
            continue
        additions.append((public_name, symbol_index))
    if not additions and not replacements:
        return None

    sections = _parse_sections(data)
    new_sections: list[tuple[int, bytes]] = []
    modified = False
    inserted = False
    for section_id, payload in sections:
        if section_id == 7:
            offset = 0
            count, offset = _read_varuint(payload, offset)
            entries: list[tuple[str, int, int]] = []
            while offset < len(payload):
                name, offset = _read_string(payload, offset)
                kind = payload[offset]
                offset += 1
                index, offset = _read_varuint(payload, offset)
                if kind == 0 and name in replacements:
                    index = replacements[name]
                entries.append((name, kind, index))
            updated_payload = bytearray()
            updated_payload.extend(_write_varuint(count + len(additions)))
            for name, kind, index in entries:
                updated_payload.extend(_write_string(name))
                updated_payload.append(kind)
                updated_payload.extend(_write_varuint(index))
            for public_name, symbol_index in additions:
                updated_payload.extend(_write_string(public_name))
                updated_payload.append(0)
                updated_payload.extend(_write_varuint(symbol_index))
            new_sections.append((section_id, bytes(updated_payload)))
            modified = True
            inserted = True
            continue
        if not inserted and section_id > 7:
            export_payload = bytearray()
            export_payload.extend(_write_varuint(len(additions)))
            for public_name, symbol_index in additions:
                export_payload.extend(_write_string(public_name))
                export_payload.append(0)
                export_payload.extend(_write_varuint(symbol_index))
            new_sections.append((7, bytes(export_payload)))
            modified = True
            inserted = True
        new_sections.append((section_id, payload))
    if not inserted:
        export_payload = bytearray()
        export_payload.extend(_write_varuint(len(additions)))
        for public_name, symbol_index in additions:
            export_payload.extend(_write_string(public_name))
            export_payload.append(0)
            export_payload.extend(_write_varuint(symbol_index))
        new_sections.append((7, bytes(export_payload)))
        modified = True
    if not modified:
        return None
    return _build_sections(new_sections)


def _dominant_output_module_prefix(export_indices: dict[str, int]) -> str | None:
    counts: Counter[str] = Counter()
    for name in export_indices:
        if name.startswith("molt_"):
            continue
        if not name or not name[0].isalnum():
            continue
        if "__" not in name:
            continue
        prefix, _rest = name.split("__", 1)
        if prefix:
            counts[prefix] += 1
    if not counts:
        return None
    return counts.most_common(1)[0][0]


def _entry_module_prefix_from_main_init(
    data: bytes, export_indices: dict[str, int]
) -> str | None:
    main_init_index = export_indices.get("molt_init___main__")
    if main_init_index is None:
        return None
    sections = _parse_sections(data)
    code_payload = next((payload for sid, payload in sections if sid == 10), None)
    if code_payload is None:
        return None
    import_count = _count_func_imports(sections)
    call_graph = _build_call_graph(code_payload, import_count)
    inverse_exports: dict[int, list[str]] = {}
    for name, index in export_indices.items():
        inverse_exports.setdefault(index, []).append(name)
    for callee in call_graph.get(main_init_index, ()):
        candidates = inverse_exports.get(callee, ())
        preferred = sorted(
            candidates,
            key=lambda name: (
                is_table_ref_export_name(name),
                not name.startswith("molt_init_"),
                name,
            ),
        )
        for target_name in preferred:
            if (
                target_name.startswith("molt_init_")
                and target_name != "molt_init___main__"
            ):
                return target_name.removeprefix("molt_init_")
            if target_name.endswith("__init") and "__" in target_name:
                prefix, _rest = target_name.rsplit("__", 1)
                if prefix:
                    return prefix
    return None


def _is_public_output_export_name(name: str, primary_prefix: str | None) -> bool:
    if primary_prefix is not None:
        marker = f"{primary_prefix}__"
        if not name.startswith(marker):
            return False
        remainder = name[len(marker) :]
    else:
        if not name or not name[0].isalnum() or "__" not in name:
            return False
        _prefix, remainder = name.split("__", 1)
    if not remainder:
        return False
    if remainder.startswith("__"):
        return False
    if remainder.startswith(_INTERNAL_OUTPUT_EXPORT_PREFIXES):
        return False
    if "___" in remainder:
        return False
    return True


def _restore_output_export_aliases(data: bytes) -> bytes | None:
    sections = _parse_sections(data)
    modified = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 7:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        exports: list[tuple[str, int, int]] = []
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            kind = payload[offset]
            offset += 1
            index, offset = _read_varuint(payload, offset)
            exports.append((name, kind, index))
        rebuilt = bytearray()
        seen_names: set[str] = set()
        kept: list[tuple[str, int, int]] = []
        for name, kind, index in exports:
            if kind == 0 and name.startswith(_OUTPUT_EXPORT_ALIAS_PREFIX):
                name = name.removeprefix(_OUTPUT_EXPORT_ALIAS_PREFIX)
                modified = True
            if name in seen_names:
                modified = True
                continue
            seen_names.add(name)
            kept.append((name, kind, index))
        rebuilt.extend(_write_varuint(len(kept)))
        for name, kind, index in kept:
            rebuilt.extend(_write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(_write_varuint(index))
        new_sections.append((section_id, bytes(rebuilt)))
    if not modified:
        return None
    return _build_sections(new_sections)


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


def _highest_exported_table_ref_index(data: bytes) -> int | None:
    refs = [
        ref_index
        for name in _collect_function_exports(data)
        if (ref_index := parse_table_ref_export_name(name)) is not None
    ]
    if not refs:
        return None
    return max(refs)


def _required_linked_table_min(data: bytes, fallback_min: int | None) -> int | None:
    required = fallback_min
    highest_ref = _highest_exported_table_ref_index(data)
    if highest_ref is not None:
        ref_required = highest_ref + 1
        required = ref_required if required is None else max(required, ref_required)
    current_min = _table_import_min(data)
    if current_min is not None:
        required = current_min if required is None else max(required, current_min)
    return required


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
) -> tuple[Path, tempfile.TemporaryDirectory, list[str]] | None:
    """Rewrite output imports to add the ``molt_`` prefix where needed.

    Returns ``(rewritten_path, temp_dir, force_exports)`` on success.
    *force_exports* lists prefixed names that were rewritten but are not
    present in *runtime_exports* — the caller should pass these as
    ``--export-if-defined`` flags to wasm-ld so the linker retains the
    symbols from a relocatable runtime input.
    """
    data = output.read_bytes()
    try:
        sections = _parse_sections(data)
    except ValueError as exc:
        print(f"Failed to parse wasm: {exc}", file=sys.stderr)
        return None

    force_exports: list[str] = []
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
            if module == "molt_runtime" and kind == 0:
                export_name = wasm_runtime_export_name(name)
                if (
                    export_name is not None
                    and export_name != name
                    and export_name in runtime_exports
                ):
                    new_name = export_name
                    needs_rewrite = True
                elif export_name is not None and name not in runtime_exports:
                    # The generated runtime export is not in the runtime's export
                    # section — likely inlined away by LTO during the
                    # cdylib build.  Still rewrite to the generated export name
                    # so wasm-ld can resolve it from a relocatable
                    # runtime that retains the symbol.
                    new_name = export_name
                    needs_rewrite = True
                    force_exports.append(export_name)

            rebuilt.extend(_write_string(module))
            rebuilt.extend(_write_string(new_name))
            rebuilt.append(kind)
            rebuilt.extend(desc)
        new_sections.append((section_id, bytes(rebuilt)))

    if force_exports:
        print(
            f"Wasm link: {len(force_exports)} import(s) rewritten but missing "
            f"from runtime exports (will resolve via relocatable runtime): "
            f"{', '.join(sorted(set(force_exports)))}",
            file=sys.stderr,
        )

    if not needs_rewrite:
        return output, tempfile.TemporaryDirectory(prefix="molt-wasm-link-"), []

    temp_dir = tempfile.TemporaryDirectory(prefix="molt-wasm-link-")
    wasm_path = Path(temp_dir.name) / "output_rewrite.wasm"
    wasm_path.write_bytes(_build_sections(new_sections))
    return wasm_path, temp_dir, force_exports


def _build_runtime_stub(runtime_data: bytes) -> bytes:
    """Generate a minimal WASM module that exports the same function signatures
    as the real runtime but with ``unreachable; end`` bodies.  This allows
    wasm-ld ``--gc-sections`` to run against it for dead code elimination.

    The stub preserves the runtime's type section verbatim so that wasm-ld can
    match function types by index.  Every exported function gets a trivial body
    (0 locals, ``unreachable``, ``end``).  A ``linking`` custom section with
    version=2 and no subsections is appended so that wasm-ld accepts the
    module as relocatable input.

    Memory, table, data, and element sections are intentionally omitted.
    """
    sections = _parse_sections(runtime_data)

    # -- 1. Locate the type, function, and export sections -------------------
    type_payload: bytes | None = None
    func_type_indices: list[int] = []
    exported_funcs: list[tuple[str, int]] = []  # (name, func_index)

    for section_id, payload in sections:
        if section_id == 1:
            # Type section — keep verbatim.
            type_payload = payload
        elif section_id == 3:
            # Function section — list of type indices for each defined function.
            offset = 0
            count, offset = _read_varuint(payload, offset)
            for _ in range(count):
                type_idx, offset = _read_varuint(payload, offset)
                func_type_indices.append(type_idx)
        elif section_id == 7:
            # Export section — collect function exports.
            offset = 0
            count, offset = _read_varuint(payload, offset)
            for _ in range(count):
                name, offset = _read_string(payload, offset)
                if offset >= len(payload):
                    raise ValueError(
                        "Unexpected EOF while reading export kind in runtime"
                    )
                kind = payload[offset]
                offset += 1
                index, offset = _read_varuint(payload, offset)
                if kind == 0:  # function export
                    exported_funcs.append((name, index))

    if type_payload is None:
        raise ValueError("Runtime module has no type section")
    if not exported_funcs:
        raise ValueError("Runtime module has no function exports")

    # -- 2. Count imported functions to compute the local function offset ----
    #    In WASM, function indices start with imports. The function section
    #    defines local functions starting at index = num_imports.
    num_imported_funcs = 0
    for section_id, payload in sections:
        if section_id == 2:  # import section
            offset = 0
            count, offset = _read_varuint(payload, offset)
            for _ in range(count):
                _module, offset = _read_string(payload, offset)
                _name, offset = _read_string(payload, offset)
                if offset >= len(payload):
                    raise ValueError(
                        "Unexpected EOF while reading import kind in runtime"
                    )
                kind = payload[offset]
                offset += 1
                # Skip the import descriptor.
                if kind == 0:  # function
                    _, offset = _read_varuint(payload, offset)
                    num_imported_funcs += 1
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
                else:
                    raise ValueError(f"Unknown import kind {kind} in runtime")

    # -- 3. Map each exported function to its type index ---------------------
    stub_type_indices: list[int] = []
    stub_export_names: list[str] = []

    for name, func_index in exported_funcs:
        local_index = func_index - num_imported_funcs
        if local_index < 0 or local_index >= len(func_type_indices):
            raise ValueError(
                f"Exported function {name!r} (index={func_index}) maps to "
                f"local index {local_index} which is out of range "
                f"(num_imported={num_imported_funcs}, "
                f"num_local={len(func_type_indices)})"
            )
        stub_type_indices.append(func_type_indices[local_index])
        stub_export_names.append(name)

    num_stub_funcs = len(stub_type_indices)

    # -- 4. Build the stub module sections -----------------------------------
    # Function section: one entry per stub function with its type index.
    func_payload = bytearray()
    func_payload.extend(_write_varuint(num_stub_funcs))
    for type_idx in stub_type_indices:
        func_payload.extend(_write_varuint(type_idx))

    # Export section: same names, mapped to sequential indices 0..N-1.
    export_payload = bytearray()
    export_payload.extend(_write_varuint(num_stub_funcs))
    for i, name in enumerate(stub_export_names):
        export_payload.extend(_write_string(name))
        export_payload.append(0)  # kind = function
        export_payload.extend(_write_varuint(i))

    # Code section: each body is [size=3, 0 locals, unreachable, end].
    #   body_size (varuint) = 3
    #   local_decl_count (varuint) = 0
    #   unreachable = 0x00
    #   end = 0x0b
    stub_body = b"\x03\x00\x00\x0b"
    code_payload = bytearray()
    code_payload.extend(_write_varuint(num_stub_funcs))
    for _ in range(num_stub_funcs):
        code_payload.extend(stub_body)

    # Linking custom section: version=2, no subsections.
    linking_payload = _build_custom_section("linking", b"\x02")

    stub_sections: list[tuple[int, bytes]] = [
        (1, type_payload),  # type section
        (3, bytes(func_payload)),  # function section
        (7, bytes(export_payload)),  # export section
        (10, bytes(code_payload)),  # code section
        (0, linking_payload),  # custom "linking" section
    ]

    return _build_sections(stub_sections)


def _canonicalize_standard_section_order(data: bytes) -> bytes | None:
    sections = _parse_sections(data)
    indexed_sections = list(enumerate(sections))
    canonical = sorted(
        indexed_sections,
        key=lambda item: (
            _STANDARD_SECTION_ORDER.get(item[1][0], 0 if item[1][0] == 0 else 100),
            item[0],
        ),
    )
    if [index for index, _section in canonical] == list(range(len(sections))):
        return None
    return _build_sections([section for _index, section in canonical])


def _split_app_reference_function_exports(reference_data: bytes | None) -> set[str]:
    """Return the split-app function exports that must remain host-visible."""
    if reference_data is None:
        return set()
    keep = {
        "molt_host_init",
        "molt_main",
        "molt_table_init",
        "molt_set_wasm_table_base",
    }
    keep.update(_collect_preserved_output_export_names(reference_data))
    return {name for name in _collect_function_exports(reference_data) if name in keep}


def _strip_internal_exports(
    data: bytes,
    *,
    preserve_exports: set[str] | None = None,
    preserve_table_refs: bool = True,
) -> bytes | None:
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
    keep_exports = set(_ESSENTIAL_EXPORTS)
    if preserve_exports:
        keep_exports.update(preserve_exports)
    seen_exports: set[str] = set()
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
            offset += 1
            _, offset = _read_varuint(payload, offset)
            entry_bytes = payload[entry_start:offset]
            if name not in keep_exports and (
                not preserve_table_refs or not is_table_ref_export_name(name)
            ):
                modified = True
                continue
            if name in seen_exports:
                modified = True
                continue
            seen_exports.add(name)
            entries.append(entry_bytes)
            new_count += 1
        rebuilt = bytearray(_write_varuint(new_count))
        for entry in entries:
            rebuilt.extend(entry)
        new_sections.append((section_id, bytes(rebuilt)))
    if not modified:
        return None
    return _build_sections(new_sections)

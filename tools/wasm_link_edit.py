#!/usr/bin/env python3
from __future__ import annotations

import sys
import tempfile
from collections.abc import Mapping
from collections import Counter
from pathlib import Path

from wasm_link_format import (
    FLAG_BINDING_GLOBAL,
    FLAG_EXPLICIT_NAME,
    FLAG_EXPORTED,
    FLAG_NO_STRIP,
    FLAG_UNDEFINED,
    SYMBOL_KIND_FUNCTION,
    SYMTAB_SUBSECTION_ID,
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES,
    WASM_EXTERNAL_NATIVE_LINK_IMPORTS,
    _ESSENTIAL_EXPORTS,
    _INTERNAL_OUTPUT_EXPORT_PREFIXES,
    _OUTPUT_EXPORT_ALIAS_PREFIX,
    _OUTPUT_RUNTIME_EXPORT_ALIASES,
    _STANDARD_SECTION_ORDER,
    _append_linking_function_symbols,
    _build_custom_section,
    _build_linking_payload,
    _build_sections,
    _collect_func_names,
    _collect_function_exports,
    _collect_imports,
    _collect_linking_function_symbols,
    _count_func_imports,
    _find_func_import_index,
    wasm_runtime_export_name,
    _parse_custom_section,
    _parse_func_type_indices,
    _parse_import_desc,
    _parse_indexed_symbol,
    _parse_linking_payload,
    _parse_sections,
    _parse_type_section,
    _read_limits,
    _read_string,
    _read_varuint,
    _write_limits,
    _write_string,
    _write_varuint,
)
from molt._wasm_runtime_exports import wasm_split_runtime_export_name_for_import
from molt.cli.external_link_providers import wasm_external_link_provider_symbols
from wasm_link_facts import callable_table_entry_rows, function_reference_rows


_CPYTHON_ABI_LINK_IMPORT_CLASS = "molt_cpython_abi_link_import"
_SYMBOL_KIND_DATA = 1


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
    output: Path,
    temp_dir: tempfile.TemporaryDirectory,
    facts: Mapping[str, object],
) -> Path:
    data = output.read_bytes()
    wrapper_specs = _collect_output_wrapper_specs(data, facts)
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
                    assert inc_ref_import_index is not None
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


def _collect_output_wrapper_specs(
    data: bytes,
    facts: Mapping[str, object],
) -> list[tuple[str, str, int, int]]:
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
    primary_prefix = _entry_module_prefix_from_main_init(export_indices, facts)
    if primary_prefix is None:
        primary_prefix = _dominant_output_module_prefix(export_indices)

    wrapper_specs: list[tuple[str, str, int, int]] = []
    for name, func_index in export_indices.items():
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


def _collect_preserved_output_export_names(
    data: bytes,
    facts: Mapping[str, object],
) -> list[str]:
    return [
        name
        for name, _alias, _type_idx, _func_idx in _collect_output_wrapper_specs(
            data, facts
        )
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
        preferred = next(
            (name for name in candidates if name.startswith("__molt_output_export_")),
            None,
        )
        if preferred is None:
            preferred = next((name for name in candidates if name == public_name), None)
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
    export_indices: dict[str, int],
    facts: Mapping[str, object],
) -> str | None:
    main_init_index = export_indices.get("molt_init___main__")
    if main_init_index is None:
        return None
    callees: list[int] = []
    for function_index, direct_calls, _ref_funcs in function_reference_rows(facts):
        if function_index != main_init_index:
            continue
        callees = direct_calls
        break
    inverse_exports: dict[int, list[str]] = {}
    for name, index in export_indices.items():
        inverse_exports.setdefault(index, []).append(name)
    for callee in callees:
        candidates = inverse_exports.get(callee, ())
        preferred = sorted(
            candidates,
            key=lambda name: (not name.startswith("molt_init_"), name),
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


def _highest_active_table_slot(facts: Mapping[str, object]) -> int | None:
    entries = callable_table_entry_rows(facts)
    if not entries:
        return None
    highest: int | None = None
    for slot, _function_index, _type_index, _role in entries:
        highest = slot if highest is None else max(highest, slot)
    return highest


def _required_linked_table_min(
    data: bytes,
    fallback_min: int | None,
    facts: Mapping[str, object],
) -> int | None:
    required = fallback_min
    highest_slot = _highest_active_table_slot(facts)
    if highest_slot is not None:
        slot_required = highest_slot + 1
        required = slot_required if required is None else max(required, slot_required)
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


def _runtime_import_rewrite_target(
    name: str, runtime_exports: set[str], *, split_runtime: bool = False
) -> tuple[str | None, bool]:
    primitive_class = WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(name)
    if primitive_class == _CPYTHON_ABI_LINK_IMPORT_CLASS:
        export_name = (
            wasm_split_runtime_export_name_for_import(name) if split_runtime else name
        )
        if export_name is None:
            return None, False
        return export_name, export_name not in runtime_exports
    if (
        name in WASM_EXTERNAL_NATIVE_LINK_IMPORTS
        or name in wasm_external_link_provider_symbols()
    ):
        return None, False
    export_name = wasm_runtime_export_name(name)
    if export_name is None:
        return None, False
    if export_name != name and export_name in runtime_exports:
        return export_name, False
    if name not in runtime_exports:
        return export_name, True
    return export_name, False


def _runtime_import_kind_can_rewrite(kind: int, name: str) -> bool:
    """Return whether an import of this wasm kind may carry a Molt ABI edge.

    Function imports (kind 0) are always eligible. CPython ABI *type objects*
    (``PyLong_Type`` and friends) are static-storage globals that source-compiled
    extensions reference as imported wasm globals (kind 3) holding the object's
    address, so those must be rewritable too — otherwise the split app keeps an
    unprefixed ``PyLong_Type`` edge the molt_-prefixed split runtime cannot
    satisfy.
    """
    if kind == 0:
        return True
    return (
        kind == 3
        and WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(name)
        == _CPYTHON_ABI_LINK_IMPORT_CLASS
    )


def _rewrite_linking_data_runtime_imports(
    data: bytes,
    *,
    runtime_exports: set[str],
    split_runtime: bool,
) -> tuple[bytes | None, list[str]]:
    """Rewrite undefined CPython ABI *data* symbols in the linking symtab.

    Source-recompiled extensions reference runtime-owned type objects
    (``PyLong_Type``, ``PyType_Type``, ...) as undefined ``data`` symbols in the
    relocatable object's linking section, distinct from function imports. The
    deployed split app must carry these as the molt_-prefixed public export
    names so the shared split runtime satisfies them at instantiation; the
    monolithic relocatable link keeps the real unprefixed ``#[no_mangle]`` names
    resolved directly against the relocatable runtime.
    """
    sections = _parse_sections(data)
    force_exports: list[str] = []
    changed = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 0:
            new_sections.append((section_id, payload))
            continue
        name, custom_payload = _parse_custom_section(payload)
        if name != "linking":
            new_sections.append((section_id, payload))
            continue
        version, subsections = _parse_linking_payload(custom_payload)
        new_subsections: list[tuple[int, bytes]] = []
        for sub_id, sub_payload in subsections:
            if sub_id != SYMTAB_SUBSECTION_ID:
                new_subsections.append((sub_id, sub_payload))
                continue
            count, offset = _read_varuint(sub_payload, 0)
            rebuilt = bytearray()
            rebuilt.extend(_write_varuint(count))
            for _ in range(count):
                entry_start = offset
                if offset >= len(sub_payload):
                    raise ValueError("Unexpected EOF while reading linking symbols")
                kind = sub_payload[offset]
                offset += 1
                flags, offset = _read_varuint(sub_payload, offset)
                if kind == SYMBOL_KIND_FUNCTION:
                    _, _, offset = _parse_indexed_symbol(sub_payload, offset, flags)
                    rebuilt.extend(sub_payload[entry_start:offset])
                    continue
                if kind in (2, 4, 5):
                    _, _, offset = _parse_indexed_symbol(sub_payload, offset, flags)
                    rebuilt.extend(sub_payload[entry_start:offset])
                    continue
                if kind == _SYMBOL_KIND_DATA:
                    symbol_name, offset = _read_string(sub_payload, offset)
                    target_name = symbol_name
                    if flags & FLAG_UNDEFINED:
                        rewrite_name, force_export = _runtime_import_rewrite_target(
                            symbol_name,
                            runtime_exports,
                            split_runtime=split_runtime,
                        )
                        if rewrite_name is not None:
                            target_name = rewrite_name
                            if target_name != symbol_name:
                                changed = True
                            if force_export:
                                force_exports.append(target_name)
                    rebuilt.append(kind)
                    rebuilt.extend(_write_varuint(flags))
                    rebuilt.extend(_write_string(target_name))
                    if not (flags & FLAG_UNDEFINED):
                        segment_index, offset = _read_varuint(sub_payload, offset)
                        data_offset, offset = _read_varuint(sub_payload, offset)
                        size, offset = _read_varuint(sub_payload, offset)
                        rebuilt.extend(_write_varuint(segment_index))
                        rebuilt.extend(_write_varuint(data_offset))
                        rebuilt.extend(_write_varuint(size))
                    continue
                if kind == 3:
                    _, offset = _read_varuint(sub_payload, offset)
                    rebuilt.extend(sub_payload[entry_start:offset])
                    continue
                raise ValueError(f"Unknown linking symbol kind: {kind}")
            new_subsections.append((sub_id, bytes(rebuilt)))
        new_sections.append(
            (
                section_id,
                _build_custom_section(
                    name,
                    _build_linking_payload(version, new_subsections),
                ),
            )
        )
    if not changed:
        return None, force_exports
    return _build_sections(new_sections), force_exports


def _rewrite_runtime_imports_in_module(
    data: bytes,
    *,
    source_module: str,
    target_module: str,
    runtime_exports: set[str],
    split_runtime: bool = False,
) -> tuple[bytes | None, list[str]]:
    sections = _parse_sections(data)
    force_exports: list[str] = []
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

            new_module = module
            new_name = name
            if module == source_module and _runtime_import_kind_can_rewrite(kind, name):
                target_name, force_export = _runtime_import_rewrite_target(
                    name, runtime_exports, split_runtime=split_runtime
                )
                if target_name is not None:
                    new_module = target_module
                    new_name = target_name
                    if new_module != module or new_name != name:
                        changed = True
                    if force_export:
                        force_exports.append(target_name)

            rebuilt.extend(_write_string(new_module))
            rebuilt.extend(_write_string(new_name))
            rebuilt.append(kind)
            rebuilt.extend(desc)
        new_sections.append((section_id, bytes(rebuilt)))

    import_rewritten = _build_sections(new_sections) if changed else data
    symbol_rewritten, symbol_force_exports = _rewrite_linking_data_runtime_imports(
        import_rewritten,
        runtime_exports=runtime_exports,
        split_runtime=split_runtime,
    )
    force_exports.extend(symbol_force_exports)
    if symbol_rewritten is not None:
        return symbol_rewritten, force_exports
    if not changed:
        return None, force_exports
    return import_rewritten, force_exports


def _rewrite_native_runtime_imports(
    native_objects: tuple[Path, ...],
    runtime_exports: set[str],
    temp_dir: tempfile.TemporaryDirectory,
    *,
    split_runtime: bool = False,
) -> tuple[tuple[Path, ...], list[str]]:
    """Rewrite native-object Molt ABI imports from ``env`` to ``molt_runtime``.

    Source-recompiled extension objects are produced by standard C/C++/Rust
    WASM toolchains, so unresolved function symbols initially appear as
    ``env::<symbol>`` imports. Molt runtime ABI symbols must share the
    split-runtime namespace used by compiler-emitted app imports; toolchain,
    libc, and other generated external-native imports remain under ``env``.
    """
    rewritten_paths: list[Path] = []
    force_exports: list[str] = []
    for index, native_object in enumerate(native_objects):
        data = native_object.read_bytes()
        try:
            rewritten, native_force_exports = _rewrite_runtime_imports_in_module(
                data,
                source_module="env",
                target_module="molt_runtime",
                runtime_exports=runtime_exports,
                split_runtime=split_runtime,
            )
        except ValueError:
            rewritten_paths.append(native_object)
            continue
        force_exports.extend(native_force_exports)
        if rewritten is None:
            rewritten_paths.append(native_object)
            continue
        staged = Path(temp_dir.name) / f"native_runtime_imports_{index}.wasm"
        staged.write_bytes(rewritten)
        rewritten_paths.append(staged)
    return tuple(rewritten_paths), force_exports


def _rewrite_runtime_import_module_namespace(
    module_path: Path,
    *,
    source_module: str,
    target_module: str,
    runtime_exports: set[str],
    temp_dir: tempfile.TemporaryDirectory,
    filename: str,
) -> tuple[Path, list[str]] | None:
    data = module_path.read_bytes()
    try:
        rewritten, force_exports = _rewrite_runtime_imports_in_module(
            data,
            source_module=source_module,
            target_module=target_module,
            runtime_exports=runtime_exports,
        )
    except ValueError as exc:
        print(f"Failed to parse wasm imports: {exc}", file=sys.stderr)
        return None
    if rewritten is None:
        return module_path, force_exports
    staged = Path(temp_dir.name) / filename
    staged.write_bytes(rewritten)
    return staged, force_exports


def _rewrite_output_imports(
    output: Path,
    runtime_exports: set[str],
    temp_dir: tempfile.TemporaryDirectory,
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
        return output, temp_dir, []

    wasm_path = Path(temp_dir.name) / "output_rewrite.wasm"
    wasm_path.write_bytes(_build_sections(new_sections))
    return wasm_path, temp_dir, force_exports


def _canonicalize_standard_section_order(data: bytes) -> bytes | None:
    sections = _parse_sections(data)
    vector_section_ids = {1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 13}
    merged_sections: list[tuple[int, bytes]] = []
    merged_indices: dict[int, int] = {}
    changed = False
    for section_id, payload in sections:
        if section_id == 0 or section_id not in merged_indices:
            merged_indices.setdefault(section_id, len(merged_sections))
            merged_sections.append((section_id, payload))
            continue
        if section_id not in vector_section_ids:
            raise ValueError(f"duplicate singleton standard section id {section_id}")
        existing_index = merged_indices[section_id]
        existing_payload = merged_sections[existing_index][1]
        if section_id == 7:
            merged_sections[existing_index] = (
                section_id,
                _merge_export_section_payloads(existing_payload, payload),
            )
            changed = True
            continue
        existing_count, existing_offset = _read_varuint(existing_payload, 0)
        added_count, added_offset = _read_varuint(payload, 0)
        merged_sections[existing_index] = (
            section_id,
            _write_varuint(existing_count + added_count)
            + existing_payload[existing_offset:]
            + payload[added_offset:],
        )
        changed = True
    indexed_sections = list(enumerate(merged_sections))
    canonical = sorted(
        indexed_sections,
        key=lambda item: (
            _STANDARD_SECTION_ORDER.get(item[1][0], 0 if item[1][0] == 0 else 100),
            item[0],
        ),
    )
    if not changed and [index for index, _section in canonical] == list(
        range(len(merged_sections))
    ):
        return None
    return _build_sections([section for _index, section in canonical])


def _merge_export_section_payloads(first: bytes, second: bytes) -> bytes:
    entries: dict[str, tuple[int, int]] = {}
    ordered: list[tuple[str, int, int]] = []
    for payload in (first, second):
        count, offset = _read_varuint(payload, 0)
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF while merging export sections")
            kind = payload[offset]
            index, offset = _read_varuint(payload, offset + 1)
            existing = entries.get(name)
            if existing is not None:
                continue
            entries[name] = (kind, index)
            ordered.append((name, kind, index))
        if offset != len(payload):
            raise ValueError("Trailing bytes while merging export sections")
    merged = bytearray(_write_varuint(len(ordered)))
    for name, kind, index in ordered:
        merged.extend(_write_string(name))
        merged.append(kind)
        merged.extend(_write_varuint(index))
    return bytes(merged)


def _standard_section_order_error(data: bytes) -> str | None:
    last_section_id: int | None = None
    last_order = 0
    seen: set[int] = set()
    for section_id, _payload in _parse_sections(data):
        if section_id == 0:
            continue
        order = _STANDARD_SECTION_ORDER.get(section_id)
        if order is None:
            return f"unknown standard section id {section_id}"
        if section_id in seen:
            return f"duplicate standard section id {section_id}"
        if order < last_order:
            return (
                f"standard section id {section_id} follows id {last_section_id}; "
                "expected canonical WebAssembly section order"
            )
        seen.add(section_id)
        last_section_id = section_id
        last_order = order
    return None


def _strip_internal_exports(
    data: bytes,
    *,
    preserve_exports: set[str] | None = None,
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
            if name not in keep_exports:
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

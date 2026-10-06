#!/usr/bin/env python3
from __future__ import annotations

from molt.wasm_artifact import skip_wasm_import_description as _parse_import_desc

import sys
from collections.abc import Mapping


from wasm_link_edit import _strip_internal_exports
from wasm_link_fact_provider import WasmFactsProvider
from wasm_link_facts import (
    active_function_element_rows,
    fact_index_set,
    table_mutation_rows,
)
from wasm_link_format import (
    _TRAP_STUB_BODY,
    _read_string,
    _read_varsint,
    _read_varuint,
    _write_string,
    _write_varuint,
)
from wasm_link_operations import (
    strip_publication_sections,
    build_sections as _build_sections,
    parse_sections as _parse_sections,
)


def _fact_index_set(
    facts: Mapping[str, object],
    field: str,
) -> set[int]:
    return fact_index_set(facts, field)


def _reachable_function_indices(facts: Mapping[str, object]) -> set[int]:
    return _fact_index_set(facts, "reachable_function_indices")


def _referenced_function_indices(facts: Mapping[str, object]) -> set[int]:
    return _fact_index_set(facts, "referenced_function_indices")


def _has_opaque_function_reference_dispatch(facts: Mapping[str, object]) -> bool:
    """Fail closed when typed function-reference targets are not fully attributable."""

    value = facts.get("reachable_function_reference_dispatch")
    if not isinstance(value, bool):
        return True
    return value


def _neutralize_dead_element_entries(
    data: bytes,
    facts: Mapping[str, object],
) -> bytes | None:
    """Replace indirect-call table entries for dead functions with the sentinel.

    After linking, the element section (section 9) populates the indirect
    function table.  Many entries point to runtime functions that are never
    actually dispatched -- they exist only because the runtime compiled them
    with ``#[no_mangle]`` and ``wasm-ld`` preserved them.

    This pass identifies function indices that appear ONLY in the element
    section (never referenced by ``call``, ``return_call``, or ``ref.func``
    in the code section) and replaces them with function index 0.

    **Safety**: when the module contains ``call_indirect`` instructions the
    pass is skipped entirely because ``call_indirect`` dispatches through
    runtime-computed table indices that cannot be resolved statically.
    """
    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    # Dynamic dispatch through call_indirect uses runtime-computed table
    # indices. Those targets are not statically attributable to direct call
    # edges, so element neutralization is unsound when any call_indirect
    # remains in the module.
    if facts.get(
        "reachable_dynamic_dispatch"
    ) is True or _has_opaque_function_reference_dispatch(facts):
        return None

    exported_tables = _fact_index_set(facts, "exported_table_indices")
    if 0 in exported_tables:
        return None

    reachable = _reachable_function_indices(facts)
    if any(
        function_index in reachable and operation == "table.init"
        for function_index, operation, _table_index, _source_table_index in table_mutation_rows(
            facts
        )
    ):
        return None

    elem_indices = {
        function_index
        for _table_index, _slot, function_index in active_function_element_rows(facts)
    }
    # Functions only in the element table, never directly called from code
    dead_indices = elem_indices - reachable

    if not dead_indices:
        return None

    # Rebuild the element section, replacing dead entries with sentinel (0)
    # in active segments, and removing dead entries from declarative segments.
    new_sections: list[tuple[int, bytes]] = []
    replaced = 0
    decl_removed = 0
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
            if flags == 0:
                # Active segment for table 0: replace dead entries with 0
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
            elif flags == 3:
                # Declarative segment (flags=0x03): element kind + count + indices.
                # Remove dead function declarations entirely so wasm-opt can
                # eliminate the corresponding function bodies.
                new_payload.append(flags)
                elem_kind = payload[offset]
                new_payload.append(elem_kind)
                offset += 1
                n, offset = _read_varuint(payload, offset)
                live_indices: list[int] = []
                for _ in range(n):
                    idx, offset = _read_varuint(payload, offset)
                    if idx in dead_indices:
                        decl_removed += 1
                    else:
                        live_indices.append(idx)
                new_payload.extend(_write_varuint(len(live_indices)))
                for idx in live_indices:
                    new_payload.extend(_write_varuint(idx))
            else:
                # Other segment types -- copy as-is
                new_payload.append(flags)
                new_payload.extend(payload[offset:])
                break
        new_sections.append((sid, bytes(new_payload)))

    if replaced == 0 and decl_removed == 0:
        return None

    print(
        f"Neutralised {replaced:,} dead element entries, "
        f"removed {decl_removed:,} dead declarative refs "
        f"({len(dead_indices):,} unique functions eligible for DCE)",
        file=sys.stderr,
    )
    return _build_sections(new_sections)


def _stub_dead_functions(
    data: bytes,
    facts: Mapping[str, object],
) -> bytes | None:
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

    if _has_opaque_function_reference_dispatch(facts):
        return None

    import_count = int(facts["function_import_count"])

    reachable = _reachable_function_indices(facts)

    defined_count = facts.get("defined_function_count")
    if not isinstance(defined_count, int) or isinstance(defined_count, bool):
        raise ValueError("WASM facts defined_function_count must be an integer")
    all_defined = set(range(import_count, import_count + defined_count))
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


def _strip_unused_module_function_imports(
    data: bytes,
    *,
    module_name: str,
    facts_provider: WasmFactsProvider,
) -> bytes | None:
    """Remove unreferenced function imports for a specific import module."""

    facts = facts_provider(data)

    def _rewrite_init_expr_func_indices(
        blob: bytes, offset: int, remap_func_index
    ) -> tuple[bytes, int]:
        out = bytearray()
        while offset < len(blob):
            opcode = blob[offset]
            instr_start = offset
            offset += 1
            if opcode == 0x0B:
                out.extend(blob[instr_start:offset])
                return bytes(out), offset
            if opcode in (0x41, 0x42, 0x23):
                _, offset = _read_varuint(blob, offset)
                out.extend(blob[instr_start:offset])
                continue
            if opcode in (0x43, 0x44):
                offset += 4 if opcode == 0x43 else 8
                out.extend(blob[instr_start:offset])
                continue
            if opcode == 0xD0:
                if offset >= len(blob):
                    raise ValueError("Unexpected EOF while reading ref.null")
                offset += 1
                out.extend(blob[instr_start:offset])
                continue
            if opcode == 0xD2:
                idx, offset = _read_varuint(blob, offset)
                out.extend(blob[instr_start : instr_start + 1])
                out.extend(_write_varuint(remap_func_index(idx)))
                continue
            raise ValueError(f"Unsupported init expr opcode 0x{opcode:02x}")
        raise ValueError("Unexpected EOF while reading init expr")

    def _rewrite_export_section(payload: bytes, remap_func_index) -> bytes:
        offset = 0
        count, offset = _read_varuint(payload, offset)
        out = bytearray()
        out.extend(_write_varuint(count))
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            kind = payload[offset]
            offset += 1
            idx, offset = _read_varuint(payload, offset)
            out.extend(_write_string(name))
            out.append(kind)
            out.extend(_write_varuint(remap_func_index(idx) if kind == 0 else idx))
        return bytes(out)

    def _rewrite_start_section(payload: bytes, remap_func_index) -> bytes:
        idx, offset = _read_varuint(payload, 0)
        if offset != len(payload):
            raise ValueError("Malformed start section")
        return _write_varuint(remap_func_index(idx))

    def _rewrite_element_section(payload: bytes, remap_func_index) -> bytes:
        offset = 0
        count, offset = _read_varuint(payload, offset)
        out = bytearray()
        out.extend(_write_varuint(count))
        for _ in range(count):
            flags, offset = _read_varuint(payload, offset)
            out.extend(_write_varuint(flags))
            if flags in (0x02, 0x06):
                table_index, offset = _read_varuint(payload, offset)
                out.extend(_write_varuint(table_index))
                expr, offset = _rewrite_init_expr_func_indices(
                    payload, offset, remap_func_index
                )
                out.extend(expr)
            elif flags in (0x00, 0x04):
                expr, offset = _rewrite_init_expr_func_indices(
                    payload, offset, remap_func_index
                )
                out.extend(expr)

            if flags in (0x00, 0x01, 0x02, 0x03):
                if flags in (0x01, 0x02, 0x03):
                    elemkind = payload[offset]
                    offset += 1
                    out.append(elemkind)
                elem_count, offset = _read_varuint(payload, offset)
                out.extend(_write_varuint(elem_count))
                for _ in range(elem_count):
                    idx, offset = _read_varuint(payload, offset)
                    out.extend(_write_varuint(remap_func_index(idx)))
                continue

            if flags in (0x05, 0x07):
                reftype = payload[offset]
                offset += 1
                out.append(reftype)

            expr_count, offset = _read_varuint(payload, offset)
            out.extend(_write_varuint(expr_count))
            for _ in range(expr_count):
                expr, offset = _rewrite_init_expr_func_indices(
                    payload, offset, remap_func_index
                )
                out.extend(expr)
        return bytes(out)

    def _rewrite_global_section(payload: bytes, remap_func_index) -> bytes:
        offset = 0
        count, offset = _read_varuint(payload, offset)
        out = bytearray()
        out.extend(_write_varuint(count))
        for _ in range(count):
            if offset + 2 > len(payload):
                raise ValueError("Unexpected EOF while reading global header")
            out.extend(payload[offset : offset + 2])
            offset += 2
            expr, offset = _rewrite_init_expr_func_indices(
                payload, offset, remap_func_index
            )
            out.extend(expr)
        return bytes(out)

    def _rewrite_code_body(body: bytes, remap_func_index) -> bytes:
        pos = 0
        local_count, pos = _read_varuint(body, pos)
        for _ in range(local_count):
            _, pos = _read_varuint(body, pos)
            pos += 1
        out = bytearray(body[:pos])
        while pos < len(body):
            instr_start = pos
            op = body[pos]
            pos += 1
            if op in (0x00, 0x01, 0x05, 0x0B, 0x0F, 0x1A, 0x1B, 0xD1, 0xD3):
                out.extend(body[instr_start:pos])
            elif op in (0x02, 0x03, 0x04):
                bt = body[pos]
                if bt in (0x40, 0x7F, 0x7E, 0x7D, 0x7C, 0x70, 0x6F, 0x7B):
                    pos += 1
                else:
                    _, pos = _read_varsint(body, pos)
                out.extend(body[instr_start:pos])
            elif op in (
                0x0C,
                0x0D,
                0x20,
                0x21,
                0x22,
                0x23,
                0x24,
                0x25,
                0x26,
                0x3F,
                0x40,
                0xD0,
                0xD4,
                0xD5,
            ):
                _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op == 0x0E:
                n, pos = _read_varuint(body, pos)
                for _ in range(n + 1):
                    _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op in (0x10, 0x12):
                idx, pos = _read_varuint(body, pos)
                out.extend(body[instr_start : instr_start + 1])
                out.extend(_write_varuint(remap_func_index(idx)))
            elif op in (0x11, 0x13):
                _, pos = _read_varuint(body, pos)
                _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op in (0x14, 0x15):
                _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op == 0xD2:
                idx, pos = _read_varuint(body, pos)
                out.extend(body[instr_start : instr_start + 1])
                out.extend(_write_varuint(remap_func_index(idx)))
            elif 0x28 <= op <= 0x3E:
                _, pos = _read_varuint(body, pos)
                _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op in (0x41, 0x42):
                _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op == 0x43:
                pos += 4
                out.extend(body[instr_start:pos])
            elif op == 0x44:
                pos += 8
                out.extend(body[instr_start:pos])
            elif 0x45 <= op <= 0xC4:
                out.extend(body[instr_start:pos])
            elif op == 0x1C:
                n, pos = _read_varuint(body, pos)
                pos += n
                out.extend(body[instr_start:pos])
            elif op == 0xFC:
                ext, pos = _read_varuint(body, pos)
                if ext <= 7:
                    pass
                elif ext in (8, 10, 12, 14):
                    _, pos = _read_varuint(body, pos)
                    _, pos = _read_varuint(body, pos)
                elif ext in (9, 11, 13, 15, 16, 17):
                    _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op == 0xFD:
                simd, pos = _read_varuint(body, pos)
                if simd <= 11:
                    _, pos = _read_varuint(body, pos)
                    _, pos = _read_varuint(body, pos)
                elif simd in (12, 13):
                    pos += 16
                elif 84 <= simd <= 91:
                    _, pos = _read_varuint(body, pos)
                    _, pos = _read_varuint(body, pos)
                    pos += 1
                elif 21 <= simd <= 34:
                    pos += 1
                elif 92 <= simd <= 93:
                    _, pos = _read_varuint(body, pos)
                    _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op == 0x1F:
                bt = body[pos]
                if bt == 0x40 or 0x7C <= bt <= 0x7F:
                    pos += 1
                else:
                    _, pos = _read_varsint(body, pos)
                n_catches, pos = _read_varuint(body, pos)
                for _ in range(n_catches):
                    catch_kind = body[pos]
                    pos += 1
                    if catch_kind in (0x00, 0x01):
                        _, pos = _read_varuint(body, pos)
                        _, pos = _read_varuint(body, pos)
                    elif catch_kind in (0x02, 0x03):
                        _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            elif op == 0xFE:
                atom, pos = _read_varuint(body, pos)
                if atom == 0x03:
                    pos += 1
                elif atom >= 0x10 or atom in (0x00, 0x01, 0x02):
                    _, pos = _read_varuint(body, pos)
                    _, pos = _read_varuint(body, pos)
                out.extend(body[instr_start:pos])
            else:
                raise ValueError(f"Unsupported opcode 0x{op:02x} during import remap")
        return bytes(out)

    def _rewrite_code_section(payload: bytes, remap_func_index) -> bytes:
        offset = 0
        func_count, offset = _read_varuint(payload, offset)
        out = bytearray()
        out.extend(_write_varuint(func_count))
        for _ in range(func_count):
            body_size, body_start = _read_varuint(payload, offset)
            body_end = body_start + body_size
            new_body = _rewrite_code_body(
                payload[body_start:body_end], remap_func_index
            )
            out.extend(_write_varuint(len(new_body)))
            out.extend(new_body)
            offset = body_end
        return bytes(out)

    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    import_entries: list[tuple[str, str, int, bytes, int | None]] = []
    import_count = 0
    for sid, payload in sections:
        if sid != 2:
            continue
        offset = 0
        total, offset = _read_varuint(payload, offset)
        for _ in range(total):
            module, offset = _read_string(payload, offset)
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF while reading import kind")
            kind = payload[offset]
            offset += 1
            desc_start = offset
            offset = _parse_import_desc(payload, offset, kind)
            func_index: int | None = None
            if kind == 0:
                func_index = import_count
                import_count += 1
            import_entries.append(
                (module, name, kind, payload[desc_start:offset], func_index)
            )
        break

    if not import_entries:
        return None

    referenced = _referenced_function_indices(facts)

    removed_sorted = sorted(
        fact.index
        for fact in facts.imports
        if fact.kind == 0
        and fact.module == module_name
        and fact.index not in referenced
    )
    if not removed_sorted:
        return None

    removed_set = set(removed_sorted)

    def remap_func_index(old_idx: int) -> int:
        if old_idx in removed_set:
            raise ValueError(f"Attempted to remap removed function import {old_idx}")
        removed_before = 0
        for removed_idx in removed_sorted:
            if removed_idx >= old_idx:
                break
            removed_before += 1
        if old_idx < import_count:
            return old_idx - removed_before
        return old_idx - len(removed_sorted)

    try:
        new_sections: list[tuple[int, bytes]] = []
        for sid, payload in sections:
            custom_name = _read_string(payload, 0)[0] if sid == 0 else None
            if custom_name is not None and (
                custom_name in {"name", "linking"} or custom_name.startswith("reloc.")
            ):
                # Function and local-name indices refer to the old import space.
                # Other debug sections remain under the selected debug policy.
                continue
            if sid == 2:
                kept_entries = [
                    (module, name, kind, desc)
                    for module, name, kind, desc, func_index in import_entries
                    if not (kind == 0 and func_index in removed_set)
                ]
                new_payload = bytearray()
                new_payload.extend(_write_varuint(len(kept_entries)))
                for module, name, kind, desc in kept_entries:
                    new_payload.extend(_write_string(module))
                    new_payload.extend(_write_string(name))
                    new_payload.append(kind)
                    new_payload.extend(desc)
                new_sections.append((sid, bytes(new_payload)))
            elif sid == 7:
                new_sections.append(
                    (sid, _rewrite_export_section(payload, remap_func_index))
                )
            elif sid == 8:
                new_sections.append(
                    (sid, _rewrite_start_section(payload, remap_func_index))
                )
            elif sid == 9:
                new_sections.append(
                    (sid, _rewrite_element_section(payload, remap_func_index))
                )
            elif sid == 6:
                new_sections.append(
                    (sid, _rewrite_global_section(payload, remap_func_index))
                )
            elif sid == 10:
                new_sections.append(
                    (sid, _rewrite_code_section(payload, remap_func_index))
                )
            else:
                new_sections.append((sid, payload))
    except ValueError:
        return None

    updated = _build_sections(new_sections)
    try:
        facts_provider(updated)
    except ValueError:
        return None

    print(
        f"Split-app import strip: removed {len(removed_sorted)} unused {module_name} imports, "
        f"{len(data):,} -> {len(updated):,} bytes",
        file=sys.stderr,
    )
    return updated


def _post_link_optimize(
    data: bytes,
    *,
    reference_data: bytes | None = None,
    preserve_exports: set[str] | None = None,
    preserve_reference_exports: bool = True,
    preserve_debug: bool = False,
    facts_provider: WasmFactsProvider,
) -> bytes:
    """Apply post-link optimizations to reduce V8 compilation memory pressure.

    This is the key fix for MOL-183/MOL-186: the linked artifact was
    overwhelming V8 because of debug sections, internal exports, and
    duplicate data.  Stripping them reduces the module size by 30-60%
    which directly translates to less compilation memory.

    *reference_data*, when provided, is the original (pre-link) user module.
    Its public exports remain roots even when the post-link artifact renamed
    or internalized them during linking.
    """
    data = strip_publication_sections(
        data, final_artifact=True, preserve_debug=preserve_debug
    )
    preserved_export_names = set(preserve_exports or ())
    if preserve_reference_exports and reference_data is not None:
        preserved_export_names.update(facts_provider(reference_data).function_exports)

    updated = _strip_internal_exports(
        data,
        preserve_exports=preserved_export_names,
    )
    if updated is not None:
        data = updated

    # The scanner computes transitive reachability to a fixed point. Reuse that
    # one immutable fact set for both index-preserving rewrites; neither pass
    # adds roots or changes the function index space.
    facts = facts_provider(data)
    updated = _neutralize_dead_element_entries(data, facts)
    if updated is not None:
        data = updated
    updated = _stub_dead_functions(data, facts)
    if updated is not None:
        data = updated

    return data

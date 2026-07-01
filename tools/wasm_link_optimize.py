#!/usr/bin/env python3
from __future__ import annotations

import re
import sys

from wasm_link_edit import _strip_internal_exports
from wasm_link_format import (
    CALL_INDIRECT_RE,
    _TRAP_STUB_BODY,
    _build_call_graph,
    _build_sections,
    _collect_element_declared_funcs,
    _collect_function_exports,
    _count_func_imports,
    is_table_ref_export_name,
    _parse_custom_section,
    _parse_func_type_indices,
    _parse_import_desc,
    _parse_sections,
    _parse_type_section,
    _read_string,
    _read_varsint,
    _read_varuint,
    _skip_init_expr,
    _validate_elements,
    _write_string,
    _write_varuint,
)


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
        if (
            name
            in (
                "name",
                "producers",
                "sourceMappingURL",
                "linking",
                "dylink.0",
            )
            or name.startswith(".debug")
            or name.startswith("reloc.")
        ):
            stripped = True
            continue
        keep.append((section_id, payload))
    if not stripped:
        return None
    return _build_sections(keep)


def _collect_code_referenced_funcs(sections: list[tuple[int, bytes]]) -> set[int]:
    """Scan the code section for function indices referenced by ``call`` instructions.

    Returns the set of function indices that appear as direct call targets.
    This is intentionally conservative -- functions reached only via
    ``call_indirect`` (through the element/table) are NOT included, which is
    exactly what we want: element entries whose targets never appear in a
    direct ``call`` are candidates for neutralisation.

    Indices outside the valid function range are discarded to avoid false
    positives from the naive byte scan (0x10 can appear as part of other
    instruction immediates).
    """
    # Compute total function count (imports + defined) for validation.
    total_funcs = _count_func_imports(sections)
    for sid, payload in sections:
        if sid == 10:
            off = 0
            n, off = _read_varuint(payload, off)
            total_funcs += n
            break

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
            # Scan for call (0x10), return_call (0x12), and ref.func (0xD2)
            # opcodes — each has a single varuint func-index immediate.
            while pos < body_end:
                b = payload[pos]
                if b in (0x10, 0x12, 0xD2):
                    pos += 1
                    try:
                        idx, pos = _read_varuint(payload, pos)
                        if idx < total_funcs:
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
            if flags == 0:
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
            elif flags == 1:
                # Passive funcref: element kind + count + indices
                offset += 1  # element kind byte
                n, offset = _read_varuint(payload, offset)
                for _ in range(n):
                    idx, offset = _read_varuint(payload, offset)
                    indices.add(idx)
            elif flags == 2:
                # Active with explicit table index: table idx + init expr + kind + count + indices
                _, offset = _read_varuint(payload, offset)  # table index
                offset = _skip_init_expr(payload, offset)  # proper LEB128-aware skip
                offset += 1  # element kind byte
                n, offset = _read_varuint(payload, offset)
                for _ in range(n):
                    idx, offset = _read_varuint(payload, offset)
                    indices.add(idx)
            elif flags == 3:
                # Declarative: element kind + count + indices
                offset += 1  # element kind byte
                n, offset = _read_varuint(payload, offset)
                for _ in range(n):
                    idx, offset = _read_varuint(payload, offset)
                    indices.add(idx)
            else:
                # Flags 4-7 use expression-based elements; skip for safety
                break
    return indices


def _code_section_has_call_indirect(sections: list[tuple[int, bytes]]) -> bool:
    """Return True if the code section contains any ``call_indirect`` (0x11).

    This is a quick byte scan.  False positives (0x11 appearing as part of
    another instruction's immediate) are safe — they merely disable the
    optimisation.
    """
    for sid, payload in sections:
        if sid == 10:
            return b"\x11" in payload
    return False


def _module_imports_host_call_indirect(sections: list[tuple[int, bytes]]) -> bool:
    """Return True when the module imports host `molt_call_indirect*` shims.

    Split-runtime/direct mode routes dynamic indirect dispatch through JS host
    imports named `env::molt_call_indirectN` instead of raw wasm
    `call_indirect` opcodes in the module body. Treat those imports as
    evidence of dynamic table dispatch so element-entry neutralization does not
    clear live trampoline slots.
    """
    for sid, payload in sections:
        if sid != 2:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            module_name, offset = _read_string(payload, offset)
            field_name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                break
            kind = payload[offset]
            offset += 1
            if kind == 0:  # function
                _, offset = _read_varuint(payload, offset)
            else:
                offset = _parse_import_desc(payload, offset, kind)
            if (
                kind == 0
                and module_name == "env"
                and CALL_INDIRECT_RE.fullmatch(field_name) is not None
            ):
                return True
    return False


def _neutralize_dead_element_entries(data: bytes) -> bytes | None:
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
    if _code_section_has_call_indirect(sections) or _module_imports_host_call_indirect(
        sections
    ):
        return None

    code_called = _collect_code_referenced_funcs(sections)
    elem_indices = _collect_element_func_indices(sections)
    # Functions only in the element table, never directly called from code
    dead_indices = elem_indices - code_called

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

    # Collect roots: start function + exports + element-section entries
    roots: set[int] = set()
    for sid, payload in sections:
        if sid == 8:  # start section
            offset = 0
            idx, _ = _read_varuint(payload, offset)
            roots.add(idx)
        elif sid == 7:  # export section
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
                if flags == 0:
                    # Active segment for table 0
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
                elif flags == 1:
                    # Passive funcref: element kind + count + indices
                    offset += 1  # element kind byte
                    n, offset = _read_varuint(payload, offset)
                    for _ in range(n):
                        idx, offset = _read_varuint(payload, offset)
                        roots.add(idx)
                elif flags == 2:
                    # Active with explicit table index
                    _, offset = _read_varuint(payload, offset)  # table index
                    offset = _skip_init_expr(
                        payload, offset
                    )  # proper LEB128-aware skip
                    offset += 1  # element kind byte
                    n, offset = _read_varuint(payload, offset)
                    for _ in range(n):
                        idx, offset = _read_varuint(payload, offset)
                        roots.add(idx)
                elif flags == 3:
                    # Declarative: element kind + count + indices
                    offset += 1  # element kind byte
                    n, offset = _read_varuint(payload, offset)
                    for _ in range(n):
                        idx, offset = _read_varuint(payload, offset)
                        roots.add(idx)
                else:
                    # Flags 4-7 use expression-based elements; skip for safety
                    break

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


def _strip_unused_module_function_imports(
    data: bytes,
    *,
    module_name: str,
) -> bytes | None:
    """Remove unreferenced function imports for a specific import module."""

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

    def _collect_global_ref_funcs(payload: bytes) -> set[int]:
        refs: set[int] = set()
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            if offset + 2 > len(payload):
                raise ValueError("Unexpected EOF while reading global header")
            offset += 2
            expr_start = offset
            _, offset = _rewrite_init_expr_func_indices(
                payload, offset, lambda idx: idx
            )
            expr = payload[expr_start:offset]
            expr_offset = 0
            while expr_offset < len(expr):
                opcode = expr[expr_offset]
                expr_offset += 1
                if opcode == 0x0B:
                    break
                if opcode == 0xD2:
                    idx, expr_offset = _read_varuint(expr, expr_offset)
                    refs.add(idx)
                elif opcode in (0x41, 0x42, 0x23):
                    _, expr_offset = _read_varuint(expr, expr_offset)
                elif opcode in (0x43, 0x44):
                    expr_offset += 4 if opcode == 0x43 else 8
                elif opcode == 0xD0:
                    expr_offset += 1
                else:
                    raise ValueError(f"Unsupported global init opcode 0x{opcode:02x}")
        return refs

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

    referenced: set[int] = set()
    for sid, payload in sections:
        if sid == 7:
            offset = 0
            count, offset = _read_varuint(payload, offset)
            for _ in range(count):
                _, offset = _read_string(payload, offset)
                kind = payload[offset]
                offset += 1
                idx, offset = _read_varuint(payload, offset)
                if kind == 0:
                    referenced.add(idx)
        elif sid == 8:
            idx, _ = _read_varuint(payload, 0)
            referenced.add(idx)
        elif sid == 9:
            referenced.update(_collect_element_declared_funcs(data))
        elif sid == 6:
            referenced.update(_collect_global_ref_funcs(payload))
        elif sid == 10:
            for callees in _build_call_graph(payload, import_count).values():
                referenced.update(callees)

    removed_sorted = sorted(
        func_index
        for module, _name, kind, _desc, func_index in import_entries
        if kind == 0
        and module == module_name
        and func_index is not None
        and func_index not in referenced
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
    ok, _err = _validate_elements(updated)
    if not ok:
        return None

    print(
        f"Split-app import strip: removed {len(removed_sorted)} unused {module_name} imports, "
        f"{len(data):,} -> {len(updated):,} bytes",
        file=sys.stderr,
    )
    return updated


def _dedup_data_segments(data: bytes) -> bytes | None:
    """Strip embedded file paths from data segments to reduce binary size.

    After wasm-ld merges modules, the data section contains Rust panic
    location paths (/rustc/..., /Users/...) that leak build info and
    waste space.  This pass rewrites those path bytes in-place with a
    short placeholder, preserving segment layout so no relocation is
    needed.

    Also reports duplicate-segment statistics for diagnostics.
    """
    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    data_payload = None
    for section_id, payload in sections:
        if section_id == 11:
            data_payload = payload
            break

    if data_payload is None:
        return None

    # Parse segments to find duplicates and collect data payloads
    offset = 0
    seg_count, offset = _read_varuint(data_payload, offset)
    seg_headers: list[bytes] = []  # raw header bytes (flags + init_expr)
    seg_raw: list[bytes] = []  # data payload bytes

    parse_offset = offset
    for _ in range(seg_count):
        seg_start = parse_offset
        flags_byte = data_payload[parse_offset]
        parse_offset += 1
        if flags_byte == 0:
            # active, memory 0, init expr (i32.const <signed LEB128> end)
            parse_offset = _skip_init_expr(data_payload, parse_offset)
        elif flags_byte == 1:
            # passive
            pass
        elif flags_byte == 2:
            # active with explicit memory index, init expr
            _, parse_offset = _read_varuint(data_payload, parse_offset)
            parse_offset = _skip_init_expr(data_payload, parse_offset)
        else:
            # Unknown flags, bail
            return None
        header_end = parse_offset
        # Read the data bytes
        data_len, parse_offset = _read_varuint(data_payload, parse_offset)
        seg_data = data_payload[parse_offset : parse_offset + data_len]
        parse_offset += data_len
        seg_headers.append(data_payload[seg_start:header_end])
        seg_raw.append(seg_data)

    if len(seg_raw) < 2:
        return None

    # --- Pass 1: report duplicate segment statistics ---
    seen: dict[bytes, int] = {}
    dup_bytes = 0
    for raw in seg_raw:
        if raw in seen:
            dup_bytes += len(raw)
        else:
            seen[raw] = len(raw)

    if dup_bytes >= 1024:
        print(
            f"Data section has ~{dup_bytes:,} bytes of duplicate segments "
            f"({dup_bytes / 1024:.1f} KB).",
            file=sys.stderr,
        )

    # --- Pass 2: scrub embedded file paths ---
    # Replace embedded source/build file paths with a short tag, padded with
    # null bytes to keep the same byte length (no relocation). Match only
    # through the first plausible file-extension boundary; linked data
    # segments can concatenate adjacent literals without NUL separators, so a
    # greedy "[^\\x00]+" scrubber is unsound and can zero live payload bytes.
    _PATH_EXT_RE = (
        rb"(?:rs|py|pyi|toml|json|ron|ya?ml|c|cc|cpp|h|hpp|m|mm|swift|"
        rb"js|jsx|ts|tsx|md|txt|lean|wat|wasm)"
    )
    _PATH_RE = re.compile(
        rb"(?:/rustc/[0-9a-f]{20,}/[^\x00]{1,512}?\."
        + _PATH_EXT_RE
        + rb"|/Users/[^\x00]{1,512}?\."
        + _PATH_EXT_RE
        + rb")"
    )
    saved_path_bytes = 0
    new_seg_raw: list[bytes] = []
    for raw in seg_raw:
        buf = bytearray(raw)
        for m in reversed(list(_PATH_RE.finditer(buf))):
            span_len = m.end() - m.start()
            tag = b"<stripped>"
            replacement = tag + b"\x00" * (span_len - len(tag))
            buf[m.start() : m.end()] = replacement
            saved_path_bytes += span_len - len(tag)
        new_seg_raw.append(bytes(buf))

    if saved_path_bytes == 0:
        return None

    # Rebuild the data section payload
    new_data = bytearray(_write_varuint(seg_count))
    for hdr, raw in zip(seg_headers, new_seg_raw):
        new_data.extend(hdr)
        new_data.extend(_write_varuint(len(raw)))
        new_data.extend(raw)

    # Replace the data section in the module
    new_sections: list[tuple[int, bytes]] = []
    for sid, payload in sections:
        if sid == 11:
            new_sections.append((sid, bytes(new_data)))
        else:
            new_sections.append((sid, payload))

    print(
        f"Scrubbed {saved_path_bytes:,} bytes of embedded file paths "
        f"from data section (null-padded, no size change)",
        file=sys.stderr,
    )
    return _build_sections(new_sections)


def _fixup_func_type_indices(
    data: bytes, reference_data: bytes | None = None, runtime_data: bytes | None = None
) -> bytes | None:
    # wasm-ld 22.1.1 produces valid type-index assignments; disable all
    # repair heuristics to avoid introducing corruption.
    return None
    """Detect and repair function-section type-index mismatches.

    After wasm-ld merges two relocatable modules, the type section is
    renumbered (merged + deduplicated).  In certain wasm-ld versions or
    when the linking section metadata is stale, the function-section
    entries (which map each defined function to its type index) can
    retain the *pre-merge* type indices.  This makes the binary invalid:
    a function whose body references ``local.get 2`` but whose assigned
    type only has 1 parameter triggers "unknown local: local index out of
    bounds" in strict validators (wasmtime, wasm-tools validate).

    The fix scans each function body for ``local.get`` / ``local.set`` /
    ``local.tee`` instructions to determine the minimum local count.  If
    the assigned type does not provide enough parameters (accounting for
    declared locals), we search the type section for a matching signature
    and patch the function section.

    Returns ``None`` if no repairs were needed.
    """
    try:
        sections = _parse_sections(data)
    except ValueError:
        return None

    linked_types = _parse_type_section(sections)
    if not linked_types:
        return None

    func_section_idx, func_type_indices = _parse_func_type_indices(sections)
    if not func_type_indices or func_section_idx < 0:
        return None

    repairs: dict[int, int] = {}  # code_index -> new_type_index

    code_payload = None
    for sid, payload in sections:
        if sid == 10:
            code_payload = payload
            break
    if code_payload is None:
        return None

    offset = 0
    func_count, offset = _read_varuint(code_payload, offset)
    if func_count != len(func_type_indices):
        return None

    # Skip past the code section bodies to set `offset` correctly.
    # (The old local-access heuristic was removed because wasm-ld 22.1.1
    #  produces valid type indices; the heuristic incorrectly re-assigned
    #  types for functions whose declared locals masked the parameter count.)
    for _f_idx in range(func_count):
        body_size, offset = _read_varuint(code_payload, offset)
        offset += body_size

    if not repairs:
        return None

    # -- Rebuild function section ------------------------------------------
    new_type_indices = list(func_type_indices)
    for f_idx, new_ti in repairs.items():
        new_type_indices[f_idx] = new_ti

    new_func_payload = bytearray(_write_varuint(len(new_type_indices)))
    for ti in new_type_indices:
        new_func_payload.extend(_write_varuint(ti))

    new_sections = list(sections)
    new_sections[func_section_idx] = (3, bytes(new_func_payload))

    print(
        f"Repaired {len(repairs):,} function type-index mismatches "
        f"(wasm-ld type remapping fixup)",
        file=sys.stderr,
    )
    return _build_sections(new_sections)


def _post_link_optimize(
    data: bytes,
    *,
    reference_data: bytes | None = None,
    preserve_exports: set[str] | None = None,
    preserve_reference_exports: bool = True,
    preserve_table_refs: bool = True,
) -> bytes:
    """Apply post-link optimizations to reduce V8 compilation memory pressure.

    This is the key fix for MOL-183/MOL-186: the linked artifact was
    overwhelming V8 because of debug sections, internal exports, and
    duplicate data.  Stripping them reduces the module size by 30-60%
    which directly translates to less compilation memory.

    *reference_data*, when provided, is the original (pre-link) user module.
    It enables exact type-index repair via signature matching rather than
    the heuristic body-scan fallback.
    """
    # First, repair any type-index corruption from wasm-ld merging.
    # This must run before any other pass to ensure all subsequent
    # analysis (call graph, dead code, etc.) sees valid function types.
    updated = _fixup_func_type_indices(data, reference_data=reference_data)
    if updated is not None:
        data = updated

    preserved_export_names = set(preserve_exports or ())
    if preserve_reference_exports and reference_data is not None:
        preserved_export_names.update(
            name
            for name in _collect_function_exports(reference_data)
            if not is_table_ref_export_name(name)
        )

    updated = _strip_debug_sections(data)
    if updated is not None:
        data = updated

    updated = _strip_internal_exports(
        data,
        preserve_exports=preserved_export_names,
        preserve_table_refs=preserve_table_refs,
    )
    if updated is not None:
        data = updated

    # Iteratively neutralize dead element-table entries and stub dead functions.
    # Each round of neutralization may expose new dead functions (whose only
    # callers were themselves dead), which in turn frees more element entries.
    # Typically converges in 2-3 rounds.
    for _dce_round in range(5):
        updated = _neutralize_dead_element_entries(data)
        if updated is not None:
            data = updated

        updated = _stub_dead_functions(data)
        if updated is not None:
            data = updated
        else:
            break  # No new dead functions found -- converged

    updated = _dedup_data_segments(data)
    if updated is not None:
        data = updated

    return data

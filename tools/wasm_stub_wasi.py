#!/usr/bin/env python3
"""Stub out WASI imports in a linked WASM module for freestanding deployment.

Replaces all ``wasi_snapshot_preview1`` function imports with internal
functions that trap (``unreachable``). The resulting module has zero WASI
imports and can run on any WASM engine without WASI support.

Usage::

    python tools/wasm_stub_wasi.py input.wasm -o output.wasm
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


WASM_MAGIC = b"\x00asm"
WASM_VERSION = b"\x01\x00\x00\x00"


def _read_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = shift = 0
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


def stub_wasi_imports(wasm_bytes: bytes) -> tuple[bytes, int]:
    """Replace WASI imports with internal unreachable stubs.

    Returns (modified_bytes, count_of_stubbed_imports).
    """
    sections = _parse_sections(wasm_bytes)

    # Phase 1: Parse imports to find WASI function imports and their type indices
    wasi_imports: list[tuple[int, int]] = []  # (import_index, type_index)
    non_wasi_imports: list[tuple[str, str, int, bytes]] = []  # (mod, name, kind, desc_bytes)
    import_section_idx = None
    total_import_funcs = 0

    for sec_idx, (section_id, payload) in enumerate(sections):
        if section_id != 2:
            continue
        import_section_idx = sec_idx
        offset = 0
        count, offset = _read_varuint(payload, offset)
        func_import_idx = 0  # tracks position in the function index space (funcs only)
        for _ in range(count):
            mod_name, offset = _read_string(payload, offset)
            field_name, offset = _read_string(payload, offset)
            kind = payload[offset]
            kind_start = offset
            offset += 1
            if kind == 0:  # function import
                type_idx, offset = _read_varuint(payload, offset)
                if mod_name == "wasi_snapshot_preview1":
                    wasi_imports.append((func_import_idx, type_idx))
                else:
                    desc_bytes = payload[kind_start:offset]
                    non_wasi_imports.append((mod_name, field_name, kind, desc_bytes))
                total_import_funcs += 1
                func_import_idx += 1
            else:
                # table/memory/global import — figure out size
                desc_start = offset
                if kind == 1:  # table
                    offset += 1  # reftype
                    flags, offset = _read_varuint(payload, offset)
                    _, offset = _read_varuint(payload, offset)
                    if flags & 1:
                        _, offset = _read_varuint(payload, offset)
                elif kind == 2:  # memory
                    flags, offset = _read_varuint(payload, offset)
                    _, offset = _read_varuint(payload, offset)
                    if flags & 1:
                        _, offset = _read_varuint(payload, offset)
                elif kind == 3:  # global
                    offset += 2  # valtype + mut
                desc_bytes = payload[kind_start:offset]
                non_wasi_imports.append((mod_name, field_name, kind, desc_bytes))
        break

    if not wasi_imports:
        return wasm_bytes, 0

    # Phase 2: Parse type section to get function signatures for WASI imports
    type_section_payload = None
    type_section_idx = None
    for sec_idx, (section_id, payload) in enumerate(sections):
        if section_id == 1:
            type_section_payload = payload
            type_section_idx = sec_idx
            break

    if type_section_payload is None:
        raise ValueError("No type section found")

    # Parse all types to get param/result counts
    type_signatures: dict[int, tuple[int, int]] = {}  # type_idx -> (n_params, n_results)
    offset = 0
    type_count, offset = _read_varuint(type_section_payload, offset)
    for tidx in range(type_count):
        if type_section_payload[offset] != 0x60:
            raise ValueError(f"Expected functype marker 0x60, got {type_section_payload[offset]:#x}")
        offset += 1
        n_params, offset = _read_varuint(type_section_payload, offset)
        for _ in range(n_params):
            offset += 1  # skip valtype
        n_results, offset = _read_varuint(type_section_payload, offset)
        for _ in range(n_results):
            offset += 1  # skip valtype
        type_signatures[tidx] = (n_params, n_results)

    # Phase 3: Rebuild import section without WASI imports
    new_import_payload = bytearray()
    new_import_count = len(non_wasi_imports)
    new_import_payload.extend(_write_varuint(new_import_count))
    for mod_name, field_name, kind, desc_bytes in non_wasi_imports:
        new_import_payload.extend(_write_string(mod_name))
        new_import_payload.extend(_write_string(field_name))
        new_import_payload.extend(desc_bytes)

    # Phase 4: Create stub functions for each removed WASI import.
    # Each stub is: `unreachable; end`
    # We need to add them to the function section and code section.
    n_stubbed = len(wasi_imports)

    # Parse existing function section
    func_section_idx = None
    func_section_types: list[int] = []
    for sec_idx, (section_id, payload) in enumerate(sections):
        if section_id == 3:
            func_section_idx = sec_idx
            offset = 0
            count, offset = _read_varuint(payload, offset)
            for _ in range(count):
                tidx, offset = _read_varuint(payload, offset)
                func_section_types.append(tidx)
            break

    # Parse existing code section
    code_section_idx = None
    code_bodies: list[bytes] = []
    for sec_idx, (section_id, payload) in enumerate(sections):
        if section_id == 10:
            code_section_idx = sec_idx
            offset = 0
            count, offset = _read_varuint(payload, offset)
            for _ in range(count):
                body_size, offset = _read_varuint(payload, offset)
                body_end = offset + body_size
                code_bodies.append(payload[offset:body_end])
                offset = body_end
            break

    # Build stub function bodies and prepend them (they replace imports at the
    # beginning of the function index space)
    stub_type_indices: list[int] = []
    stub_bodies: list[bytes] = []
    for _import_idx, type_idx in wasi_imports:
        stub_type_indices.append(type_idx)
        # Function body: 0 locals, unreachable, end
        body = bytes([0x00, 0x00, 0x0B])  # 0 local decls, unreachable, end
        stub_bodies.append(body)

    # New function section: stubs first, then existing functions
    new_func_types = stub_type_indices + func_section_types
    new_func_payload = bytearray()
    new_func_payload.extend(_write_varuint(len(new_func_types)))
    for tidx in new_func_types:
        new_func_payload.extend(_write_varuint(tidx))

    # New code section: stubs first, then existing code
    new_code_bodies = stub_bodies + code_bodies
    new_code_payload = bytearray()
    new_code_payload.extend(_write_varuint(len(new_code_bodies)))
    for body in new_code_bodies:
        new_code_payload.extend(_write_varuint(len(body)))
        new_code_payload.extend(body)

    # Phase 5: Fix function index references
    # Removing N WASI imports from the import section shifts all imported function
    # indices down by N. But we're adding N stub functions at the start of the
    # function section, so the net effect on function indices is zero!
    # (Import funcs [0..M] become: non-WASI imports [0..M-N] + stubs [M-N..M])
    # Wait — that's not quite right. The function index space is:
    #   [imported functions] [defined functions]
    # We're removing N imported functions and adding N defined functions.
    # The total count stays the same, so indices don't shift. But the ORDER
    # might change if the WASI imports weren't at the end.
    #
    # Actually, we need to maintain the same function index mapping. The WASI
    # imports occupied specific indices in the original import list. Let's
    # rebuild properly by keeping function indices stable.

    # Simpler approach: instead of reordering, just replace the WASI imports
    # with internal imports from a "molt_wasi_stub" module that we never actually
    # import from — but wait, that still leaves imports.
    #
    # Simplest correct approach: replace WASI import module names with empty
    # function bodies by keeping them as imports but from a module we control.
    # No — the goal is ZERO WASI imports.
    #
    # The correct approach: we must maintain function indices. So we:
    # 1. Keep the import section order but replace WASI imports with stub functions
    # 2. The stubs take the same index position as the original imports
    #
    # To do this properly, we rebuild the function index space:
    # - Non-WASI imports keep their original positions relative to each other
    # - WASI import positions become stub function positions
    # This requires an index remapping.
    #
    # Actually the simplest truly correct approach is even simpler. Instead of
    # removing WASI imports, we replace their module name with an empty stub
    # approach by converting them from imports to internal functions. But this
    # changes the function index space layout.
    #
    # Let me use the truly simplest approach: keep all imports but rewrite
    # WASI import entries as function definitions in the code section.
    # This means the function index numbering stays identical.
    pass

    # ===== SIMPLEST APPROACH =====
    # Instead of the complex rewrite above, just replace each WASI import with
    # an unreachable function body inline by:
    # 1. Removing WASI entries from the import section
    # 2. Inserting stub function types + bodies at the START of function/code sections
    # 3. Since we removed N imports and added N functions, the function index space
    #    stays exactly the same size and all indices remain valid.
    #
    # The key insight: in WASM, the function index space is:
    #   indices [0, num_imports) = imported functions
    #   indices [num_imports, num_imports + num_funcs) = defined functions
    #
    # If we remove K WASI imports (shifting down), we need to add K stub functions
    # at position 0 of the defined functions. The indices work out:
    #   Old: import[0..M] then func[0..F] → indices 0..M+F
    #   New: import[0..M-K] then stub[0..K] then func[0..F] → indices 0..M+F
    # Total is the same. But INDEX MAPPING changes:
    #   Old import i (non-WASI) → may shift to i' in new imports
    #   Old import i (WASI) → becomes stub at position (M-K) + stub_position
    #
    # This means we need to build an index remap table and rewrite ALL function
    # references in the module (calls, element segments, exports, etc.)
    # This is complex but correct.

    # Build the index remap: old_func_idx -> new_func_idx
    n_original_imports = total_import_funcs
    # old_non_wasi_import_indices and old_wasi_import_indices
    wasi_import_indices = {idx for idx, _ in wasi_imports}
    remap: dict[int, int] = {}
    new_import_func_idx = 0
    for old_idx in range(n_original_imports):
        if old_idx not in wasi_import_indices:
            remap[old_idx] = new_import_func_idx
            new_import_func_idx += 1

    # WASI imports become stubs starting at (n_original_imports - n_stubbed) + their stub position
    new_defined_base = n_original_imports - n_stubbed
    stub_pos = 0
    for old_idx in sorted(wasi_import_indices):
        remap[old_idx] = new_defined_base + stub_pos
        stub_pos += 1

    # Existing defined functions shift: their old index was (n_original_imports + i),
    # new index is (n_original_imports - n_stubbed + n_stubbed + i) = (n_original_imports + i)
    # Wait — that means they DON'T shift! Let me recalculate:
    # New layout:
    #   imports: (n_original_imports - n_stubbed) non-WASI imports
    #   defined: n_stubbed stubs + original defined functions
    # So defined function i (originally at index n_original_imports + i) is now at:
    #   (n_original_imports - n_stubbed) + n_stubbed + i = n_original_imports + i
    # The same! So existing defined functions keep their indices.
    # Only the import function indices change.

    # Since existing defined function indices don't change, we only need to
    # remap references to import functions.
    # For simplicity, build the full remap including identity for defined funcs.
    n_defined = len(func_section_types)
    for i in range(n_defined):
        old_idx = n_original_imports + i
        remap[old_idx] = old_idx  # identity

    # Check if any function indices actually changed.  When removing N WASI
    # imports and inserting N stub functions the total function count stays the
    # same and – in many common layouts – every function keeps its original
    # index.  In that case we can skip the expensive (and potentially risky,
    # e.g. SIMD/atomics) body/element/export rewriting entirely.
    changed_indices = {old: new for old, new in remap.items() if old != new}

    # Now rebuild the sections
    new_sections: list[tuple[int, bytes]] = []
    for sec_idx, (section_id, payload) in enumerate(sections):
        if section_id == 2:
            new_sections.append((2, bytes(new_import_payload)))
        elif section_id == 3:
            new_sections.append((3, bytes(new_func_payload)))
        elif section_id == 10:
            new_sections.append((10, bytes(new_code_payload)))
        elif section_id == 7:
            # Export section — remap function indices (only if indices changed)
            if changed_indices:
                new_sections.append((7, _remap_export_section(payload, remap)))
            else:
                new_sections.append((section_id, payload))
        elif section_id == 9:
            # Element section — remap function indices (only if indices changed)
            if changed_indices:
                new_sections.append((9, _remap_element_section(payload, remap)))
            else:
                new_sections.append((section_id, payload))
        else:
            new_sections.append((section_id, payload))

    # If any import indices actually changed value, rewrite call instructions
    # in the code section.  When indices are all identity-mapped we skip this
    # entirely — which also avoids entering the body rewriter where SIMD or
    # atomics instructions would cause an error.
    if changed_indices:
        code_sec_idx = None
        for i, (sid, _) in enumerate(new_sections):
            if sid == 10:
                code_sec_idx = i
                break
        if code_sec_idx is not None:
            new_sections[code_sec_idx] = (
                10,
                _remap_code_section(new_sections[code_sec_idx][1], remap),
            )

    result_bytes = _build_sections(new_sections)

    # Optionally validate the output with wasm-validate
    valid, msg = validate_wasm(result_bytes)
    if not valid:
        print(
            f"wasm-validate warning: stubbed output failed validation: {msg}",
            file=sys.stderr,
        )

    return result_bytes, n_stubbed


def validate_wasm(wasm_bytes: bytes) -> tuple[bool, str]:
    """Validate a WASM binary using wasm-validate from wabt (if installed).

    Returns (True, "") on success, (False, stderr) on failure.
    If wasm-validate is not installed, returns (True, "wasm-validate not installed").
    """
    exe = shutil.which("wasm-validate")
    if exe is None:
        return True, "wasm-validate not installed"
    with tempfile.NamedTemporaryFile(suffix=".wasm", delete=False) as f:
        f.write(wasm_bytes)
        f.flush()
        tmp_path = f.name
    try:
        result = subprocess.run(
            [exe, tmp_path],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0:
            return True, ""
        return False, result.stderr.strip()
    except Exception as exc:
        return False, str(exc)
    finally:
        try:
            Path(tmp_path).unlink()
        except OSError:
            pass


def _remap_export_section(payload: bytes, remap: dict[int, int]) -> bytes:
    """Remap function indices in the export section."""
    output = bytearray()
    offset = 0
    count, offset = _read_varuint(payload, offset)
    output.extend(_write_varuint(count))
    for _ in range(count):
        name, offset = _read_string(payload, offset)
        output.extend(_write_string(name))
        kind = payload[offset]
        output.append(kind)
        offset += 1
        idx, offset = _read_varuint(payload, offset)
        if kind == 0:  # function export
            idx = remap.get(idx, idx)
        output.extend(_write_varuint(idx))
    return bytes(output)


def _remap_element_section(payload: bytes, remap: dict[int, int]) -> bytes:
    """Remap function indices in element segments.

    This handles the common element segment formats (flags 0x00 and 0x02).
    """
    # For correctness, we'd need to handle all 8 element segment formats.
    # For now, do a best-effort: parse format 0x00 (active, funcref, func indices).
    output = bytearray()
    offset = 0
    count, offset = _read_varuint(payload, offset)
    output.extend(_write_varuint(count))

    for _ in range(count):
        if offset >= len(payload):
            break
        flags, offset = _read_varuint(payload, offset)
        output.extend(_write_varuint(flags))

        if flags in (0x02, 0x06):
            # table index
            tidx, offset = _read_varuint(payload, offset)
            output.extend(_write_varuint(tidx))

        if flags in (0x00, 0x02, 0x04, 0x06):
            # offset expression — uses signed LEB128 for i32.const/i64.const
            while offset < len(payload):
                opcode = payload[offset]
                output.append(opcode)
                offset += 1
                if opcode == 0x0B:
                    break
                elif opcode == 0x41:  # i32.const (signed LEB128)
                    val, offset = _read_signed_leb128(payload, offset)
                    output.extend(_write_signed_leb128(val))
                elif opcode == 0x42:  # i64.const (signed LEB128)
                    val, offset = _read_signed_leb128(payload, offset)
                    output.extend(_write_signed_leb128(val))
                elif opcode == 0x23:  # global.get (unsigned index)
                    val, offset = _read_varuint(payload, offset)
                    output.extend(_write_varuint(val))

        if flags in (0x01, 0x02, 0x03, 0x05, 0x07):
            # elemkind (0x01-0x03) or reftype (0x05, 0x07)
            output.append(payload[offset])
            offset += 1

        if flags in (0x00, 0x01, 0x02, 0x03):
            # vector of function indices
            elem_count, offset = _read_varuint(payload, offset)
            output.extend(_write_varuint(elem_count))
            for _ in range(elem_count):
                func_idx, offset = _read_varuint(payload, offset)
                func_idx = remap.get(func_idx, func_idx)
                output.extend(_write_varuint(func_idx))
        else:
            # Expression-based elements (flags 4-7)
            elem_count, offset = _read_varuint(payload, offset)
            output.extend(_write_varuint(elem_count))
            for _ in range(elem_count):
                while offset < len(payload):
                    opcode = payload[offset]
                    output.append(opcode)
                    offset += 1
                    if opcode == 0x0B:
                        break
                    elif opcode == 0xD2:  # ref.func
                        func_idx, offset = _read_varuint(payload, offset)
                        func_idx = remap.get(func_idx, func_idx)
                        output.extend(_write_varuint(func_idx))
                    elif opcode == 0xD0:  # ref.null
                        output.append(payload[offset])
                        offset += 1

    return bytes(output)


def _remap_code_section(payload: bytes, remap: dict[int, int]) -> bytes:
    """Remap call instruction targets in the code section.

    This handles `call` (0x10) instructions. The `call_indirect` (0x11)
    instruction uses a table index + type index, not a function index directly.
    """
    # Only remap if there are actually changed indices
    changed = {old: new for old, new in remap.items() if old != new}
    if not changed:
        return payload

    # We need to find and rewrite `call` instructions (opcode 0x10) followed
    # by a function index. This requires parsing the full instruction stream
    # for each function body.
    #
    # For safety and correctness, we use the varuint-aware approach: parse
    # each function body's instruction stream and rewrite call targets.
    offset = 0
    count, offset = _read_varuint(payload, offset)
    new_payload = bytearray(_write_varuint(count))

    for _ in range(count):
        body_size, offset = _read_varuint(payload, offset)
        body_start = offset
        body_end = offset + body_size
        body = payload[body_start:body_end]

        new_body = _remap_function_body(body, changed)
        new_payload.extend(_write_varuint(len(new_body)))
        new_payload.extend(new_body)
        offset = body_end

    return bytes(new_payload)


def _remap_function_body(body: bytes, remap: dict[int, int]) -> bytes:
    """Rewrite call instruction targets in a single function body.

    This is a simplified parser that handles the most common instructions.
    It may miss some edge cases in complex WASM but handles the standard
    instruction set used by wasm-ld output.
    """
    # Parse local declarations first
    output = bytearray()
    offset = 0
    n_local_decls, offset = _read_varuint(body, offset)
    output.extend(_write_varuint(n_local_decls))
    for _ in range(n_local_decls):
        count, offset = _read_varuint(body, offset)
        output.extend(_write_varuint(count))
        output.append(body[offset])  # valtype
        offset += 1

    # Now parse instructions
    while offset < len(body):
        opcode = body[offset]
        output.append(opcode)
        offset += 1

        if opcode in (0x10, 0x12):  # call, return_call — func index immediate
            func_idx, offset = _read_varuint(body, offset)
            func_idx = remap.get(func_idx, func_idx)
            output.extend(_write_varuint(func_idx))
        elif opcode == 0x0B:  # end
            pass
        elif opcode == 0x00:  # unreachable
            pass
        elif opcode == 0x01:  # nop
            pass
        elif opcode in (0x02, 0x03, 0x04):  # block, loop, if
            output.append(body[offset])  # blocktype
            offset += 1
        elif opcode == 0x05:  # else
            pass
        elif opcode == 0x08:  # throw — tag index immediate
            idx, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(idx))
        elif opcode == 0x09:  # rethrow — label immediate
            idx, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(idx))
        elif opcode in (0x0C, 0x0D):  # br, br_if
            idx, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(idx))
        elif opcode == 0x0E:  # br_table
            n, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(n))
            for _ in range(n + 1):
                idx, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(idx))
        elif opcode == 0x0F:  # return
            pass
        elif opcode in (0x11, 0x13):  # call_indirect, return_call_indirect
            type_idx, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(type_idx))
            table_idx, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(table_idx))
        elif opcode == 0x1A:  # drop
            pass
        elif opcode == 0x1B:  # select
            pass
        elif opcode == 0x1F:  # try_table — blocktype + catch vector
            output.append(body[offset])  # blocktype byte
            offset += 1
            n_catches, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(n_catches))
            for _ in range(n_catches):
                catch_kind = body[offset]
                output.append(catch_kind)
                offset += 1
                if catch_kind in (0x00, 0x01):  # catch, catch_ref — tag + label
                    tag_idx, offset = _read_varuint(body, offset)
                    output.extend(_write_varuint(tag_idx))
                    label, offset = _read_varuint(body, offset)
                    output.extend(_write_varuint(label))
                elif catch_kind in (0x02, 0x03):  # catch_all, catch_all_ref — label only
                    label, offset = _read_varuint(body, offset)
                    output.extend(_write_varuint(label))
        elif opcode in (0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26):
            # local.get/set/tee, global.get/set, table.get/set
            idx, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(idx))
        elif 0x28 <= opcode <= 0x3E:  # load/store instructions (align + offset)
            align, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(align))
            mem_offset, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(mem_offset))
        elif opcode in (0x3F, 0x40):  # memory.size, memory.grow
            output.append(body[offset])  # memory index
            offset += 1
        elif opcode == 0x41:  # i32.const
            val, offset = _read_signed_leb128(body, offset)
            output.extend(_write_signed_leb128(val))
        elif opcode == 0x42:  # i64.const
            val, offset = _read_signed_leb128(body, offset)
            output.extend(_write_signed_leb128(val))
        elif opcode == 0x43:  # f32.const
            output.extend(body[offset:offset + 4])
            offset += 4
        elif opcode == 0x44:  # f64.const
            output.extend(body[offset:offset + 8])
            offset += 8
        elif opcode == 0xD2:  # ref.func
            func_idx, offset = _read_varuint(body, offset)
            func_idx = remap.get(func_idx, func_idx)
            output.extend(_write_varuint(func_idx))
        elif opcode == 0xD0:  # ref.null
            output.append(body[offset])  # reftype
            offset += 1
        elif opcode == 0xFC:  # multi-byte prefix (misc instructions)
            sub_opcode, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(sub_opcode))
            if sub_opcode in (8, 10, 12, 14):  # memory.init, memory.copy, table.init, table.copy
                idx1, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(idx1))
                idx2, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(idx2))
            elif sub_opcode in (9, 11, 13):  # data.drop, memory.fill, elem.drop
                idx, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(idx))
            elif sub_opcode in (15, 16, 17):  # table.grow, table.size, table.fill
                idx, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(idx))
        elif opcode == 0xFD:  # SIMD prefix
            # SIMD instructions never contain function indices, so we can
            # copy them through safely. Parse the sub-opcode and its immediates.
            sub_opcode, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(sub_opcode))
            if sub_opcode == 12:  # v128.const — 16 byte immediate
                output.extend(body[offset:offset + 16])
                offset += 16
            elif sub_opcode == 13:  # i8x16.shuffle — 16 byte lane mask
                output.extend(body[offset:offset + 16])
                offset += 16
            elif 0 <= sub_opcode <= 11:
                # v128.load, v128.store variants — memarg (align + offset)
                align, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(align))
                mem_offset, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(mem_offset))
            elif 84 <= sub_opcode <= 91:
                # v128.loadN_lane, v128.storeN_lane — memarg + lane index
                align, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(align))
                mem_offset, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(mem_offset))
                output.append(body[offset])  # lane index
                offset += 1
            elif sub_opcode in (21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34):
                # extractlane/replacelane — 1 byte lane index
                output.append(body[offset])
                offset += 1
            # All other SIMD opcodes (14-20, 35-83, 92+) have no immediates
        elif opcode == 0xFE:  # Atomics prefix
            # Atomic instructions don't contain function indices.
            sub_opcode, offset = _read_varuint(body, offset)
            output.extend(_write_varuint(sub_opcode))
            if sub_opcode == 0:  # memory.atomic.notify — memarg
                align, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(align))
                mem_offset, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(mem_offset))
            elif sub_opcode == 1 or sub_opcode == 2:
                # memory.atomic.wait32/wait64 — memarg
                align, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(align))
                mem_offset, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(mem_offset))
            elif sub_opcode == 3:  # atomic.fence — 1 byte (0x00)
                output.append(body[offset])
                offset += 1
            elif 16 <= sub_opcode <= 78:
                # atomic load/store/rmw/cmpxchg — all take memarg
                align, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(align))
                mem_offset, offset = _read_varuint(body, offset)
                output.extend(_write_varuint(mem_offset))
        # All other opcodes (0x45-0xC4 numeric ops, etc.) have no immediates

    return bytes(output)


def _read_signed_leb128(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if byte & 0x80 == 0:
            if byte & 0x40:
                result -= 1 << shift
            break
    return result, offset


def _write_signed_leb128(value: int) -> bytes:
    parts: list[int] = []
    while True:
        byte = value & 0x7F
        value >>= 7
        if (value == 0 and not (byte & 0x40)) or (value == -1 and (byte & 0x40)):
            parts.append(byte)
            break
        parts.append(byte | 0x80)
    return bytes(parts)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Stub out WASI imports in a linked WASM module for freestanding deployment.",
    )
    parser.add_argument("input", type=Path, help="Input .wasm file")
    parser.add_argument("-o", "--output", type=Path, help="Output .wasm file")
    args = parser.parse_args()

    if not args.input.exists():
        print(f"Input not found: {args.input}", file=sys.stderr)
        return 1

    output = args.output or args.input.with_stem(args.input.stem + "_freestanding")

    wasm_bytes = args.input.read_bytes()
    result, n_stubbed = stub_wasi_imports(wasm_bytes)
    output.write_bytes(result)

    if n_stubbed:
        print(
            f"Stubbed {n_stubbed} WASI imports → {output} "
            f"({len(result):,} bytes)",
            file=sys.stderr,
        )
    else:
        print("No WASI imports found; output is identical.", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

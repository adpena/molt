#!/usr/bin/env python3
from __future__ import annotations

import argparse
import functools
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import warnings
from collections import Counter
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

# SHA-256 integrity hashes for known runtime binaries.  When a filename appears
# in this dict the linker will verify the on-disk file matches the expected hash
# before passing it to wasm-ld.  Populate entries when cutting a release:
#
#   python3 -c "import hashlib, sys; print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())" molt_runtime.wasm
#
# Pin SHA-256 hashes of known-good runtime binaries.  The linker will reject
# any runtime whose hash does not match.  Update this dict when cutting a
# release or after rebuilding the runtime (run: shasum -a 256 molt_runtime.wasm).
RUNTIME_EXPECTED_HASHES: dict[str, str] = {
    "molt_runtime.wasm": "ed92be1505a4865c1b458db5944032b34d53fa0d8e0b98fcfc50ca935687f12a",
}
_OUTPUT_RUNTIME_EXPORT_ALIASES = (
    "molt_isolate_bootstrap",
    "molt_isolate_import",
)
_OUTPUT_EXPORT_ALIAS_PREFIX = "__molt_export_alias__"
_INTERNAL_OUTPUT_EXPORT_PREFIXES = (
    "molt_module_chunk_",
    "genexpr_",
    "listcomp_",
    "dictcomp_",
    "setcomp_",
    "lambda_",
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


def _default_dist_artifact_path(name: str) -> Path:
    ext_root = os.environ.get("MOLT_EXT_ROOT")
    external_root = Path(ext_root).expanduser() if ext_root else None
    if external_root is not None and external_root.is_dir():
        return external_root / "dist" / name
    return Path("dist") / name


def _default_input_path() -> Path:
    return _default_dist_artifact_path("output.wasm")


def _default_output_path() -> Path:
    return _default_dist_artifact_path("output_linked.wasm")


def _is_wasm_binary(data: bytes) -> bool:
    return len(data) >= 8 and data[:4] == WASM_MAGIC and data[4:8] == WASM_VERSION


def _runtime_integrity_sidecar_path(path: Path) -> Path:
    return path.with_name(f"{path.name}.sha256")


def _read_runtime_integrity_sidecar(path: Path) -> str | None:
    sidecar = _runtime_integrity_sidecar_path(path)
    if not sidecar.exists():
        return None
    raw = sidecar.read_text(encoding="utf-8").strip()
    match = re.search(r"\b([0-9a-fA-F]{64})\b", raw)
    if match is None:
        raise SystemExit(
            f"Runtime integrity sidecar is malformed: {sidecar}"
        )
    return match.group(1).lower()


def _verify_runtime_integrity(path: Path) -> None:
    """Verify SHA-256 integrity of the runtime binary.

    Raises ``SystemExit`` when a hash mismatch is detected.  The check can be
    bypassed by setting ``MOLT_SKIP_RUNTIME_VERIFY=1`` in the environment
    (intended for local development only).
    """
    if os.environ.get("MOLT_SKIP_RUNTIME_VERIFY") == "1":
        return

    # Reject path-traversal components before reading the file.
    for part in path.parts:
        if part == "..":
            raise SystemExit(
                f"Runtime path contains '..' traversal component: {path}"
            )

    data = path.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    filename = path.name
    sidecar_expected = _read_runtime_integrity_sidecar(path)
    if sidecar_expected is not None:
        if digest != sidecar_expected:
            raise SystemExit(
                f"Runtime integrity check failed for {path}\n"
                f"  source: sidecar { _runtime_integrity_sidecar_path(path) }\n"
                f"  expected SHA-256: {sidecar_expected}\n"
                f"  actual   SHA-256: {digest}\n"
            )
        return

    if not RUNTIME_EXPECTED_HASHES:
        warnings.warn(
            "RUNTIME_EXPECTED_HASHES is empty — runtime integrity is NOT verified. "
            "Pin hashes before releasing.",
            stacklevel=2,
        )
        return

    if filename not in RUNTIME_EXPECTED_HASHES:
        warnings.warn(
            f"Runtime file '{filename}' has no pinned SHA-256 hash in "
            f"RUNTIME_EXPECTED_HASHES — integrity not verified.",
            stacklevel=2,
        )
        return

    expected = RUNTIME_EXPECTED_HASHES[filename]
    if digest != expected:
        raise SystemExit(
            f"Runtime integrity check failed for {path}\n"
            f"  expected SHA-256: {expected}\n"
            f"  actual   SHA-256: {digest}\n"
            f"Set MOLT_SKIP_RUNTIME_VERIFY=1 to bypass (development only)."
        )


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
            (section_id, _build_custom_section(name, _build_linking_payload(version, new_subsections)))
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
                    _build_linking_payload(2, [(SYMTAB_SUBSECTION_ID, bytes(payload_bytes))]),
                ),
            )
        )
        modified = True
    if not modified:
        return None
    return _build_sections(new_sections)


def _dump_symbols(path: Path, wasm_tools: str) -> list[tuple[int, int, str, str]]:
    try:
        data = path.read_bytes()
    except OSError as exc:
        print(f"Failed to read wasm symbols from {path}: {exc}", file=sys.stderr)
        return []
    try:
        parsed = _collect_linking_function_symbols(data)
    except ValueError as exc:
        print(
            f"Failed to parse linking symbol table from {path}: {exc}",
            file=sys.stderr,
        )
        parsed = []
    if parsed:
        return parsed
    if not wasm_tools:
        return []
    res = subprocess.run(
        [wasm_tools, "dump", str(path)],
        capture_output=True,
        text=True,
        timeout=120,
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
                    0x00, 0x01, 0x05, 0x0B, 0x0F, 0x1A, 0x1B,
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
                    0x0C, 0x0D,  # br, br_if
                    0x20, 0x21, 0x22, 0x23, 0x24,  # local/global ops
                    0x25, 0x26,  # table.get, table.set
                    0xD0,  # ref.null (heaptype)
                    0xD4, 0xD5,  # br_on_null, br_on_non_null
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


def _append_table_ref_elements(
    data: bytes,
    *,
    min_table_index: int = 0,
    allowed_table_indices: set[int] | None = None,
) -> bytes | None:
    table_refs: dict[int, int] = {}
    for func_idx, name in _collect_func_names(data).items():
        match = re.fullmatch(r"__molt_table_ref_(\d+)", name)
        if match is not None:
            table_idx = int(match.group(1))
            if table_idx >= min_table_index and (
                allowed_table_indices is None or table_idx in allowed_table_indices
            ):
                table_refs[table_idx] = func_idx
    for name, func_idx in _collect_function_exports(data).items():
        match = re.fullmatch(r"__molt_table_ref_(\d+)", name)
        if match is not None:
            table_idx = int(match.group(1))
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


def _linked_runtime_active_table_end(data: bytes) -> int:
    """Return the exclusive end of the runtime-owned active table prefix."""
    for section_id, payload in _parse_sections(data):
        if section_id != 9:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_varuint(payload, offset)
            if flags == 0:
                if offset >= len(payload) or payload[offset] != 0x41:
                    return 0
                start, offset = _read_varuint(payload, offset + 1)
                if offset >= len(payload) or payload[offset] != 0x0B:
                    return 0
                offset += 1
                n, offset = _read_varuint(payload, offset)
                if start == 1:
                    return start + n
                for _item in range(n):
                    _, offset = _read_varuint(payload, offset)
                continue
            if flags == 1:
                offset += 1
                n, offset = _read_varuint(payload, offset)
                for _item in range(n):
                    _, offset = _read_varuint(payload, offset)
                continue
            return 0
    return 0


def _find_call_indirect_mangled(runtime: Path) -> dict[str, str]:
    wasm_tools = _find_tool(["wasm-tools"])
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
    if not names and not wasm_tools:
        print(
            "wasm-tools not found; cannot extract call_indirect symbol name.",
            file=sys.stderr,
        )
    if not names:
        print("Unable to locate runtime call_indirect symbol names.", file=sys.stderr)
    return names


def _find_output_call_indirect_symbol(output: Path) -> dict[str, tuple[int, int]]:
    wasm_tools = _find_tool(["wasm-tools"])
    symbols: dict[str, tuple[int, int]] = {}
    for flags, index, name, _ in _dump_symbols(output, wasm_tools):
        if CALL_INDIRECT_RE.fullmatch(name):
            symbols[name] = (index, flags)
    if not symbols and not wasm_tools:
        print(
            "wasm-tools not found; cannot extract output symbol info.", file=sys.stderr
        )
    if not symbols:
        print("Unable to locate output call_indirect symbols.", file=sys.stderr)
    return symbols


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
            alias_entry.extend(
                _write_varuint(entry_flags | FLAG_EXPLICIT_NAME)
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
    data = output.read_bytes()
    wrapper_specs = _collect_output_wrapper_specs(data)
    if not wrapper_specs:
        return output
    try:
        sections = _parse_sections(data)
    except ValueError as exc:
        print(f"Failed to parse output module for export aliasing: {exc}", file=sys.stderr)
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
            for i, (_name, alias_name, _type_idx, _target_idx) in enumerate(wrapper_specs):
                wrapper_func_index = import_count + original_func_count + i
                wrapper_index_by_name[alias_name] = wrapper_func_index
                updated_payload.extend(_write_string(alias_name))
                updated_payload.append(0)
                updated_payload.extend(_write_varuint(wrapper_func_index))
                wrapper_symbol_entries.append(
                    (
                        alias_name,
                        wrapper_func_index,
                        FLAG_BINDING_GLOBAL | FLAG_EXPLICIT_NAME | FLAG_EXPORTED | FLAG_NO_STRIP,
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
                local_count = 1 if results and len(results) == 1 and inc_ref_import_index is not None else 0
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
        if name.startswith("__molt_table_ref_"):
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
    return [name for name, _alias, _type_idx, _func_idx in _collect_output_wrapper_specs(data)]


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
                (name for name in candidates if name.startswith("__molt_output_export_")),
                None,
            )
        if preferred is None and candidates:
            preferred = candidates[0]
        if preferred is not None:
            mapping[public_name] = preferred
    return mapping


def _inject_output_runtime_entrypoint_aliases(
    output: Path, temp_dir: tempfile.TemporaryDirectory
) -> Path:
    data = output.read_bytes()
    export_indices = _collect_function_exports(data)
    symbol_map = _collect_output_export_symbol_map(data)
    updated = data
    modified = False
    for public_name in _OUTPUT_RUNTIME_EXPORT_ALIASES:
        target_symbol = symbol_map.get(public_name)
        func_index = export_indices.get(public_name)
        if target_symbol is None or func_index is None or target_symbol == public_name:
            continue
        next_data = _add_symtab_alias(
            updated,
            public_name,
            func_index,
            FLAG_BINDING_GLOBAL | FLAG_EXPLICIT_NAME | FLAG_NO_STRIP,
            preserve_export=False,
        )
        if next_data is not None:
            updated = next_data
            modified = True
    if not modified:
        return output
    alias_path = Path(temp_dir.name) / "output_runtime_aliases.wasm"
    alias_path.write_bytes(updated)
    return alias_path


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
            key=lambda name: (name.startswith("__molt_table_ref_"), not name.startswith("molt_init_"), name),
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


def _find_func_import_index(data: bytes, module_name: str, import_name: str) -> int | None:
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


def _highest_exported_table_ref_index(data: bytes) -> int | None:
    refs = [
        int(name.removeprefix("__molt_table_ref_"))
        for name in _collect_function_exports(data)
        if name.startswith("__molt_table_ref_")
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


_EMPTY_FUNC_BODY = bytes([0x00, 0x0B])


def _neutralize_linked_table_init(data: bytes) -> bytes | None:
    """Replace linked-output ``molt_table_init`` with a no-op body.

    Relocatable app modules need ``molt_table_init`` to install table entries
    into a separate runtime table. A fully linked monolith must not replay that
    initializer because its pre-link table indices can overlap runtime-owned
    active table slots after wasm-ld. App-owned slots are materialized by
    ``_append_table_ref_elements`` after the runtime-owned prefix is known.
    """
    export_indices = _collect_function_exports(data)
    table_init_index = export_indices.get("molt_table_init")
    if table_init_index is None:
        return None

    sections = _parse_sections(data)
    import_count = _count_func_imports(sections)
    local_index = table_init_index - import_count
    if local_index < 0:
        return None

    types = _parse_type_section(sections)
    _func_section_idx, func_type_indices = _parse_func_type_indices(sections)
    if local_index >= len(func_type_indices):
        return None
    type_index = func_type_indices[local_index]
    params, results = types[type_index]
    if params or results:
        raise ValueError("molt_table_init must have no params and no results")

    changed = False
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 10:
            new_sections.append((section_id, payload))
            continue

        offset = 0
        func_count, offset = _read_varuint(payload, offset)
        if local_index >= func_count:
            return None
        rebuilt = bytearray(_write_varuint(func_count))
        for idx in range(func_count):
            body_size, body_start = _read_varuint(payload, offset)
            body_end = body_start + body_size
            if idx == local_index:
                rebuilt.extend(_write_varuint(len(_EMPTY_FUNC_BODY)))
                rebuilt.extend(_EMPTY_FUNC_BODY)
                changed = True
            else:
                rebuilt.extend(_write_varuint(body_size))
                rebuilt.extend(payload[body_start:body_end])
            offset = body_end
        new_sections.append((section_id, bytes(rebuilt)))

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
            if module == "molt_runtime" and kind == 0 and not name.startswith("molt_"):
                prefixed = f"molt_{name}"
                if prefixed in runtime_exports:
                    new_name = prefixed
                    needs_rewrite = True
                elif name not in runtime_exports:
                    # The prefixed symbol is not in the runtime's export
                    # section — likely inlined away by LTO during the
                    # cdylib build.  Still rewrite to the prefixed name
                    # so wasm-ld can resolve it from a relocatable
                    # runtime that retains the symbol.
                    new_name = prefixed
                    needs_rewrite = True
                    force_exports.append(prefixed)

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


def _collect_module_imports(
    wasm_data: bytes, module_name: str
) -> set[str]:
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


def _wasm_link_build_state_root() -> Path:
    override = os.environ.get("MOLT_BUILD_STATE_DIR", "").strip()
    if override:
        root = Path(override).expanduser()
    else:
        target_dir = os.environ.get("CARGO_TARGET_DIR", "").strip()
        root = Path(target_dir).expanduser() if target_dir else Path("target")
        root = root / ".molt_state"
    if not root.is_absolute():
        root = (Path.cwd() / root).resolve()
    return root


def _tree_shake_runtime_cache_root() -> Path:
    return _wasm_link_build_state_root() / "wasm_link_cache" / "runtime_tree_shake"


@functools.lru_cache(maxsize=1)
def _wasm_link_source_digest() -> str:
    return hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


@functools.lru_cache(maxsize=4)
def _wasm_opt_version(executable: str) -> str:
    try:
        result = subprocess.run(
            [executable, "--version"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except Exception:
        return "unknown"
    output = (result.stdout or result.stderr or "").strip()
    return output or "unknown"


def _tree_shake_runtime_cache_key(
    *,
    optimized_baseline: bytes,
    normalized_required_exports: set[str],
    wasm_opt: str,
    feature_flags: list[str],
) -> str:
    hasher = hashlib.sha256()
    hasher.update(optimized_baseline)
    hasher.update(b"\0exports\0")
    for name in sorted(normalized_required_exports):
        hasher.update(name.encode("utf-8"))
        hasher.update(b"\0")
    hasher.update(b"\0wasm-opt\0")
    hasher.update(_wasm_opt_version(wasm_opt).encode("utf-8"))
    hasher.update(b"\0flags\0")
    for flag in feature_flags:
        hasher.update(flag.encode("utf-8"))
        hasher.update(b"\0")
    hasher.update(b"\0tool\0")
    hasher.update(_wasm_link_source_digest().encode("utf-8"))
    return hasher.hexdigest()


def _read_cached_tree_shaken_runtime(path: Path) -> bytes | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:8] != WASM_MAGIC + WASM_VERSION:
        return None
    return data


def _write_cached_tree_shaken_runtime(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_suffix(path.suffix + ".tmp")
    tmp_path.write_bytes(data)
    tmp_path.replace(path)


def _canonical_split_runtime_required_exports(runtime_data: bytes) -> set[str]:
    """Return runtime exports that remain app-visible split-runtime contracts."""
    return {
        name
        for name in _collect_function_exports(runtime_data)
        if name not in _ESSENTIAL_EXPORTS
        and name not in {"molt_exception_pending"}
        and not name.startswith("__molt_table_ref_")
    }


def _tree_shake_runtime(
    runtime_data: bytes,
    required_exports: set[str],
) -> bytes:
    """Strip unused exports from the runtime module and eliminate dead code.

    Rewrites the export section to only include functions in *required_exports*
    (plus memory/table/global exports which are always kept).  After stripping
    exports, runs wasm-opt ``--remove-unused-module-elements`` to GC dead
    functions.

    Returns the tree-shaken WASM bytes.  If wasm-opt is unavailable, falls back
    to export-stripping only (which still reduces the module somewhat since
    engines skip compiling unexported, unreferenced functions in some cases).
    """
    sections = _parse_sections(runtime_data)

    # Canonicalize the app import surface to the runtime export naming
    # convention.  The app imports the unprefixed ABI names (e.g. `alloc`,
    # `module_import`), while the runtime exports the corresponding
    # `molt_*` symbols.  Without this normalization, split-runtime
    # tree-shaking strips every function export even when the app has a
    # large live runtime dependency surface.
    normalized_required_exports = set(required_exports)
    normalized_required_exports.update(f"molt_{name}" for name in required_exports)
    normalized_required_exports.update(_ESSENTIAL_EXPORTS)
    # Preserve the minimal exception-inspection surface used by the direct
    # runner and browser host to marshal JS values and turn pending runtime
    # exceptions into actionable diagnostics.
    normalized_required_exports.update(
        {
            "molt_alloc",
            "molt_handle_resolve",
            "molt_header_size",
            "molt_scratch_alloc",
            "molt_scratch_free",
            "molt_bytes_from_bytes",
            "molt_string_from_bytes",
            "molt_string_as_ptr",
            "molt_exception_last",
            "molt_exception_kind",
            "molt_exception_message",
            "molt_traceback_format_exc",
            "molt_type_tag_of_bits",
            "molt_object_repr",
            "molt_profile_dump",
            "molt_dec_ref_obj",
        }
    )
    raw_dynamic_exports = os.environ.get("MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS", "").strip()
    if raw_dynamic_exports:
        normalized_required_exports.update(
            name.strip()
            for name in raw_dynamic_exports.split(",")
            if name.strip()
        )

    # Rewrite export section: keep memory/table/global exports and only
    # function exports that are in the required set.
    new_sections: list[tuple[int, bytes]] = []
    kept_exports = 0
    stripped_exports = 0

    for section_id, payload in sections:
        if section_id != 7:  # not export section
            new_sections.append((section_id, payload))
            continue

        # Parse and filter exports.
        offset = 0
        count, offset = _read_varuint(payload, offset)
        filtered: list[tuple[str, int, int]] = []  # (name, kind, index)
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF reading export kind")
            kind = payload[offset]
            offset += 1
            index, offset = _read_varuint(payload, offset)
            if kind != 0:
                # Memory (2), table (1), global (3) -- always keep.
                filtered.append((name, kind, index))
                kept_exports += 1
            elif name in normalized_required_exports:
                filtered.append((name, kind, index))
                kept_exports += 1
            else:
                stripped_exports += 1

        # Rebuild export section.
        new_payload = bytearray()
        new_payload.extend(_write_varuint(len(filtered)))
        for name, kind, index in filtered:
            new_payload.extend(_write_string(name))
            new_payload.append(kind)
            new_payload.extend(_write_varuint(index))
        new_sections.append((7, bytes(new_payload)))

    print(
        f"Runtime tree-shake: kept {kept_exports} exports, "
        f"stripped {stripped_exports} unused function exports",
        file=sys.stderr,
    )

    stripped_data = _build_sections(new_sections)
    optimized_baseline = _post_link_optimize(
        stripped_data,
        preserve_exports=normalized_required_exports,
    )
    if len(optimized_baseline) != len(stripped_data):
        print(
            f"Runtime post-link optimize: {len(stripped_data):,} -> {len(optimized_baseline):,} bytes "
            f"({len(stripped_data) - len(optimized_baseline):,} bytes eliminated)",
            file=sys.stderr,
        )

    # Use wasm-opt to eliminate dead code (functions no longer reachable
    # from the reduced export set).
    wasm_opt = shutil.which("wasm-opt")
    if not wasm_opt:
        print(
            "wasm-opt not found; skipping dead-code elimination "
            "(export stripping only)",
            file=sys.stderr,
        )
        return optimized_baseline

    with tempfile.TemporaryDirectory(prefix="molt-treeshake-") as tmp:
        input_path = Path(tmp) / "runtime_stripped.wasm"
        output_path = Path(tmp) / "runtime_shaken.wasm"
        input_path.write_bytes(optimized_baseline)

        # Feature flags matching wasm_optimize.py defaults -- avoid
        # --all-features which enables custom-descriptors (rejected by V8).
        feature_flags = [
            "--enable-bulk-memory",
            "--enable-mutable-globals",
            "--enable-sign-ext",
            "--enable-nontrapping-float-to-int",
            "--enable-simd",
            "--enable-multivalue",
            "--enable-reference-types",
            "--enable-gc",
            "--enable-tail-call",
            "--disable-custom-descriptors",
        ]

        cache_path = (
            _tree_shake_runtime_cache_root()
            / (
                _tree_shake_runtime_cache_key(
                    optimized_baseline=optimized_baseline,
                    normalized_required_exports=normalized_required_exports,
                    wasm_opt=wasm_opt,
                    feature_flags=feature_flags,
                )
                + ".wasm"
            )
        )
        cached = _read_cached_tree_shaken_runtime(cache_path)
        if cached is not None:
            print(
                f"Runtime tree-shake cache hit: {cache_path}",
                file=sys.stderr,
            )
            return cached

        cmd = [
            wasm_opt,
            str(input_path),
            "-o", str(output_path),
            "-Oz",
            "--converge",
            "--remove-unused-module-elements",
            "--closed-world",
            "--strip-debug",
            "--strip-producers",
            "--vacuum",
        ] + feature_flags

        try:
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
        except subprocess.TimeoutExpired:
            print(
                "wasm-opt tree-shake timed out (non-fatal); keeping post-link-optimized runtime",
                file=sys.stderr,
            )
            return optimized_baseline

        if result.returncode != 0:
            # wasm-opt may fail on some modules (e.g. unsupported features).
            # Fall back gracefully to export-stripped version.
            err = result.stderr.strip()
            print(
                f"wasm-opt tree-shake failed (non-fatal): {err}",
                file=sys.stderr,
            )
            return optimized_baseline

        shaken_data = output_path.read_bytes()
        savings = len(optimized_baseline) - len(shaken_data)
        print(
            f"wasm-opt tree-shake: {len(runtime_data):,} -> {len(shaken_data):,} bytes "
            f"({savings:,} bytes eliminated, "
            f"{savings / len(runtime_data) * 100:.1f}% reduction)",
            file=sys.stderr,
        )

        final_path = Path(tmp) / "runtime_final.wasm"
        final_path.write_bytes(shaken_data)
        if _run_wasm_opt_via_optimize(final_path, level="Oz"):
            final_data = final_path.read_bytes()
            try:
                _write_cached_tree_shaken_runtime(cache_path, final_data)
            except OSError:
                pass
            print(
                f"Runtime final optimize: {len(runtime_data):,} -> {len(final_data):,} bytes "
                f"({len(runtime_data) - len(final_data):,} bytes eliminated, "
                f"{(len(runtime_data) - len(final_data)) / len(runtime_data) * 100:.1f}% reduction)",
                file=sys.stderr,
            )
            return final_data
        try:
            _write_cached_tree_shaken_runtime(cache_path, shaken_data)
        except OSError:
            pass
        return shaken_data


def _optimize_split_app_module(
    app_data: bytes,
    *,
    reference_data: bytes | None,
    optimize: bool,
    optimize_level: str,
) -> bytes:
    """Deforest the split-runtime app artifact without collapsing its imports.

    The split app module must remain unlinked so it can continue importing the
    deploy runtime, but it still benefits from the same post-link cleanup passes
    as the fully linked artifact. Apply those cleanup passes first, then run
    wasm-opt when requested.
    """
    optimized = _post_link_optimize(
        app_data,
        reference_data=reference_data,
        preserve_exports=_split_app_reference_function_exports(reference_data),
        preserve_reference_exports=False,
    )
    stripped = _strip_unused_module_function_imports(
        optimized,
        module_name="molt_runtime",
    )
    if stripped is not None:
        optimized = stripped
    if not optimize:
        return optimized

    with tempfile.TemporaryDirectory(prefix="molt-split-app-opt-") as tmp:
        app_path = Path(tmp) / "app_split_preopt.wasm"
        app_path.write_bytes(optimized)
        if not _run_wasm_opt_via_optimize(app_path, level=optimize_level):
            return optimized
        return app_path.read_bytes()


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
                    raise ValueError(
                        f"Unknown import kind {kind} in runtime"
                    )

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
        (1, type_payload),                  # type section
        (3, bytes(func_payload)),           # function section
        (7, bytes(export_payload)),         # export section
        (10, bytes(code_payload)),          # code section
        (0, linking_payload),               # custom "linking" section
    ]

    return _build_sections(stub_sections)


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
    "molt_alloc",
    "molt_bytes_as_ptr",
    "molt_bytes_from_bytes",
    "molt_dec_ref_obj",
    "molt_exception_kind",
    "molt_exception_last",
    "molt_exception_message",
    "molt_exception_pending",
    "molt_exception_pending_fast",
    "molt_handle_resolve",
    "molt_header_size",
    "memory",
    "molt_memory",
    "molt_host_init",
    "molt_list_builder_append",
    "molt_list_builder_finish",
    "molt_list_builder_new",
    "molt_main",
    "molt_object_repr",
    "molt_scratch_alloc",
    "molt_scratch_free",
    "molt_string_as_ptr",
    "molt_string_from_bytes",
    "molt_table_init",
    "molt_table",
    "molt_traceback_format_exc",
    "molt_type_tag_of_bits",
    "molt_set_wasm_table_base",
    "__indirect_function_table",
})


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
    return {
        name for name in _collect_function_exports(reference_data) if name in keep
    }


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
            if (
                name not in keep_exports
                and (not preserve_table_refs or not name.startswith("__molt_table_ref_"))
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
            elif kind == 1:  # table
                offset += 1
                flags, offset = _read_varuint(payload, offset)
                _, offset = _read_varuint(payload, offset)
                if flags & 0x1:
                    _, offset = _read_varuint(payload, offset)
            elif kind == 2:  # memory
                flags, offset = _read_varuint(payload, offset)
                _, offset = _read_varuint(payload, offset)
                if flags & 0x1:
                    _, offset = _read_varuint(payload, offset)
            elif kind == 3:  # global
                offset += 2
            else:
                raise ValueError(f"Unknown import kind {kind}")
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
    if _code_section_has_call_indirect(sections) or _module_imports_host_call_indirect(sections):
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
                0x00, 0x01, 0x05, 0x0B, 0x0F, 0x1A, 0x1B,
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
                0x0C, 0x0D,  # br, br_if
                0x20, 0x21, 0x22, 0x23, 0x24,  # local/global ops
                0x25, 0x26,  # table.get, table.set
                0x3F, 0x40,  # memory.size, memory.grow
                0xD0,  # ref.null (heaptype)
                0xD4, 0xD5,  # br_on_null, br_on_non_null
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
                    _, pos = _read_varsint(code_payload, pos)  # type index (signed LEB128)
                # Catch vector
                n_catches, pos = _read_varuint(code_payload, pos)
                for _ in range(n_catches):
                    catch_kind = code_payload[pos]
                    pos += 1
                    if catch_kind in (0x00, 0x01):  # catch / catch_ref: tag_index + label
                        _, pos = _read_varuint(code_payload, pos)
                        _, pos = _read_varuint(code_payload, pos)
                    elif catch_kind in (0x02, 0x03):  # catch_all / catch_all_ref: label only
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
                    offset = _skip_init_expr(payload, offset)  # proper LEB128-aware skip
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
            _, offset = _rewrite_init_expr_func_indices(payload, offset, lambda idx: idx)
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
            new_body = _rewrite_code_body(payload[body_start:body_end], remap_func_index)
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
            import_entries.append((module, name, kind, payload[desc_start:offset], func_index))
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
                new_sections.append((sid, _rewrite_export_section(payload, remap_func_index)))
            elif sid == 8:
                new_sections.append((sid, _rewrite_start_section(payload, remap_func_index)))
            elif sid == 9:
                new_sections.append((sid, _rewrite_element_section(payload, remap_func_index)))
            elif sid == 6:
                new_sections.append((sid, _rewrite_global_section(payload, remap_func_index)))
            elif sid == 10:
                new_sections.append((sid, _rewrite_code_section(payload, remap_func_index)))
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


def _parse_type_section(sections: list[tuple[int, bytes]]) -> list[tuple[tuple[int, ...], tuple[int, ...]]]:
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


def _parse_func_type_indices(sections: list[tuple[int, bytes]]) -> tuple[int, list[int]]:
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


def _fixup_func_type_indices(data: bytes, reference_data: bytes | None = None, runtime_data: bytes | None = None) -> bytes | None:
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
            if not name.startswith("__molt_table_ref_")
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
    # Run wasm-tools validate for full structural validation (type-index
    # consistency, local-index bounds, etc.).  This catches wasm-ld
    # type-remapping bugs that simpler checks miss.
    exe = shutil.which("wasm-tools")
    if exe is not None:
        validate_data = _strip_debug_sections(data) or data
        with tempfile.NamedTemporaryFile(suffix=".wasm", delete=False) as f:
            f.write(validate_data)
            f.flush()
            tmp_path = f.name
        try:
            result = subprocess.run(
                [exe, "validate", tmp_path],
                capture_output=True,
                text=True,
                timeout=60,
            )
            if result.returncode != 0:
                print(
                    f"Linked wasm failed structural validation: "
                    f"{result.stderr.strip()[:500]}",
                    file=sys.stderr,
                )
                return False
        except Exception as exc:
            print(
                f"wasm-tools validate warning: {exc}",
                file=sys.stderr,
            )
        finally:
            try:
                Path(tmp_path).unlink()
            except OSError:
                pass
    return True


# Pass pipelines from docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md Section 4.4.
_OZ_PASSES: list[str] = [
    "--closed-world",
    "--remove-unused-module-elements",
    "--remove-unused-names",
    "--strip-debug",
    "--strip-producers",
    "--coalesce-locals",
    "--reorder-locals",
    "--merge-locals",
    "--dce",
    "--vacuum",
    "--duplicate-function-elimination",
    "--code-folding",
    "--merge-similar-functions",
    "--simplify-globals-optimizing",
    "--precompute",
    "--merge-blocks",
    "--optimize-stack-ir",
    "--reorder-functions",
    "--dae-optimizing",
]

_O3_PASSES: list[str] = [
    "--closed-world",
    "--remove-unused-module-elements",
    "--remove-unused-names",
    "--strip-producers",
    "--coalesce-locals",
    "--reorder-locals",
    "--merge-locals",
    "--dce",
    "--vacuum",
    "--inlining",
    "--flatten",
    "--local-cse",
    "--optimize-stack-ir",
    "--reorder-functions",
    "--precompute",
]

_LEVEL_PASSES: dict[str, list[str]] = {
    "Oz": _OZ_PASSES,
    "O3": _O3_PASSES,
}


def _run_wasm_opt_via_optimize(
    linked: Path,
    level: str = "Oz",
    *,
    converge: bool = True,
    required_exports: set[str] | None = None,
) -> bool:
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
    if required_exports is None:
        try:
            required_exports = set(_collect_function_exports(linked.read_bytes()))
        except (OSError, ValueError):
            required_exports = set()
    result = mod.optimize(
        linked,
        output_path=temp_output,
        level=level,
        extra_passes=extra_passes,
        converge=converge,
        required_exports=required_exports,
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
    split_runtime: bool = False,
    split_output_dir: Path | None = None,
    deploy_runtime_override: Path | None = None,
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
        # The runtime might be a relocatable object with no export section.
        # Search sibling directories for a non-relocatable build that has
        # exports (e.g. wasm-release profile).
        for sibling_dir in (
            runtime.parent.parent / "wasm-release",
            runtime.parent.parent / "debug",
        ):
            candidate = sibling_dir / runtime.name
            if candidate.exists() and candidate != runtime:
                try:
                    runtime_exports = _collect_exports(candidate.read_bytes())
                except ValueError:
                    runtime_exports = set()
                if runtime_exports:
                    print(
                        f"Using exports from {candidate} "
                        f"({len(runtime_exports)} exports)",
                        file=sys.stderr,
                    )
                    break
    if not runtime_exports:
        print("Runtime exports unavailable for linking.", file=sys.stderr)
        return 1
    output_data = output.read_bytes()
    preserved_output_exports = _collect_preserved_output_export_names(output_data)
    export_symbol_map = _collect_output_export_symbol_map(output_data)
    user_export_symbol_names = [
        export_symbol_map[name]
        for name in preserved_output_exports
        if name in export_symbol_map
    ]
    rewritten = _rewrite_output_imports(output, runtime_exports)
    if rewritten is None:
        return 1
    rewritten_path, temp_dir, force_exports = rewritten
    rewritten_path = _inject_call_indirect_alias(rewritten_path, runtime, temp_dir)
    rewritten_path = _inject_output_runtime_entrypoint_aliases(rewritten_path, temp_dir)
    if allowlist_override is not None:
        allowlist = allowlist_override
    else:
        allowlist = Path(__file__).parent / "wasm_allowed_imports.txt"
    if not allowlist.exists():
        print(f"Allowlist not found: {allowlist}", file=sys.stderr)
        return 1

    # When imports were rewritten to prefixed names that are missing from
    # the non-relocatable runtime's export section (e.g. inlined away by
    # LTO), check whether the actual link runtime is a relocatable object
    # that retains the symbols in its linking section.  If so, wasm-ld
    # will resolve them — no extra action needed.  If the link runtime is
    # the non-relocatable module itself, we need the relocatable variant.
    if force_exports:
        is_reloc_runtime = runtime.name.endswith("_reloc.wasm")
        if is_reloc_runtime:
            # The relocatable runtime retains all symbols — the pre-check
            # against the non-reloc export list was overly conservative.
            pass
        else:
            # Non-reloc runtime is missing these exports; try the reloc.
            reloc_candidate = runtime.with_name(
                runtime.name.replace(".wasm", "_reloc.wasm")
            )
            if reloc_candidate.exists():
                print(
                    f"Wasm link: switching to relocatable runtime "
                    f"{reloc_candidate.name} to resolve "
                    f"{len(force_exports)} missing export(s)",
                    file=sys.stderr,
                )
                runtime = reloc_candidate
            else:
                missing_list = ", ".join(sorted(set(force_exports)))
                print(
                    f"Wasm link failed: {len(force_exports)} import(s) "
                    f"missing from runtime exports and no relocatable "
                    f"runtime available: {missing_list}",
                    file=sys.stderr,
                )
                return 1

    if not split_runtime and not runtime.name.endswith("_reloc.wasm"):
        reloc_candidate = runtime.with_name(runtime.name.replace(".wasm", "_reloc.wasm"))
        if reloc_candidate.exists():
            runtime = reloc_candidate

    # When split_runtime is enabled, generate a stub runtime that has the
    # same exported function signatures but trivial (unreachable) bodies.
    # wasm-ld links against the stub so --gc-sections can eliminate dead
    # code, producing a genuinely small app.wasm.
    link_runtime_path = runtime
    if split_runtime:
        # The stub builder needs the non-relocatable runtime because it reads
        # the standard WASM export section (section 7) to discover function
        # signatures.  Relocatable modules store symbols in the linking custom
        # section instead and have no export section.
        stub_source = runtime
        if stub_source.name.endswith("_reloc.wasm"):
            non_reloc = stub_source.with_name(
                stub_source.name.replace("_reloc.wasm", ".wasm")
            )
            if non_reloc.exists():
                stub_source = non_reloc
        try:
            stub_data = _build_runtime_stub(stub_source.read_bytes())
        except ValueError as exc:
            print(
                f"Failed to build runtime stub: {exc}", file=sys.stderr
            )
            return 1
        stub_path = Path(temp_dir.name) / "molt_runtime_stub.wasm"
        stub_path.write_bytes(stub_data)
        link_runtime_path = stub_path
        print(
            f"Split-runtime: stub {len(stub_data):,} bytes "
            f"(real runtime {runtime.stat().st_size:,} bytes)",
            file=sys.stderr,
        )

    cmd = [
        wasm_ld,
        "--no-entry",
        "--gc-sections",
        "--export-all",
        f"--allow-undefined-file={str(allowlist)}",
        "--import-table",
        # Place the stack before data segments in linear memory so that the
        # stack (which grows downward from __stack_pointer) cannot overwrite
        # data segments.  Without this flag wasm-ld may place data segments
        # in the address range reserved for the stack, causing corruption
        # when function calls push frames that overlap string constants and
        # other read-only data (manifests as NameError / AttributeError with
        # null-byte names).
        "--stack-first",
        "-z", "stack-size=1048576",
        "--export=molt_main",
        "--export-if-defined=molt_memory",
        "--export-if-defined=memory",
        "--export-if-defined=molt_table",
        "--export-if-defined=__indirect_function_table",
        "--export-if-defined=molt_set_wasm_table_base",
    ]
    # Force-export symbols that were rewritten but missing from the
    # non-relocatable runtime — they exist in the relocatable runtime
    # and wasm-ld needs to know to keep them in the linked output.
    for sym in force_exports:
        cmd.append(f"--export-if-defined={sym}")
    for sym in sorted(_ESSENTIAL_EXPORTS - {"__indirect_function_table", "memory", "molt_main"}):
        cmd.append(f"--export-if-defined={sym}")
    for sym in user_export_symbol_names:
        cmd.append(f"--export={sym}")
    cmd += [
        "-o",
        str(linked),
        str(rewritten_path),
        str(link_runtime_path),
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
        public_export_map = {
            name: export_symbol_map[name]
            for name in preserved_output_exports
            if name in export_symbol_map
        }
        public_export_map.update(
            {
                name: export_symbol_map[name]
                for name in ("molt_host_init", "molt_main", "molt_table_init")
                if name in export_symbol_map
            }
        )
        public_export_map.update(
            {
                name: export_symbol_map[name]
                for name in _collect_function_exports(output_data)
                if name.startswith("__molt_table_ref_") and name in export_symbol_map
            }
        )
        updated = _ensure_function_exports_by_symbol_names(
            linked_bytes, public_export_map
        )
        if updated is not None:
            linked.write_bytes(updated)
            linked_bytes = updated
        rename_map = {
            export_symbol_map[name]: name
            for name in preserved_output_exports
            if name in export_symbol_map and export_symbol_map[name] != name
        }
        updated = _rename_export_names(linked_bytes, rename_map)
        if updated is not None:
            linked.write_bytes(updated)
            linked_bytes = updated
        updated = _restore_output_export_aliases(linked_bytes)
        if updated is not None:
            linked.write_bytes(updated)
            linked_bytes = updated

        if not split_runtime:
            try:
                updated = _neutralize_linked_table_init(linked_bytes)
            except ValueError as exc:
                print(f"Failed to neutralize linked table init: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                linked.write_bytes(updated)
                linked_bytes = updated

        # MOL-183/MOL-186: Post-link optimization to reduce V8 OOM risk.
        # Strip debug sections, internal exports, and report data duplicates.
        # Pass the original user module as reference_data so the type-index
        # repair can use exact signature matching (Strategy 1) instead of
        # the heuristic body-scan fallback.
        pre_opt_size = len(linked_bytes)
        try:
            output_reference = output.read_bytes()
        except OSError:
            output_reference = None
        linked_bytes = _post_link_optimize(
            linked_bytes,
            reference_data=output_reference,
            preserve_table_refs=True,
        )
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
            if _run_wasm_opt_via_optimize(linked, level=optimize_level, converge=False):
                # Re-read after optimization since the file changed on disk
                linked_bytes = linked.read_bytes()

        output_table_min = _table_import_min(output.read_bytes())
        required_table_min = _required_linked_table_min(linked_bytes, output_table_min)
        if required_table_min is not None:
            try:
                updated = _rewrite_table_import_min(linked_bytes, required_table_min)
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
        append_table_refs_raw = os.environ.get("MOLT_WASM_LINK_APPEND_TABLE_REFS")
        append_table_refs = (
            True
            if append_table_refs_raw is None
            else append_table_refs_raw.strip().lower() not in {"0", "false", "no", "off"}
        )
        if append_table_refs:
            try:
                allowed_table_indices = None
                if not split_runtime:
                    allowed_table_indices = {
                        int(name.removeprefix("__molt_table_ref_"))
                        for name in _collect_function_exports(output.read_bytes())
                        if name.startswith("__molt_table_ref_")
                    }
                updated = _append_table_ref_elements(
                    linked_bytes,
                    allowed_table_indices=allowed_table_indices,
                )
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
                f"Warning: skipping ref.func element declaration: {exc}",
                file=sys.stderr,
            )
            updated = None
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
        if not split_runtime:
            updated = _strip_internal_exports(
                linked_bytes,
                preserve_exports=set(preserved_output_exports),
                preserve_table_refs=False,
            )
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

        # -- Split-runtime: emit app.wasm + molt_runtime.wasm ---------------
        if split_runtime:
            out_dir = split_output_dir or linked.parent
            out_dir.mkdir(parents=True, exist_ok=True)

            app_wasm = out_dir / "app.wasm"
            rt_wasm = out_dir / "molt_runtime.wasm"

            # For split-runtime, the app artifact must remain unlinked while
            # preserving the runtime ABI rewrite performed earlier in the link
            # pipeline.  Copying the fully linked binary here collapses the
            # split contract, while copying the raw frontend output would leave
            # stale unprefixed runtime imports that do not match the deploy
            # runtime's export ABI.  The correct artifact is the rewritten,
            # still-unlinked module.
            rewritten_data = rewritten_path.read_bytes()
            optimized_app = _optimize_split_app_module(
                rewritten_data,
                reference_data=output.read_bytes(),
                optimize=optimize,
                optimize_level=optimize_level,
            )
            app_wasm.write_bytes(optimized_app)

            # Resolve the deploy-ready (non-relocatable) runtime.
            env_deploy_runtime = os.environ.get("MOLT_WASM_DEPLOY_RUNTIME", "").strip()
            deploy_runtime = (
                Path(env_deploy_runtime).expanduser()
                if env_deploy_runtime
                else deploy_runtime_override or runtime
            )
            if not deploy_runtime.exists():
                fallback_candidates: list[Path] = []
                if deploy_runtime_override is not None:
                    fallback_candidates.append(deploy_runtime_override)
                fallback_candidates.append(runtime)
                if deploy_runtime.name.endswith("_reloc.wasm"):
                    fallback_candidates.append(
                        deploy_runtime.with_name(
                            deploy_runtime.name.replace("_reloc.wasm", ".wasm")
                        )
                    )
                for candidate in fallback_candidates:
                    if candidate.exists():
                        deploy_runtime = candidate
                        break
                else:
                    raise FileNotFoundError(
                        f"split deploy runtime not found: {deploy_runtime}"
                    )
            if (
                not env_deploy_runtime
                and deploy_runtime_override is None
                and deploy_runtime.name.endswith("_reloc.wasm")
            ):
                non_reloc = deploy_runtime.with_name(
                    deploy_runtime.name.replace("_reloc.wasm", ".wasm")
                )
                if non_reloc.exists():
                    deploy_runtime = non_reloc

            # Tree-shake the runtime: strip exports the app doesn't import,
            # then run wasm-opt to eliminate dead code.  This is the key step
            # that reduces the runtime from ~8MB to ~1-2MB for typical apps.
            full_rt_size = deploy_runtime.stat().st_size
            try:
                app_imports = _collect_module_imports(
                    app_wasm.read_bytes(), "molt_runtime"
                )
                print(
                    f"App imports {len(app_imports)} functions from molt_runtime",
                    file=sys.stderr,
                )
                shaken_runtime = _tree_shake_runtime(
                    deploy_runtime.read_bytes(), app_imports
                )
                rt_wasm.write_bytes(shaken_runtime)
            except Exception as exc:
                print(
                    f"Runtime tree-shake failed (falling back to full copy): {exc}",
                    file=sys.stderr,
                )
                shutil.copy2(str(deploy_runtime), str(rt_wasm))

            app_size = app_wasm.stat().st_size
            rt_size = rt_wasm.stat().st_size
            total = app_size + rt_size
            print(
                f"Split-runtime output: "
                f"{app_wasm.name} ({app_size:,} bytes, {app_size // 1024}KB) + "
                f"{rt_wasm.name} ({rt_size:,} bytes, {rt_size // 1024}KB) = "
                f"{total:,} bytes total "
                f"(runtime: {full_rt_size:,} -> {rt_size:,}, "
                f"{(1 - rt_size / full_rt_size) * 100:.0f}% reduction)",
                file=sys.stderr,
            )

        if freestanding:
            if not _validate_freestanding(linked_bytes):
                return 1
        stripped_debug = _strip_debug_sections(linked_bytes)
        if stripped_debug is not None:
            linked.write_bytes(stripped_debug)
            linked_bytes = stripped_debug
        linked_ok = _validate_linked(linked)
        if not linked_ok:
            if split_runtime:
                print(
                    "Warning: linked wasm validation failed after split-runtime outputs were emitted; "
                    "continuing because split artifacts do not depend on the linked binary.",
                    file=sys.stderr,
                )
                return 0
            return 1

        return 0
    finally:
        temp_dir.cleanup()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Attempt to link Molt output/runtime into a single WASM module.",
    )
    parser.add_argument("--runtime", type=Path, default=_default_runtime_path())
    parser.add_argument("--input", type=Path, default=_default_input_path())
    parser.add_argument("--output", type=Path, default=_default_output_path())
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
    parser.add_argument(
        "--split-runtime", action="store_true", default=False,
        help="Generate app.wasm + molt_runtime.wasm instead of a single linked binary",
    )
    parser.add_argument(
        "--split-output-dir", type=Path, default=None,
        help="Directory for split-runtime output files (default: same as --output parent)",
    )
    parser.add_argument(
        "--deploy-runtime", type=Path, default=None,
        dest="deploy_runtime_override",
        help="Override the deploy runtime wasm path (non-relocatable variant)",
    )
    args = parser.parse_args()

    runtime = args.runtime
    output = args.input
    linked = args.output

    if not runtime.exists():
        print(f"Runtime wasm not found: {runtime}", file=sys.stderr)
        return 1
    _verify_runtime_integrity(runtime)
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
        split_runtime=args.split_runtime,
        split_output_dir=args.split_output_dir,
        deploy_runtime_override=args.deploy_runtime_override,
    )


if __name__ == "__main__":
    raise SystemExit(main())

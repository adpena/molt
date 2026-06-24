from __future__ import annotations

import functools
import importlib
import json
import re
from pathlib import Path
from typing import Any, Mapping, Sequence


def _cli_module() -> Any:
    return importlib.import_module("molt.cli")


def _run_completed_command(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._run_completed_command(*args, **kwargs)


def _which(executable: str) -> str | None:
    return _cli_module().shutil.which(executable)


def _atomic_write_bytes(path: Path, data: bytes) -> None:
    _cli_module()._atomic_write_bytes(path, data)


def _read_wasm_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading wasm varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
    raise AssertionError("unreachable")


def _read_wasm_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_wasm_varuint(data, offset)
    end = offset + length
    if end > len(data):
        raise ValueError("Unexpected EOF while reading wasm string")
    return data[offset:end].decode("utf-8"), end


def _write_wasm_varuint(value: int) -> bytes:
    if value < 0:
        raise ValueError("wasm varuint must be non-negative")
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def _write_wasm_string(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return _write_wasm_varuint(len(encoded)) + encoded


def _parse_wasm_sections(data: bytes) -> list[tuple[int, bytes]]:
    if len(data) < 8 or data[:4] != b"\x00asm":
        raise ValueError("Invalid wasm binary")
    offset = 8
    sections: list[tuple[int, bytes]] = []
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_wasm_varuint(data, offset)
        section_end = offset + section_size
        if section_end > len(data):
            raise ValueError("Invalid wasm section length")
        sections.append((section_id, data[offset:section_end]))
        offset = section_end
    return sections


def _build_wasm_sections(sections: Sequence[tuple[int, bytes]]) -> bytes:
    out = bytearray(b"\x00asm\x01\x00\x00\x00")
    for section_id, payload in sections:
        out.append(section_id)
        out.extend(_write_wasm_varuint(len(payload)))
        out.extend(payload)
    return bytes(out)


def _skip_wasm_init_expr(data: bytes, offset: int) -> tuple[int, int | None]:
    opcode = data[offset]
    offset += 1
    value: int | None = None
    if opcode == 0x41:  # i32.const
        value, offset = _read_wasm_varuint(data, offset)
    elif opcode == 0x23:  # global.get
        _, offset = _read_wasm_varuint(data, offset)
    else:
        raise ValueError(f"Unsupported wasm init expr opcode 0x{opcode:02x}")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Malformed wasm init expr")
    return offset + 1, value


def _read_wasm_ref_func_expr(data: bytes, offset: int) -> tuple[int, int | None]:
    opcode = data[offset]
    offset += 1
    func_index: int | None = None
    if opcode == 0xD2:  # ref.func
        func_index, offset = _read_wasm_varuint(data, offset)
    elif opcode == 0xD0:  # ref.null
        offset += 1
    else:
        raise ValueError(f"Unsupported wasm element expr opcode 0x{opcode:02x}")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Malformed wasm ref.func expr")
    return offset + 1, func_index


def _collect_wasm_active_table_function_slots(data: bytes) -> dict[int, int]:
    sections = _parse_wasm_sections(data)
    slots: dict[int, int] = {}
    for section_id, payload in sections:
        if section_id != 9:
            continue
        offset = 0
        count, offset = _read_wasm_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_wasm_varuint(payload, offset)
            table_index = 0
            base_offset: int | None = None
            if flags == 0:
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    func_index, offset = _read_wasm_varuint(payload, offset)
                    if base_offset is not None:
                        slots[base_offset + elem_index] = func_index
            elif flags == 1:
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    _, offset = _read_wasm_varuint(payload, offset)
            elif flags == 2:
                table_index, offset = _read_wasm_varuint(payload, offset)
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    func_index, offset = _read_wasm_varuint(payload, offset)
                    if table_index == 0 and base_offset is not None:
                        slots[base_offset + elem_index] = func_index
            elif flags == 3:
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    _, offset = _read_wasm_varuint(payload, offset)
            elif flags == 4:
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    offset, func_index = _read_wasm_ref_func_expr(payload, offset)
                    if func_index is not None and base_offset is not None:
                        slots[base_offset + elem_index] = func_index
            elif flags == 5:
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    offset, _ = _read_wasm_ref_func_expr(payload, offset)
            elif flags == 6:
                table_index, offset = _read_wasm_varuint(payload, offset)
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    offset, func_index = _read_wasm_ref_func_expr(payload, offset)
                    if (
                        table_index == 0
                        and func_index is not None
                        and base_offset is not None
                    ):
                        slots[base_offset + elem_index] = func_index
            elif flags == 7:
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    offset, _ = _read_wasm_ref_func_expr(payload, offset)
            else:
                raise ValueError(f"Unsupported wasm element flags {flags}")
    return slots


def _export_wasm_table_refs(path: Path) -> None:
    data = path.read_bytes()
    sections = _parse_wasm_sections(data)
    slot_to_func = _collect_wasm_active_table_function_slots(data)
    if not slot_to_func:
        return

    exports: list[tuple[str, int, int]] = []
    existing_names: set[str] = set()
    new_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 7:
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_wasm_varuint(payload, offset)
        for _ in range(count):
            name, offset = _read_wasm_string(payload, offset)
            kind = payload[offset]
            offset += 1
            index, offset = _read_wasm_varuint(payload, offset)
            exports.append((name, kind, index))
            existing_names.add(name)

    additions = [
        (f"__molt_table_ref_{slot}", 0, func_index)
        for slot, func_index in sorted(slot_to_func.items())
        if f"__molt_table_ref_{slot}" not in existing_names
    ]
    if not additions:
        return

    export_payload = bytearray()
    merged = exports + additions
    export_payload.extend(_write_wasm_varuint(len(merged)))
    for name, kind, index in merged:
        export_payload.extend(_write_wasm_string(name))
        export_payload.append(kind)
        export_payload.extend(_write_wasm_varuint(index))

    inserted = False
    rebuilt_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id == 7:
            rebuilt_sections.append((7, bytes(export_payload)))
            inserted = True
            continue
        if not inserted and section_id > 7:
            rebuilt_sections.append((7, bytes(export_payload)))
            inserted = True
        rebuilt_sections.append((section_id, payload))
    if not inserted:
        rebuilt_sections.append((7, bytes(export_payload)))
    _atomic_write_bytes(path, _build_wasm_sections(rebuilt_sections))


def _wasm_import_minima(path: Path) -> tuple[int | None, int | None]:
    data = path.read_bytes()
    if len(data) < 8 or data[:4] != b"\x00asm":
        raise ValueError(f"Invalid wasm binary: {path}")

    memory_min: int | None = None
    table_min: int | None = None
    offset = 8
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_wasm_varuint(data, offset)
        section_end = offset + section_size
        if section_end > len(data):
            raise ValueError(f"Invalid wasm section length in {path}")
        if section_id == 2:
            count, cursor = _read_wasm_varuint(data, offset)
            for _ in range(count):
                module_name, cursor = _read_wasm_string(data, cursor)
                field_name, cursor = _read_wasm_string(data, cursor)
                kind = data[cursor]
                cursor += 1
                if kind == 0:
                    _, cursor = _read_wasm_varuint(data, cursor)
                elif kind == 1:
                    cursor += 1  # elemtype
                    flags, cursor = _read_wasm_varuint(data, cursor)
                    minimum, cursor = _read_wasm_varuint(data, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(data, cursor)
                    if (
                        module_name == "env"
                        and field_name == "__indirect_function_table"
                    ):
                        table_min = minimum
                elif kind == 2:
                    flags, cursor = _read_wasm_varuint(data, cursor)
                    minimum, cursor = _read_wasm_varuint(data, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(data, cursor)
                    if module_name == "env" and field_name == "memory":
                        memory_min = minimum
                elif kind == 3:
                    cursor += 2
                elif kind == 4:
                    cursor += 1
                    _, cursor = _read_wasm_varuint(data, cursor)
                else:
                    raise ValueError(f"Unknown wasm import kind {kind} in {path}")
            break
        offset = section_end
    return memory_min, table_min


def _wasm_import_function_result_kinds(
    path: Path, *, module_name: str
) -> dict[str, str]:
    wasm_objdump = _which("wasm-objdump")
    if wasm_objdump is None:
        return {}
    result = _run_completed_command(
        [wasm_objdump, "-x", str(path)],
        capture_output=True,
        env=None,
        cwd=path.parent,
        memory_guard_prefix="MOLT_BUILD",
    )
    text = (result.stdout or "") + ("\n" + result.stderr if result.stderr else "")
    if not text:
        return {}

    type_kinds: dict[int, str] = {}
    for line in text.splitlines():
        match = re.match(r"\s*-\s*type\[(\d+)\]\s+\(.*\)\s+->\s+(.+)", line)
        if not match:
            continue
        type_kinds[int(match.group(1))] = match.group(2).strip()

    result_kinds: dict[str, str] = {}
    import_re = re.compile(
        rf"\s*-\s*func\[\d+\]\s+sig=(\d+)\s+<{re.escape(module_name)}\.([^>]+)>\s+<-\s+{re.escape(module_name)}\.[^\s]+"
    )
    for line in text.splitlines():
        match = import_re.match(line)
        if not match:
            continue
        sig = int(match.group(1))
        name = match.group(2)
        result_kind = type_kinds.get(sig)
        if result_kind:
            result_kinds[name] = result_kind
    return result_kinds


def _wasm_import_function_signatures(
    path: Path, *, module_name: str
) -> dict[str, dict[str, object]]:
    wasm_objdump = _which("wasm-objdump")
    if wasm_objdump is None:
        return {}
    result = _run_completed_command(
        [wasm_objdump, "-x", str(path)],
        capture_output=True,
        env=None,
        cwd=path.parent,
        memory_guard_prefix="MOLT_BUILD",
    )
    text = (result.stdout or "") + ("\n" + result.stderr if result.stderr else "")
    if not text:
        return {}

    type_signatures: dict[int, tuple[list[str], str]] = {}
    for line in text.splitlines():
        match = re.match(r"\s*-\s*type\[(\d+)\]\s+\((.*)\)\s+->\s+(.+)", line)
        if not match:
            continue
        type_index = int(match.group(1))
        raw_params = match.group(2).strip()
        params = [part.strip() for part in raw_params.split(",") if part.strip()]
        result_kind = match.group(3).strip()
        type_signatures[type_index] = (params, result_kind)

    signatures: dict[str, dict[str, object]] = {}
    import_re = re.compile(
        rf"\s*-\s*func\[\d+\]\s+sig=(\d+)\s+<{re.escape(module_name)}\.([^>]+)>\s+<-\s+{re.escape(module_name)}\.[^\s]+"
    )
    for line in text.splitlines():
        match = import_re.match(line)
        if not match:
            continue
        sig = int(match.group(1))
        name = match.group(2)
        signature = type_signatures.get(sig)
        if signature is None:
            continue
        params, result_kind = signature
        signatures[name] = {"params": params, "result": result_kind}
    return signatures


def _wasm_export_function_signatures(
    path: Path, *, export_name_prefix: str
) -> dict[str, dict[str, object]]:
    wasm_objdump = _which("wasm-objdump")
    if wasm_objdump is None:
        return {}
    result = _run_completed_command(
        [wasm_objdump, "-x", str(path)],
        capture_output=True,
        env=None,
        cwd=path.parent,
        memory_guard_prefix="MOLT_BUILD",
    )
    text = (result.stdout or "") + ("\n" + result.stderr if result.stderr else "")
    if not text:
        return {}

    type_signatures: dict[int, tuple[list[str], str]] = {}
    for line in text.splitlines():
        match = re.match(r"\s*-\s*type\[(\d+)\]\s+\((.*)\)\s+->\s+(.+)", line)
        if not match:
            continue
        type_index = int(match.group(1))
        raw_params = match.group(2).strip()
        params = [part.strip() for part in raw_params.split(",") if part.strip()]
        result_kind = match.group(3).strip()
        type_signatures[type_index] = (params, result_kind)

    func_type_indices: dict[int, int] = {}
    for line in text.splitlines():
        match = re.match(r"\s*-\s*func\[(\d+)\]\s+sig=(\d+)", line)
        if not match:
            continue
        func_type_indices[int(match.group(1))] = int(match.group(2))

    export_signatures: dict[str, dict[str, object]] = {}
    export_re = re.compile(r'\s*-\s*func\[(\d+)\]\s+<[^>]+>\s+->\s+"([^"]+)"')
    for line in text.splitlines():
        match = export_re.match(line)
        if not match:
            continue
        func_index = int(match.group(1))
        export_name = match.group(2)
        if not export_name.startswith(export_name_prefix):
            continue
        type_index = func_type_indices.get(func_index)
        if type_index is None:
            continue
        signature = type_signatures.get(type_index)
        if signature is None:
            continue
        params, result_kind = signature
        export_signatures[export_name] = {
            "params": params,
            "result": result_kind,
        }
    return export_signatures


def _infer_wasm_table_base_from_export_names(
    export_signatures: Mapping[str, Mapping[str, object]],
    *,
    export_name_prefix: str,
) -> int | None:
    slots: list[int] = []
    for name in export_signatures:
        if not name.startswith(export_name_prefix):
            continue
        raw = name[len(export_name_prefix) :]
        try:
            slot = int(raw)
        except ValueError:
            continue
        if slot > 0:
            slots.append(slot)
    if not slots:
        return None
    return min(slots)


def _effective_split_worker_table_base(
    *,
    wasm_table_base: int | None,
    runtime_table_min: int | None,
    app_table_ref_signatures: Mapping[str, Mapping[str, object]],
) -> int | None:
    if wasm_table_base is not None:
        return wasm_table_base
    inferred = _infer_wasm_table_base_from_export_names(
        app_table_ref_signatures,
        export_name_prefix="__molt_table_ref_",
    )
    if inferred is not None:
        return inferred
    return runtime_table_min


@functools.lru_cache(maxsize=1)
def _reserved_wasm_runtime_callable_count() -> int:
    include_path = (
        Path(__file__).resolve().parents[2] / "runtime" / "wasm_runtime_callables.inc"
    )
    pattern = re.compile(r"^\s*\((\d+),")
    count = 0
    for line in include_path.read_text().splitlines():
        if pattern.match(line):
            count += 1
    return count


def _generate_split_worker_js(
    *,
    shared_memory_initial_pages: int,
    shared_table_initial: int,
    shared_table_base: int | None,
    runtime_import_result_kinds: Mapping[str, str] | None = None,
    runtime_import_signatures: Mapping[str, Mapping[str, object]] | None = None,
    app_table_ref_signatures: Mapping[str, Mapping[str, object]] | None = None,
    runtime_table_ref_signatures: Mapping[str, Mapping[str, object]] | None = None,
) -> str:
    """Generate a Cloudflare Workers shim for split-runtime deployment.

    The runtime WASM module is loaded separately and can be cached by the
    CDN independently of the app module.  Both modules share linear memory
    through WASI imports.
    """
    runtime_import_result_kinds_json = json.dumps(
        dict(runtime_import_result_kinds or {}), sort_keys=True
    )
    runtime_import_signatures_json = json.dumps(
        dict(runtime_import_signatures or {}), sort_keys=True
    )
    app_table_ref_signatures_json = json.dumps(
        dict(app_table_ref_signatures or {}), sort_keys=True
    )
    runtime_table_ref_signatures_json = json.dumps(
        dict(runtime_table_ref_signatures or {}), sort_keys=True
    )
    legacy_wasm_table_base = 256
    reserved_runtime_callable_base = 33
    reserved_runtime_shared_prefix_len = (
        reserved_runtime_callable_base + _reserved_wasm_runtime_callable_count() * 2
    )
    worker_js = """// Molt split-runtime Cloudflare Workers shim
// Runtime module is cached independently by the CDN.
import "./molt_vfs_browser.js";
import runtimeModule from "./molt_runtime.wasm";
import appModule from "./app.wasm";

class ProcExit { constructor(code) { this.code = code; } }

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    const urlPath = url.pathname;
    const queryString = url.search ? url.search.slice(1) : "";

    const stdoutChunks = [];
    const stderrChunks = [];
    const stdoutDecoder = new TextDecoder();
    const stderrDecoder = new TextDecoder();
    const utf8Decoder = new TextDecoder();
    const encoder = new TextEncoder();
    const wasmMemory = new WebAssembly.Memory({ initial: __MOLT_SHARED_MEMORY_PAGES__ });
    const vfs = new globalThis.MoltVfs();
    let appInstance = null;

    const assetBytes = async (name) => {
      if (!env.__STATIC_CONTENT || typeof env.__STATIC_CONTENT.get !== "function") {
        return null;
      }
      const asset = await env.__STATIC_CONTENT.get(name);
      if (!asset) return null;
      if (asset instanceof Uint8Array) return asset;
      if (asset instanceof ArrayBuffer) return new Uint8Array(asset);
      if (typeof asset.arrayBuffer === "function") {
        return new Uint8Array(await asset.arrayBuffer());
      }
      if (typeof asset.bytes === "function") {
        return new Uint8Array(await asset.bytes());
      }
      return null;
    };

    const readPathUtf8 = (pathPtr, pathLen) =>
      utf8Decoder.decode(
        new Uint8Array(wasmMemory.buffer, pathPtr >>> 0, pathLen >>> 0)
      );

    const writeErrno = (err, fallback) => {
      if (err && typeof err.message === "string") {
        if (err.message.startsWith("ENOENT")) return ENOENT;
        if (err.message.startsWith("ENOSPC")) return 28;
        if (err.message.startsWith("EINVAL")) return EINVAL;
      }
      return fallback;
    };

    const wasiArgs = ["molt", urlPath, queryString];
    const argsEncoded = wasiArgs.map(a => encoder.encode(a + "\\0"));
    const argsTotalSize = argsEncoded.reduce((s, a) => s + a.length, 0);

    const envVars = [
      "MOLT_TRUSTED=1",
      ...(__MOLT_SHARED_TABLE_BASE__ !== null
        ? [`MOLT_WASM_TABLE_BASE=${__MOLT_SHARED_TABLE_BASE__}`]
        : []),
      ...(queryString ? [`QUERY_STRING=${queryString}`] : []),
    ];
    const envEncoded = envVars.map(e => encoder.encode(e + "\\0"));
    const envTotalSize = envEncoded.reduce((s, e) => s + e.length, 0);
    const ENOSYS = 38;
    const EINVAL = 22;
    const EBADF = 9;
    const ENOENT = 2;
    const ENOSPC = 28;
    const EROFS = 30;
    const EISDIR = 21;
    const ENOTDIR = 20;
    const ESPIPE = 29;
    const QNAN = 0x7ff8000000000000n;
    const TAG_INT = 0x0001000000000000n;
    const TAG_NONE = 0x0003000000000000n;
    const INT_MASK = (1n << 47n) - 1n;
    const NONE_BITS = QNAN | TAG_NONE;
    const runtimeImportResultKinds = __MOLT_RUNTIME_IMPORT_RESULT_KINDS__;
    const runtimeImportSignatures = __MOLT_RUNTIME_IMPORT_SIGNATURES__;
    const appTableRefSignatures = __MOLT_APP_TABLE_REF_SIGNATURES__;
    const runtimeTableRefSignatures = __MOLT_RUNTIME_TABLE_REF_SIGNATURES__;
    const LEGACY_WASM_TABLE_BASE = __MOLT_LEGACY_WASM_TABLE_BASE__;
    const RESERVED_RUNTIME_CALLABLE_BASE = __MOLT_RESERVED_RUNTIME_CALLABLE_BASE__;
    const RESERVED_RUNTIME_SHARED_PREFIX_LEN = __MOLT_RESERVED_RUNTIME_SHARED_PREFIX_LEN__;
    const WASI_FILETYPE_CHARACTER_DEVICE = 2;
    const WASI_FILETYPE_DIRECTORY = 3;
    const WASI_FILETYPE_REGULAR_FILE = 4;
    const WASI_PREOPENTYPE_DIR = 0;
    const WASI_RIGHTS_ALL = 0xffffffffffffffffn;
    const WASI_OFLAGS_CREAT = 1;
    const WASI_OFLAGS_DIRECTORY = 2;
    const WASI_OFLAGS_EXCL = 4;
    const WASI_OFLAGS_TRUNC = 8;
    const WASI_WHENCE_SET = 0;
    const WASI_WHENCE_CUR = 1;
    const WASI_WHENCE_END = 2;
    const WASI_ERRNO_BADF = 8;
    const WASI_ERRNO_INVAL = 28;
    const WASI_ERRNO_ISDIR = 31;
    const WASI_ERRNO_NOENT = 44;
    const WASI_ERRNO_NOSYS = 52;
    const WASI_ERRNO_NOTDIR = 54;
    const WASI_ERRNO_ROFS = 69;
    const WASI_ERRNO_SPIPE = 70;
    const wasiFiles = new Map();
    const wasiPreopens = [
      { fd: 3, path: "/bundle" },
      { fd: 4, path: "/tmp" },
      { fd: 5, path: "/dev" },
    ];
    let wasiNextFd = 6;

    const toNumber = (value) =>
      typeof value === "bigint" ? Number(value) : Number(value >>> 0);

    const writeBytesToMemory = (memory, ptr, bytes) => {
      if (!memory) return false;
      const data = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
      const start = Number(ptr);
      const end = start + data.length;
      if (start < 0 || end > memory.buffer.byteLength) return false;
      new Uint8Array(memory.buffer, start, data.length).set(data);
      return true;
    };

    const writeWasiU32 = (ptr, value) => {
      if (!wasmMemory) return false;
      new DataView(wasmMemory.buffer).setUint32(Number(ptr), Number(value) >>> 0, true);
      return true;
    };

    const writeWasiU64 = (ptr, value) => {
      if (!wasmMemory) return false;
      new DataView(wasmMemory.buffer).setBigUint64(Number(ptr), BigInt(value), true);
      return true;
    };

    const writeFilestat = (ptr, stat) => {
      if (!wasmMemory) return false;
      const view = new DataView(wasmMemory.buffer);
      const base = Number(ptr);
      new Uint8Array(wasmMemory.buffer, base, 64).fill(0);
      view.setUint8(
        base + 16,
        stat.isDir ? WASI_FILETYPE_DIRECTORY : WASI_FILETYPE_REGULAR_FILE,
      );
      view.setBigUint64(base + 32, BigInt(stat.size || 0), true);
      view.setBigUint64(base + 40, BigInt(stat.size || 0), true);
      view.setBigUint64(base + 48, 0n, true);
      view.setBigUint64(base + 56, 0n, true);
      return true;
    };

    const wasiUnsupported = () => WASI_ERRNO_NOSYS;
    const preopenByFd = (fdNum) =>
      wasiPreopens.find((entry) => entry.fd === fdNum) || null;
    const readGuestPath = (ptr, len) => {
      if (!wasmMemory) return null;
      return utf8Decoder.decode(
        new Uint8Array(wasmMemory.buffer, Number(ptr), Number(len)),
      );
    };
    const normalizeRelativePath = (rawPath) => {
      const parts = [];
      for (const part of rawPath.split("/")) {
        if (!part || part === ".") continue;
        if (part === "..") {
          if (parts.length === 0) return null;
          parts.pop();
          continue;
        }
        parts.push(part);
      }
      return parts.join("/");
    };
    const absoluteVfsPath = (preopen, relativePath) =>
      relativePath ? `${preopen.path}/${relativePath}` : preopen.path;
    const statResolvedPath = (absolutePath) => {
      const resolved = vfs.resolve(absolutePath);
      if (!resolved || !resolved.mount || typeof resolved.mount.stat !== "function") {
        return null;
      }
      const stat = resolved.mount.stat(resolved.rel);
      if (!stat) return null;
      return { resolved, stat };
    };
    const openResolvedPath = (preopen, relativePath, oflags) => {
      const wantDirectory = (oflags & WASI_OFLAGS_DIRECTORY) !== 0;
      const absolutePath = absoluteVfsPath(preopen, relativePath);
      let info = statResolvedPath(absolutePath);
      if (!info) {
        if ((oflags & WASI_OFLAGS_CREAT) === 0) {
          return { errno: WASI_ERRNO_NOENT };
        }
        if (preopen.path !== "/tmp" || !vfs.tmp) {
          return { errno: WASI_ERRNO_ROFS };
        }
        vfs.tmp.write(relativePath, new Uint8Array(0));
        info = statResolvedPath(absolutePath);
        if (!info) {
          return { errno: WASI_ERRNO_NOENT };
        }
      } else if ((oflags & WASI_OFLAGS_EXCL) !== 0 && (oflags & WASI_OFLAGS_CREAT) !== 0) {
        return { errno: WASI_ERRNO_INVAL };
      }
      if (info.stat.isDir) {
        if (!wantDirectory) return { errno: WASI_ERRNO_ISDIR };
        const fd = wasiNextFd++;
        wasiFiles.set(fd, {
          kind: "dir",
          absolutePath,
          resolved: info.resolved,
          readable: true,
          writable: false,
          pos: 0,
        });
        return { errno: 0, fd };
      }
      if (wantDirectory) {
        return { errno: WASI_ERRNO_NOTDIR };
      }
      if ((oflags & WASI_OFLAGS_TRUNC) !== 0) {
        if (info.resolved.prefix !== "/tmp") {
          return { errno: WASI_ERRNO_ROFS };
        }
        info.resolved.mount.write(info.resolved.rel, new Uint8Array(0));
        info = statResolvedPath(absolutePath);
        if (!info) {
          return { errno: WASI_ERRNO_NOENT };
        }
      }
      let buffer;
      try {
        buffer = info.resolved.mount.read(info.resolved.rel);
      } catch {
        return { errno: WASI_ERRNO_NOENT };
      }
      const fd = wasiNextFd++;
      wasiFiles.set(fd, {
        kind: "file",
        absolutePath,
        resolved: info.resolved,
        readable: true,
        writable: info.resolved.prefix === "/tmp",
        pos: 0,
        buffer: new Uint8Array(buffer),
      });
      return { errno: 0, fd };
    };
    const syncWritableFile = (entry) => {
      if (!entry || entry.kind !== "file" || !entry.writable) {
        return 0;
      }
      try {
        entry.resolved.mount.write(entry.resolved.rel, entry.buffer);
        return 0;
      } catch {
        return WASI_ERRNO_INVAL;
      }
    };
    const writeFdstat = (statPtr, filetype) => {
      if (!wasmMemory) return WASI_ERRNO_NOSYS;
      const view = new DataView(wasmMemory.buffer);
      const base = Number(statPtr);
      view.setUint8(base, filetype);
      view.setUint16(base + 2, 0, true);
      view.setBigUint64(base + 8, WASI_RIGHTS_ALL, true);
      view.setBigUint64(base + 16, WASI_RIGHTS_ALL, true);
      return 0;
    };

    const wasi = {
      fd_write(fd, iovs, iovsLen, nwritten) {
        if ((fd === 1 || fd === 2) && wasmMemory) {
          const view = new DataView(wasmMemory.buffer);
          let totalWritten = 0;
          for (let i = 0; i < iovsLen; i++) {
            const ptr = view.getUint32(iovs + i * 8, true);
            const len = view.getUint32(iovs + i * 8 + 4, true);
            const bytes = new Uint8Array(wasmMemory.buffer, ptr, len);
            const decoder = (fd === 1) ? stdoutDecoder : stderrDecoder;
            const text = decoder.decode(bytes, { stream: true });
            if (fd === 1) stdoutChunks.push(text);
            else stderrChunks.push(text);
            totalWritten += len;
          }
          view.setUint32(nwritten, totalWritten, true);
        }
        return 0;
      },
      fd_read(fd, iovsPtr, iovsLen, outReadPtr) {
        if (!wasmMemory) return WASI_ERRNO_NOSYS;
        const fdNum = toNumber(fd);
        const view = new DataView(wasmMemory.buffer);
        if (fdNum === 0 && vfs.dev) {
          const basePtr = toNumber(iovsPtr);
          const count = toNumber(iovsLen);
          let totalRead = 0;
          for (let index = 0; index < count; index += 1) {
            const ptr = view.getUint32(basePtr + index * 8, true);
            const len = view.getUint32(basePtr + index * 8 + 4, true);
            if (len === 0) continue;
            const chunk = vfs.dev.readStdin(len);
            new Uint8Array(wasmMemory.buffer, ptr, chunk.length).set(chunk);
            totalRead += chunk.length;
            if (chunk.length < len) break;
          }
          if (outReadPtr) {
            view.setUint32(Number(outReadPtr), totalRead >>> 0, true);
          }
          return 0;
        }
        const entry = wasiFiles.get(fdNum);
        if (!entry || entry.kind !== "file" || !entry.readable) {
          return WASI_ERRNO_BADF;
        }
        const basePtr = toNumber(iovsPtr);
        const count = toNumber(iovsLen);
        let totalRead = 0;
        for (let index = 0; index < count; index += 1) {
          const ptr = view.getUint32(basePtr + index * 8, true);
          const len = view.getUint32(basePtr + index * 8 + 4, true);
          if (len === 0) continue;
          const remaining = entry.buffer.subarray(entry.pos, entry.pos + len);
          new Uint8Array(wasmMemory.buffer, ptr, remaining.length).set(remaining);
          entry.pos += remaining.length;
          totalRead += remaining.length;
          if (remaining.length < len) break;
        }
        if (outReadPtr) {
          view.setUint32(Number(outReadPtr), totalRead >>> 0, true);
        }
        return 0;
      },
      fd_close(fd) {
        const fdNum = toNumber(fd);
        const entry = wasiFiles.get(fdNum);
        if (!entry) return 0;
        const rc = syncWritableFile(entry);
        if (rc !== 0) return rc;
        wasiFiles.delete(fdNum);
        return 0;
      },
      fd_seek(fd, offset, whence, outOffsetPtr) {
        const fdNum = toNumber(fd);
        const entry = wasiFiles.get(fdNum);
        if (!entry || entry.kind !== "file") {
          return WASI_ERRNO_BADF;
        }
        const delta = typeof offset === "bigint" ? Number(offset) : Number(offset);
        let next = 0;
        if (whence === WASI_WHENCE_SET) {
          next = delta;
        } else if (whence === WASI_WHENCE_CUR) {
          next = entry.pos + delta;
        } else if (whence === WASI_WHENCE_END) {
          next = entry.buffer.length + delta;
        } else {
          return WASI_ERRNO_INVAL;
        }
        if (next < 0) return WASI_ERRNO_INVAL;
        entry.pos = next;
        if (outOffsetPtr) writeWasiU64(outOffsetPtr, BigInt(next));
        return 0;
      },
      fd_prestat_get(fd, prestatPtr) {
        if (!wasmMemory) return WASI_ERRNO_NOSYS;
        const fdNum = toNumber(fd);
        const preopen = preopenByFd(fdNum);
        if (!preopen) return WASI_ERRNO_BADF;
        const view = new DataView(wasmMemory.buffer);
        view.setUint8(Number(prestatPtr), WASI_PREOPENTYPE_DIR);
        view.setUint8(Number(prestatPtr) + 1, 0);
        view.setUint8(Number(prestatPtr) + 2, 0);
        view.setUint8(Number(prestatPtr) + 3, 0);
        view.setUint32(Number(prestatPtr) + 4, preopen.path.length, true);
        return 0;
      },
      fd_prestat_dir_name(fd, pathPtr, pathLen) {
        if (!wasmMemory) return WASI_ERRNO_NOSYS;
        const fdNum = toNumber(fd);
        const preopen = preopenByFd(fdNum);
        if (!preopen) return WASI_ERRNO_BADF;
        const bytes = encoder.encode(preopen.path);
        if (toNumber(pathLen) < bytes.length) return WASI_ERRNO_INVAL;
        return writeBytesToMemory(wasmMemory, pathPtr, bytes) ? 0 : WASI_ERRNO_INVAL;
      },
      fd_fdstat_get(fd, statPtr) {
        const fdNum = toNumber(fd);
        if (fdNum === 0 || fdNum === 1 || fdNum === 2) {
          return writeFdstat(statPtr, WASI_FILETYPE_CHARACTER_DEVICE);
        }
        if (preopenByFd(fdNum)) {
          return writeFdstat(statPtr, WASI_FILETYPE_DIRECTORY);
        }
        const entry = wasiFiles.get(fdNum);
        if (!entry) return WASI_ERRNO_BADF;
        return writeFdstat(
          statPtr,
          entry.kind === "dir" ? WASI_FILETYPE_DIRECTORY : WASI_FILETYPE_REGULAR_FILE,
        );
      },
      fd_tell(fd, outOffsetPtr) {
        const fdNum = toNumber(fd);
        const entry = wasiFiles.get(fdNum);
        if (!entry || entry.kind !== "file") {
          return WASI_ERRNO_SPIPE;
        }
        return writeWasiU64(outOffsetPtr, BigInt(entry.pos)) ? 0 : WASI_ERRNO_NOSYS;
      },
      fd_filestat_get(fd, bufPtr) {
        const fdNum = toNumber(fd);
        if (preopenByFd(fdNum)) {
          return writeFilestat(bufPtr, { isDir: true, isFile: false, size: 0 })
            ? 0
            : WASI_ERRNO_NOSYS;
        }
        const entry = wasiFiles.get(fdNum);
        if (!entry) return WASI_ERRNO_BADF;
        const stat =
          entry.kind === "dir"
            ? { isDir: true, isFile: false, size: 0 }
            : { isDir: false, isFile: true, size: entry.buffer.length };
        return writeFilestat(bufPtr, stat) ? 0 : WASI_ERRNO_NOSYS;
      },
      fd_filestat_set_size: wasiUnsupported,
      fd_filestat_set_times: wasiUnsupported,
      fd_readdir: wasiUnsupported,
      fd_advise: wasiUnsupported,
      fd_datasync: wasiUnsupported,
      fd_fdstat_set_flags: wasiUnsupported,
      fd_fdstat_set_rights: wasiUnsupported,
      fd_pread: wasiUnsupported,
      fd_pwrite: wasiUnsupported,
      fd_allocate: wasiUnsupported,
      fd_renumber: wasiUnsupported,
      fd_sync: wasiUnsupported,
      path_filestat_set_times: wasiUnsupported,
      path_link: wasiUnsupported,
      path_symlink: wasiUnsupported,
      sock_accept: wasiUnsupported,
      sock_recv: wasiUnsupported,
      sock_send: wasiUnsupported,
      sock_shutdown: wasiUnsupported,
      environ_sizes_get(countPtr, sizePtr) {
        if (wasmMemory) {
          const view = new DataView(wasmMemory.buffer);
          view.setUint32(countPtr, envVars.length, true);
          view.setUint32(sizePtr, envTotalSize, true);
        }
        return 0;
      },
      environ_get(environPtr, environBufPtr) {
        if (wasmMemory) {
          const view = new DataView(wasmMemory.buffer);
          let bufOffset = environBufPtr;
          for (let i = 0; i < envEncoded.length; i++) {
            view.setUint32(environPtr + i * 4, bufOffset, true);
            new Uint8Array(wasmMemory.buffer, bufOffset, envEncoded[i].length).set(envEncoded[i]);
            bufOffset += envEncoded[i].length;
          }
        }
        return 0;
      },
      args_sizes_get(countPtr, sizePtr) {
        if (wasmMemory) {
          const view = new DataView(wasmMemory.buffer);
          view.setUint32(countPtr, wasiArgs.length, true);
          view.setUint32(sizePtr, argsTotalSize, true);
        }
        return 0;
      },
      args_get(argvPtr, argvBufPtr) {
        if (wasmMemory) {
          const view = new DataView(wasmMemory.buffer);
          let bufOffset = argvBufPtr;
          for (let i = 0; i < argsEncoded.length; i++) {
            view.setUint32(argvPtr + i * 4, bufOffset, true);
            new Uint8Array(wasmMemory.buffer, bufOffset, argsEncoded[i].length).set(argsEncoded[i]);
            bufOffset += argsEncoded[i].length;
          }
        }
        return 0;
      },
      clock_time_get(id, precision, outPtr) {
        if (wasmMemory) {
          new DataView(wasmMemory.buffer).setBigUint64(outPtr, BigInt(Date.now()) * 1000000n, true);
        }
        return 0;
      },
      clock_res_get(id, outPtr) {
        if (wasmMemory) {
          new DataView(wasmMemory.buffer).setBigUint64(outPtr, 1000000n, true);
        }
        return 0;
      },
      random_get(ptr, len) {
        if (wasmMemory) crypto.getRandomValues(new Uint8Array(wasmMemory.buffer, ptr, len));
        return 0;
      },
      proc_exit(code) { throw new ProcExit(code); },
      proc_raise() { return WASI_ERRNO_NOSYS; },
      sched_yield() { return 0; },
      poll_oneoff() { return 0; },
      path_open(fd, _dirflags, pathPtr, pathLen, oflags, _rightsBase, _rightsInheriting, _fdflags, openedFdPtr) {
        const fdNum = toNumber(fd);
        const preopen = preopenByFd(fdNum);
        if (!preopen) return WASI_ERRNO_BADF;
        const rawPath = readGuestPath(pathPtr, pathLen);
        if (rawPath === null) return WASI_ERRNO_NOSYS;
        const relativePath = normalizeRelativePath(rawPath);
        if (relativePath === null) return WASI_ERRNO_INVAL;
        const opened = openResolvedPath(preopen, relativePath, toNumber(oflags));
        if (opened.errno !== 0) return opened.errno;
        return writeWasiU32(openedFdPtr, opened.fd) ? 0 : WASI_ERRNO_NOSYS;
      },
      path_filestat_get(fd, _flags, pathPtr, pathLen, bufPtr) {
        const fdNum = toNumber(fd);
        const preopen = preopenByFd(fdNum);
        if (!preopen) return WASI_ERRNO_BADF;
        const rawPath = readGuestPath(pathPtr, pathLen);
        if (rawPath === null) return WASI_ERRNO_NOSYS;
        const relativePath = normalizeRelativePath(rawPath);
        if (relativePath === null) return WASI_ERRNO_INVAL;
        const info = statResolvedPath(absoluteVfsPath(preopen, relativePath));
        if (!info) return WASI_ERRNO_NOENT;
        return writeFilestat(bufPtr, info.stat) ? 0 : WASI_ERRNO_NOSYS;
      },
      path_rename: wasiUnsupported,
      path_readlink: wasiUnsupported,
      path_unlink_file: wasiUnsupported,
      path_create_directory: wasiUnsupported,
      path_remove_directory: wasiUnsupported,
    };

    const boxInt = (value) => {
      let v = BigInt(value);
      if (v < 0n) {
        v = (1n << 47n) + v;
      }
      return QNAN | TAG_INT | (v & INT_MASK);
    };

    const normalizeI64Result = (value) =>
      value === undefined || value === null
        ? NONE_BITS
        : typeof value === "bigint"
          ? value
          : BigInt(value);

    const normalizeImportResult = (value, resultKind) => {
      if (resultKind === "i64") {
        return normalizeI64Result(value);
      }
      if (resultKind === "i32") {
        return typeof value === "bigint" ? Number(value) : Number(value);
      }
      return value;
    };

    const normalizeValueForKind = (value, kind) => {
      if (kind === "i64") {
        return normalizeI64Result(value);
      }
      if (kind === "i32") {
        return typeof value === "bigint" ? Number(value) : Number(value);
      }
      return value;
    };

    const formatDebugValue = (value) => {
      if (typeof value === "bigint") {
        return `${value}n`;
      }
      if (Array.isArray(value)) {
        return `[${value.map((item) => formatDebugValue(item)).join(", ")}]`;
      }
      if (value && typeof value === "object") {
        return Object.prototype.toString.call(value);
      }
      return String(value);
    };

    const callWithSignature = (fn, signature, args) => {
      if (!signature || !Array.isArray(signature.params)) {
        return fn(...args);
      }
      const callArgs = args.map((value, index) =>
        normalizeValueForKind(value, signature.params[index] || null),
      );
      const out = fn(...callArgs);
      return normalizeImportResult(out, signature.result || null);
    };

    const installTableRefs = (instance, table) => {
      if (!instance || !table) {
        return;
      }
      const refs = [];
      for (const [name, value] of Object.entries(instance.exports)) {
        const match = /^__molt_table_ref_(\\d+)$/.exec(name);
        if (!match || typeof value !== "function") {
          continue;
        }
        refs.push({ index: Number(match[1]), fn: value });
      }
      if (refs.length === 0) {
        return;
      }
      refs.sort((a, b) => a.index - b.index);
      const maxIndex = refs[refs.length - 1].index;
      if (maxIndex >= table.length) {
        table.grow(maxIndex + 1 - table.length);
      }
      for (const ref of refs) {
        table.set(ref.index, ref.fn);
      }
    };

    const ensureTableCapacityForExportedRefs = (instance, table) => {
      if (!instance || !table) {
        return;
      }
      let maxIndex = -1;
      for (const name of Object.keys(instance.exports)) {
        const match = /^__molt_table_ref_(\\d+)$/.exec(name);
        if (!match) {
          continue;
        }
        const idx = Number(match[1]);
        if (Number.isInteger(idx) && idx > maxIndex) {
          maxIndex = idx;
        }
      }
      if (maxIndex < 0 || maxIndex < table.length) {
        return;
      }
      table.grow(maxIndex + 1 - table.length);
    };

    const remapLegacyRuntimeSharedIdx = (idx) => {
      if (__MOLT_SHARED_TABLE_BASE__ === null || __MOLT_SHARED_TABLE_BASE__ <= LEGACY_WASM_TABLE_BASE) {
        return idx;
      }
      if (
        idx >= LEGACY_WASM_TABLE_BASE + RESERVED_RUNTIME_CALLABLE_BASE &&
        idx < LEGACY_WASM_TABLE_BASE + RESERVED_RUNTIME_SHARED_PREFIX_LEN
      ) {
        return idx - LEGACY_WASM_TABLE_BASE + __MOLT_SHARED_TABLE_BASE__;
      }
      return idx;
    };

    const buildRuntimeImports = (module, runtimeInstance) => {
      const imports = {};
      const callBindIc = runtimeInstance.exports.molt_call_bind_ic;
      const callargsNew = runtimeInstance.exports.molt_callargs_new;
      const callargsPushPos = runtimeInstance.exports.molt_callargs_push_pos;
      const dictSet = runtimeInstance.exports.molt_dict_set;
      const dictGetitemBorrowed = runtimeInstance.exports.molt_dict_getitem_borrowed;
      const tupleGetitemBorrowed = runtimeInstance.exports.molt_tuple_getitem_borrowed;
      const makeCallBindFallback = (arity) => {
        if (
          typeof callBindIc !== "function" ||
          typeof callargsNew !== "function" ||
          typeof callargsPushPos !== "function"
        ) {
          return null;
        }
        return (methodBits, ...argBits) => {
          const builderBits = callargsNew(boxInt(arity), boxInt(0));
          for (const argBitsValue of argBits) {
            callargsPushPos(builderBits, argBitsValue);
          }
          return callBindIc(boxInt(0), methodBits, builderBits);
        };
      };
      for (const entry of WebAssembly.Module.imports(module)) {
        if (entry.module !== "molt_runtime") continue;
        const exportName = entry.name.startsWith("molt_")
          ? entry.name
          : `molt_${entry.name}`;
        let fn = runtimeInstance.exports[exportName];
        if (typeof fn !== "function") {
          if (entry.name === "fast_list_append") {
            fn = makeCallBindFallback(1);
          } else if (entry.name === "fast_str_join") {
            fn = makeCallBindFallback(1);
          } else if (entry.name === "fast_dict_get") {
            fn = makeCallBindFallback(2);
          } else if (entry.name === "dict_setitem") {
            fn = typeof dictSet === "function" ? dictSet : null;
          } else if (entry.name === "dict_getitem") {
            fn = typeof dictGetitemBorrowed === "function" ? dictGetitemBorrowed : null;
          } else if (entry.name === "tuple_getitem") {
            fn = typeof tupleGetitemBorrowed === "function" ? tupleGetitemBorrowed : null;
          }
        }
        if (typeof fn !== "function") {
          throw new Error(`molt_runtime missing export ${exportName}`);
        }
        const signature = runtimeImportSignatures[entry.name] || null;
        const resultKind = runtimeImportResultKinds[entry.name] || null;
        imports[entry.name] = (...args) => {
          const callArgs = signature && Array.isArray(signature.params)
            ? args.map((value, index) => normalizeValueForKind(value, signature.params[index] || null))
            : args;
          let out;
          try {
            out = fn(...callArgs);
          } catch (err) {
            const detail = err && typeof err.message === "string" ? err.message : String(err);
            throw new Error(
              `runtime import ${entry.name} failed: ${detail}; args=${formatDebugValue(callArgs)}; signature=${formatDebugValue(signature)}`,
            );
          }
          return normalizeImportResult(out, resultKind);
        };
      }
      return imports;
    };

    const hostEnv = {
      memory: wasmMemory,
      molt_vfs_read(pathPtr, pathLen, outPtr, outCapacity, outLenPtr) {
        if (!wasmMemory) return -ENOSYS;
        const path = readPathUtf8(pathPtr, pathLen);
        const resolved = vfs.resolve(path);
        if (!resolved || !resolved.mount || typeof resolved.mount.read !== "function") {
          return -ENOENT;
        }
        try {
          const data = resolved.mount.read(resolved.rel);
          const cap = outCapacity >>> 0;
          if (data.byteLength > cap) return -EINVAL;
          new Uint8Array(wasmMemory.buffer, outPtr >>> 0, data.byteLength).set(data);
          new DataView(wasmMemory.buffer).setUint32(outLenPtr >>> 0, data.byteLength, true);
          return 0;
        } catch (err) {
          return writeErrno(err, ENOENT) * -1;
        }
      },
      molt_vfs_write(pathPtr, pathLen, dataPtr, dataLen) {
        if (!wasmMemory) return -ENOSYS;
        const path = readPathUtf8(pathPtr, pathLen);
        const resolved = vfs.resolve(path);
        if (!resolved || !resolved.mount || typeof resolved.mount.write !== "function") {
          return -EINVAL;
        }
        const bytes = new Uint8Array(wasmMemory.buffer, dataPtr >>> 0, dataLen >>> 0);
        try {
          resolved.mount.write(resolved.rel, bytes);
          return 0;
        } catch (err) {
          return writeErrno(err, EINVAL) * -1;
        }
      },
      molt_vfs_exists(pathPtr, pathLen) {
        if (!wasmMemory) return 0;
        const path = readPathUtf8(pathPtr, pathLen);
        const resolved = vfs.resolve(path);
        if (!resolved || !resolved.mount || typeof resolved.mount.exists !== "function") {
          return 0;
        }
        return resolved.mount.exists(resolved.rel) ? 1 : 0;
      },
      molt_vfs_unlink(pathPtr, pathLen) {
        if (!wasmMemory) return -ENOSYS;
        const path = readPathUtf8(pathPtr, pathLen);
        const resolved = vfs.resolve(path);
        if (!resolved || !resolved.mount || typeof resolved.mount.unlink !== "function") {
          return -EINVAL;
        }
        try {
          resolved.mount.unlink(resolved.rel);
          return 0;
        } catch (err) {
          return writeErrno(err, EINVAL) * -1;
        }
      },
      molt_isolate_import(...args) {
        if (!appInstance || !appInstance.exports.molt_isolate_import) {
          throw new Error("molt_isolate_import called before app instantiation");
        }
        return normalizeI64Result(appInstance.exports.molt_isolate_import(...args));
      },
      molt_time_timezone_host()  { return 0n; },
      molt_time_local_offset_host() { return 0n; },
      molt_getpid_host()         { return 1n; },
      molt_socket_clone_host()   { return -1n; },
      molt_socket_detach_host()  { return -1n; },
      molt_socket_accept_host()  { return -1n; },
      molt_socket_new_host()     { return -1n; },
      molt_time_tzname_host()    { return -1; },
      molt_process_write_host()  { return -1; },
      molt_process_close_stdin_host() { return -1; },
      molt_socket_wait_host()    { return -1; },
      molt_db_exec_host()        { return -1; },
      molt_db_query_host()       { return -1; },
      molt_ws_recv_host()        { return -1; },
      molt_ws_send_host()        { return -1; },
      molt_ws_close_host()       { return -1; },
      molt_socket_poll_host()    { return 0; },
      molt_ws_poll_host()        { return 0; },
      molt_process_terminate_host() { return -1; },
      molt_os_close_host()       { return 0; },
      molt_process_kill_host()   { return -1; },
      molt_process_wait_host()   { return -1; },
      molt_process_spawn_host()  { return -1; },
      molt_process_stdio_host()  { return -1; },
      molt_socket_bind_host()    { return -1; },
      molt_socket_close_host()   { return 0; },
      molt_socket_connect_host() { return -1; },
      molt_socket_connect_ex_host() { return -1; },
      molt_socket_getaddrinfo_host() { return -1; },
      molt_socket_gethostname_host() { return -1; },
      molt_socket_getpeername_host() { return -1; },
      molt_socket_getservbyname_host() { return -1; },
      molt_socket_getservbyport_host() { return -1; },
      molt_socket_getsockname_host() { return -1; },
      molt_socket_getsockopt_host() { return -1; },
      molt_socket_has_ipv6_host() { return 0; },
      molt_socket_listen_host()  { return -1; },
      molt_socket_recv_host()    { return -1; },
      molt_socket_recvfrom_host() { return -1; },
      molt_socket_recvmsg_host() { return -1; },
      molt_socket_send_host()    { return -1; },
      molt_socket_sendmsg_host() { return -1; },
      molt_socket_sendto_host()  { return -1; },
      molt_socket_setsockopt_host() { return -1; },
      molt_socket_shutdown_host() { return -1; },
      molt_socket_socketpair_host() { return -1; },
      molt_db_host_poll()        { return 0; },
      molt_process_host_poll()   { return 0; },
      molt_ws_connect_host()     { return -1; },
      molt_gpu_webgpu_dispatch_host() { return -38; },
    };

    for (let arity = 0; arity <= 13; arity++) {
      hostEnv[`molt_call_indirect${arity}`] = (fnIndex, ...args) => {
        const indirectName = `molt_call_indirect${arity}`;
        const idx = Number(fnIndex);
        const dispatchIdx = remapLegacyRuntimeSharedIdx(idx);
        const directName = `__molt_table_ref_${dispatchIdx}`;
        const indirectFn = appInstance?.exports?.[indirectName];
        if (typeof indirectFn === "function") {
          try {
            return indirectFn(fnIndex, ...args);
          } catch (err) {
            const detail = err && typeof err.message === "string" ? err.message : String(err);
            throw new Error(`${indirectName} app export failed at idx=${idx}: ${detail}; fnLen=${indirectFn.length}; argsLen=${args.length}`);
          }
        }
        const tableFn = sharedTable.get(dispatchIdx);
        if (typeof tableFn === "function") {
          try {
            const signature = appTableRefSignatures[directName] || runtimeTableRefSignatures[directName] || null;
            return callWithSignature(tableFn, signature, args);
          } catch (err) {
            const detail = err && typeof err.message === "string" ? err.message : String(err);
            const fnName = tableFn.name || "<anon>";
            throw new Error(`${indirectName} shared-table entry failed at idx=${dispatchIdx}: ${detail}; fnName=${fnName}; fnLen=${tableFn.length}; argsLen=${args.length}`);
          }
        }
        const rtDirectFn = rtInstance?.exports?.[directName];
        if (typeof rtDirectFn === "function") {
          try {
            return callWithSignature(rtDirectFn, runtimeTableRefSignatures[directName] || null, args);
          } catch (err) {
            const detail = err && typeof err.message === "string" ? err.message : String(err);
            throw new Error(`${indirectName} runtime direct export ${directName} failed: ${detail}; fnLen=${rtDirectFn.length}; argsLen=${args.length}`);
          }
        }
        if (typeof tableFn !== "function") {
          throw new Error(`${indirectName} missing table entry at ${dispatchIdx}`);
        }
        return tableFn(...args);
      };
    }

    // Shared table for indirect calls — both modules reference the same table.
    const sharedTable = new WebAssembly.Table({ initial: __MOLT_SHARED_TABLE_INITIAL__, element: "anyfunc" });

    let rtInstance = null;
    let pendingError = null;
    let procExit = null;
    try {
      const bundleBytes = await assetBytes("bundle.tar");
      if (bundleBytes) {
        vfs.loadBundleFromTar(bundleBytes);
      }
      // 1. Instantiate the runtime module first.
      //    The runtime imports host-owned shared memory/table plus host bridges.
      const rtImports = {
        wasi_snapshot_preview1: wasi,
        env: { ...hostEnv, __indirect_function_table: sharedTable },
      };
      rtInstance = await WebAssembly.instantiate(runtimeModule, rtImports);
      if (__MOLT_SHARED_TABLE_BASE__ !== null && rtInstance.exports.molt_set_wasm_table_base) {
        rtInstance.exports.molt_set_wasm_table_base(BigInt(__MOLT_SHARED_TABLE_BASE__));
      }
      installTableRefs(rtInstance, sharedTable);
      // 2. Instantiate the app module.
      //    It imports the runtime ABI exports plus the same host-owned memory/table.
      const appImports = {
        wasi_snapshot_preview1: wasi,
        env: {
          ...hostEnv,
          memory: wasmMemory,
          __indirect_function_table: sharedTable,
        },
        molt_runtime: buildRuntimeImports(appModule, rtInstance),
      };
      appInstance = await WebAssembly.instantiate(appModule, appImports);
      ensureTableCapacityForExportedRefs(appInstance, sharedTable);

      // 3. Initialize and run
      if (rtInstance.exports._initialize) rtInstance.exports._initialize();
      if (appInstance.exports.molt_table_init) appInstance.exports.molt_table_init();
      installTableRefs(appInstance, sharedTable);
      if (appInstance.exports.molt_main) appInstance.exports.molt_main();
      else if (appInstance.exports._start) appInstance.exports._start();
    } catch (err) {
      if (err instanceof ProcExit) procExit = err;
      else pendingError = err;
    } finally {
      if (rtInstance && rtInstance.exports.molt_runtime_shutdown) {
        try {
          rtInstance.exports.molt_runtime_shutdown();
        } catch (shutdownErr) {
          if (!pendingError) pendingError = shutdownErr;
        }
      }
      vfs.clear();
    }

    const stdoutTail = stdoutDecoder.decode();
    if (stdoutTail) stdoutChunks.push(stdoutTail);
    const stderrTail = stderrDecoder.decode();
    if (stderrTail) stderrChunks.push(stderrTail);

    if (pendingError) throw pendingError;

    const output = stdoutChunks.join("");
    const trimmed = output.trimStart();
    const contentType = (trimmed.startsWith("<!DOCTYPE html>") || trimmed.startsWith("<html"))
      ? "text/html; charset=utf-8"
      : "text/plain; charset=utf-8";
    return new Response(output, {
      status: procExit && procExit.code !== 0 ? 500 : 200,
      headers: { "content-type": contentType },
    });
  }
};
"""
    return (
        worker_js.replace(
            "__MOLT_SHARED_MEMORY_PAGES__", str(shared_memory_initial_pages)
        )
        .replace("__MOLT_SHARED_TABLE_INITIAL__", str(shared_table_initial))
        .replace(
            "__MOLT_RUNTIME_IMPORT_RESULT_KINDS__",
            runtime_import_result_kinds_json,
        )
        .replace(
            "__MOLT_RUNTIME_IMPORT_SIGNATURES__",
            runtime_import_signatures_json,
        )
        .replace(
            "__MOLT_APP_TABLE_REF_SIGNATURES__",
            app_table_ref_signatures_json,
        )
        .replace(
            "__MOLT_RUNTIME_TABLE_REF_SIGNATURES__",
            runtime_table_ref_signatures_json,
        )
        .replace(
            "__MOLT_LEGACY_WASM_TABLE_BASE__",
            str(legacy_wasm_table_base),
        )
        .replace(
            "__MOLT_RESERVED_RUNTIME_CALLABLE_BASE__",
            str(reserved_runtime_callable_base),
        )
        .replace(
            "__MOLT_RESERVED_RUNTIME_SHARED_PREFIX_LEN__",
            str(reserved_runtime_shared_prefix_len),
        )
        .replace(
            "__MOLT_SHARED_TABLE_BASE__",
            "null" if shared_table_base is None else str(shared_table_base),
        )
    )


def _generate_split_wrangler_jsonc(compatibility_date: str) -> str:
    return (
        "{\n"
        '  "name": "molt-app",\n'
        '  "main": "worker.js",\n'
        f'  "compatibility_date": "{compatibility_date}",\n'
        '  "no_bundle": true,\n'
        '  "find_additional_modules": true,\n'
        '  "rules": [\n'
        "    {\n"
        '      "type": "ESModule",\n'
        '      "globs": ["worker.js", "molt_vfs_browser.js"],\n'
        '      "fallthrough": false\n'
        "    },\n"
        "    {\n"
        '      "type": "CompiledWasm",\n'
        '      "globs": ["app.wasm", "molt_runtime.wasm"],\n'
        '      "fallthrough": false\n'
        "    }\n"
        "  ]\n"
        "}\n"
    )

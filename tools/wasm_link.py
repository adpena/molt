#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import functools
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from pathlib import Path

TOOLS_ROOT = Path(__file__).resolve().parent
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
SRC_ROOT = TOOLS_ROOT.parent / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

import harness_memory_guard  # noqa: E402
import artifact_publish  # noqa: E402
from molt.cli import wasm_toolchain  # noqa: E402

from wasm_link_format import (  # noqa: E402
    CALL_INDIRECT_MANGLED_RE as CALL_INDIRECT_MANGLED_RE,
    CALL_INDIRECT_RE as CALL_INDIRECT_RE,
    FLAG_BINDING_GLOBAL as FLAG_BINDING_GLOBAL,
    FLAG_EXPLICIT_NAME as FLAG_EXPLICIT_NAME,
    FLAG_EXPORTED as FLAG_EXPORTED,
    FLAG_NO_STRIP as FLAG_NO_STRIP,
    FLAG_UNDEFINED as FLAG_UNDEFINED,
    SYMBOL_DUMP_RE as SYMBOL_DUMP_RE,
    SYMBOL_KIND_FUNCTION as SYMBOL_KIND_FUNCTION,
    SYMTAB_SUBSECTION_ID as SYMTAB_SUBSECTION_ID,
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES as WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES,
    WASM_EXTERNAL_NATIVE_LINK_IMPORTS as WASM_EXTERNAL_NATIVE_LINK_IMPORTS,
    WASM_MAGIC as WASM_MAGIC,
    WASM_VERSION as WASM_VERSION,
    _ESSENTIAL_EXPORTS as _ESSENTIAL_EXPORTS,
    _OUTPUT_EXPORT_ALIAS_PREFIX as _OUTPUT_EXPORT_ALIAS_PREFIX,
    _append_linking_function_symbols as _append_linking_function_symbols,
    _build_custom_section as _build_custom_section,
    _build_linking_payload as _build_linking_payload,
    _build_sections as _build_sections,
    _collect_custom_names as _collect_custom_names,
    _collect_element_declared_funcs as _collect_element_declared_funcs,
    _collect_exports as _collect_exports,
    _collect_func_names as _collect_func_names,
    _collect_function_exports as _collect_function_exports,
    _collect_imports as _collect_imports,
    _collect_linking_function_symbols as _collect_linking_function_symbols,
    _collect_module_imports as _collect_module_imports,
    _count_func_imports as _count_func_imports,
    _declare_ref_func_elements as _declare_ref_func_elements,
    _ensure_table_export as _ensure_table_export,
    _find_func_import_index as _find_func_import_index,
    _flatten_rec_groups as _flatten_rec_groups,
    _has_table as _has_table,
    _is_wasm_binary as _is_wasm_binary,
    _parse_custom_section as _parse_custom_section,
    _parse_func_type_indices as _parse_func_type_indices,
    _parse_import_desc as _parse_import_desc,
    _parse_linking_payload as _parse_linking_payload,
    _parse_sections as _parse_sections,
    _parse_symbol_flags as _parse_symbol_flags,
    _parse_type_section as _parse_type_section,
    _read_string as _read_string,
    _read_varuint as _read_varuint,
    _read_varsint as _read_varsint,
    call_indirect_import_name_for_arity as call_indirect_import_name_for_arity,
    is_table_ref_export_name as is_table_ref_export_name,
    is_call_indirect_import_name as is_call_indirect_import_name,
    parse_table_ref_export_name as parse_table_ref_export_name,
    table_ref_export_name as table_ref_export_name,
    wasm_runtime_export_name as wasm_runtime_export_name,
    _repair_out_of_bounds_func_refs as _repair_out_of_bounds_func_refs,
    _safe_repair_out_of_bounds_func_refs as _safe_repair_out_of_bounds_func_refs,
    _scan_code_ref_funcs as _scan_code_ref_funcs,
    _skip_init_expr as _skip_init_expr,
    _validate_elements as _validate_elements,
    _validate_linked_table_import_contract as _validate_linked_table_import_contract,
    _write_string as _write_string,
    _write_varuint as _write_varuint,
)
from wasm_link_edit import (  # noqa: E402
    _add_symtab_alias as _add_symtab_alias,
    _append_table_ref_elements as _append_table_ref_elements,
    _build_runtime_stub as _build_runtime_stub,
    _canonicalize_standard_section_order as _canonicalize_standard_section_order,
    _collect_output_export_symbol_map as _collect_output_export_symbol_map,
    _collect_output_wrapper_specs as _collect_output_wrapper_specs,
    _collect_preserved_output_export_names as _collect_preserved_output_export_names,
    _dominant_output_module_prefix as _dominant_output_module_prefix,
    _drop_linked_app_active_table_elements as _drop_linked_app_active_table_elements,
    _ensure_function_exports_by_symbol_names as _ensure_function_exports_by_symbol_names,
    _entry_module_prefix_from_main_init as _entry_module_prefix_from_main_init,
    _highest_exported_table_ref_index as _highest_exported_table_ref_index,
    _inject_output_export_aliases as _inject_output_export_aliases,
    _is_public_output_export_name as _is_public_output_export_name,
    _memory_import_min as _memory_import_min,
    _rename_export_names as _rename_export_names,
    _required_linked_table_min as _required_linked_table_min,
    _restore_output_export_aliases as _restore_output_export_aliases,
    _rewrite_native_runtime_imports as _rewrite_native_runtime_imports,
    _rewrite_memory_min as _rewrite_memory_min,
    _rewrite_output_imports as _rewrite_output_imports,
    _rewrite_runtime_import_module_namespace as _rewrite_runtime_import_module_namespace,
    _rewrite_table_import_min as _rewrite_table_import_min,
    _split_app_reference_function_exports as _split_app_reference_function_exports,
    _strip_internal_exports as _strip_internal_exports,
    _table_import_min as _table_import_min,
)
from wasm_link_optimize import (  # noqa: E402
    _dedup_data_segments as _dedup_data_segments,
    _neutralize_dead_element_entries as _neutralize_dead_element_entries,
    _post_link_optimize as _post_link_optimize,
    _strip_debug_sections as _strip_debug_sections,
    _strip_unused_module_function_imports as _strip_unused_module_function_imports,
    _stub_dead_functions as _stub_dead_functions,
)


# Rust wasm symbol names include a hash suffix like "17h<hex...>E". Capture the arity
# digits that precede the 2-digit hash-length tag so 10+ arities don't get truncated.


def _run_external_tool(
    cmd: Sequence[str],
    *,
    capture_output: bool = True,
    text: bool = True,
    timeout: float | None = None,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    result = harness_memory_guard.guarded_completed_process(
        list(cmd),
        prefix="MOLT_WASM_LINK",
        cwd=cwd,
        env=env,
        capture_output=capture_output,
        text=text,
        timeout=timeout,
    )
    if (
        timeout is not None
        and result.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
        and "memory_guard: timeout after" in (result.stderr or "")
    ):
        raise subprocess.TimeoutExpired(
            list(cmd),
            timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
    return result


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


def _runtime_integrity_sidecar_path(path: Path) -> Path:
    return path.with_name(f"{path.name}.sha256")


_RUNTIME_INTEGRITY_PAIR_ATTEMPTS = 8
_RUNTIME_INTEGRITY_PAIR_RETRY_DELAY_SEC = 0.05


def _read_runtime_integrity_sidecar(path: Path) -> str | None:
    sidecar = _runtime_integrity_sidecar_path(path)
    if not sidecar.exists():
        return None
    raw = sidecar.read_text(encoding="utf-8").strip()
    match = re.search(r"\b([0-9a-fA-F]{64})\b", raw)
    if match is None:
        raise SystemExit(f"Runtime integrity sidecar is malformed: {sidecar}")
    return match.group(1).lower()


def _verify_runtime_integrity(path: Path) -> None:
    """Verify SHA-256 integrity of the runtime binary.

    Raises ``SystemExit`` when no integrity authority exists or a hash mismatch
    is detected.
    """
    # Reject path-traversal components before reading the file.
    for part in path.parts:
        if part == "..":
            raise SystemExit(f"Runtime path contains '..' traversal component: {path}")

    sidecar_mismatch: tuple[str, str] | None = None
    missing_sidecar_digest: str | None = None
    for attempt in range(_RUNTIME_INTEGRITY_PAIR_ATTEMPTS):
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        sidecar_expected = _read_runtime_integrity_sidecar(path)
        if sidecar_expected is None:
            missing_sidecar_digest = digest
            break
        if digest == sidecar_expected:
            return
        sidecar_mismatch = (sidecar_expected, digest)
        if attempt + 1 < _RUNTIME_INTEGRITY_PAIR_ATTEMPTS:
            time.sleep(_RUNTIME_INTEGRITY_PAIR_RETRY_DELAY_SEC)

    if sidecar_mismatch is not None:
        sidecar_expected, digest = sidecar_mismatch
        raise SystemExit(
            f"Runtime integrity check failed for {path}\n"
            f"  source: sidecar {_runtime_integrity_sidecar_path(path)}\n"
            f"  expected SHA-256: {sidecar_expected}\n"
            f"  actual   SHA-256: {digest}\n"
        )

    digest_line = (
        f"  actual   SHA-256: {missing_sidecar_digest}\n"
        if missing_sidecar_digest is not None
        else ""
    )
    raise SystemExit(
        "Runtime integrity check failed for "
        f"{path}\n  no sidecar was found at "
        f"{_runtime_integrity_sidecar_path(path)}\n"
        f"{digest_line}"
        "  publish the matching .sha256 sidecar; hardcoded runtime hash pins "
        "are not supported."
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
    res = _run_external_tool(
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


def _find_call_indirect_mangled(runtime: Path) -> dict[str, str]:
    wasm_tools = _find_tool(["wasm-tools"])
    names: dict[str, str] = {}
    for flags, _, name, _ in _dump_symbols(runtime, wasm_tools):
        if not (flags & FLAG_UNDEFINED):
            continue
        match = CALL_INDIRECT_RE.fullmatch(name)
        if match:
            import_name = call_indirect_import_name_for_arity(match.group(1))
            if import_name is not None:
                names[import_name] = name
            continue
        mangled_match = CALL_INDIRECT_MANGLED_RE.search(name)
        if mangled_match:
            import_name = call_indirect_import_name_for_arity(mangled_match.group(1))
            if import_name is not None:
                names[import_name] = name
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
        if is_call_indirect_import_name(name):
            symbols[name] = (index, flags)
    if not symbols and not wasm_tools:
        print(
            "wasm-tools not found; cannot extract output symbol info.", file=sys.stderr
        )
    if not symbols:
        print("Unable to locate output call_indirect symbols.", file=sys.stderr)
    return symbols


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
        result = _run_external_tool(
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
    tmp_path = artifact_publish.staged_output_path(path)
    try:
        tmp_path.write_bytes(data)
        artifact_publish.publish_validated_outputs([(tmp_path, path)])
    finally:
        with contextlib.suppress(OSError):
            tmp_path.unlink()


def _canonical_split_runtime_required_exports(runtime_data: bytes) -> set[str]:
    """Return runtime exports that remain app-visible split-runtime contracts."""
    return {
        name
        for name in _collect_function_exports(runtime_data)
        if name not in _ESSENTIAL_EXPORTS
        and name not in {"molt_exception_pending"}
        and not is_table_ref_export_name(name)
    }


def _read_link_allowlist_symbols(path: Path) -> list[str]:
    return [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]


_COMPILER_RT_LINK_IMPORT_CLASS = "wasm_compiler_rt_link_import"
_DEFAULT_SPLIT_APP_GLOBAL_BASE = 64 * 1024 * 1024


def _external_native_host_link_imports() -> tuple[str, ...]:
    return tuple(
        symbol
        for symbol in WASM_EXTERNAL_NATIVE_LINK_IMPORTS
        if WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(symbol)
        != _COMPILER_RT_LINK_IMPORT_CLASS
    )


def _compiler_rt_link_imports() -> frozenset[str]:
    return frozenset(
        symbol
        for symbol, primitive_class in WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.items()
        if primitive_class == _COMPILER_RT_LINK_IMPORT_CLASS
    )


def _native_wasm_import_names(path: Path) -> set[str]:
    try:
        data = path.read_bytes()
    except OSError:
        return set()
    if not _is_wasm_binary(data):
        return set()
    try:
        return {
            name for _module, name, kind, _desc in _collect_imports(data) if kind == 0
        }
    except ValueError:
        return set()


def _compiler_rt_imports_required_by_native_objects(
    native_objects: Sequence[Path],
) -> frozenset[str]:
    compiler_rt_imports = _compiler_rt_link_imports()
    return frozenset(
        sorted(
            name
            for native_object in native_objects
            for name in _native_wasm_import_names(native_object)
            if name in compiler_rt_imports
        )
    )


def _is_compiler_rt_provider_path(path: Path) -> bool:
    return path.name == "libcompiler_builtins.rlib" or (
        path.name.startswith("libcompiler_builtins-") and path.suffix == ".rlib"
    )


def _compiler_rt_provider_inputs(
    native_objects: Sequence[Path],
    required_symbols: frozenset[str],
) -> tuple[Path, ...]:
    if not required_symbols:
        return ()
    if any(_is_compiler_rt_provider_path(path) for path in native_objects):
        return ()
    provider = wasm_toolchain.wasm_compiler_builtins_archive()
    if provider is None:
        missing = ", ".join(sorted(required_symbols))
        raise ValueError(
            "wasm_compiler_rt_link_import symbols require Rust wasm32-wasip1 "
            f"libcompiler_builtins provider; missing provider for: {missing}"
        )
    provider = provider.resolve(strict=False)
    if not provider.exists():
        raise ValueError(
            f"wasm_compiler_rt_link_import provider does not exist: {provider}"
        )
    return (provider,)


def _resolve_native_link_inputs(native_objects: Sequence[Path]) -> tuple[Path, ...]:
    native_inputs = tuple(native_objects)
    required_compiler_rt = _compiler_rt_imports_required_by_native_objects(
        native_inputs
    )
    return (
        *native_inputs,
        *_compiler_rt_provider_inputs(native_inputs, required_compiler_rt),
    )


def _read_const_i32_init_expr(data: bytes, offset: int) -> tuple[int, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading data offset expression")
    opcode = data[offset]
    offset += 1
    if opcode != 0x41:
        raise ValueError(
            f"Unsupported data offset expression opcode 0x{opcode:02x}; "
            "expected i32.const"
        )
    value, offset = _read_varsint(data, offset)
    if offset >= len(data):
        raise ValueError("Unexpected EOF after data offset expression")
    terminator = data[offset]
    offset += 1
    if terminator != 0x0B:
        raise ValueError(
            f"Unsupported data offset expression terminator 0x{terminator:02x}; "
            "expected end"
        )
    return value, offset


def _active_data_segment_min(data: bytes) -> int | None:
    minimum: int | None = None
    for section_id, payload in _parse_sections(data):
        if section_id != 11:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_varuint(payload, offset)
            if flags == 1:
                size, offset = _read_varuint(payload, offset)
                offset += size
                continue
            if flags == 2:
                _memory_index, offset = _read_varuint(payload, offset)
            elif flags != 0:
                raise ValueError(f"Unsupported data segment flags: {flags}")
            data_offset, offset = _read_const_i32_init_expr(payload, offset)
            size, offset = _read_varuint(payload, offset)
            offset += size
            if data_offset >= 0:
                minimum = data_offset if minimum is None else min(minimum, data_offset)
    return minimum


def _split_app_global_base(output_data: bytes) -> int:
    active_min = _active_data_segment_min(output_data)
    if active_min is not None and active_min > 0:
        return active_min
    return _DEFAULT_SPLIT_APP_GLOBAL_BASE


def _public_output_export_symbol_map(
    output_data: bytes,
    *,
    preserved_output_exports: Sequence[str],
    export_symbol_map: Mapping[str, str],
) -> dict[str, str]:
    public_export_map = {
        name: export_symbol_map[name]
        for name in preserved_output_exports
        if name in export_symbol_map
    }
    public_export_map.update(
        {
            name: export_symbol_map[name]
            for name in (
                "molt_host_init",
                "molt_main",
                "molt_table_init",
                "molt_set_wasm_table_base",
            )
            if name in export_symbol_map
        }
    )
    public_export_map.update(
        {
            name: export_symbol_map[name]
            for name in _collect_function_exports(output_data)
            if is_table_ref_export_name(name) and name in export_symbol_map
        }
    )
    return public_export_map


def _restore_public_output_exports(
    data: bytes,
    public_export_map: Mapping[str, str],
) -> bytes:
    restored = data
    updated = _ensure_function_exports_by_symbol_names(restored, public_export_map)
    if updated is not None:
        restored = updated
    rename_map = {
        symbol_name: public_name
        for public_name, symbol_name in public_export_map.items()
        if symbol_name != public_name
    }
    updated = _rename_export_names(restored, rename_map)
    if updated is not None:
        restored = updated
    updated = _restore_output_export_aliases(restored)
    if updated is not None:
        restored = updated
    return restored


_TRAP_FUNC_BODY = bytes([0x00, 0x00, 0x0B])


def _required_native_direct_symbols(output_data: bytes) -> tuple[str, ...]:
    return tuple(
        sorted(
            {
                name
                for module, name, kind, _desc in _collect_imports(output_data)
                if module == "molt_native" and kind == 0
            }
        )
    )


def _function_body_payloads_by_index(data: bytes) -> dict[int, bytes]:
    sections = _parse_sections(data)
    import_count = _count_func_imports(sections)
    for section_id, payload in sections:
        if section_id != 10:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        bodies: dict[int, bytes] = {}
        for local_index in range(count):
            body_size, body_start = _read_varuint(payload, offset)
            body_end = body_start + body_size
            if body_end > len(payload):
                raise ValueError("Unexpected EOF while reading function body")
            bodies[import_count + local_index] = payload[body_start:body_end]
            offset = body_end
        return bodies
    return {}


def _validate_required_native_direct_symbols(
    linked_data: bytes,
    required_symbols: Sequence[str],
    *,
    description: str,
) -> str | None:
    if not required_symbols:
        return None
    exports = _collect_function_exports(linked_data)
    bodies = _function_body_payloads_by_index(linked_data)
    missing: list[str] = []
    unresolved: list[str] = []
    trap_stubs: list[str] = []
    for symbol in required_symbols:
        func_index = exports.get(symbol)
        if func_index is None:
            missing.append(symbol)
            continue
        body = bodies.get(func_index)
        if body is None:
            unresolved.append(symbol)
            continue
        if body == _TRAP_FUNC_BODY:
            trap_stubs.append(symbol)
    if missing or unresolved or trap_stubs:
        parts: list[str] = []
        if missing:
            parts.append("missing export(s): " + ", ".join(missing))
        if unresolved:
            parts.append("exported unresolved import(s): " + ", ".join(unresolved))
        if trap_stubs:
            parts.append("trap stub(s): " + ", ".join(trap_stubs))
        return (
            f"{description} did not link required native direct symbol(s): "
            + "; ".join(parts)
        )
    return None


def _compose_wasm_ld_allowlist(
    *,
    base_allowlist: Path,
    native_objects: Sequence[Path],
    temp_dir: tempfile.TemporaryDirectory,
) -> Path:
    """Return the wasm-ld allowlist for this link transaction.

    The checked-in allowlist is the runtime/user-program import contract.  Native
    package objects need the generated external-native toolchain/libc/C++ import
    surface too; keep that authority generated and transaction-local so the base
    runtime allowlist does not grow a second copy of package closure policy.
    """
    if not native_objects:
        return base_allowlist
    symbols = sorted(
        {
            *_read_link_allowlist_symbols(base_allowlist),
            *_external_native_host_link_imports(),
        }
    )
    composed = Path(temp_dir.name) / "wasm_allowed_imports.external_native.txt"
    composed.write_text(
        "\n".join(
            [
                "# @generated transaction-local by tools/wasm_link.py",
                "# runtime allowlist + generated external native link imports",
                *symbols,
                "",
            ]
        ),
        encoding="utf-8",
    )
    return composed


def _compose_split_runtime_native_allowlist(
    *,
    base_allowlist: Path,
    native_objects: Sequence[Path],
    runtime_exports: set[str],
    temp_dir: tempfile.TemporaryDirectory,
) -> Path:
    """Return the deployed split-app allowlist for static native extensions.

    The monolithic validation link resolves Molt ABI symbols against the runtime
    stub. The deployed split app deliberately leaves those same symbols as
    ``molt_runtime`` imports, so wasm-ld must allow the generated runtime export
    surface only for that transaction-local app link.
    """
    if not native_objects:
        return base_allowlist
    symbols = sorted(
        {
            *_read_link_allowlist_symbols(base_allowlist),
            *_external_native_host_link_imports(),
            *runtime_exports,
        }
    )
    composed = Path(temp_dir.name) / "wasm_allowed_imports.split_runtime_native.txt"
    composed.write_text(
        "\n".join(
            [
                "# @generated transaction-local by tools/wasm_link.py",
                "# split-runtime native app imports: host + external-native + runtime ABI",
                *symbols,
                "",
            ]
        ),
        encoding="utf-8",
    )
    return composed


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
    for name in required_exports:
        export_name = wasm_runtime_export_name(name)
        if export_name is not None:
            normalized_required_exports.add(export_name)
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
    raw_dynamic_exports = os.environ.get(
        "MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS", ""
    ).strip()
    if raw_dynamic_exports:
        normalized_required_exports.update(
            name.strip() for name in raw_dynamic_exports.split(",") if name.strip()
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
        # `--disable-gc` keeps wasm-opt from re-encoding the type section as a
        # GC-proposal recursive type group (`0x4E`), which non-GC engines (the
        # molt host runner, Cloudflare V8) reject — see wasm_optimize.py.
        feature_flags = [
            "--enable-bulk-memory",
            "--enable-mutable-globals",
            "--enable-sign-ext",
            "--enable-nontrapping-float-to-int",
            "--enable-simd",
            "--enable-multivalue",
            "--enable-reference-types",
            "--disable-gc",
            "--enable-tail-call",
            "--disable-custom-descriptors",
        ]

        cache_path = _tree_shake_runtime_cache_root() / (
            _tree_shake_runtime_cache_key(
                optimized_baseline=optimized_baseline,
                normalized_required_exports=normalized_required_exports,
                wasm_opt=wasm_opt,
                feature_flags=feature_flags,
            )
            + ".wasm"
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
            "-o",
            str(output_path),
            "-Oz",
            "--converge",
            "--remove-unused-module-elements",
            "--closed-world",
            "--strip-debug",
            "--strip-producers",
            "--vacuum",
        ] + feature_flags

        try:
            result = _run_external_tool(
                cmd,
                capture_output=True,
                text=True,
                timeout=120,
            )
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


def _canonicalize_wasm_ld_output(data: bytes, *, description: str) -> bytes:
    try:
        flattened = _flatten_rec_groups(data)
    except ValueError as exc:
        raise ValueError(
            f"Failed to flatten {description} wasm rec groups: {exc}"
        ) from exc
    return data if flattened is None else flattened


# Minimal function body: 0 locals, ``unreachable``, ``end``.


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
        (module, name) for module, name, _, _ in imports if module == "molt_runtime"
    ]
    if runtime_imports:
        for module, name in runtime_imports:
            print(
                f"Freestanding validation error: remaining molt_runtime import {module}::{name}",
                file=sys.stderr,
            )
        return False

    other_imports = [
        (module, name) for module, name, _, _ in imports if module != "env"
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
            result = _run_external_tool(
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
        if module == "env" and kind == 0 and is_call_indirect_import_name(name)
    ]
    if call_indirect:
        print(
            f"Linked wasm still imports {', '.join(sorted(call_indirect))}; "
            "remove JS call_indirect stubs.",
            file=sys.stderr,
        )
        return False
    ok, err = _validate_linked_table_import_contract(imports)
    if not ok:
        print(f"Linked wasm table import validation failed: {err}", file=sys.stderr)
        return False
    if any(kind == 1 for _, _, kind, _ in imports):
        print(
            "Linked wasm retains env::__indirect_function_table under the "
            "host-table contract.",
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
            result = _run_external_tool(
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


def _validate_split_runtime_outputs(app_wasm: Path, rt_wasm: Path) -> bool:
    try:
        app_data = app_wasm.read_bytes()
        rt_data = rt_wasm.read_bytes()
    except OSError as exc:
        print(f"Failed to read split-runtime staged output: {exc}", file=sys.stderr)
        return False
    if not _is_wasm_binary(app_data):
        print(
            f"Split-runtime app output is not a wasm binary: {app_wasm}",
            file=sys.stderr,
        )
        return False
    if not _is_wasm_binary(rt_data):
        print(
            f"Split-runtime shared runtime output is not a wasm binary: {rt_wasm}",
            file=sys.stderr,
        )
        return False
    try:
        app_imports = _collect_module_imports(app_data, "molt_runtime")
        rt_exports = _collect_function_exports(rt_data)
        app_memory_min = _memory_import_min(app_data)
    except ValueError as exc:
        print(f"Failed to parse split-runtime staged output: {exc}", file=sys.stderr)
        return False
    if app_memory_min is None:
        print(
            "Split-runtime app must import env.memory; a private app memory "
            "breaks pointer-bearing runtime ABI calls.",
            file=sys.stderr,
        )
        return False
    missing: list[str] = []
    for name in app_imports:
        export_name = wasm_runtime_export_name(name)
        if name in rt_exports:
            continue
        if export_name is not None and export_name in rt_exports:
            continue
        if name in _ESSENTIAL_EXPORTS:
            continue
        missing.append(name)
    missing.sort()
    if missing:
        print(
            "Split-runtime app imports are absent from staged shared runtime: "
            f"{', '.join(missing)}",
            file=sys.stderr,
        )
        return False
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
    temp_output = artifact_publish.staged_output_path(linked)
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
        with contextlib.suppress(OSError):
            temp_output.unlink()
        return False

    artifact_publish.publish_validated_outputs([(temp_output, linked)])

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
    native_objects: Sequence[Path] = (),
) -> int:
    for native_object in native_objects:
        if not native_object.exists():
            print(f"Native WASM link input not found: {native_object}", file=sys.stderr)
            return 1
    try:
        native_objects = _resolve_native_link_inputs(tuple(native_objects))
    except ValueError as exc:
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        return 1
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
    output_memory_min = _memory_import_min(output_data)
    required_native_direct_symbols = _required_native_direct_symbols(output_data)
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
    native_link_inputs, native_force_exports = _rewrite_native_runtime_imports(
        tuple(native_objects),
        runtime_exports,
        temp_dir,
    )
    force_exports.extend(native_force_exports)
    rewritten_path = _inject_call_indirect_alias(rewritten_path, runtime, temp_dir)
    if allowlist_override is not None:
        base_allowlist = allowlist_override
    else:
        base_allowlist = Path(__file__).parent / "wasm_allowed_imports.txt"
    if not base_allowlist.exists():
        print(f"Allowlist not found: {base_allowlist}", file=sys.stderr)
        return 1
    allowlist = _compose_wasm_ld_allowlist(
        base_allowlist=base_allowlist,
        native_objects=native_objects,
        temp_dir=temp_dir,
    )
    split_native_allowlist = _compose_split_runtime_native_allowlist(
        base_allowlist=base_allowlist,
        native_objects=native_link_inputs,
        runtime_exports=runtime_exports,
        temp_dir=temp_dir,
    )
    stub_link_rewritten_path = rewritten_path
    stub_link_native_inputs = native_link_inputs
    if split_runtime and native_objects:
        # The validation link resolves Molt ABI imports against a generated
        # runtime stub in the normal wasm-ld symbol namespace. The deployed
        # split app keeps the same imports as ``molt_runtime`` ABI edges.
        stub_rewrite = _rewrite_runtime_import_module_namespace(
            rewritten_path,
            source_module="molt_runtime",
            target_module="env",
            runtime_exports=runtime_exports,
            temp_dir=temp_dir,
            filename="output_stub_link_imports.wasm",
        )
        if stub_rewrite is None:
            return 1
        stub_link_rewritten_path, stub_force_exports = stub_rewrite
        force_exports.extend(stub_force_exports)
        stub_link_native_inputs = tuple(native_objects)
    staged_outputs: list[Path] = []
    work_linked = artifact_publish.staged_output_path(linked)
    staged_outputs.append(work_linked)
    app_wasm: Path | None = None
    rt_wasm: Path | None = None
    app_stage: Path | None = None
    rt_stage: Path | None = None

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
        reloc_candidate = runtime.with_name(
            runtime.name.replace(".wasm", "_reloc.wasm")
        )
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
            print(f"Failed to build runtime stub: {exc}", file=sys.stderr)
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
        "-z",
        "stack-size=1048576",
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
    for sym in sorted(
        _ESSENTIAL_EXPORTS - {"__indirect_function_table", "memory", "molt_main"}
    ):
        cmd.append(f"--export-if-defined={sym}")
    for sym in required_native_direct_symbols:
        cmd.append(f"--undefined={sym}")
        cmd.append(f"--export-if-defined={sym}")
    for sym in user_export_symbol_names:
        cmd.append(f"--export={sym}")
    cmd += [
        "-o",
        str(work_linked),
        str(stub_link_rewritten_path),
        str(link_runtime_path),
    ]
    cmd.extend(str(native_object) for native_object in stub_link_native_inputs)

    split_native_app_path: Path | None = None
    split_native_app_cmd: list[str] | None = None
    if split_runtime and native_objects:
        split_native_app_path = Path(temp_dir.name) / "app_native_linked.wasm"
        split_app_global_base = _split_app_global_base(output_data)
        split_native_prefix = [
            f"--allow-undefined-file={split_native_allowlist}"
            if part.startswith("--allow-undefined-file=")
            else "--no-stack-first"
            if part == "--stack-first"
            else part
            for part in cmd[: cmd.index("-o")]
        ]
        split_native_app_cmd = [
            *split_native_prefix,
            "--import-memory",
            f"--global-base={split_app_global_base}",
            "-o",
            str(split_native_app_path),
            str(rewritten_path),
            *(str(native_object) for native_object in native_link_inputs),
        ]

    res = _run_external_tool(cmd, capture_output=True, text=True)
    try:
        if res.returncode != 0:
            err = res.stderr.strip() or res.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            return res.returncode
        if not work_linked.exists():
            print(
                "wasm-ld exited successfully but produced no linked output: "
                f"{work_linked}",
                file=sys.stderr,
            )
            return 1
        linked_bytes = _read_wasm_bytes_with_retry(work_linked)
        if not _is_wasm_binary(linked_bytes):
            print(
                "wasm-ld produced non-wasm linked output "
                f"({work_linked}, size={len(linked_bytes)} bytes)",
                file=sys.stderr,
            )
            return 1
        # wasm-ld 22 emits the merged type section as a GC-proposal recursive
        # type group even when every member is a plain MVP func type. Flatten it
        # back to standalone types before ANY downstream step: the molt host
        # runner / Cloudflare V8 / wasm-opt all reject the `0x4E` rec-group
        # encoding without the GC proposal, and the linker's own
        # `_parse_type_section` assumes the standalone-`func` form. Doing this
        # first keeps every later type-section-aware pass operating on a
        # canonical MVP type section.
        try:
            canonical_linked_bytes = _canonicalize_wasm_ld_output(
                linked_bytes, description="linked"
            )
        except ValueError as exc:
            print(str(exc), file=sys.stderr)
            return 1
        if canonical_linked_bytes != linked_bytes:
            work_linked.write_bytes(canonical_linked_bytes)
            linked_bytes = canonical_linked_bytes
        public_export_map = _public_output_export_symbol_map(
            output_data,
            preserved_output_exports=preserved_output_exports,
            export_symbol_map=export_symbol_map,
        )
        restored_linked_bytes = _restore_public_output_exports(
            linked_bytes, public_export_map
        )
        if restored_linked_bytes != linked_bytes:
            work_linked.write_bytes(restored_linked_bytes)
            linked_bytes = restored_linked_bytes
        try:
            native_link_error = _validate_required_native_direct_symbols(
                linked_bytes,
                required_native_direct_symbols,
                description="Wasm native link",
            )
        except ValueError as exc:
            print(f"Failed to inspect native direct symbols: {exc}", file=sys.stderr)
            return 1
        if native_link_error is not None:
            print(native_link_error, file=sys.stderr)
            return 1

        if not split_runtime:
            try:
                updated = _neutralize_linked_table_init(linked_bytes)
            except ValueError as exc:
                print(f"Failed to neutralize linked table init: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
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
            work_linked.write_bytes(linked_bytes)

        if not split_runtime:
            try:
                updated = _drop_linked_app_active_table_elements(linked_bytes)
            except ValueError as exc:
                print(
                    f"Failed to drop linked app table elements: {exc}",
                    file=sys.stderr,
                )
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated

        if optimize:
            if _run_wasm_opt_via_optimize(
                work_linked,
                level=optimize_level,
                converge=False,
            ):
                # Re-read after optimization since the file changed on disk
                linked_bytes = work_linked.read_bytes()

        output_table_min = _table_import_min(output.read_bytes())
        required_table_min = _required_linked_table_min(linked_bytes, output_table_min)
        if required_table_min is not None:
            try:
                updated = _rewrite_table_import_min(linked_bytes, required_table_min)
            except ValueError as exc:
                print(f"Failed to rewrite linked table min: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        if output_memory_min is not None:
            try:
                updated = _rewrite_memory_min(linked_bytes, output_memory_min)
            except ValueError as exc:
                print(f"Failed to rewrite linked memory min: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        append_table_refs_raw = os.environ.get("MOLT_WASM_LINK_APPEND_TABLE_REFS")
        append_table_refs = (
            True
            if append_table_refs_raw is None
            else append_table_refs_raw.strip().lower()
            not in {"0", "false", "no", "off"}
        )
        if append_table_refs and split_runtime:
            try:
                updated = _append_table_ref_elements(
                    linked_bytes,
                )
            except ValueError as exc:
                print(f"Failed to append table ref elements: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        try:
            referenced_ref_funcs = _scan_code_ref_funcs(linked_bytes)
        except ValueError as exc:
            print(f"Failed to inspect ref.func instructions: {exc}", file=sys.stderr)
            return 1
        if referenced_ref_funcs:
            try:
                updated = _declare_ref_func_elements(linked_bytes)
            except ValueError as exc:
                print(f"Failed to declare ref.func elements: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        try:
            updated = _ensure_table_export(linked_bytes)
        except ValueError as exc:
            print(f"Failed to ensure table export: {exc}", file=sys.stderr)
            return 1
        if updated is not None:
            work_linked.write_bytes(updated)
            linked_bytes = updated
        if not split_runtime:
            updated = _strip_internal_exports(
                linked_bytes,
                preserve_exports=set(preserved_output_exports),
                preserve_table_refs=False,
            )
            if updated is not None:
                work_linked.write_bytes(updated)
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
                    work_linked.write_bytes(linked_bytes)
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
            app_stage = artifact_publish.staged_output_path(app_wasm)
            rt_stage = artifact_publish.staged_output_path(rt_wasm)
            staged_outputs.extend([app_stage, rt_stage])

            if split_native_app_cmd is not None:
                assert split_native_app_path is not None
                split_native_res = _run_external_tool(
                    split_native_app_cmd,
                    capture_output=True,
                    text=True,
                )
                if split_native_res.returncode != 0:
                    err = (
                        split_native_res.stderr.strip()
                        or split_native_res.stdout.strip()
                    )
                    if err:
                        print(err, file=sys.stderr)
                    return split_native_res.returncode
                if not split_native_app_path.exists():
                    print(
                        "wasm-ld exited successfully but produced no split app "
                        f"native-linked output: {split_native_app_path}",
                        file=sys.stderr,
                    )
                    return 1
                rewritten_data = _read_wasm_bytes_with_retry(split_native_app_path)
                if not _is_wasm_binary(rewritten_data):
                    print(
                        "wasm-ld produced non-wasm split app native-linked output "
                        f"({split_native_app_path}, size={len(rewritten_data)} bytes)",
                        file=sys.stderr,
                    )
                    return 1
                try:
                    canonical_rewritten_data = _canonicalize_wasm_ld_output(
                        rewritten_data, description="split app native-linked"
                    )
                except ValueError as exc:
                    print(str(exc), file=sys.stderr)
                    return 1
                if canonical_rewritten_data != rewritten_data:
                    split_native_app_path.write_bytes(canonical_rewritten_data)
                    rewritten_data = canonical_rewritten_data
                restored_rewritten_data = _restore_public_output_exports(
                    rewritten_data, public_export_map
                )
                if restored_rewritten_data != rewritten_data:
                    split_native_app_path.write_bytes(restored_rewritten_data)
                    rewritten_data = restored_rewritten_data
                try:
                    native_link_error = _validate_required_native_direct_symbols(
                        rewritten_data,
                        required_native_direct_symbols,
                        description="Split-runtime native app link",
                    )
                except ValueError as exc:
                    print(
                        f"Failed to inspect split-runtime native direct symbols: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                if native_link_error is not None:
                    print(native_link_error, file=sys.stderr)
                    return 1
            else:
                # For split-runtime without external native objects, the app
                # artifact must remain unlinked while preserving the runtime ABI
                # rewrite performed earlier in the link pipeline. Copying the
                # fully linked binary here collapses the split contract, while
                # copying the raw frontend output would leave stale unprefixed
                # runtime imports that do not match the deploy runtime's export
                # ABI. The correct artifact is the rewritten, still-unlinked
                # module.
                rewritten_data = rewritten_path.read_bytes()
            optimized_app = _optimize_split_app_module(
                rewritten_data,
                reference_data=output_data,
                optimize=optimize,
                optimize_level=optimize_level,
            )
            if output_memory_min is not None:
                try:
                    updated = _rewrite_memory_min(optimized_app, output_memory_min)
                except ValueError as exc:
                    print(
                        f"Failed to rewrite split app memory min: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                if updated is not None:
                    optimized_app = updated
            if native_objects:
                native_imports = _collect_module_imports(optimized_app, "molt_native")
                if native_imports:
                    print(
                        "Split-runtime native link left unresolved molt_native "
                        "import(s): " + ", ".join(sorted(native_imports)),
                        file=sys.stderr,
                    )
                    return 1
            app_stage.write_bytes(optimized_app)

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

            # Tree-shake the runtime against its OWN canonical, app-independent
            # public export surface — never against the current app's import
            # subset.  The shared runtime is a single artifact cached once by the
            # CDN and reused by every app, so it MUST be byte-identical across
            # builds (see test_runtime_hash_identical).  Shaking by per-app
            # imports made appA (a class) and appB (fib) keep different export
            # sets, which drove wasm-opt's dead-code GC to retain different
            # functions and produced divergent runtime bytes — silently breaking
            # CDN cacheability.  Keeping the full canonical ABI lets wasm-opt
            # strip only functions unreachable from ANY public export (debug
            # tables, producers, dead internal helpers) while every app's import
            # surface still resolves.  Per-app shrinkage comes entirely from
            # app.wasm (the intrinsic manifest + wasm-ld --gc-sections), which is
            # the correct split-runtime model: one large cached runtime + a tiny
            # per-app payload.
            full_rt_size = deploy_runtime.stat().st_size
            deploy_runtime_data = deploy_runtime.read_bytes()
            try:
                canonical_required_exports = _canonical_split_runtime_required_exports(
                    deploy_runtime_data
                )
                app_imports = _collect_module_imports(
                    app_stage.read_bytes(), "molt_runtime"
                )
                missing_runtime_imports: list[str] = []
                for name in app_imports:
                    export_name = wasm_runtime_export_name(name)
                    if name in canonical_required_exports:
                        continue
                    if (
                        export_name is not None
                        and export_name in canonical_required_exports
                    ):
                        continue
                    if name in _ESSENTIAL_EXPORTS:
                        continue
                    missing_runtime_imports.append(name)
                missing_runtime_imports.sort()
                if missing_runtime_imports:
                    # The app imports a runtime symbol the canonical export
                    # surface does not advertise.  This is a hard ABI contract
                    # violation (the shared runtime cannot satisfy the app).
                    # Raising here is deliberately caught below and degrades to
                    # shipping the full (un-shaken) runtime — which is itself
                    # byte-identical across apps, so CDN cacheability survives
                    # the fallback — rather than papering over the mismatch with
                    # a per-app reshake that would reintroduce the cacheability
                    # bug.  The raise also surfaces the offending symbols so the
                    # runtime export allowlist can be fixed at the source.
                    raise ValueError(
                        "split-runtime app imports runtime symbols absent from the "
                        f"canonical shared-runtime export surface: {missing_runtime_imports}"
                    )
                print(
                    f"App imports {len(app_imports)} functions from molt_runtime; "
                    f"shaking shared runtime against {len(canonical_required_exports)} "
                    "canonical exports (app-independent, CDN-cacheable)",
                    file=sys.stderr,
                )
                shaken_runtime = _tree_shake_runtime(
                    deploy_runtime_data, canonical_required_exports
                )
                rt_stage.write_bytes(shaken_runtime)
            except Exception as exc:
                print(
                    f"Runtime tree-shake failed (falling back to full copy): {exc}",
                    file=sys.stderr,
                )
                shutil.copy2(str(deploy_runtime), str(rt_stage))

            app_size = app_stage.stat().st_size
            rt_size = rt_stage.stat().st_size
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
        # Repair out-of-bounds function references that may have been
        # introduced by post-link optimization passes (export stripping,
        # dead-code elimination, wasm-opt).  This must run AFTER all
        # element/code-rewriting passes and BEFORE the final validation.
        repaired = _safe_repair_out_of_bounds_func_refs(linked_bytes)
        if repaired is not None:
            work_linked.write_bytes(repaired)
            linked_bytes = repaired
        stripped_debug = _strip_debug_sections(linked_bytes)
        if stripped_debug is not None:
            work_linked.write_bytes(stripped_debug)
            linked_bytes = stripped_debug
        canonical_sections = _canonicalize_standard_section_order(linked_bytes)
        if canonical_sections is not None:
            work_linked.write_bytes(canonical_sections)
            linked_bytes = canonical_sections
        linked_ok = _validate_linked(work_linked)
        if not linked_ok:
            if split_runtime:
                print(
                    "Linked wasm validation failed before split-runtime publication; "
                    "failing because linked validation is the canonical table/memory/import guard.",
                    file=sys.stderr,
                )
            return 1

        publish_pairs = [(work_linked, linked)]
        if split_runtime:
            assert app_stage is not None
            assert rt_stage is not None
            assert app_wasm is not None
            assert rt_wasm is not None
            if not _validate_split_runtime_outputs(app_stage, rt_stage):
                return 1
            publish_pairs.extend([(rt_stage, rt_wasm), (app_stage, app_wasm)])
        try:
            artifact_publish.publish_validated_outputs(publish_pairs)
        except OSError as exc:
            print(f"Failed to publish wasm linker outputs: {exc}", file=sys.stderr)
            return 1

        return 0
    finally:
        for staged_output in staged_outputs:
            with contextlib.suppress(OSError):
                staged_output.unlink()
        temp_dir.cleanup()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Attempt to link Molt output/runtime into a single WASM module.",
    )
    parser.add_argument("--runtime", type=Path, default=_default_runtime_path())
    parser.add_argument("--input", type=Path, default=_default_input_path())
    parser.add_argument("--output", type=Path, default=_default_output_path())
    parser.add_argument(
        "--freestanding",
        action="store_true",
        default=False,
        help="Stub out WASI imports post-link for freestanding deployment",
    )
    parser.add_argument(
        "--optimize",
        action="store_true",
        default=False,
        help="Run wasm-opt after linking (requires Binaryen)",
    )
    parser.add_argument(
        "--optimize-level",
        default="Oz",
        help="wasm-opt optimization level (O1/O2/O3/O4/Os/Oz, default: Oz)",
    )
    parser.add_argument(
        "--split-runtime",
        action="store_true",
        default=False,
        help="Generate app.wasm + molt_runtime.wasm instead of a single linked binary",
    )
    parser.add_argument(
        "--split-output-dir",
        type=Path,
        default=None,
        help="Directory for split-runtime output files (default: same as --output parent)",
    )
    parser.add_argument(
        "--deploy-runtime",
        type=Path,
        default=None,
        dest="deploy_runtime_override",
        help="Override the deploy runtime wasm path (non-relocatable variant)",
    )
    parser.add_argument(
        "--native-object",
        type=Path,
        action="append",
        default=[],
        dest="native_objects",
        help="Validated external static package WASM object/archive input",
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
        native_objects=tuple(args.native_objects),
    )


if __name__ == "__main__":
    raise SystemExit(main())

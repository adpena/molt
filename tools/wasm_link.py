#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import contextvars
import functools
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any, cast

TOOLS_ROOT = Path(__file__).resolve().parent
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
SRC_ROOT = TOOLS_ROOT.parent / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

import harness_memory_guard  # noqa: E402
import artifact_publish  # noqa: E402
from wasm_optimize import find_wasm_opt  # noqa: E402
from wasm_metrics import wasm_metrics  # noqa: E402
from molt.cli import wasm_toolchain  # noqa: E402
from molt.cli.external_link_providers import (  # noqa: E402
    WASM_COMPILER_RT_LINK_IMPORT_CLASS,
    WASM_LIBCXX_LINK_IMPORT_CLASS,
    WASM_LIBC_LINK_IMPORT_CLASS,
    wasm_external_link_provider_symbols,
)
from molt.cli.runtime_wasm_validation import (  # noqa: E402
    _runtime_wasm_integrity_pin_paths,
)
from molt.cli.wasm_link_cache import (  # noqa: E402
    WasmLinkCacheEntry,
    _default_wasm_link_cache,
    _invalidate_wasm_link_cache_entry,
    _locked_wasm_link_cache_entry,
    _publish_wasm_link_cache_entry,
    _read_wasm_link_cache_entry,
    _wasm_link_cache_entry,
)
from molt._wasm_runtime_exports import (  # noqa: E402
    wasm_cpython_abi_data_symbol_names,
    wasm_split_runtime_export_name_for_import,
    wasm_split_runtime_import_name_for_export,
)
from molt._wasm_abi_generated import (  # noqa: E402
    WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME,
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS,
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLES,
)
from molt.wasm_artifact import (  # noqa: E402
    WasmSplitRuntimeCallableLayout,
    read_wasm_split_runtime_callable_layout,
    strip_wasm_publication_sections as _strip_wasm_publication_sections_raw,
)

from wasm_link_format import (  # noqa: E402
    CALL_INDIRECT_MANGLED_RE as CALL_INDIRECT_MANGLED_RE,
    CALL_INDIRECT_RE as CALL_INDIRECT_RE,
    CallableTableLayout as CallableTableLayout,
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
    WasmModuleFacts as WasmModuleFacts,
    _ESSENTIAL_EXPORTS as _ESSENTIAL_EXPORTS,
    _OUTPUT_EXPORT_ALIAS_PREFIX as _OUTPUT_EXPORT_ALIAS_PREFIX,
    _append_linking_function_symbols as _append_linking_function_symbols,
    _build_custom_section as _build_custom_section,
    _build_linking_payload as _build_linking_payload,
    _build_sections as _build_sections_raw,
    _collect_custom_names as _collect_custom_names,
    _collect_exports as _collect_exports,
    _collect_func_names as _collect_func_names,
    _collect_function_exports as _collect_function_exports,
    _collect_imports as _collect_imports,
    _collect_linking_function_symbols as _collect_linking_function_symbols,
    _collect_module_imports as _collect_module_imports,
    _count_func_imports as _count_func_imports,
    _ensure_table_export as _ensure_table_export,
    _find_func_import_index as _find_func_import_index,
    _flatten_rec_groups as _flatten_rec_groups,
    _has_table as _has_table,
    _is_wasm_binary as _is_wasm_binary,
    _parse_custom_section as _parse_custom_section,
    _parse_func_type_indices as _parse_func_type_indices,
    _parse_import_desc as _parse_import_desc,
    _parse_indexed_symbol as _parse_indexed_symbol,
    _parse_linking_payload as _parse_linking_payload,
    _parse_sections as _parse_sections_raw,
    _parse_symbol_flags as _parse_symbol_flags,
    _parse_type_section as _parse_type_section,
    _read_string as _read_string,
    _read_varuint as _read_varuint,
    _read_varsint as _read_varsint,
    call_indirect_import_name_for_arity as call_indirect_import_name_for_arity,
    is_call_indirect_import_name as is_call_indirect_import_name,
    wasm_runtime_export_name as wasm_runtime_export_name,
    _get_total_func_count as _get_total_func_count,
    parse_wasm_module_facts as _parse_wasm_module_facts_raw,
    _skip_init_expr as _skip_init_expr,
    _validate_elements as _validate_elements,
    _validate_linked_table_import_contract as _validate_linked_table_import_contract,
    _write_string as _write_string,
    _write_varuint as _write_varuint,
)
from wasm_link_edit import (  # noqa: E402
    _add_symtab_alias as _add_symtab_alias,
    _canonicalize_standard_section_order as _canonicalize_standard_section_order,
    _collect_output_export_symbol_map as _collect_output_export_symbol_map,
    _collect_output_wrapper_specs as _collect_output_wrapper_specs,
    _collect_preserved_output_export_names as _collect_preserved_output_export_names,
    _dominant_output_module_prefix as _dominant_output_module_prefix,
    _ensure_function_exports_by_symbol_names as _ensure_function_exports_by_symbol_names,
    _entry_module_prefix_from_main_init as _entry_module_prefix_from_main_init,
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
    _strip_internal_exports as _strip_internal_exports,
    _standard_section_order_error as _standard_section_order_error,
    _table_import_min as _table_import_min,
)
from wasm_link_optimize import (  # noqa: E402
    _dedup_data_segments as _dedup_data_segments,
    _neutralize_dead_element_entries as _neutralize_dead_element_entries,
    _post_link_optimize as _post_link_optimize,
    _reachable_function_indices as _reachable_function_indices,
    _strip_debug_sections as _strip_debug_sections,
    _strip_unused_module_function_imports as _strip_unused_module_function_imports,
    _stub_dead_functions as _stub_dead_functions,
)


_WHOLE_ARTIFACT_OPERATION_COUNTS: contextvars.ContextVar[
    dict[str, int | float] | None
] = contextvars.ContextVar("wasm_whole_artifact_operation_counts", default=None)


def _increment_whole_artifact_operation(name: str, amount: int = 1) -> None:
    counts = _WHOLE_ARTIFACT_OPERATION_COUNTS.get()
    if counts is not None:
        key = f"wasm_whole_artifact_{name}"
        counts[key] = counts.get(key, 0) + amount


def _parse_sections(data: bytes) -> list[tuple[int, bytes]]:
    _increment_whole_artifact_operation("section_walks")
    return _parse_sections_raw(data)


def _build_sections(sections: list[tuple[int, bytes]]) -> bytes:
    _increment_whole_artifact_operation("reserializations")
    return _build_sections_raw(sections)


def parse_wasm_module_facts(data: bytes) -> WasmModuleFacts:
    _increment_whole_artifact_operation("full_binary_parses")
    return _parse_wasm_module_facts_raw(data)


def strip_wasm_publication_sections(
    data: bytes,
    *,
    final_artifact: bool,
    preserve_debug: bool,
) -> bytes:
    _increment_whole_artifact_operation("section_walks")
    stripped = _strip_wasm_publication_sections_raw(
        data,
        final_artifact=final_artifact,
        preserve_debug=preserve_debug,
    )
    if stripped != data:
        _increment_whole_artifact_operation("reserializations")
    return stripped


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
    guarded_cmd = list(cmd)
    response_path: Path | None = None
    if (
        os.name == "nt"
        and Path(guarded_cmd[0]).stem.casefold() == "wasm-ld"
        and len(subprocess.list2cmdline(guarded_cmd)) >= 30_000
    ):
        handle = tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            suffix=".rsp",
            prefix="molt-wasm-ld-",
            delete=False,
        )
        try:
            response_path = Path(handle.name)
            handle.write(
                "\n".join(
                    subprocess.list2cmdline([argument])
                    for argument in guarded_cmd[1:]
                )
            )
            handle.write("\n")
        finally:
            handle.close()
        guarded_cmd = [guarded_cmd[0], f"@{response_path}"]
    try:
        result = cast(
            subprocess.CompletedProcess[str],
            harness_memory_guard.guarded_completed_process(
                guarded_cmd,
                prefix="MOLT_WASM_LINK",
                cwd=cwd,
                env=env,
                capture_output=capture_output,
                text=text,
                timeout=timeout,
            ),
        )
    finally:
        if response_path is not None:
            with contextlib.suppress(OSError):
                response_path.unlink()
    if (
        timeout is not None
        and result.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
        and "memory_guard: timeout after" in (result.stderr or "")
    ):
        raise subprocess.TimeoutExpired(
            guarded_cmd,
            timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
    return result


def _wasm_ld_signature_mismatch_warning(stderr: str | None) -> str | None:
    if not stderr or "function signature mismatch:" not in stderr:
        return None
    return stderr.strip()


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


_RUNTIME_INTEGRITY_PAIR_ATTEMPTS = 8
_RUNTIME_INTEGRITY_PAIR_RETRY_DELAY_SEC = 0.05


def _read_runtime_integrity_pins(path: Path) -> dict[Path, str]:
    """Read every keyed integrity pin published next to ``path``.

    Pins are keyed by the runtime build's fingerprint meta digest
    (``<artifact>.<meta_digest>.sha256``) â€” one slot per resolved
    profile/feature identity â€” so like builds verify against like pins and
    different-profile builds never contend for a single pinned hash.
    """
    pins: dict[Path, str] = {}
    for sidecar in _runtime_wasm_integrity_pin_paths(path):
        try:
            raw = sidecar.read_text(encoding="utf-8").strip()
        except FileNotFoundError:
            continue
        match = re.search(r"\b([0-9a-fA-F]{64})\b", raw)
        if match is None:
            raise SystemExit(f"Runtime integrity sidecar is malformed: {sidecar}")
        pins[sidecar] = match.group(1).lower()
    return pins


def _verify_runtime_integrity(path: Path) -> None:
    """Verify SHA-256 integrity of the runtime binary against keyed pins.

    Raises ``SystemExit`` when no keyed integrity pin exists or the artifact
    hash matches none of the published pins.
    """
    # Reject path-traversal components before reading the file.
    for part in path.parts:
        if part == "..":
            raise SystemExit(f"Runtime path contains '..' traversal component: {path}")

    pins: dict[Path, str] = {}
    digest = ""
    for attempt in range(_RUNTIME_INTEGRITY_PAIR_ATTEMPTS):
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        pins = _read_runtime_integrity_pins(path)
        if digest in pins.values():
            return
        if attempt + 1 < _RUNTIME_INTEGRITY_PAIR_ATTEMPTS:
            time.sleep(_RUNTIME_INTEGRITY_PAIR_RETRY_DELAY_SEC)

    if pins:
        pin_lines = "".join(
            f"  pinned SHA-256 ({sidecar.name}): {expected}\n"
            for sidecar, expected in sorted(pins.items())
        )
        raise SystemExit(
            f"Runtime integrity check failed for {path}\n"
            f"  source: keyed integrity sidecars in {path.parent}\n"
            f"{pin_lines}"
            f"  actual SHA-256: {digest}\n"
        )
    raise SystemExit(
        "Runtime integrity check failed for "
        f"{path}\n  no keyed integrity sidecar "
        f"({path.name}.<fingerprint-meta-digest>.sha256) was found in "
        f"{path.parent}\n"
        f"  actual SHA-256: {digest}\n"
        "  publish the matching keyed .sha256 sidecar (the molt runtime build "
        "writes it); unkeyed single-slot sidecars and hardcoded runtime hash "
        "pins are not supported."
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
    """Return the attested `wasm-ld` selected by WASI toolchain authority."""
    try:
        identity = wasm_toolchain.resolve_wasm_linker()
    except wasm_toolchain.WasmLinkerContractError as exc:
        print(f"Wasm linker contract failed: {exc}", file=sys.stderr)
        return None
    if identity is None:
        return None
    print(f"Wasm linker identity: {identity.diagnostic}", file=sys.stderr)
    return str(identity.path)


def _deduplicated_export_flags(*groups: Iterable[str]) -> list[str]:
    flags: list[str] = []
    seen: set[str] = set()
    for group in groups:
        for flag in group:
            if flag in seen:
                continue
            seen.add(flag)
            flags.append(flag)
    return flags


def _preflight_relocatable_runtime(
    wasm_ld: str, runtime: Path, temp_dir: Any
) -> str | None:
    if not runtime.name.endswith("_reloc.wasm"):
        return None
    output = Path(temp_dir.name) / "runtime_reloc_preflight.wasm"
    result = _run_external_tool(
        [wasm_ld, "-r", "-o", str(output), str(runtime)],
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        return None
    detail = (result.stderr or result.stdout or "").strip()
    crash = (
        "PLEASE submit a bug report" in detail
        or result.returncode < 0
        or result.returncode > 255
    )
    if crash:
        return (
            "relocatable runtime linker metadata is inconsistent: wasm-ld crashed "
            f"while relinking {runtime}. This commonly means publication stripping "
            "removed sections without rewriting the linking/reloc custom-section "
            "indices; rebuild and republish the reloc runtime from one build identity. "
            f"linker={wasm_ld} returncode={result.returncode}"
        )
    return f"relocatable runtime preflight failed for {runtime}: {detail}"


def _dump_symbols(
    path: Path, wasm_tools: str | None
) -> list[tuple[int, int, str, str]]:
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


def _wasm_link_cache_root() -> Path:
    return _default_wasm_link_cache()


_TREE_SHAKE_RUNTIME_CACHE_SCHEMA = "runtime-tree-shake-v2"
_SPLIT_APP_OPTIMIZE_CACHE_SCHEMA = "split-app-optimize-v2"
_WASM_LINK_CACHE_METRIC_SUFFIXES = (
    "requests",
    "hits",
    "misses",
    "corruptions",
    "bytes_read",
    "bytes_written",
    "lock_wait_ms",
    "lookup_ms",
    "publish_ms",
    "wall_ms",
    "optimizer_wall_ms",
    "optimizer_peak_rss_kb",
    "optimizer_peak_total_rss_kb",
    "publish_errors",
    "timeouts",
    "failures",
    "identity_errors",
)


def _empty_wasm_link_cache_metrics() -> dict[str, int | float]:
    return {
        f"{prefix}_{suffix}": 0
        for prefix in ("runtime_tree_shake_cache", "split_app_optimize_cache")
        for suffix in _WASM_LINK_CACHE_METRIC_SUFFIXES
    }


def _cache_metric_add(
    metrics: dict[str, int | float] | None,
    name: str,
    value: int | float,
) -> None:
    if metrics is None:
        return
    metrics[name] = round(float(metrics.get(name, 0)) + float(value), 6)


def _cache_metric_max(
    metrics: dict[str, int | float] | None,
    name: str,
    value: int | float | None,
) -> None:
    if metrics is None or value is None:
        return
    metrics[name] = max(float(metrics.get(name, 0)), float(value))


def _record_guarded_process_cache_metrics(
    metrics: dict[str, int | float] | None,
    prefix: str,
    process: subprocess.CompletedProcess[str],
) -> None:
    elapsed_s = getattr(process, "elapsed_s", None)
    if isinstance(elapsed_s, (int, float)):
        _cache_metric_add(metrics, f"{prefix}_optimizer_wall_ms", elapsed_s * 1000.0)
    peak = getattr(process, "peak", None)
    peak_total = getattr(process, "peak_total", None)
    _cache_metric_max(
        metrics, f"{prefix}_optimizer_peak_rss_kb", getattr(peak, "rss_kb", None)
    )
    _cache_metric_max(
        metrics,
        f"{prefix}_optimizer_peak_total_rss_kb",
        getattr(peak_total, "rss_kb", None),
    )


def _record_wasm_opt_attestation_cache_metrics(
    metrics: dict[str, int | float] | None,
    prefix: str,
    attestation: Mapping[str, object],
) -> None:
    wall_ms = attestation.get("wasm_opt_wall_ms")
    if isinstance(wall_ms, (int, float)):
        _cache_metric_add(metrics, f"{prefix}_optimizer_wall_ms", wall_ms)
    for suffix in ("peak_rss_kb", "peak_total_rss_kb"):
        value = attestation.get(f"wasm_opt_{suffix}")
        if isinstance(value, (int, float)):
            _cache_metric_max(metrics, f"{prefix}_optimizer_{suffix}", value)


def _publish_wasm_link_cache_result(
    entry: WasmLinkCacheEntry,
    data: bytes,
    *,
    metrics: dict[str, int | float] | None,
    metric_prefix: str,
    label: str,
    payload: Mapping[str, object] | None = None,
) -> None:
    """Atomically publish one linker-cache result and record one metric shape."""

    publish_started = time.perf_counter()
    try:
        _publish_wasm_link_cache_entry(entry, data, payload=payload)
    except OSError as exc:
        _cache_metric_add(metrics, f"{metric_prefix}_publish_errors", 1)
        print(f"{label} cache publication failed: {exc}", file=sys.stderr)
    else:
        _cache_metric_add(metrics, f"{metric_prefix}_bytes_written", len(data))
    _cache_metric_add(
        metrics,
        f"{metric_prefix}_publish_ms",
        (time.perf_counter() - publish_started) * 1000.0,
    )


def _split_app_optimize_cache_key(
    *,
    app_data: bytes,
    reference_data: bytes | None,
    optimize: bool,
    optimize_level: str,
    contract_keep_set: set[str],
) -> str | None:
    hasher = hashlib.sha256()
    hasher.update(_SPLIT_APP_OPTIMIZE_CACHE_SCHEMA.encode("ascii"))
    hasher.update(b"\0app\0")
    hasher.update(app_data)
    hasher.update(b"\0reference\0")
    if reference_data is not None:
        hasher.update(reference_data)
    hasher.update(b"\0optimize\0")
    hasher.update(str(int(optimize)).encode("ascii"))
    hasher.update(b"\0level\0")
    hasher.update(optimize_level.encode("utf-8"))
    hasher.update(b"\0exports\0")
    for name in sorted(contract_keep_set):
        hasher.update(name.encode("utf-8") + b"\0")
    if optimize:
        wasm_opt = find_wasm_opt()
        identity = _wasm_opt_executable_identity(wasm_opt) if wasm_opt else None
        if identity is None:
            return None
        _resolved_path, executable_sha256, _version = identity
        hasher.update(b"\0wasm-opt-sha256\0")
        hasher.update(executable_sha256.encode("ascii"))
    hasher.update(b"\0tool\0")
    hasher.update(_wasm_link_transform_authority_digest().encode("ascii"))
    return hasher.hexdigest()


@functools.lru_cache(maxsize=1)
def _wasm_link_transform_authority_digest() -> str:
    return _transform_authority_digest(
        tuple(
            Path(__file__).with_name(name)
            for name in ("wasm_link.py", "wasm_link_optimize.py", "wasm_optimize.py")
        )
    )


def _transform_authority_digest(paths: Sequence[Path]) -> str:
    hasher = hashlib.sha256()
    for path in paths:
        hasher.update(path.name.encode("utf-8"))
        hasher.update(b"\0")
        hasher.update(path.read_bytes())
        hasher.update(b"\0")
    return hasher.hexdigest()


def _snapshot_link_input(
    source: Path,
    snapshot_root: Path,
    *,
    label: str,
    attempts: int = 100,
    retry_delay_seconds: float = 0.05,
    accept: Callable[[bytes], bool] | None = None,
    accept_path: Callable[[Path], bool] | None = None,
) -> Path:
    """Capture one complete immutable linker input from a mutable build path."""
    last_observation = "unreadable"
    snapshot_root.mkdir(parents=True, exist_ok=True)
    for _attempt in range(attempts):
        try:
            before = source.stat()
            first = source.read_bytes()
            middle = source.stat()
            second = source.read_bytes()
            after = source.stat()
        except OSError as exc:
            last_observation = str(exc)
            time.sleep(retry_delay_seconds)
            continue
        stable_identity = (
            before.st_size == middle.st_size == after.st_size == len(first)
            and before.st_mtime_ns == middle.st_mtime_ns == after.st_mtime_ns
        )
        if stable_identity and first == second:
            if accept is not None and not accept(first):
                last_observation = "stable bytes failed linker input contract"
                time.sleep(retry_delay_seconds)
                continue
            digest = hashlib.sha256(first).hexdigest()
            snapshot_dir = snapshot_root / label
            snapshot_dir.mkdir(parents=True, exist_ok=True)
            snapshot = snapshot_dir / source.name
            snapshot.write_bytes(first)
            if hashlib.sha256(snapshot.read_bytes()).hexdigest() != digest:
                raise OSError(f"Failed to attest linker input snapshot: {snapshot}")
            if accept_path is not None and not accept_path(snapshot):
                last_observation = "stable snapshot failed linker metadata preflight"
                time.sleep(retry_delay_seconds)
                continue
            return snapshot
        last_observation = (
            f"size={before.st_size}/{middle.st_size}/{after.st_size} "
            f"mtime={before.st_mtime_ns}/{middle.st_mtime_ns}/{after.st_mtime_ns}"
        )
        time.sleep(retry_delay_seconds)
    raise OSError(
        f"Linker input remained mutable while snapshotting {label}: "
        f"{source} ({last_observation})"
    )


@functools.lru_cache(maxsize=4)
def _wasm_opt_version(executable: str) -> str:
    try:
        result = _run_external_tool(
            [executable, "--version"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except Exception:
        return "unknown"
    output = (result.stdout or result.stderr or "").strip()
    return output or "unknown"


def _wasm_opt_stat_identity(stat: os.stat_result) -> tuple[int, int, int, int]:
    return (stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns, stat.st_ino)


@functools.lru_cache(maxsize=16)
def _wasm_opt_executable_sha256_cached(
    resolved_path: str,
    stat_identity: tuple[int, int, int, int],
) -> str | None:
    path = Path(resolved_path)
    try:
        before = path.stat()
        if _wasm_opt_stat_identity(before) != stat_identity:
            return None
        hasher = hashlib.sha256()
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                hasher.update(chunk)
        after = path.stat()
    except OSError:
        return None
    if _wasm_opt_stat_identity(after) != stat_identity:
        return None
    return hasher.hexdigest()


def _wasm_opt_executable_identity(
    executable: str,
) -> tuple[str, str, str] | None:
    """Return immutable Binaryen custody identity or disable cache admission."""

    try:
        path = Path(executable).expanduser().resolve(strict=True)
        stat_identity = _wasm_opt_stat_identity(path.stat())
    except OSError:
        return None
    digest = _wasm_opt_executable_sha256_cached(os.fspath(path), stat_identity)
    if digest is None:
        return None
    return os.fspath(path), digest, _wasm_opt_version(os.fspath(path))


def _tree_shake_runtime_cache_key(
    *,
    runtime_data: bytes,
    normalized_required_exports: set[str],
    wasm_opt_sha256: str,
    feature_flags: list[str],
) -> str:
    hasher = hashlib.sha256()
    hasher.update(_TREE_SHAKE_RUNTIME_CACHE_SCHEMA.encode("ascii"))
    hasher.update(b"\0")
    hasher.update(runtime_data)
    hasher.update(b"\0exports\0")
    for name in sorted(normalized_required_exports):
        hasher.update(name.encode("utf-8"))
        hasher.update(b"\0")
    hasher.update(b"\0wasm-opt-sha256\0")
    hasher.update(wasm_opt_sha256.encode("ascii"))
    hasher.update(b"\0flags\0")
    for flag in feature_flags:
        hasher.update(flag.encode("utf-8"))
        hasher.update(b"\0")
    hasher.update(b"\0tool\0")
    hasher.update(_wasm_link_transform_authority_digest().encode("ascii"))
    return hasher.hexdigest()


def _canonical_split_runtime_required_exports(runtime_data: bytes) -> set[str]:
    """Return runtime exports that remain app-visible split-runtime contracts."""
    return {
        name
        for name in _collect_function_exports(runtime_data)
        if name not in _ESSENTIAL_EXPORTS and name not in {"molt_exception_pending"}
    }


def _read_link_allowlist_symbols(path: Path) -> list[str]:
    return [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]


_COMPILER_RT_LINK_IMPORT_CLASS = "wasm_compiler_rt_link_import"
_CPYTHON_ABI_LINK_IMPORT_CLASS = "molt_cpython_abi_link_import"
_SYMBOL_KIND_DATA = 1


class _SplitRuntimeExportContractEntry:
    __slots__ = ("artifact", "kind", "canonical_name", "accepted_names")

    def __init__(
        self,
        *,
        artifact: str,
        kind: int,
        canonical_name: str,
        accepted_names: tuple[str, ...],
    ) -> None:
        self.artifact = artifact
        self.kind = kind
        self.canonical_name = canonical_name
        self.accepted_names = accepted_names


_SPLIT_RUNTIME_EXPORT_CONTRACT = (
    _SplitRuntimeExportContractEntry(
        artifact="app",
        kind=0,
        canonical_name="molt_main",
        accepted_names=("molt_main",),
    ),
    _SplitRuntimeExportContractEntry(
        artifact="app",
        kind=2,
        canonical_name="molt_memory",
        accepted_names=("molt_memory", "memory"),
    ),
    _SplitRuntimeExportContractEntry(
        artifact="app",
        kind=1,
        canonical_name="molt_table",
        accepted_names=("molt_table", "__indirect_function_table"),
    ),
)


def _split_runtime_export_contract(
    artifact: str,
) -> tuple[_SplitRuntimeExportContractEntry, ...]:
    return tuple(
        entry for entry in _SPLIT_RUNTIME_EXPORT_CONTRACT if entry.artifact == artifact
    )


def _split_runtime_contract_export_names(artifact: str) -> set[str]:
    return {
        name
        for entry in _split_runtime_export_contract(artifact)
        for name in entry.accepted_names
    }


def _split_artifact_contract_keep_set(
    artifact: str,
    *,
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
) -> set[str]:
    """Return the external export contract for a split publication artifact.

    Callable table-ref aliases are intentionally absent: both split modules
    import the shared ``env.__indirect_function_table``, and active element
    segments install those functions by slot. Keeping the aliases public would
    make every target a Binaryen DCE root without adding cross-module linkage.
    """
    public_exports = set(public_export_map or ())
    return (
        _split_runtime_contract_export_names(artifact)
        | public_exports
        | set(required_native_direct_symbols)
    )


def _split_artifact_contract_function_symbols(
    artifact: str,
    *,
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
) -> dict[str, str]:
    export_map = public_export_map or {}
    keep = _split_artifact_contract_keep_set(
        artifact,
        public_export_map=export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    function_symbols = {
        public_name: symbol_name
        for public_name, symbol_name in export_map.items()
        if public_name in keep
    }
    function_symbols.update({name: name for name in required_native_direct_symbols})
    for entry in _split_runtime_export_contract(artifact):
        if entry.kind == 0:
            function_symbols.setdefault(entry.canonical_name, entry.canonical_name)
    return function_symbols


def _external_native_host_link_imports() -> tuple[str, ...]:
    generated = {
        symbol
        for symbol in WASM_EXTERNAL_NATIVE_LINK_IMPORTS
        if WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(symbol)
        not in {_COMPILER_RT_LINK_IMPORT_CLASS, _CPYTHON_ABI_LINK_IMPORT_CLASS}
    }
    provider_symbols = wasm_external_link_provider_symbols(
        primitive_classes=frozenset(
            {WASM_LIBC_LINK_IMPORT_CLASS, WASM_LIBCXX_LINK_IMPORT_CLASS}
        )
    )
    return tuple(sorted(generated | provider_symbols))


def _iter_linking_data_symbols(
    data: bytes, *, undefined: bool
) -> Iterable[tuple[str, int | None, int | None]]:
    """Yield linking-section data symbols from a wasm object.

    For defined data symbols, yields ``(name, data_offset, size)``. For
    undefined symbols, ``data_offset`` and ``size`` are ``None``.
    """
    for section_id, payload in _parse_sections(data):
        if section_id != 0:
            continue
        name, custom_payload = _parse_custom_section(payload)
        if name != "linking":
            continue
        _version, subsections = _parse_linking_payload(custom_payload)
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
                    _index, _name, offset = _parse_indexed_symbol(
                        sub_payload, offset, flags
                    )
                    continue
                if kind in (2, 4, 5):
                    _index, _name, offset = _parse_indexed_symbol(
                        sub_payload, offset, flags
                    )
                    continue
                if kind == 3:
                    _index, offset = _read_varuint(sub_payload, offset)
                    continue
                if kind != _SYMBOL_KIND_DATA:
                    raise ValueError(f"Unknown linking symbol kind: {kind}")
                symbol_name, offset = _read_string(sub_payload, offset)
                is_undefined = bool(flags & FLAG_UNDEFINED)
                if is_undefined:
                    if undefined:
                        yield symbol_name, None, None
                    continue
                segment_index, offset = _read_varuint(sub_payload, offset)
                data_offset, offset = _read_varuint(sub_payload, offset)
                size, offset = _read_varuint(sub_payload, offset)
                del segment_index
                if not undefined:
                    yield symbol_name, data_offset, size


def _defined_runtime_data_symbol_offsets(
    runtime_data: bytes,
) -> dict[str, tuple[int, int]]:
    return {
        name: (offset, size)
        for name, offset, size in _iter_linking_data_symbols(
            runtime_data, undefined=False
        )
        if offset is not None and size is not None
    }


def _count_imported_globals(runtime_data: bytes) -> int:
    for section_id, payload in _parse_sections(runtime_data):
        if section_id != 2:  # import section
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        imported_globals = 0
        for _ in range(count):
            _module, offset = _read_string(payload, offset)
            _field, offset = _read_string(payload, offset)
            kind = payload[offset]
            offset += 1
            if kind == 0x00:  # function: typeidx
                _typeidx, offset = _read_varuint(payload, offset)
            elif kind == 0x01:  # table: reftype + limits
                offset += 1  # reftype
                limit_flags = payload[offset]
                offset += 1
                _min, offset = _read_varuint(payload, offset)
                if limit_flags & 0x01:
                    _max, offset = _read_varuint(payload, offset)
            elif kind == 0x02:  # memory: limits
                limit_flags = payload[offset]
                offset += 1
                _min, offset = _read_varuint(payload, offset)
                if limit_flags & 0x01:
                    _max, offset = _read_varuint(payload, offset)
            elif kind == 0x03:  # global: valtype + mut
                offset += 2
                imported_globals += 1
            else:
                raise ValueError(f"Unsupported import kind: 0x{kind:02x}")
        return imported_globals
    return 0


def _defined_global_i32_inits(runtime_data: bytes) -> list[int | None]:
    """Return the i32.const init value of each *defined* global, indexed by
    defined-global order. Non-i32-const globals yield ``None``."""
    inits: list[int | None] = []
    for section_id, payload in _parse_sections(runtime_data):
        if section_id != 6:  # global section
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            _valtype = payload[offset]
            offset += 1
            _mut = payload[offset]
            offset += 1
            if offset < len(payload) and payload[offset] == 0x41:  # i32.const
                value, offset = _read_const_i32_init_expr(payload, offset)
                inits.append(value)
            else:
                offset = _skip_init_expr(payload, offset)
                inits.append(None)
        break
    return inits


def _runtime_exported_data_symbol_addresses(runtime_data: bytes) -> dict[str, int]:
    """Map data-symbol name -> linear-memory address from the runtime's
    exported address globals.

    wasm-ld exports a *defined data symbol* (requested via
    ``--export[-if-defined]``) as an immutable ``i32`` global whose init value
    is the symbol's absolute linear-memory address. Reading those exports from
    the deploy runtime is the authoritative source for the runtime's canonical
    singleton/type/exception addresses â€” the split app links against imported
    memory and shares those addresses at run time.
    """
    imported_globals = _count_imported_globals(runtime_data)
    defined_inits = _defined_global_i32_inits(runtime_data)
    addresses: dict[str, int] = {}
    for section_id, payload in _parse_sections(runtime_data):
        if section_id != 7:  # export section
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            kind = payload[offset]
            offset += 1
            index, offset = _read_varuint(payload, offset)
            if kind != 0x03:  # not a global export
                continue
            defined_index = index - imported_globals
            if 0 <= defined_index < len(defined_inits):
                value = defined_inits[defined_index]
                if value is not None:
                    addresses[name] = value
        break
    return addresses


def _undefined_cpython_abi_data_symbols(
    native_objects: Sequence[Path],
) -> tuple[str, ...]:
    symbols: set[str] = set()
    for native_object in native_objects:
        try:
            data = native_object.read_bytes()
        except OSError:
            continue
        if not _is_wasm_binary(data):
            continue
        try:
            undefined_symbols = _iter_linking_data_symbols(data, undefined=True)
            for split_name, _offset, _size in undefined_symbols:
                canonical = wasm_split_runtime_import_name_for_export(split_name)
                if canonical is None:
                    canonical = split_name
                if (
                    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(canonical)
                    == _CPYTHON_ABI_LINK_IMPORT_CLASS
                    and WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS.get(canonical)
                    == "data"
                ):
                    symbols.add(split_name)
        except ValueError:
            continue
    return tuple(sorted(symbols))


def _data_symbol_entry(
    *,
    name: str,
    segment_index: int,
    size: int,
) -> bytes:
    entry = bytearray()
    entry.append(_SYMBOL_KIND_DATA)
    entry.extend(_write_varuint(FLAG_EXPLICIT_NAME))
    entry.extend(_write_string(name))
    entry.extend(_write_varuint(segment_index))
    entry.extend(_write_varuint(0))
    entry.extend(_write_varuint(size))
    return bytes(entry)


def _build_runtime_data_alias_object(
    symbol_offsets: Sequence[tuple[str, int, int]],
) -> bytes:
    """Build a tiny wasm object that defines runtime-owned data symbols.

    Each definition uses an empty active data segment at the runtime-owned
    address. The segment gives wasm-ld a concrete symbol value for native
    relocations while copying zero bytes into imported memory, so the split app
    does not duplicate or reinitialize runtime storage.
    """
    sections: list[tuple[int, bytes]] = []

    import_payload = bytearray()
    import_payload.extend(_write_varuint(1))
    import_payload.extend(_write_string("env"))
    import_payload.extend(_write_string("memory"))
    import_payload.append(2)  # memory import
    import_payload.append(0)  # min-only limits
    import_payload.extend(_write_varuint(1))
    sections.append((2, bytes(import_payload)))

    data_payload = bytearray()
    data_payload.extend(_write_varuint(len(symbol_offsets)))
    symbol_entries: list[bytes] = []
    for segment_index, (name, address, size) in enumerate(symbol_offsets):
        data_payload.append(0)  # active segment, memory 0 implicit
        data_payload.append(0x41)  # i32.const
        data_payload.extend(_write_varuint(address))
        data_payload.append(0x0B)  # end
        data_payload.extend(_write_varuint(0))  # empty payload
        symbol_entries.append(
            _data_symbol_entry(
                name=name,
                segment_index=segment_index,
                size=size,
            )
        )
    sections.append((11, bytes(data_payload)))

    symbol_payload = _write_varuint(len(symbol_entries)) + b"".join(symbol_entries)
    sections.append(
        (
            0,
            _build_custom_section(
                "linking",
                _build_linking_payload(2, [(SYMTAB_SUBSECTION_ID, symbol_payload)]),
            ),
        )
    )
    return _build_sections(sections)


def _resolve_deploy_runtime(
    runtime: Path, deploy_runtime_override: Path | None
) -> Path:
    """Resolve the deploy-ready (non-relocatable) runtime shared at run time.

    Mirrors the split-runtime publication path: honor
    ``MOLT_WASM_DEPLOY_RUNTIME`` / an explicit override, otherwise fall back to
    ``runtime`` and prefer its non-relocatable sibling (``*_reloc.wasm`` ->
    ``*.wasm``). The returned artifact is the one whose linear-memory data
    addresses the split app must agree with.
    """
    if deploy_runtime_override is not None:
        if not deploy_runtime_override.exists():
            raise FileNotFoundError(
                f"explicit split deploy runtime not found: {deploy_runtime_override}"
            )
        return deploy_runtime_override
    env_deploy_runtime = os.environ.get("MOLT_WASM_DEPLOY_RUNTIME", "").strip()
    if env_deploy_runtime:
        ambient = Path(env_deploy_runtime).expanduser()
        if ambient.exists():
            return ambient
    if runtime.name.endswith("_reloc.wasm"):
        non_reloc = runtime.with_name(runtime.name.replace("_reloc.wasm", ".wasm"))
        if non_reloc.exists():
            return non_reloc
    if runtime.exists():
        return runtime
    raise FileNotFoundError(f"split deploy runtime not found: {runtime}")


def _split_runtime_data_alias_object(
    *,
    native_objects: Sequence[Path],
    deploy_runtime: Path,
    temp_dir: tempfile.TemporaryDirectory,
    reloc_runtime: Path | None = None,
) -> Path | None:
    """Build a wasm object that defines each CPython-ABI data symbol a native
    extension (numpy/scipy) leaves undefined, pointing it at the *deploy
    runtime's* canonical linear-memory address.

    The absolute addresses come from the deploy (shared, non-relocatable)
    runtime's exported address globals â€” wasm-ld exports a defined data symbol
    as an immutable i32 global whose init value is that symbol's address (see
    ``_runtime_exported_data_symbol_addresses``). The split app links against
    imported memory and shares those addresses with the deployed runtime at run
    time, so numpy's ``Py_None``/``Py_False``/``PyExc_*``/``Py*_Type`` resolve to
    the runtime's single canonical copy instead of an app-local duplicate.

    The relocatable runtime's linking symtab is used only for the (cosmetic)
    symbol *size* metadata; its segment-relative offsets are NOT addresses.
    """
    required_symbols = _undefined_cpython_abi_data_symbols(native_objects)
    if not required_symbols:
        return None
    deploy_addresses = _runtime_exported_data_symbol_addresses(
        deploy_runtime.read_bytes()
    )
    reloc_sizes: dict[str, tuple[int, int]] = {}
    if reloc_runtime is not None and reloc_runtime.exists():
        with contextlib.suppress(ValueError):
            reloc_sizes = _defined_runtime_data_symbol_offsets(
                reloc_runtime.read_bytes()
            )
    alias_symbols: list[tuple[str, int, int]] = []
    missing: list[str] = []
    for split_name in required_symbols:
        canonical = wasm_split_runtime_import_name_for_export(split_name)
        if canonical is None:
            canonical = split_name
        address = deploy_addresses.get(canonical)
        if address is None:
            missing.append(f"{split_name} (canonical {canonical})")
            continue
        size = reloc_sizes.get(canonical, (0, 8))[1]
        alias_symbols.append((split_name, address, size))
    if missing:
        raise ValueError(
            "split-runtime native data symbol bridge: deploy runtime "
            f"{deploy_runtime.name} exports no address global for CPython-ABI "
            "data symbol(s): "
            + ", ".join(missing)
            + " â€” the shared runtime must publish these via "
            "--export-if-defined (wasm_cpython_abi_data_symbol_names / "
            "wasm_runtime_shared_export_link_args)."
        )
    alias_path = Path(temp_dir.name) / "split_runtime_data_aliases.wasm"
    alias_path.write_bytes(_build_runtime_data_alias_object(alias_symbols))
    return alias_path


# wasm-ld names the GOT data global it synthesises for a PIC object's data
# relocation (R_WASM_GLOBAL_INDEX_LEB against a data symbol) as
# ``GOT.data.internal.<sym>`` in the linked module's name section.
_GOT_DATA_INTERNAL_PREFIX = "GOT.data.internal."


def _write_sleb128(value: int) -> bytes:
    """Encode a signed integer as LEB128 (the encoding of an ``i32.const``
    immediate)."""
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if (value == 0 and not (byte & 0x40)) or (value == -1 and (byte & 0x40)):
            out.append(byte)
            return bytes(out)
        out.append(byte | 0x80)


def _wasm_name_section_global_names(data: bytes) -> dict[int, str]:
    """Map global index -> name from the custom ``name`` section (global-name
    subsection, id 7). Empty if the module carries no name section."""
    names: dict[int, str] = {}
    for section_id, payload in _parse_sections(data):
        if section_id != 0:
            continue
        section_name, custom_payload = _parse_custom_section(payload)
        if section_name != "name":
            continue
        offset = 0
        while offset < len(custom_payload):
            sub_id = custom_payload[offset]
            offset += 1
            size, offset = _read_varuint(custom_payload, offset)
            sub_payload = custom_payload[offset : offset + size]
            offset += size
            if sub_id != 7:  # global names subsection
                continue
            sub_offset = 0
            count, sub_offset = _read_varuint(sub_payload, sub_offset)
            for _ in range(count):
                index, sub_offset = _read_varuint(sub_payload, sub_offset)
                name, sub_offset = _read_string(sub_payload, sub_offset)
                names[index] = name
        break
    return names


def _rewrite_global_section_i32_inits(
    data: bytes, new_inits: Mapping[int, int]
) -> bytes:
    """Return ``data`` with the i32.const init of each defined global in
    ``new_inits`` (keyed by *defined-global* index) replaced. Every targeted
    global must currently hold an ``i32.const`` init expression."""
    if not new_inits:
        return data
    new_sections: list[tuple[int, bytes]] = []
    rewrote = False
    for section_id, payload in _parse_sections(data):
        if section_id != 6:  # global section
            new_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        rebuilt = bytearray()
        rebuilt.extend(_write_varuint(count))
        for defined_index in range(count):
            valtype = payload[offset]
            mut = payload[offset + 1]
            body_start = offset + 2
            if payload[body_start] == 0x41:  # i32.const
                _value, after = _read_const_i32_init_expr(payload, body_start)
            else:
                after = _skip_init_expr(payload, body_start)
            if defined_index in new_inits:
                if payload[body_start] != 0x41:
                    raise ValueError(
                        "cannot retarget non-i32.const GOT data global at "
                        f"defined index {defined_index}"
                    )
                rebuilt.append(valtype)
                rebuilt.append(mut)
                rebuilt.append(0x41)  # i32.const
                rebuilt.extend(_write_sleb128(new_inits[defined_index]))
                rebuilt.append(0x0B)  # end
                rewrote = True
            else:
                rebuilt.extend(payload[offset:after])
            offset = after
        new_sections.append((section_id, bytes(rebuilt)))
    if not rewrote:
        return data
    return _build_sections(new_sections)


def _rewrite_split_app_got_data_globals(
    data: bytes,
    *,
    runtime_addresses: Mapping[str, int],
    description: str,
) -> tuple[bytes, int]:
    """Retarget the split app's CPython-ABI GOT data globals to the shared
    runtime's canonical linear-memory addresses.

    A PIC extension (numpy ``_multiarray_umath``) references the runtime
    singletons/type/exception objects (``Py_None``/``Py_False``/``PyExc_*``/
    ``Py*_Type``/...) through GOT data relocations. wasm-ld resolves each against
    the active-data-segment alias and emits a *defined*
    ``GOT.data.internal.molt_<sym>`` global â€” but the alias's zero-size segments
    are relocated into the split app's own region, so every such global
    initialises to the same app-local placeholder address (the extension then
    reads an all-zero object, ``ob_type == NULL``). Those globals are the exact
    word the extension loads at run time, so we retarget each to the runtime's
    canonical address (published as an address-bearing wasm global on the shared
    runtime; read here from the deploy runtime â€” the runtime is byte-identical
    across apps, so the address is stable and CDN-safe).

    Fails loud (M34) if a ``GOT.data.internal.molt_<sym>`` global names a
    CPython-ABI data symbol for which the runtime publishes no address â€” a silent
    skip would leave the extension reading the app-local placeholder.

    Returns ``(new_bytes, retargeted_count)``.
    """
    global_names = _wasm_name_section_global_names(data)
    if not global_names:
        return data, 0
    imported_globals = _count_imported_globals(data)
    cpython_abi_data_symbols = set(wasm_cpython_abi_data_symbol_names())
    new_inits: dict[int, int] = {}
    missing: list[str] = []
    for global_index, name in global_names.items():
        if not name.startswith(_GOT_DATA_INTERNAL_PREFIX):
            continue
        split_name = name[len(_GOT_DATA_INTERNAL_PREFIX) :]
        canonical = wasm_split_runtime_import_name_for_export(split_name)
        if canonical is None:
            canonical = split_name
        if canonical not in cpython_abi_data_symbols:
            # Not a runtime-owned CPython-ABI singleton/type/exception (e.g. a
            # GOT global for the extension's own or libc data) â€” leave as linked.
            continue
        address = runtime_addresses.get(canonical)
        if address is None:
            missing.append(f"{split_name} (canonical {canonical})")
            continue
        defined_index = global_index - imported_globals
        if defined_index < 0:
            missing.append(f"{split_name} (imported global, unexpected)")
            continue
        new_inits[defined_index] = address
    if missing:
        raise ValueError(
            f"{description}: split-runtime GOT data bridge â€” deploy runtime "
            "publishes no canonical address for CPython-ABI data symbol(s): "
            + ", ".join(sorted(missing))
            + " â€” the shared runtime must export these via --export-if-defined "
            "(wasm_cpython_abi_data_symbol_names / "
            "wasm_runtime_shared_export_link_args)."
        )
    if not new_inits:
        return data, 0
    return _rewrite_global_section_i32_inits(data, new_inits), len(new_inits)


def _compiler_rt_link_imports() -> frozenset[str]:
    generated = {
        symbol
        for symbol, primitive_class in WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.items()
        if primitive_class == _COMPILER_RT_LINK_IMPORT_CLASS
    }
    return frozenset(
        generated
        | wasm_external_link_provider_symbols(
            primitive_classes=frozenset({WASM_COMPILER_RT_LINK_IMPORT_CLASS})
        )
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


def _sealed_native_init_symbols(native_objects: Sequence[Path]) -> tuple[str, ...]:
    symbols: set[str] = set()
    for native_object in native_objects:
        manifest_path = native_object.with_name(
            native_object.name + ".extension_manifest.json"
        )
        if not manifest_path.exists():
            continue
        try:
            payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(
                f"sealed native extension manifest is unreadable: {manifest_path}: {exc}"
            ) from exc
        init_symbol = payload.get("init_symbol")
        if not isinstance(init_symbol, str) or not init_symbol.startswith("PyInit_"):
            raise ValueError(
                "sealed native extension manifest has invalid init_symbol: "
                f"{manifest_path}: {init_symbol!r}"
            )
        symbols.add(init_symbol)
    return tuple(sorted(symbols))


def _split_app_native_link_args(native_inputs: Sequence[Path]) -> list[str]:
    """wasm-ld args for the SPLIT app link, overriding wasi-libc's ``%L`` stub.

    The split ``app.wasm`` statically links numpy/scipy + their own wasi-libc
    ``libc.a`` but â€” unlike the combined ``output_linked.wasm`` â€” does NOT link
    the reloc runtime object, so numpy's ``NumPyOS_ascii_formatl`` ->
    ``snprintf("%Lg")`` binds ``libc.a``'s ``long_double_not_supported`` stub
    (raw ``unreachable`` trap at ``_multiarray_umath`` import).

    Applies the SINGLE long-double link authority
    (:func:`wasm_toolchain.resolve_long_double_link_policy` +
    :func:`wasm_toolchain.long_double_whole_archive_link_argv`) â€” the SAME policy
    the reloc runtime and deploy cdylib links apply: whole-archive
    ``libc-printscan-long-double.a`` ahead of ``libc.a`` so its real
    ``vfprintf``/``__floatscan``/``strtold`` override the stub objects (they stay
    lazy once defined), + the binary128 soft-float builtins. Scoped to the split
    app ONLY: the combined link already carries these from the reloc runtime, so
    whole-archiving there duplicate-symbols. Non-numpy builds (no ``libc.a``) get
    the plain passthrough.
    """
    inputs = list(native_inputs)
    if not any(path.name == "libc.a" for path in inputs):
        return [str(path) for path in inputs]
    # libc.a present => numpy/scipy static tier: a missing formatter archive is a
    # HARD ERROR (relinking the abort stub would trap at import).
    policy = wasm_toolchain.resolve_long_double_link_policy(required=True)
    if policy.error is not None:
        raise ValueError(policy.error)
    return wasm_toolchain.long_double_whole_archive_link_argv(
        policy, whole_archive=[], trailing=[str(path) for path in inputs]
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


def _active_data_segment_intervals(data: bytes) -> tuple[tuple[int, int], ...]:
    """Return validated, ordered ``[start, end)`` active data intervals."""

    intervals: list[tuple[int, int]] = []
    for section_id, payload in _parse_sections(data):
        if section_id != 11:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_varuint(payload, offset)
            if flags == 1:
                size, offset = _read_varuint(payload, offset)
                if size > len(payload) - offset:
                    raise ValueError(
                        "Passive data segment extends beyond the data section payload: "
                        f"size={size}, remaining={len(payload) - offset}"
                    )
                offset += size
                continue
            if flags == 2:
                _memory_index, offset = _read_varuint(payload, offset)
            elif flags != 0:
                raise ValueError(f"Unsupported data segment flags: {flags}")
            data_offset, offset = _read_const_i32_init_expr(payload, offset)
            data_offset &= 0xFFFF_FFFF
            size, offset = _read_varuint(payload, offset)
            if size > len(payload) - offset:
                raise ValueError(
                    "Active data segment extends beyond the data section payload: "
                    f"offset={data_offset}, size={size}, remaining={len(payload) - offset}"
                )
            offset += size
            data_end = data_offset + size
            if data_end > 1 << 32:
                raise ValueError(
                    "Active data segment exceeds the wasm32 address space: "
                    f"offset={data_offset}, size={size}, end={data_end}"
                )
            if size:
                intervals.append((data_offset, data_end))
        if offset != len(payload):
            raise ValueError(
                "Data section has trailing bytes after its declared segments: "
                f"parsed={offset}, size={len(payload)}"
            )

    intervals.sort()
    for previous, current in zip(intervals, intervals[1:], strict=False):
        if current[0] < previous[1]:
            raise ValueError(
                "Active data segments overlap: "
                f"previous=[{previous[0]}, {previous[1]}), "
                f"current=[{current[0]}, {current[1]})"
            )
    return tuple(intervals)


def _split_app_global_base(output_data: bytes) -> int:
    """Place native linked data after every output-owned active data byte."""

    intervals = _active_data_segment_intervals(output_data)
    if not intervals:
        raise ValueError("split app output has no active data placement authority")
    active_end = max(end for _start, end in intervals)
    aligned_end = (active_end + 15) & ~15
    if aligned_end >= 1 << 32:
        raise ValueError(
            "Aligned split-app data end exceeds the wasm32 address space: "
            f"active_end={active_end}, aligned_end={aligned_end}"
        )
    return aligned_end


def _validate_split_app_data_layout(
    output_data: bytes,
    linked_data: bytes,
    *,
    planned_base: int,
) -> tuple[tuple[tuple[int, int], ...], tuple[tuple[int, int], ...]]:
    """Validate and return original/final active-data intervals."""

    original = _active_data_segment_intervals(output_data)
    linked = _active_data_segment_intervals(linked_data)
    if not original:
        raise ValueError("split app output has no active data placement authority")
    if not linked:
        raise ValueError("linked split app has no active data segments")
    original_extent = (original[0][0], max(end for _start, end in original))
    linked_extent = (linked[0][0], max(end for _start, end in linked))
    if planned_base != (original_extent[1] + 15) & ~15:
        raise ValueError(
            "split app native data base does not match the aligned output data end: "
            f"output_end={original_extent[1]}, planned_base={planned_base}"
        )
    if linked_extent[0] < planned_base:
        raise ValueError(
            "linked split app data overlaps output-owned active data: "
            f"output_extent=[{original_extent[0]}, {original_extent[1]}), "
            f"linked_extent=[{linked_extent[0]}, {linked_extent[1]}), "
            f"planned_base={planned_base}"
        )
    return original, linked


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
                "molt_set_wasm_table_base",
            )
            if name in export_symbol_map
        }
    )
    return public_export_map


def _restore_public_output_exports(
    data: bytes,
    public_export_map: Mapping[str, str],
    *,
    preserved_symbol_names: Sequence[str] = (),
) -> bytes:
    restored = data
    updated = _ensure_function_exports_by_symbol_names(
        restored, dict(public_export_map)
    )
    if updated is not None:
        restored = updated
    rename_map = {
        symbol_name: public_name
        for public_name, symbol_name in public_export_map.items()
        if symbol_name != public_name and symbol_name not in preserved_symbol_names
    }
    updated = _rename_export_names(restored, rename_map)
    if updated is not None:
        restored = updated
    updated = _restore_output_export_aliases(restored)
    if updated is not None:
        restored = updated
    updated = _ensure_function_exports_by_symbol_names(
        restored,
        {name: name for name in preserved_symbol_names},
    )
    if updated is not None:
        restored = updated
    return restored


def _import_index_for_kind(
    data: bytes,
    *,
    module: str,
    name: str,
    kind: int,
) -> int | None:
    index = 0
    for import_module, import_name, import_kind, _desc in _collect_imports(data):
        if import_kind != kind:
            continue
        if import_module == module and import_name == name:
            return index
        index += 1
    return None


def _ensure_export_by_index(
    data: bytes,
    *,
    name: str,
    kind: int,
    index: int,
) -> bytes | None:
    sections = _parse_sections(data)
    rebuilt_sections: list[tuple[int, bytes]] = []
    inserted = False
    for section_id, payload in sections:
        if section_id == 7:
            count, offset = _read_varuint(payload, 0)
            rebuilt = bytearray(_write_varuint(count + 1))
            rebuilt.extend(payload[offset:])
            rebuilt.extend(_write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(_write_varuint(index))
            rebuilt_sections.append((section_id, bytes(rebuilt)))
            inserted = True
            continue
        rebuilt_sections.append((section_id, payload))
    if not inserted:
        export_payload = bytearray(_write_varuint(1))
        export_payload.extend(_write_string(name))
        export_payload.append(kind)
        export_payload.extend(_write_varuint(index))
        rebuilt_sections.append((7, bytes(export_payload)))
    rebuilt = _build_sections(rebuilt_sections)
    canonical = _canonicalize_standard_section_order(rebuilt)
    return rebuilt if canonical is None else canonical


def _ensure_defined_memory_export(data: bytes) -> bytes | None:
    facts = parse_wasm_module_facts(data)
    if any(
        facts.export_kinds.get(name, (None, None))[0] == 2
        for name in ("molt_memory", "memory")
    ):
        return None
    memory_imports = [entry for entry in facts.imports if entry[2] == 2]
    if memory_imports:
        raise ValueError("cannot restore linked memory export from an imported memory")
    memory_sections = [
        payload for section_id, payload in _parse_sections(data) if section_id == 5
    ]
    if not memory_sections:
        return None
    if len(memory_sections) != 1:
        raise ValueError(
            "cannot restore linked memory export without exactly one memory section"
        )
    memory_count, _ = _read_varuint(memory_sections[0], 0)
    if memory_count != 1:
        raise ValueError(
            "cannot restore linked memory export without exactly one defined memory"
        )
    return _ensure_export_by_index(data, name="molt_memory", kind=2, index=0)


def _restore_split_runtime_contract_exports(
    data: bytes,
    *,
    artifact: str,
    stage: str = "unspecified",
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
    operation_counts: dict[str, int | float] | None = None,
) -> bytes:
    function_symbols = _split_artifact_contract_function_symbols(
        artifact,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    input_exports = _collect_function_exports(data)
    input_bodies = _function_body_payloads_by_index(data)
    contract_function_bodies = {
        public_name: input_bodies[index]
        for public_name, symbol_name in function_symbols.items()
        if (index := input_exports.get(public_name, input_exports.get(symbol_name)))
        is not None
        and index in input_bodies
        and input_bodies[index] != _TRAP_FUNC_BODY
    }
    restored = _restore_public_output_exports(
        data,
        public_export_map or {},
        preserved_symbol_names=required_native_direct_symbols,
    )
    updated = _ensure_function_exports_by_symbol_names(restored, function_symbols)
    if updated is not None:
        restored = updated
    current_exports = _collect_function_exports(restored)
    current_bodies = _function_body_payloads_by_index(restored)
    body_indices: dict[bytes, list[int]] = {}
    for index, body in current_bodies.items():
        if body != _TRAP_FUNC_BODY:
            body_indices.setdefault(body, []).append(index)
    for public_name, body in contract_function_bodies.items():
        if public_name in current_exports:
            continue
        matches = body_indices.get(body, [])
        if len(matches) != 1:
            continue
        updated = _ensure_export_by_index(
            restored,
            name=public_name,
            kind=0,
            index=matches[0],
        )
        if updated is not None:
            restored = updated
            current_exports[public_name] = matches[0]
    missing_native_direct = sorted(
        set(required_native_direct_symbols) - set(current_exports)
    )
    if missing_native_direct:
        details = []
        for name in missing_native_direct:
            body = contract_function_bodies.get(name)
            details.append(
                f"{name}(input_export={name in input_exports}, "
                f"body_matches={len(body_indices.get(body, [])) if body else 0})"
            )
        raise ValueError(
            f"Split-runtime {artifact} cannot relocate required native direct "
            f"function export(s) at {stage}: {', '.join(details)}"
        )
    import_names = {1: "__indirect_function_table", 2: "memory"}
    contract = _split_runtime_export_contract(artifact)
    facts = parse_wasm_module_facts(restored)
    export_kinds = dict(facts.export_kinds)
    if operation_counts is not None:
        eliminated = max(0, len(contract) - 1)
        operation_counts["wasm_whole_artifact_redundant_parses_eliminated"] = (
            operation_counts.get("wasm_whole_artifact_redundant_parses_eliminated", 0)
            + eliminated
        )
    for entry in contract:
        if any(
            export_kinds.get(name, (None, None))[0] == entry.kind
            for name in entry.accepted_names
        ):
            continue
        if entry.kind == 0:
            raise ValueError(
                f"Split-runtime {artifact} is missing app-owned function export "
                f"{entry.canonical_name} after symbol restoration at {stage}"
            )
        import_name = import_names.get(entry.kind)
        if import_name is None:
            raise ValueError(
                f"Split-runtime {artifact} has no restoration source for export "
                f"{entry.canonical_name} kind {entry.kind}"
            )
        index = _import_index_for_kind(
            restored,
            module="env",
            name=import_name,
            kind=entry.kind,
        )
        if index is None:
            raise ValueError(
                f"Split-runtime {artifact} cannot restore {entry.canonical_name}: "
                f"missing env.{import_name} kind {entry.kind} import"
            )
        updated = _ensure_export_by_index(
            restored,
            name=entry.canonical_name,
            kind=entry.kind,
            index=index,
        )
        if updated is not None:
            restored = updated
            export_kinds[entry.canonical_name] = (entry.kind, index)
    return restored


def _strip_and_restore_split_artifact(
    data: bytes,
    *,
    artifact: str,
    stage: str,
    preserve_debug: bool,
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
    operation_counts: dict[str, int | float] | None = None,
) -> bytes:
    keep_set = _split_artifact_contract_keep_set(
        artifact,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    stripped = strip_wasm_publication_sections(
        data,
        final_artifact=True,
        preserve_debug=preserve_debug,
    )
    restored = _restore_split_runtime_contract_exports(
        stripped,
        artifact=artifact,
        stage=stage,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
        operation_counts=operation_counts,
    )
    facts = parse_wasm_module_facts(restored)
    missing = sorted(
        name
        for name in keep_set
        if name not in facts.export_kinds
        and name not in _split_runtime_contract_export_names(artifact)
    )
    if missing:
        raise ValueError(
            f"Split-runtime {artifact} publication lost required export(s) at "
            f"{stage}: {', '.join(missing)}"
        )
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


def _rewrite_required_native_direct_imports(
    module_path: Path,
    required_symbols: Sequence[str],
    temp_dir: tempfile.TemporaryDirectory,
) -> Path:
    required = set(required_symbols)
    if not required:
        return module_path
    sections = _parse_sections(module_path.read_bytes())
    changed = False
    rebuilt_sections: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 2:
            rebuilt_sections.append((section_id, payload))
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        rebuilt = bytearray(_write_varuint(count))
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
            if module == "molt_native" and kind == 0 and name in required:
                module = "env"
                changed = True
            rebuilt.extend(_write_string(module))
            rebuilt.extend(_write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(desc)
        rebuilt_sections.append((section_id, bytes(rebuilt)))
    if not changed:
        return module_path
    rewritten_path = Path(temp_dir.name) / "output_native_direct_imports.wasm"
    rewritten_path.write_bytes(_build_sections(rebuilt_sections))
    return rewritten_path


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
    *,
    facts_provider: Callable[[bytes], dict[str, object]],
    operation_counts: dict[str, int | float] | None = None,
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
    # Canonicalize the app import surface to the runtime export naming
    # convention.  The app imports the unprefixed ABI names (e.g. `alloc`,
    # `module_import`), while the runtime exports the corresponding
    # `molt_*` symbols.  Without this normalization, split-runtime
    # tree-shaking strips every function export even when the app has a
    # large live runtime dependency surface.
    normalized_required_exports = set(required_exports)
    for name in required_exports:
        export_name = wasm_split_runtime_export_name_for_import(name)
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

    # Resolve and probe the content-addressed result before parsing or rewriting
    # the input.  The key includes the complete source artifact, normalized
    # contract, Binaryen identity, pass flags, and transform implementation, so
    # a hit can bypass the whole linker-owned transformation family rather than
    # merely skipping the final Binaryen subprocess.
    wasm_opt = find_wasm_opt()
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
    cache_entry: WasmLinkCacheEntry | None = None
    wasm_opt_identity = _wasm_opt_executable_identity(wasm_opt) if wasm_opt else None
    cache_started = time.perf_counter()
    metric_prefix = "runtime_tree_shake_cache"
    if wasm_opt:
        _cache_metric_add(operation_counts, f"{metric_prefix}_requests", 1)
    if wasm_opt and wasm_opt_identity is None:
        _cache_metric_add(operation_counts, f"{metric_prefix}_identity_errors", 1)
    if wasm_opt_identity is not None:
        _wasm_opt_path, wasm_opt_sha256, _wasm_opt_version_text = wasm_opt_identity
        cache_key = _tree_shake_runtime_cache_key(
            runtime_data=runtime_data,
            normalized_required_exports=normalized_required_exports,
            wasm_opt_sha256=wasm_opt_sha256,
            feature_flags=feature_flags,
        )
        cache_entry = _wasm_link_cache_entry(
            "runtime_tree_shake",
            _TREE_SHAKE_RUNTIME_CACHE_SCHEMA,
            cache_key,
            cache_root=_wasm_link_cache_root(),
        )
        with _locked_wasm_link_cache_entry(cache_entry) as lock_wait_ms:
            _cache_metric_add(
                operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
            )
            lookup_started = time.perf_counter()
            cached = _read_wasm_link_cache_entry(cache_entry)
            _cache_metric_add(
                operation_counts,
                f"{metric_prefix}_lookup_ms",
                (time.perf_counter() - lookup_started) * 1000.0,
            )
            if cached.data is not None:
                _cache_metric_add(operation_counts, f"{metric_prefix}_hits", 1)
                _cache_metric_add(
                    operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
                )
                _cache_metric_add(
                    operation_counts,
                    f"{metric_prefix}_wall_ms",
                    (time.perf_counter() - cache_started) * 1000.0,
                )
                print(
                    f"Runtime tree-shake cache hit: {cache_entry.root}", file=sys.stderr
                )
                return cached.data
            _cache_metric_add(operation_counts, f"{metric_prefix}_misses", 1)
            if cached.status == "corrupt":
                _cache_metric_add(operation_counts, f"{metric_prefix}_corruptions", 1)
                _invalidate_wasm_link_cache_entry(cache_entry)

    sections = _parse_sections(runtime_data)

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
        facts_provider=facts_provider,
    )
    if len(optimized_baseline) != len(stripped_data):
        print(
            f"Runtime post-link optimize: {len(stripped_data):,} -> {len(optimized_baseline):,} bytes "
            f"({len(stripped_data) - len(optimized_baseline):,} bytes eliminated)",
            file=sys.stderr,
        )

    # Use wasm-opt to eliminate dead code (functions no longer reachable
    # from the reduced export set). Resolution goes through the one
    # toolchain authority (MOLT_WASM_OPT, PATH, then the managed
    # MOLT_TARGET_ROOT/toolchains/binaryen-* root).
    if not wasm_opt:
        print(
            "wasm-opt not found; skipping dead-code elimination "
            "(export stripping only). Provision Binaryen on PATH, set "
            "MOLT_WASM_OPT, or unpack a binaryen-* release under "
            "MOLT_TARGET_ROOT/toolchains/.",
            file=sys.stderr,
        )
        return optimized_baseline

    with tempfile.TemporaryDirectory(prefix="molt-treeshake-") as tmp:
        input_path = Path(tmp) / "runtime_stripped.wasm"
        output_path = Path(tmp) / "runtime_shaken.wasm"
        input_path.write_bytes(optimized_baseline)

        lock_context = (
            _locked_wasm_link_cache_entry(cache_entry)
            if cache_entry is not None
            else contextlib.nullcontext(0.0)
        )
        with lock_context as lock_wait_ms:
            if cache_entry is not None:
                _cache_metric_add(
                    operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
                )
                lookup_started = time.perf_counter()
                cached = _read_wasm_link_cache_entry(cache_entry)
                _cache_metric_add(
                    operation_counts,
                    f"{metric_prefix}_lookup_ms",
                    (time.perf_counter() - lookup_started) * 1000.0,
                )
                if cached.data is not None:
                    _cache_metric_add(operation_counts, f"{metric_prefix}_hits", 1)
                    _cache_metric_add(
                        operation_counts,
                        f"{metric_prefix}_bytes_read",
                        cached.bytes_read,
                    )
                    _cache_metric_add(
                        operation_counts,
                        f"{metric_prefix}_wall_ms",
                        (time.perf_counter() - cache_started) * 1000.0,
                    )
                    print(
                        f"Runtime tree-shake cache hit: {cache_entry.root}",
                        file=sys.stderr,
                    )
                    return cached.data
                if cached.status == "corrupt":
                    _cache_metric_add(
                        operation_counts, f"{metric_prefix}_corruptions", 1
                    )
                    _invalidate_wasm_link_cache_entry(cache_entry)

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
                    timeout=300,
                )
            except subprocess.TimeoutExpired:
                _cache_metric_add(operation_counts, f"{metric_prefix}_timeouts", 1)
                _cache_metric_add(
                    operation_counts,
                    f"{metric_prefix}_wall_ms",
                    (time.perf_counter() - cache_started) * 1000.0,
                )
                print(
                    "wasm-opt tree-shake timed out (non-fatal); keeping post-link-optimized runtime",
                    file=sys.stderr,
                )
                return optimized_baseline
            _record_guarded_process_cache_metrics(
                operation_counts, metric_prefix, result
            )

            if result.returncode != 0:
                # wasm-opt may fail on some modules (e.g. unsupported features).
                # Fall back gracefully to export-stripped version.
                _cache_metric_add(operation_counts, f"{metric_prefix}_failures", 1)
                _cache_metric_add(
                    operation_counts,
                    f"{metric_prefix}_wall_ms",
                    (time.perf_counter() - cache_started) * 1000.0,
                )
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
            final_attestation: dict[str, object] = {}
            if _run_wasm_opt_via_optimize(
                final_path,
                level="Oz",
                apply_level=False,
                attestation=final_attestation,
            ):
                final_data = final_path.read_bytes()
                _record_wasm_opt_attestation_cache_metrics(
                    operation_counts, metric_prefix, final_attestation
                )
                result_data = final_data
                print(
                    f"Runtime final optimize: {len(runtime_data):,} -> {len(final_data):,} bytes "
                    f"({len(runtime_data) - len(final_data):,} bytes eliminated, "
                    f"{(len(runtime_data) - len(final_data)) / len(runtime_data) * 100:.1f}% reduction)",
                    file=sys.stderr,
                )
            else:
                result_data = shaken_data
            if cache_entry is not None:
                _publish_wasm_link_cache_result(
                    cache_entry,
                    result_data,
                    metrics=operation_counts,
                    metric_prefix=metric_prefix,
                    label="Runtime tree-shake",
                    payload={
                        "result_kind": "optimized",
                        "wasm_opt_path": wasm_opt_identity[0],
                        "wasm_opt_sha256": wasm_opt_identity[1],
                        "wasm_opt_version": wasm_opt_identity[2],
                    },
                )
            _cache_metric_add(
                operation_counts,
                f"{metric_prefix}_wall_ms",
                (time.perf_counter() - cache_started) * 1000.0,
            )
            return result_data


def _optimize_split_app_module(
    app_data: bytes,
    *,
    reference_data: bytes | None,
    optimize: bool,
    optimize_level: str,
    contract_keep_set: set[str],
    attestation: dict[str, object] | None = None,
    operation_counts: dict[str, int | float] | None = None,
    facts_provider: Callable[[bytes], dict[str, object]],
) -> bytes:
    """Deforest the split-runtime app artifact without collapsing its imports.

    The split app module must remain unlinked so it can continue importing the
    deploy runtime, but it still benefits from the same post-link cleanup passes
    as the fully linked artifact. Apply those cleanup passes first, then run
    wasm-opt when requested.
    """
    if operation_counts is not None:
        operation_counts["split_app_optimize_requests"] = 1
    cache_key = _split_app_optimize_cache_key(
        app_data=app_data,
        reference_data=reference_data,
        optimize=optimize,
        optimize_level=optimize_level,
        contract_keep_set=contract_keep_set,
    )
    cache_started = time.perf_counter()
    metric_prefix = "split_app_optimize_cache"
    _cache_metric_add(operation_counts, f"{metric_prefix}_requests", 1)
    cache_entry = (
        _wasm_link_cache_entry(
            "split_app_optimize",
            _SPLIT_APP_OPTIMIZE_CACHE_SCHEMA,
            cache_key,
            cache_root=_wasm_link_cache_root(),
        )
        if cache_key is not None
        else None
    )
    if cache_entry is None:
        _cache_metric_add(operation_counts, f"{metric_prefix}_identity_errors", 1)
    lock_context = (
        _locked_wasm_link_cache_entry(cache_entry)
        if cache_entry is not None
        else contextlib.nullcontext(0.0)
    )
    with lock_context as lock_wait_ms:
        if cache_entry is not None:
            _cache_metric_add(
                operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
            )
            lookup_started = time.perf_counter()
            cached = _read_wasm_link_cache_entry(cache_entry)
            _cache_metric_add(
                operation_counts,
                f"{metric_prefix}_lookup_ms",
                (time.perf_counter() - lookup_started) * 1000.0,
            )
            if cached.data is not None:
                _cache_metric_add(operation_counts, f"{metric_prefix}_hits", 1)
                _cache_metric_add(
                    operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
                )
                if attestation is not None:
                    attestation.update(cached.payload or {})
                    attestation["cache_hit"] = True
                _cache_metric_add(
                    operation_counts,
                    f"{metric_prefix}_wall_ms",
                    (time.perf_counter() - cache_started) * 1000.0,
                )
                return cached.data
            _cache_metric_add(operation_counts, f"{metric_prefix}_misses", 1)
            if cached.status == "corrupt":
                _cache_metric_add(operation_counts, f"{metric_prefix}_corruptions", 1)
                _invalidate_wasm_link_cache_entry(cache_entry)

        optimized = _post_link_optimize(
            app_data,
            reference_data=reference_data,
            preserve_exports=contract_keep_set,
            preserve_reference_exports=False,
            facts_provider=facts_provider,
        )
        stripped = _strip_unused_module_function_imports(
            optimized,
            module_name="molt_runtime",
            facts=facts_provider(optimized),
        )
        if stripped is not None:
            optimized = stripped
        result = optimized
        active_attestation = attestation if attestation is not None else {}
        if optimize:
            with tempfile.TemporaryDirectory(prefix="molt-split-app-opt-") as tmp:
                app_path = Path(tmp) / "app_split_preopt.wasm"
                app_path.write_bytes(optimized)
                required_function_exports = (
                    set(_collect_function_exports(optimized)) & contract_keep_set
                )
                _cache_metric_add(operation_counts, "split_app_wasm_opt_runs", 1)
                if _run_wasm_opt_via_optimize(
                    app_path,
                    level=optimize_level,
                    required_exports=required_function_exports,
                    apply_level=optimize_level != "Oz",
                    attestation=active_attestation,
                ):
                    result = app_path.read_bytes()
                _record_wasm_opt_attestation_cache_metrics(
                    operation_counts, metric_prefix, active_attestation
                )
        cache_payload = dict(active_attestation)
        cache_payload["cache_hit"] = False
        if optimize:
            wasm_opt = find_wasm_opt()
            identity = _wasm_opt_executable_identity(wasm_opt) if wasm_opt else None
            if identity is not None:
                cache_payload.update(
                    {
                        "wasm_opt_path": identity[0],
                        "wasm_opt_sha256": identity[1],
                        "wasm_opt_version": identity[2],
                    }
                )
        if cache_entry is not None:
            _publish_wasm_link_cache_result(
                cache_entry,
                result,
                metrics=operation_counts,
                metric_prefix=metric_prefix,
                label="Split app optimize",
                payload=cache_payload,
            )
        _cache_metric_add(
            operation_counts,
            f"{metric_prefix}_wall_ms",
            (time.perf_counter() - cache_started) * 1000.0,
        )
        return result


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


def _validate_wasm_structural(data: bytes, *, description: str) -> bool:
    """Run the canonical wasm structural validator when available."""
    section_order_error = _standard_section_order_error(data)
    if section_order_error is not None:
        print(
            f"{description} failed canonical section-order validation: "
            f"{section_order_error}",
            file=sys.stderr,
        )
        return False
    exe = shutil.which("wasm-tools")
    if exe is None:
        return True
    try:
        validate_data = _strip_debug_sections(data) or data
    except ValueError as exc:
        print(
            f"{description} debug-section stripping warning: {exc}; "
            "validating original bytes",
            file=sys.stderr,
        )
        validate_data = data
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
                f"{description} failed structural validation: "
                f"{result.stderr.strip()[:500]}",
                file=sys.stderr,
            )
            return False
    except Exception as exc:
        print(f"wasm-tools validate warning: {exc}", file=sys.stderr)
    finally:
        try:
            Path(tmp_path).unlink()
        except OSError:
            pass
    return True


def _validate_linked(linked: Path) -> bool:
    data = linked.read_bytes()
    try:
        facts = parse_wasm_module_facts(data)
    except ValueError as exc:
        print(f"Failed to parse linked wasm: {exc}", file=sys.stderr)
        return False
    imports = list(facts.imports)
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
    custom_names = facts.custom_names
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
    exports = facts.exports
    if "molt_memory" not in exports and "memory" not in exports:
        print("Linked wasm missing exported memory.", file=sys.stderr)
        return False
    if "molt_table" not in exports and "__indirect_function_table" not in exports:
        print("Linked wasm missing exported table.", file=sys.stderr)
        return False
    if facts.element_validation_error is not None:
        print(
            f"Linked wasm element validation failed: {facts.element_validation_error}",
            file=sys.stderr,
        )
        return False
    return _validate_wasm_structural(data, description="Linked wasm")


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
        app_facts = parse_wasm_module_facts(app_data)
        rt_facts = parse_wasm_module_facts(rt_data)
    except ValueError as exc:
        print(f"Failed to parse split-runtime staged output: {exc}", file=sys.stderr)
        return False
    app_imports = app_facts.module_imports.get("molt_runtime", frozenset())
    rt_exports = rt_facts.function_exports
    app_memory_min = app_facts.memory_import_mins.get(("env", "memory"))
    if app_memory_min is None:
        print(
            "Split-runtime app must import env.memory; a private app memory "
            "breaks pointer-bearing runtime ABI calls.",
            file=sys.stderr,
        )
        return False
    for entry in _split_runtime_export_contract("app"):
        if any(
            app_facts.export_kinds.get(name, (None, None))[0] == entry.kind
            for name in entry.accepted_names
        ):
            continue
        print(
            f"Split-runtime app missing contract export {entry.canonical_name} "
            f"(kind {entry.kind}).",
            file=sys.stderr,
        )
        return False
    missing: list[str] = []
    for name in app_imports:
        export_name = wasm_split_runtime_export_name_for_import(name)
        if export_name is not None and export_name in rt_exports:
            continue
        if export_name is None and name in rt_exports:
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
    if not _validate_wasm_structural(app_data, description="Split-runtime app"):
        return False
    if not _validate_wasm_structural(
        rt_data, description="Split-runtime shared runtime"
    ):
        return False
    return True


# Pass pipelines from docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md Section 4.4.
_OZ_PASSES: list[str] = [
    "--remove-unused-module-elements",
    "--strip-debug",
    "--strip-producers",
    "--dae-optimizing",
    "--simplify-locals",
    "--merge-blocks",
    "--dce",
    "--vacuum",
    "--zero-filled-memory",
    "--memory-packing",
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
    apply_level: bool = True,
    attestation: dict[str, object] | None = None,
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
        apply_level=apply_level,
    )

    if not result["ok"]:
        err = result.get("error", "unknown error")
        print(f"wasm-opt failed (non-fatal): {err}", file=sys.stderr)
        with contextlib.suppress(OSError):
            temp_output.unlink()
        return False

    artifact_publish.publish_validated_outputs([(temp_output, linked)])
    if attestation is not None:
        attestation.update(
            {
                "binaryen_version": result.get("binaryen_version", ""),
                "pipeline": result.get("pipeline", []),
                "before": result.get("before", {}),
                "after": result.get("after", {}),
                "wasm_opt_wall_ms": round(
                    float(result.get("elapsed_s", 0.0)) * 1000.0, 6
                ),
                "wasm_opt_peak_rss_kb": result.get("peak_rss_kb"),
                "wasm_opt_peak_total_rss_kb": result.get("peak_total_rss_kb"),
            }
        )

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


def _decode_wasm_facts_response(
    process: subprocess.CompletedProcess[str],
    *,
    operation: str,
) -> dict[str, object]:
    try:
        payload = json.loads(process.stdout)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"{operation} returned invalid JSON: {exc}") from exc
    if not isinstance(payload, dict) or payload.get("schema_version") != 4:
        raise ValueError(f"{operation} returned an unsupported response schema")
    if process.returncode != 0 or payload.get("ok") is not True:
        error = payload.get("error")
        detail = error if isinstance(error, str) and error else process.stderr.strip()
        raise ValueError(f"{operation} failed: {detail or 'unknown scanner error'}")
    facts = payload.get("facts")
    if not isinstance(facts, dict) or facts.get("schema_version") != 4:
        raise ValueError(f"{operation} returned an unsupported facts schema")
    return facts


def _make_rust_wasm_facts_provider(
    scanner: Path,
    scratch_root: Path,
    metrics: dict[str, float] | None = None,
) -> Callable[[bytes], dict[str, object]]:
    if not scanner.is_file():
        raise ValueError(f"WASM facts scanner is not a file: {scanner}")
    cache: dict[str, dict[str, object]] = {}
    if metrics is not None:
        metrics.update(
            {
                "wasm_facts_hash_ms": 0.0,
                "wasm_facts_scan_ms": 0.0,
                "wasm_facts_scan_calls": 0.0,
                "wasm_facts_cache_hits": 0.0,
                "wasm_facts_input_bytes": 0.0,
                "wasm_facts_response_chars": 0.0,
            }
        )

    def provide(data: bytes) -> dict[str, object]:
        hash_start = time.perf_counter()
        digest = hashlib.sha256(data).hexdigest()
        if metrics is not None:
            metrics["wasm_facts_hash_ms"] += max(
                0.0, (time.perf_counter() - hash_start) * 1000.0
            )
        cached = cache.get(digest)
        if cached is not None:
            if metrics is not None:
                metrics["wasm_facts_cache_hits"] += 1.0
            return cached
        artifact = scratch_root / f"wasm-facts-{digest}.wasm"
        artifact.write_bytes(data)
        scan_start = time.perf_counter()
        try:
            process = subprocess.run(
                [str(scanner), "--scan-wasm-link-facts", str(artifact)],
                text=True,
                encoding="utf-8",
                errors="replace",
                capture_output=True,
                check=False,
            )
            if metrics is not None:
                metrics["wasm_facts_scan_ms"] += max(
                    0.0, (time.perf_counter() - scan_start) * 1000.0
                )
                metrics["wasm_facts_scan_calls"] += 1.0
                metrics["wasm_facts_input_bytes"] += float(len(data))
                metrics["wasm_facts_response_chars"] += float(len(process.stdout))
            facts = _decode_wasm_facts_response(
                process,
                operation=f"Rust WASM facts scan for {artifact.name}",
            )
            cache[digest] = facts
            return facts
        finally:
            with contextlib.suppress(OSError):
                artifact.unlink()

    return provide


def _publish_rust_wasm_link_facts(
    scanner: Path,
    artifact: Path,
    *,
    layout: CallableTableLayout | None = None,
    role: str = "monolithic",
) -> dict[str, object]:
    if not scanner.is_file():
        raise ValueError(f"WASM facts scanner is not a file: {scanner}")
    command = [
        str(scanner),
        "--publish-wasm-link-facts",
        str(artifact),
        "--output",
        str(artifact),
    ]
    if layout is not None:
        command.extend(
            [
                "--callable-table-layout",
                ",".join(
                    str(value)
                    for value in (
                        layout.fixed_prefix_base,
                        layout.fixed_prefix_len,
                        layout.finalized_app_base,
                        layout.app_entry_count,
                    )
                ),
            ]
        )
    if role not in {"monolithic", "app", "runtime"}:
        raise ValueError(f"unknown callable-table artifact role: {role}")
    if role != "monolithic" and layout is None:
        raise ValueError(f"callable-table {role} publication requires a layout")
    command.extend(["--callable-table-role", role])
    process = subprocess.run(
        command,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    facts = _decode_wasm_facts_response(
        process,
        operation=f"Rust WASM facts publication for {artifact}",
    )
    if facts.get("callable_table_attestation_present") is not True:
        raise ValueError("Rust WASM facts publication omitted final attestation")
    return facts


def _callable_layout_from_wasm_facts(
    facts: Mapping[str, object],
) -> CallableTableLayout | None:
    raw_layout = facts.get("callable_table_layout")
    if raw_layout is None:
        return None
    if not isinstance(raw_layout, dict):
        raise ValueError("WASM facts callable_table_layout must be an object or null")
    names = (
        "fixed_prefix_base",
        "fixed_prefix_len",
        "finalized_app_base",
        "app_entry_count",
    )
    values = tuple(raw_layout.get(name) for name in names)
    if not all(
        isinstance(value, int)
        and not isinstance(value, bool)
        and 0 <= value <= 0xFFFF_FFFF
        for value in values
    ):
        raise ValueError("WASM facts callable-table layout fields must be u32 integers")
    layout_values = tuple(cast(int, value) for value in values)
    return CallableTableLayout(*layout_values)


def _reconcile_split_callable_layout(
    app_layout: CallableTableLayout,
    runtime_layout: WasmSplitRuntimeCallableLayout,
) -> CallableTableLayout:
    if (
        app_layout.fixed_prefix_base != runtime_layout.runtime_callable_base
        or app_layout.fixed_prefix_len != runtime_layout.fixed_prefix_len
    ):
        raise ValueError(
            "app compiler callable prefix disagrees with the executable runtime: "
            f"app=({app_layout.fixed_prefix_base},{app_layout.fixed_prefix_len}) "
            f"runtime=({runtime_layout.runtime_callable_base},"
            f"{runtime_layout.fixed_prefix_len})"
        )
    if runtime_layout.runtime_occupied_end > app_layout.finalized_app_base:
        raise ValueError(
            "runtime callable entries overlap the app-owned callable region: "
            f"runtime_occupied_end={runtime_layout.runtime_occupied_end}, "
            f"app_base={app_layout.finalized_app_base}"
        )
    reconciled = CallableTableLayout(
        runtime_layout.runtime_callable_base,
        runtime_layout.fixed_prefix_len,
        app_layout.finalized_app_base,
        app_layout.app_entry_count,
    )
    reconciled.validate()
    return reconciled


def _callable_entry_export_name(slot: int) -> str:
    return f"{WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME}.entry.{slot}"


def _write_varsint32(value: int) -> bytes:
    if value < -(1 << 31) or value >= 1 << 31:
        raise ValueError("callable-table fixed prefix base must fit i32")
    out = bytearray()
    remaining = value
    while True:
        byte = remaining & 0x7F
        remaining >>= 7
        done = (remaining == 0 and byte & 0x40 == 0) or (
            remaining == -1 and byte & 0x40 != 0
        )
        out.append(byte if done else byte | 0x80)
        if done:
            return bytes(out)


def _install_callable_table_layout(
    data: bytes,
    layout: CallableTableLayout,
    *,
    entry_symbol_names: Sequence[str] | None = None,
    include_fixed_prefix: bool = True,
    override_reserved_direct: bool = True,
) -> bytes:
    total_entry_count = layout.fixed_prefix_len + layout.app_entry_count
    if total_entry_count == 0:
        return data
    if entry_symbol_names is not None and len(entry_symbol_names) != total_entry_count:
        raise ValueError(
            "callable-table entry symbol count disagrees with the published layout: "
            f"symbols={len(entry_symbol_names)}, entries={total_entry_count}"
        )
    exports = _collect_function_exports(data)
    named_indices: dict[str, set[int]] = {}
    if entry_symbol_names is not None:
        function_import_index = 0
        for _module, import_name, import_kind, _description in _collect_imports(data):
            if import_kind != 0:
                continue
            named_indices.setdefault(import_name, set()).add(function_import_index)
            function_import_index += 1
        for function_index, function_name in _collect_func_names(data).items():
            named_indices.setdefault(function_name, set()).add(function_index)

    def resolve_entry(slot: int) -> int:
        name = _callable_entry_export_name(slot)
        function_index = exports.get(name)
        symbol_name = entry_symbol_names[slot] if entry_symbol_names is not None else None
        if function_index is None and symbol_name is not None:
            function_index = exports.get(symbol_name)
        if function_index is None and symbol_name is not None:
            candidates = named_indices.get(symbol_name, set())
            if len(candidates) > 1:
                raise ValueError(
                    "linked wasm has ambiguous callable-table function identity "
                    f"for {symbol_name}: {candidates}"
                )
            if candidates:
                function_index = next(iter(candidates))
        if function_index is None:
            suffix = (
                f" or retained linker symbol {symbol_name}"
                if symbol_name is not None
                else ""
            )
            raise ValueError(
                f"linked wasm is missing callable-table entry export {name}{suffix}"
            )
        return function_index

    fixed_indices = (
        [resolve_entry(slot) for slot in range(layout.fixed_prefix_len)]
        if include_fixed_prefix
        else []
    )
    app_indices = [
        resolve_entry(slot)
        for slot in range(layout.fixed_prefix_len, total_entry_count)
    ]
    reserved_end = WASM_RESERVED_RUNTIME_CALLABLE_BASE + len(
        WASM_RESERVED_RUNTIME_CALLABLES
    )
    if (
        include_fixed_prefix
        and WASM_RESERVED_RUNTIME_CALLABLE_BASE < len(fixed_indices) < reserved_end
    ):
        raise ValueError("callable table truncates reserved runtime callable region")
    if (
        include_fixed_prefix
        and override_reserved_direct
        and len(fixed_indices) >= reserved_end
    ):
        for index, runtime_name, _import_name, _arity, dispatch in (
            WASM_RESERVED_RUNTIME_CALLABLES
        ):
            if dispatch != "direct":
                continue
            logical_slot = WASM_RESERVED_RUNTIME_CALLABLE_BASE + index
            runtime_function_index = exports.get(runtime_name)
            if runtime_function_index is None:
                candidates = named_indices.get(runtime_name, set())
                if len(candidates) == 1:
                    runtime_function_index = next(iter(candidates))
            if runtime_function_index is None:
                raise ValueError(
                    f"linked wasm is missing reserved runtime export {runtime_name}"
                )
            fixed_indices[logical_slot] = runtime_function_index
    sections = _parse_sections(data)
    element_indices = [
        index for index, (section_id, _payload) in enumerate(sections) if section_id == 9
    ]
    if len(element_indices) != 1:
        raise ValueError(
            "linked wasm must contain exactly one element section before fixed-prefix publication"
        )
    section_index = element_indices[0]
    section_id, payload = sections[section_index]
    segment_count, segment_offset = _read_varuint(payload, 0)
    added_segment_count = int(bool(fixed_indices)) + int(bool(app_indices))
    appended = bytearray(_write_varuint(segment_count + added_segment_count))
    appended.extend(payload[segment_offset:])
    for base, indices in (
        (layout.fixed_prefix_base, fixed_indices),
        (layout.finalized_app_base, app_indices),
    ):
        if not indices:
            continue
        appended.extend(_write_varuint(0))
        appended.append(0x41)
        appended.extend(_write_varsint32(base))
        appended.append(0x0B)
        appended.extend(_write_varuint(len(indices)))
        for function_index in indices:
            appended.extend(_write_varuint(function_index))
    sections[section_index] = (section_id, bytes(appended))
    return _build_sections(sections)


def _run_wasm_ld_with_custodied_inputs(
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
    preserve_debug_sections: bool = False,
    phase_timings_file: Path | None = None,
    wasm_facts_scanner: Path,
) -> int:
    phase_timings_ms: dict[str, float] = {}
    facts_metrics: dict[str, float] = {}
    operation_counts: dict[str, int | float] = {
        "wasm_whole_artifact_full_binary_parses": 0,
        "wasm_whole_artifact_section_walks": 0,
        "wasm_whole_artifact_reserializations": 0,
        "wasm_whole_artifact_redundant_parses_eliminated": 0,
        **_empty_wasm_link_cache_metrics(),
    }
    total_start = time.perf_counter()
    # The finally block records partial-failure timing even when lld rejects an
    # input before split processing begins.
    split_runtime_start = total_start
    for native_object in native_objects:
        if not native_object.exists():
            print(f"Native WASM link input not found: {native_object}", file=sys.stderr)
            return 1
    try:
        native_objects = _resolve_native_link_inputs(tuple(native_objects))
    except ValueError as exc:
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        return 1
    runtime_exports: set[str]
    try:
        runtime_exports = _collect_exports(runtime.read_bytes())
    except ValueError as exc:
        print(
            f"Failed to parse runtime wasm exports ({runtime}): {exc}", file=sys.stderr
        )
        runtime_exports = set()
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
                runtime_exports = set()
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
    temp_dir = tempfile.TemporaryDirectory(prefix="molt-wasm-link-")
    try:
        facts_provider = _make_rust_wasm_facts_provider(
            wasm_facts_scanner,
            Path(temp_dir.name),
            facts_metrics,
        )
        output_facts = facts_provider(output_data)
        output_callable_layout = _callable_layout_from_wasm_facts(output_facts)
    except ValueError as exc:
        temp_dir.cleanup()
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        return 1
    output_memory_min = _memory_import_min(output_data)
    output_table_min = _table_import_min(output_data)
    callable_entry_export_names = (
        tuple(
            _callable_entry_export_name(slot)
            for slot in range(
                output_callable_layout.fixed_prefix_len
                + output_callable_layout.app_entry_count
            )
        )
        if output_callable_layout is not None
        else ()
    )
    reserved_runtime_link_exports = tuple(
        runtime_name
        for _index, runtime_name, _import_name, _arity, dispatch in (
            WASM_RESERVED_RUNTIME_CALLABLES
        )
        if dispatch == "direct"
    )
    split_callable_layout: CallableTableLayout | None = None
    monolithic_callable_layout = output_callable_layout
    deploy_runtime_path: Path | None = None
    if split_runtime:
        if output_callable_layout is None:
            print(
                "Split app is missing explicit callable-table layout authority.",
                file=sys.stderr,
            )
            temp_dir.cleanup()
            return 1
        try:
            deploy_runtime_path = _resolve_deploy_runtime(
                runtime, deploy_runtime_override
            )
            runtime_callable_layout = read_wasm_split_runtime_callable_layout(
                deploy_runtime_path
            )
        except (OSError, ValueError) as exc:
            print(
                f"Split runtime executable callable layout is invalid: {exc}",
                file=sys.stderr,
            )
            temp_dir.cleanup()
            return 1
        try:
            split_callable_layout = _reconcile_split_callable_layout(
                output_callable_layout,
                runtime_callable_layout,
            )
        except ValueError as exc:
            print(f"Split callable-table layout is invalid: {exc}", file=sys.stderr)
            temp_dir.cleanup()
            return 1
        expected_table_min = split_callable_layout.finalized_app_base + (
            split_callable_layout.app_entry_count
        )
        table_boundary_matches = output_table_min == expected_table_min or (
            expected_table_min == 0 and output_table_min is None
        )
        if expected_table_min > 0xFFFF_FFFF or not table_boundary_matches:
            print(
                "Split app table import boundary does not match explicit callable layout: "
                f"import_min={output_table_min}, expected={expected_table_min}",
                file=sys.stderr,
            )
            temp_dir.cleanup()
            return 1
    required_native_direct_symbols = tuple(
        sorted(
            set(_required_native_direct_symbols(output_data))
            | set(_sealed_native_init_symbols(native_objects))
        )
    )
    export_symbol_map = _collect_output_export_symbol_map(output_data)
    callable_entry_export_map = {
        name: export_symbol_map[name]
        for name in callable_entry_export_names
        if name in export_symbol_map
    }
    if len(callable_entry_export_map) != len(callable_entry_export_names):
        missing_callable_exports = sorted(
            set(callable_entry_export_names) - callable_entry_export_map.keys()
        )
        print(
            "Wasm link failed: callable-table entry exports are missing linker symbols: "
            + ", ".join(missing_callable_exports),
            file=sys.stderr,
        )
        return 1
    callable_entry_symbol_names = tuple(
        dict.fromkeys(callable_entry_export_map.values())
    )
    callable_entry_symbol_names_by_slot = tuple(
        callable_entry_export_map[name] for name in callable_entry_export_names
    )
    preserved_output_exports = list(
        dict.fromkeys(
            [
                *_collect_preserved_output_export_names(output_data, output_facts),
                *(
                    entry.canonical_name
                    for entry in _split_runtime_export_contract("app")
                    if entry.kind == 0 and entry.canonical_name in export_symbol_map
                ),
            ]
        )
    )
    user_export_symbol_names = [
        export_symbol_map[name]
        for name in preserved_output_exports
        if name in export_symbol_map
    ]
    rewritten = _rewrite_output_imports(output, runtime_exports, temp_dir)
    if rewritten is None:
        temp_dir.cleanup()
        return 1
    rewritten_path, temp_dir, force_exports = rewritten
    try:
        rewritten_path = _rewrite_required_native_direct_imports(
            rewritten_path,
            required_native_direct_symbols,
            temp_dir,
        )
    except ValueError as exc:
        print(f"Failed to rewrite native direct imports: {exc}", file=sys.stderr)
        return 1
    native_link_inputs, native_force_exports = _rewrite_native_runtime_imports(
        tuple(native_objects),
        runtime_exports,
        temp_dir,
        split_runtime=split_runtime,
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
    linked_rewritten_path = rewritten_path
    linked_native_inputs = native_link_inputs
    if split_runtime and native_objects:
        # The published linked artifact resolves Molt ABI imports in the normal
        # wasm-ld symbol namespace. The deployed split app keeps the same
        # imports as ``molt_runtime`` ABI edges.
        linked_rewrite = _rewrite_runtime_import_module_namespace(
            rewritten_path,
            source_module="molt_runtime",
            target_module="env",
            runtime_exports=runtime_exports,
            temp_dir=temp_dir,
            filename="output_linked_runtime_imports.wasm",
        )
        if linked_rewrite is None:
            return 1
        linked_rewritten_path, linked_force_exports = linked_rewrite
        force_exports.extend(linked_force_exports)
        linked_native_inputs = tuple(native_objects)
    staged_outputs: list[Path] = []
    work_linked = artifact_publish.staged_output_path(linked)
    staged_outputs.append(work_linked)
    app_wasm: Path | None = None
    rt_wasm: Path | None = None
    app_stage: Path | None = None
    rt_stage: Path | None = None
    size_attestation: dict[str, object] = {}
    size_attestation_path: Path | None = None
    size_attestation_stage: Path | None = None

    # When imports were rewritten to prefixed names that are missing from
    # the non-relocatable runtime's export section (e.g. inlined away by
    # LTO), check whether the actual link runtime is a relocatable object
    # that retains the symbols in its linking section.  If so, wasm-ld
    # will resolve them â€” no extra action needed.  If the link runtime is
    # the non-relocatable module itself, we need the relocatable variant.
    if force_exports:
        is_reloc_runtime = runtime.name.endswith("_reloc.wasm")
        if is_reloc_runtime:
            # The relocatable runtime retains all symbols â€” the pre-check
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

    if not runtime.name.endswith("_reloc.wasm"):
        reloc_candidate = runtime.with_name(
            runtime.name.replace(".wasm", "_reloc.wasm")
        )
        if reloc_candidate.exists():
            runtime = reloc_candidate

    # The published linked artifact is a runnable Node/WASI artifact, even when
    # the deployment output is split-runtime. Keep split app deforestation in
    # split_app_cmd below; never link output_linked.wasm against an
    # unreachable runtime stub.
    link_runtime_path = runtime

    preflight_error = _preflight_relocatable_runtime(
        wasm_ld, link_runtime_path, temp_dir
    )
    if preflight_error is not None:
        print(f"Wasm link failed: {preflight_error}", file=sys.stderr)
        return 1

    cmd = [
        wasm_ld,
        "--no-entry",
        "--gc-sections",
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
    if (
        output_callable_layout is not None
        and output_callable_layout.fixed_prefix_len > 0
    ):
        fixed_prefix_end = (
            output_callable_layout.fixed_prefix_base
            + output_callable_layout.fixed_prefix_len
        )
        cmd.insert(cmd.index("--import-table") + 1, f"--table-base={fixed_prefix_end}")
    # Force-export symbols that were rewritten but missing from the
    # non-relocatable runtime â€” they exist in the relocatable runtime
    # and wasm-ld needs to know to keep them in the linked output.
    cmd.extend(
        _deduplicated_export_flags(
            (f"--export-if-defined={sym}" for sym in force_exports),
            (
                f"--export-if-defined={sym}"
                for sym in sorted(
                    _ESSENTIAL_EXPORTS
                    - {"__indirect_function_table", "memory", "molt_main"}
                )
            ),
            (f"--export={sym}" for sym in required_native_direct_symbols),
            (f"--export={sym}" for sym in user_export_symbol_names),
            (
                f"--export-if-defined={name}"
                for name in callable_entry_symbol_names
            ),
            (
                f"--export-if-defined={name}"
                for name in reserved_runtime_link_exports
            ),
        )
    )
    cmd += [
        "-o",
        str(work_linked),
        str(linked_rewritten_path),
        str(link_runtime_path),
    ]
    cmd.extend(str(native_object) for native_object in linked_native_inputs)

    split_linked_app_path: Path | None = None
    split_app_cmd: list[str] | None = None
    split_app_got_runtime_addresses: dict[str, int] = {}
    if split_runtime:
        split_native_inputs = native_link_inputs
        # Keep the active-data-segment alias: it defines each CPython-ABI data
        # symbol (Py_None/Py_False/PyExc_*/Py*_Type/...) so the split app link
        # resolves both a PIC extension's GOT references (numpy, via
        # R_WASM_GLOBAL_INDEX_LEB -> wasm-ld emits a defined
        # `GOT.data.internal.molt_<sym>` global) and a non-PIC extension's
        # absolute references (scipy, via R_WASM_MEMORY_ADDR_SLEB). wasm-ld
        # relocates the alias's zero-size segments into the split app's own
        # region, so every one of those GOT globals initialises to the same
        # app-local placeholder address (ob_type==NULL). The GOT globals are the
        # exact word numpy reads at run time, so after the link we retarget each
        # `GOT.data.internal.molt_<sym>` global to the shared runtime's canonical
        # linear-memory address (see _rewrite_split_app_got_data_globals). This
        # keeps the split app non-PIC (no import-dynamic, so no GOT.func / no
        # loader change) and does not disturb any statically resolved symbol.
        if native_objects:
            try:
                assert deploy_runtime_path is not None
                data_alias_object = _split_runtime_data_alias_object(
                    native_objects=native_link_inputs,
                    deploy_runtime=deploy_runtime_path,
                    temp_dir=temp_dir,
                    reloc_runtime=link_runtime_path,
                )
                split_app_got_runtime_addresses = (
                    _runtime_exported_data_symbol_addresses(
                        deploy_runtime_path.read_bytes()
                    )
                )
            except ValueError as exc:
                print(str(exc), file=sys.stderr)
                return 1
            if data_alias_object is not None:
                split_native_inputs = (*native_link_inputs, data_alias_object)
        split_native_allowlist = _compose_split_runtime_native_allowlist(
            base_allowlist=base_allowlist,
            native_objects=split_native_inputs,
            runtime_exports=runtime_exports,
            temp_dir=temp_dir,
        )
        split_linked_app_path = Path(temp_dir.name) / "app_split_linked.wasm"
        try:
            split_app_data_base = _split_app_global_base(output_data)
        except ValueError as exc:
            print(f"WASM split app memory layout is invalid: {exc}", file=sys.stderr)
            return 1
        assert output_callable_layout is not None
        assert split_callable_layout is not None
        split_app_table_base = split_callable_layout.finalized_app_base
        split_app_prefix = [
            f"--allow-undefined-file={split_native_allowlist}"
            if part.startswith("--allow-undefined-file=")
            else "--no-stack-first"
            if part == "--stack-first"
            else part
            for part in cmd[: cmd.index("-o")]
            if part != "--export=molt_main" and not part.startswith("--table-base=")
        ]
        try:
            split_app_link_args = _split_app_native_link_args(split_native_inputs)
        except ValueError as exc:
            print(f"WASM split app native link failed: {exc}", file=sys.stderr)
            return 1
        split_app_cmd = [
            *split_app_prefix,
            "--import-memory",
            f"--global-base={split_app_data_base}",
            f"--table-base={split_app_table_base}",
            "-o",
            str(split_linked_app_path),
            str(rewritten_path),
            *split_app_link_args,
        ]
        operation_counts["split_app_data_base_bytes"] = split_app_data_base

    res = _run_external_tool(cmd, capture_output=True, text=True)
    whole_artifact_counts_token = _WHOLE_ARTIFACT_OPERATION_COUNTS.set(operation_counts)
    try:
        if res.returncode != 0:
            err = res.stderr.strip() or res.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            return res.returncode
        signature_mismatch = _wasm_ld_signature_mismatch_warning(res.stderr)
        if signature_mismatch is not None:
            print(signature_mismatch, file=sys.stderr)
            return 1
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
        if output_callable_layout is not None:
            try:
                raw_linked_facts = facts_provider(linked_bytes)
                raw_callable_entries = raw_linked_facts.get(
                    "callable_table_entries"
                )
                if not isinstance(raw_callable_entries, list):
                    raise ValueError(
                        "linked WASM facts omitted callable-table entries"
                    )
                raw_occupied_end = max(
                    (
                        int(entry[0]) + 1
                        for entry in raw_callable_entries
                        if isinstance(entry, list)
                        and len(entry) == 4
                        and isinstance(entry[0], int)
                    ),
                    default=output_callable_layout.finalized_app_base,
                )
                monolithic_callable_layout = CallableTableLayout(
                    output_callable_layout.fixed_prefix_base,
                    output_callable_layout.fixed_prefix_len,
                    max(
                        output_callable_layout.finalized_app_base,
                        raw_occupied_end,
                    ),
                    output_callable_layout.app_entry_count,
                )
                monolithic_callable_layout.validate()
                linked_bytes = _install_callable_table_layout(
                    linked_bytes,
                    monolithic_callable_layout,
                    entry_symbol_names=callable_entry_symbol_names_by_slot,
                )
            except ValueError as exc:
                print(
                    f"Failed to publish linked callable table: {exc}",
                    file=sys.stderr,
                )
                return 1
            work_linked.write_bytes(linked_bytes)
        public_export_map = _public_output_export_symbol_map(
            output_data,
            preserved_output_exports=preserved_output_exports,
            export_symbol_map=export_symbol_map,
        )
        restored_linked_bytes = _restore_public_output_exports(
            linked_bytes,
            public_export_map,
            preserved_symbol_names=required_native_direct_symbols,
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
        split_app_contract_keep_set = _split_artifact_contract_keep_set(
            "app",
            public_export_map=public_export_map,
            required_native_direct_symbols=required_native_direct_symbols,
        )
        post_link_preserve_exports = set(split_app_contract_keep_set)
        if not split_runtime:
            post_link_preserve_exports.update(preserved_output_exports)
        linked_bytes = _post_link_optimize(
            linked_bytes,
            reference_data=output_reference,
            preserve_exports=post_link_preserve_exports,
            facts_provider=facts_provider,
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

        if optimize:
            if _run_wasm_opt_via_optimize(
                work_linked,
                level=optimize_level,
                converge=False,
                apply_level=not split_runtime,
                required_exports=(
                    set(_collect_function_exports(linked_bytes))
                    & post_link_preserve_exports
                ),
            ):
                # Re-read after optimization since the file changed on disk
                linked_bytes = work_linked.read_bytes()

        required_table_min = _required_linked_table_min(
            linked_bytes,
            output_table_min,
            facts_provider(linked_bytes),
        )
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
        try:
            updated = _ensure_table_export(linked_bytes)
        except ValueError as exc:
            print(f"Failed to ensure table export: {exc}", file=sys.stderr)
            return 1
        if updated is not None:
            work_linked.write_bytes(updated)
            linked_bytes = updated
        if not any(entry[2] == 2 for entry in _collect_imports(linked_bytes)):
            try:
                updated = _ensure_defined_memory_export(linked_bytes)
            except ValueError as exc:
                print(f"Failed to ensure memory export: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        if not split_runtime:
            updated = _strip_internal_exports(
                linked_bytes,
                preserve_exports=set(preserved_output_exports),
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
        split_runtime_start = time.perf_counter()
        if split_runtime:
            out_dir = split_output_dir or linked.parent
            out_dir.mkdir(parents=True, exist_ok=True)

            app_wasm = out_dir / "app.wasm"
            rt_wasm = out_dir / "molt_runtime.wasm"
            app_stage = artifact_publish.staged_output_path(app_wasm)
            rt_stage = artifact_publish.staged_output_path(rt_wasm)
            size_attestation_path = out_dir / "wasm_size_attestation.json"
            size_attestation_stage = artifact_publish.staged_output_path(
                size_attestation_path
            )
            staged_outputs.extend([app_stage, rt_stage, size_attestation_stage])

            if split_app_cmd is not None:
                assert split_linked_app_path is not None
                split_app_res = _run_external_tool(
                    split_app_cmd,
                    capture_output=True,
                    text=True,
                )
                if split_app_res.returncode != 0:
                    err = split_app_res.stderr.strip() or split_app_res.stdout.strip()
                    if err:
                        print(err, file=sys.stderr)
                    return split_app_res.returncode
                signature_mismatch = _wasm_ld_signature_mismatch_warning(
                    split_app_res.stderr
                )
                if signature_mismatch is not None:
                    print(signature_mismatch, file=sys.stderr)
                    return 1
                if not split_linked_app_path.exists():
                    print(
                        "wasm-ld exited successfully but produced no split app "
                        f"linked output: {split_linked_app_path}",
                        file=sys.stderr,
                    )
                    return 1
                rewritten_data = _read_wasm_bytes_with_retry(split_linked_app_path)
                if not _is_wasm_binary(rewritten_data):
                    print(
                        "wasm-ld produced non-wasm split app linked output "
                        f"({split_linked_app_path}, size={len(rewritten_data)} bytes)",
                        file=sys.stderr,
                    )
                    return 1
                try:
                    output_intervals, linked_intervals = (
                        _validate_split_app_data_layout(
                            output_data,
                            rewritten_data,
                            planned_base=split_app_data_base,
                        )
                    )
                except ValueError as exc:
                    print(
                        f"WASM split app memory layout is invalid: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                operation_counts["split_app_output_data_segment_count"] = len(
                    output_intervals
                )
                output_extent = (
                    output_intervals[0][0],
                    max(end for _start, end in output_intervals),
                )
                operation_counts["split_app_output_data_min_bytes"] = output_extent[0]
                operation_counts["split_app_output_data_end_bytes"] = output_extent[1]
                operation_counts["split_app_linked_data_segment_count"] = len(
                    linked_intervals
                )
                linked_extent = (
                    linked_intervals[0][0],
                    max(end for _start, end in linked_intervals),
                )
                operation_counts["split_app_linked_data_min_bytes"] = linked_extent[0]
                operation_counts["split_app_linked_data_end_bytes"] = linked_extent[1]
                size_attestation["split_app_data_layout"] = {
                    "alignment_bytes": 16,
                    "planned_native_base": split_app_data_base,
                    "output_active_intervals": output_intervals,
                    "output_extent": output_extent,
                    "linked_active_intervals": linked_intervals,
                    "linked_extent": linked_extent,
                }
                try:
                    canonical_rewritten_data = _canonicalize_wasm_ld_output(
                        rewritten_data, description="split app linked"
                    )
                except ValueError as exc:
                    print(str(exc), file=sys.stderr)
                    return 1
                if canonical_rewritten_data != rewritten_data:
                    split_linked_app_path.write_bytes(canonical_rewritten_data)
                    rewritten_data = canonical_rewritten_data
                assert split_callable_layout is not None
                try:
                    raw_split_facts = facts_provider(rewritten_data)
                    raw_split_entries = raw_split_facts.get(
                        "callable_table_entries"
                    )
                    if not isinstance(raw_split_entries, list):
                        raise ValueError(
                            "split-app WASM facts omitted callable-table entries"
                        )
                    raw_split_occupied_end = max(
                        (
                            int(entry[0]) + 1
                            for entry in raw_split_entries
                            if isinstance(entry, list)
                            and len(entry) == 4
                            and isinstance(entry[0], int)
                        ),
                        default=split_callable_layout.finalized_app_base,
                    )
                    split_callable_layout = CallableTableLayout(
                        split_callable_layout.fixed_prefix_base,
                        split_callable_layout.fixed_prefix_len,
                        max(
                            split_callable_layout.finalized_app_base,
                            raw_split_occupied_end,
                        ),
                        split_callable_layout.app_entry_count,
                    )
                    split_callable_layout.validate()
                    rewritten_data = _install_callable_table_layout(
                        rewritten_data,
                        split_callable_layout,
                        entry_symbol_names=callable_entry_symbol_names_by_slot,
                        override_reserved_direct=False,
                    )
                except ValueError as exc:
                    print(
                        f"Failed to publish split-app callable table: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                split_linked_app_path.write_bytes(rewritten_data)
                try:
                    restored_rewritten_data = _restore_split_runtime_contract_exports(
                        rewritten_data,
                        artifact="app",
                        stage="native-link",
                        public_export_map=public_export_map,
                        required_native_direct_symbols=required_native_direct_symbols,
                        operation_counts=operation_counts,
                    )
                except ValueError as exc:
                    print(str(exc), file=sys.stderr)
                    return 1
                if restored_rewritten_data != rewritten_data:
                    split_linked_app_path.write_bytes(restored_rewritten_data)
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
                    failure_artifact = linked.with_name(
                        linked.stem + ".split-native-link-failure.wasm"
                    )
                    failure_artifact.write_bytes(rewritten_data)
                    print(
                        f"Split-runtime native app failure artifact: {failure_artifact}",
                        file=sys.stderr,
                    )
                    print(
                        "Split-runtime native app linker argv: "
                        + shlex.join(split_app_cmd),
                        file=sys.stderr,
                    )
                    return 1
                # Retarget the CPython-ABI GOT data globals wasm-ld emitted for
                # the PIC extension(s) from the app-local placeholder address to
                # the shared runtime's canonical singleton/type/exception copy.
                # Must run before the split-app optimizer strips the name
                # section that identifies each `GOT.data.internal.molt_<sym>`.
                try:
                    rewritten_data, got_retargeted = (
                        _rewrite_split_app_got_data_globals(
                            rewritten_data,
                            runtime_addresses=split_app_got_runtime_addresses,
                            description="Split-runtime native app link",
                        )
                    )
                except ValueError as exc:
                    print(str(exc), file=sys.stderr)
                    return 1
                if got_retargeted:
                    split_linked_app_path.write_bytes(rewritten_data)
                    print(
                        "Split-runtime GOT data bridge: retargeted "
                        f"{got_retargeted} CPython-ABI GOT data global(s) to the "
                        "shared runtime's canonical addresses",
                        file=sys.stderr,
                    )
            optimized_app = _optimize_split_app_module(
                rewritten_data,
                reference_data=output_data,
                optimize=optimize,
                optimize_level=optimize_level,
                contract_keep_set=_split_artifact_contract_keep_set(
                    "app",
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                ),
                attestation=size_attestation,
                operation_counts=operation_counts,
                facts_provider=facts_provider,
            )
            assert split_callable_layout is not None
            required_split_app_table_min = (
                split_callable_layout.finalized_app_base
                + split_callable_layout.app_entry_count
            )
            try:
                updated = _rewrite_table_import_min(
                    optimized_app,
                    required_split_app_table_min,
                )
            except ValueError as exc:
                print(
                    f"Failed to rewrite split app table min: {exc}",
                    file=sys.stderr,
                )
                return 1
            if updated is not None:
                optimized_app = updated
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
            try:
                optimized_app = _restore_split_runtime_contract_exports(
                    optimized_app,
                    artifact="app",
                    stage="optimized-app",
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                    operation_counts=operation_counts,
                )
            except ValueError as exc:
                print(str(exc), file=sys.stderr)
                return 1
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
            assert deploy_runtime_path is not None
            deploy_runtime = deploy_runtime_path

            # Tree-shake the runtime against its OWN canonical, app-independent
            # public export surface â€” never against the current app's import
            # subset.  The shared runtime is a single artifact cached once by the
            # CDN and reused by every app, so it MUST be byte-identical across
            # builds (see test_runtime_hash_identical).  Shaking by per-app
            # imports made appA (a class) and appB (fib) keep different export
            # sets, which drove wasm-opt's dead-code GC to retain different
            # functions and produced divergent runtime bytes â€” silently breaking
            # CDN cacheability.  Keeping the full canonical ABI lets wasm-opt
            # strip only functions unreachable from ANY public export (debug
            # tables, producers, dead internal helpers) while every app's import
            # surface still resolves.  Per-app shrinkage comes entirely from
            # app.wasm (the intrinsic manifest + wasm-ld --gc-sections), which is
            # the correct split-runtime model: one large cached runtime + a tiny
            # per-app payload.
            full_rt_size = deploy_runtime.stat().st_size
            deploy_runtime_data = deploy_runtime.read_bytes()
            size_attestation["runtime_before"] = wasm_metrics(deploy_runtime_data)
            try:
                canonical_required_exports = _canonical_split_runtime_required_exports(
                    deploy_runtime_data
                )
                app_imports = _collect_module_imports(
                    app_stage.read_bytes(), "molt_runtime"
                )
                missing_runtime_imports: list[str] = []
                for name in app_imports:
                    export_name = wasm_split_runtime_export_name_for_import(name)
                    if (
                        export_name is not None
                        and export_name in canonical_required_exports
                    ):
                        continue
                    if export_name is None and name in canonical_required_exports:
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
                    # shipping the full (un-shaken) runtime â€” which is itself
                    # byte-identical across apps, so CDN cacheability survives
                    # the fallback â€” rather than papering over the mismatch with
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
                    deploy_runtime_data,
                    canonical_required_exports,
                    facts_provider=facts_provider,
                    operation_counts=operation_counts,
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
        if split_runtime:
            phase_timings_ms["split_runtime_processing"] = round(
                max(0.0, (time.perf_counter() - split_runtime_start) * 1000.0), 6
            )

        validation_start = time.perf_counter()
        if freestanding:
            if not _validate_freestanding(linked_bytes):
                return 1
        phase_timings_ms["fail_closed_validation"] = round(
            max(0.0, (time.perf_counter() - validation_start) * 1000.0), 6
        )
        strip_start = time.perf_counter()
        stripped_debug = _strip_debug_sections(linked_bytes)
        if stripped_debug is not None:
            work_linked.write_bytes(stripped_debug)
            linked_bytes = stripped_debug
        canonical_sections = _canonicalize_standard_section_order(linked_bytes)
        if canonical_sections is not None:
            work_linked.write_bytes(canonical_sections)
            linked_bytes = canonical_sections
        published_linked = strip_wasm_publication_sections(
            work_linked.read_bytes(),
            final_artifact=True,
            preserve_debug=preserve_debug_sections,
        )
        work_linked.write_bytes(published_linked)
        try:
            _publish_rust_wasm_link_facts(
                wasm_facts_scanner,
                work_linked,
                layout=monolithic_callable_layout,
            )
        except ValueError as exc:
            print(
                f"Failed to attest final linked callable table: {exc}", file=sys.stderr
            )
            return 1
        if split_runtime:
            assert app_stage is not None
            assert rt_stage is not None
            try:
                published_app = _strip_and_restore_split_artifact(
                    app_stage.read_bytes(),
                    artifact="app",
                    stage="publication-strip",
                    preserve_debug=preserve_debug_sections,
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                    operation_counts=operation_counts,
                )
            except ValueError as exc:
                print(str(exc), file=sys.stderr)
                return 1
            published_runtime = strip_wasm_publication_sections(
                rt_stage.read_bytes(),
                final_artifact=True,
                preserve_debug=preserve_debug_sections,
            )
            app_stage.write_bytes(published_app)
            rt_stage.write_bytes(published_runtime)
            try:
                assert split_callable_layout is not None
                app_facts = _publish_rust_wasm_link_facts(
                    wasm_facts_scanner,
                    app_stage,
                    layout=split_callable_layout,
                    role="app",
                )
                final_split_callable_layout = _callable_layout_from_wasm_facts(
                    app_facts
                )
                if final_split_callable_layout is None:
                    raise ValueError(
                        "final split app publication omitted callable-table layout"
                    )
                _publish_rust_wasm_link_facts(
                    wasm_facts_scanner,
                    rt_stage,
                    layout=final_split_callable_layout,
                    role="runtime",
                )
            except ValueError as exc:
                print(
                    f"Failed to attest final split callable table: {exc}",
                    file=sys.stderr,
                )
                return 1
            assert size_attestation_stage is not None
            size_attestation["published"] = {
                "app": wasm_metrics(app_stage.read_bytes()),
                "runtime": wasm_metrics(rt_stage.read_bytes()),
            }
            size_attestation_stage.write_text(
                json.dumps(size_attestation, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
        phase_timings_ms["wasm_strip"] = round(
            max(0.0, (time.perf_counter() - strip_start) * 1000.0), 6
        )

        linked_ok = _validate_linked(work_linked)
        if not linked_ok:
            failed_validation = linked.with_name(
                f"{linked.stem}.failed-validation.wasm"
            )
            failed_validation.write_bytes(work_linked.read_bytes())
            print(
                f"Preserved failed linked validation artifact: {failed_validation}",
                file=sys.stderr,
            )
            if split_runtime:
                print(
                    "Linked wasm validation failed before split-runtime publication; "
                    "failing because linked validation is the canonical table/memory/import guard.",
                    file=sys.stderr,
                )
            return 1

        publish_pairs = [(work_linked, linked)]
        validation_start = time.perf_counter()
        if split_runtime:
            assert app_stage is not None
            assert rt_stage is not None
            assert app_wasm is not None
            assert rt_wasm is not None
            assert size_attestation_path is not None
            assert size_attestation_stage is not None
            if not _validate_split_runtime_outputs(app_stage, rt_stage):
                return 1
            publish_pairs.extend(
                [
                    (rt_stage, rt_wasm),
                    (app_stage, app_wasm),
                    (size_attestation_stage, size_attestation_path),
                ]
            )
        phase_timings_ms["fail_closed_validation"] = round(
            phase_timings_ms.get("fail_closed_validation", 0.0)
            + max(0.0, (time.perf_counter() - validation_start) * 1000.0),
            6,
        )
        try:
            artifact_publish.publish_validated_outputs(publish_pairs)
        except OSError as exc:
            print(f"Failed to publish wasm linker outputs: {exc}", file=sys.stderr)
            return 1

        return 0
    finally:
        if split_runtime and "split_runtime_processing" not in phase_timings_ms:
            phase_timings_ms["split_runtime_processing"] = round(
                max(0.0, (time.perf_counter() - split_runtime_start) * 1000.0), 6
            )
        phase_timings_ms.setdefault("wasm_strip", 0.0)
        phase_timings_ms.setdefault("fail_closed_validation", 0.0)
        phase_timings_ms.update(
            {name: round(value, 6) for name, value in facts_metrics.items()}
        )
        phase_timings_ms.update(operation_counts)
        phase_timings_ms["wasm_link_total"] = round(
            max(0.0, (time.perf_counter() - total_start) * 1000.0), 6
        )
        if phase_timings_file is not None:
            phase_timings_file.parent.mkdir(parents=True, exist_ok=True)
            phase_timings_file.write_text(
                json.dumps(phase_timings_ms, sort_keys=True) + "\n",
                encoding="utf-8",
            )
        _WHOLE_ARTIFACT_OPERATION_COUNTS.reset(whole_artifact_counts_token)
        for staged_output in staged_outputs:
            with contextlib.suppress(OSError):
                staged_output.unlink()
        temp_dir.cleanup()


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
    preserve_debug_sections: bool = False,
    phase_timings_file: Path | None = None,
    wasm_facts_scanner: Path,
) -> int:
    for native_object in native_objects:
        if not native_object.exists():
            print(f"Native WASM link input not found: {native_object}", file=sys.stderr)
            return 1
    try:
        with tempfile.TemporaryDirectory(prefix="molt-wasm-link-custody-") as tmp:
            snapshot_root = Path(tmp)
            runtime_snapshot_root = snapshot_root / "runtime-pair"
            runtime_snapshot = _snapshot_link_input(
                runtime,
                runtime_snapshot_root,
                label="selected",
                accept_path=(
                    lambda path: (
                        _preflight_relocatable_runtime(
                            wasm_ld, path, type("CustodyDir", (), {"name": tmp})()
                        )
                        is None
                    )
                )
                if runtime.name.endswith("_reloc.wasm")
                else None,
                retry_delay_seconds=0.25,
            )
            runtime_snapshot = (
                runtime_snapshot_root / runtime_snapshot.parent.name / runtime.name
            )
            for sibling_name in {
                runtime.name.replace("_reloc.wasm", ".wasm"),
                runtime.name.replace(".wasm", "_reloc.wasm"),
            }:
                sibling = runtime.with_name(sibling_name)
                if sibling != runtime and sibling.exists():
                    sibling_snapshot = _snapshot_link_input(
                        sibling,
                        runtime_snapshot_root,
                        label=f"sibling-{sibling_name}",
                        accept_path=(
                            lambda path: (
                                _preflight_relocatable_runtime(
                                    wasm_ld,
                                    path,
                                    type("CustodyDir", (), {"name": tmp})(),
                                )
                                is None
                            )
                        )
                        if sibling.name.endswith("_reloc.wasm")
                        else None,
                        retry_delay_seconds=0.25,
                    )
                    target = runtime_snapshot.parent / sibling.name
                    if sibling_snapshot != target:
                        target.write_bytes(sibling_snapshot.read_bytes())
            output_snapshot = _snapshot_link_input(
                output,
                snapshot_root,
                label="app",
                accept=_is_wasm_binary,
            )
            native_snapshot_list: list[Path] = []
            for index, native_object in enumerate(native_objects):
                native_snapshot = _snapshot_link_input(
                    native_object,
                    snapshot_root,
                    label=f"native-{index}",
                )
                manifest = native_object.with_name(
                    native_object.name + ".extension_manifest.json"
                )
                if manifest.exists():
                    manifest_snapshot = _snapshot_link_input(
                        manifest,
                        snapshot_root,
                        label=f"native-{index}-manifest",
                    )
                    (native_snapshot.parent / manifest.name).write_bytes(
                        manifest_snapshot.read_bytes()
                    )
                native_snapshot_list.append(native_snapshot)
            native_snapshots = tuple(native_snapshot_list)
            deploy_runtime = _resolve_deploy_runtime(runtime, deploy_runtime_override)
            deploy_runtime_snapshot = _snapshot_link_input(
                deploy_runtime,
                snapshot_root,
                label="deploy-runtime",
            )
            return _run_wasm_ld_with_custodied_inputs(
                wasm_ld,
                runtime_snapshot,
                output_snapshot,
                linked,
                allowlist_override=allowlist_override,
                optimize=optimize,
                optimize_level=optimize_level,
                freestanding=freestanding,
                split_runtime=split_runtime,
                split_output_dir=split_output_dir,
                deploy_runtime_override=deploy_runtime_snapshot,
                native_objects=native_snapshots,
                preserve_debug_sections=preserve_debug_sections,
                phase_timings_file=phase_timings_file,
                wasm_facts_scanner=wasm_facts_scanner,
            )
    except OSError as exc:
        print(f"Failed to establish wasm linker input custody: {exc}", file=sys.stderr)
        return 1


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
    parser.add_argument(
        "--preserve-debug-sections",
        action="store_true",
        help="Preserve name and DWARF sections while still removing final-link metadata",
    )
    parser.add_argument("--phase-timings-file", type=Path, default=None)
    parser.add_argument("--wasm-facts-scanner", type=Path, required=True)
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

    wasm_ld = _find_wasm_ld()
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
        preserve_debug_sections=args.preserve_debug_sections,
        phase_timings_file=args.phase_timings_file,
        wasm_facts_scanner=args.wasm_facts_scanner,
    )


if __name__ == "__main__":
    raise SystemExit(main())

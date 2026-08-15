#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import contextvars
import functools
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any, Literal, cast

TOOLS_ROOT = Path(__file__).resolve().parent
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
SRC_ROOT = TOOLS_ROOT.parent / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

import harness_memory_guard  # noqa: E402
import artifact_publish as artifact_publish  # noqa: E402
from command_execution import CommandExecutor  # noqa: E402
from wasm_optimize import find_wasm_opt, optimize as optimize_wasm  # noqa: E402
from wasm_metrics import wasm_metrics as wasm_metrics  # noqa: E402
from molt.cli import wasm_toolchain  # noqa: E402
from molt.cli.app_export_contract import (  # noqa: E402
    app_export_call_abi,
    excluded_app_symbols,
    exported_app_symbols,
    load_app_export_contract as load_app_export_contract,
)
from molt.cli.external_link_providers import (  # noqa: E402
    WASM_COMPILER_RT_LINK_IMPORT_CLASS,
    WASM_LIBCXX_LINK_IMPORT_CLASS,
    WASM_LIBC_LINK_IMPORT_CLASS,
    wasm_external_link_provider_symbols,
)
from molt.cli.runtime_build_identity import RuntimeBuildIdentity  # noqa: E402
from molt.cli.source_extension_link_requirements import (  # noqa: E402
    source_extension_link_requirements as source_extension_link_requirements,
)
from molt.cli.runtime_wasm_generation import (  # noqa: E402
    RuntimeWasmGeneration,
    read_runtime_wasm_generation,
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
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS,
    WASM_OUTPUT_RUNTIME_EXPORT_ALIASES as WASM_OUTPUT_RUNTIME_EXPORT_ALIASES,
    WASM_RESERVED_RUNTIME_CALLABLES as WASM_RESERVED_RUNTIME_CALLABLES,
)
from molt.wasm_artifact import (  # noqa: E402
    WASM_EXTERN_KIND_GLOBAL,
    WASM_VALUE_TYPE_I32,
    parse_wasm_defined_globals,
    parse_wasm_exports,
    read_wasm_split_runtime_callable_layout as read_wasm_split_runtime_callable_layout,
    strip_wasm_publication_sections as _strip_wasm_publication_sections_raw,
)
from molt.wasm_linking_symbols import parse_wasm_linking_symbols  # noqa: E402
from molt.wasm_optimization import WASM_OPT_LEVELS, wasm_link_policy  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__)

RuntimeLinkInputRole = Literal["reloc", "shared"]

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
    _ensure_function_exports_by_symbol_names as _ensure_function_exports_by_symbol_names,
    _inject_app_export_adapters as _inject_app_export_adapters,
    _validate_app_export_adapters as _validate_app_export_adapters,
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
import wasm_link_callable_table as _callable_table  # noqa: E402
import wasm_link_fact_provider as _link_facts  # noqa: E402
import wasm_link_pipeline as _link_pipeline  # noqa: E402
import wasm_link_validation as _link_validation  # noqa: E402


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
                    subprocess.list2cmdline([argument]) for argument in guarded_cmd[1:]
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


def _verify_runtime_generation(
    *,
    reloc: Path,
    shared: Path,
    generation_manifest: Path,
    expected_identity: Path,
) -> RuntimeWasmGeneration:
    """Verify both runtime members against caller-produced trusted identity."""

    for path in (reloc, shared, generation_manifest, expected_identity):
        if ".." in path.parts:
            raise SystemExit(f"Runtime custody path contains '..': {path}")
    try:
        payload = json.loads(expected_identity.read_text(encoding="utf-8"))
        if (
            not isinstance(payload, dict)
            or payload.get("schema") != "molt.runtime-wasm-expected-pair.v1"
        ):
            raise ValueError("expected pair schema is invalid")
        shared_identity = RuntimeBuildIdentity.from_dict(payload.get("shared"))
        reloc_identity = RuntimeBuildIdentity.from_dict(payload.get("reloc"))
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        raise SystemExit(f"Trusted runtime pair identity is invalid: {exc}") from exc
    generation = read_runtime_wasm_generation(
        generation_manifest,
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    if generation is None:
        raise SystemExit(
            "Runtime generation does not match the trusted caller identity: "
            f"{generation_manifest}"
        )
    if reloc.resolve(strict=False) != generation.reloc.resolve(
        strict=False
    ) or shared.resolve(strict=False) != generation.shared.resolve(strict=False):
        raise SystemExit(
            "Runtime link inputs are not the immutable members selected by "
            f"{generation_manifest}"
        )
    return generation


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
        parsed = [
            (symbol.flags, symbol.index, symbol.name, "")
            for symbol in parse_wasm_linking_symbols(data).function_symbols
            if symbol.index is not None
        ]
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


_TREE_SHAKE_RUNTIME_CACHE_SCHEMA = "runtime-tree-shake-v5"
_SPLIT_APP_OPTIMIZE_CACHE_SCHEMA = "split-app-optimize-v3"
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
    "publish_errors",
)
_WASM_OPT_CACHE_METRIC_SUFFIXES = (
    "optimizer_wall_ms",
    "optimizer_peak_rss_kb",
    "optimizer_peak_total_rss_kb",
    "timeouts",
    "failures",
    "identity_errors",
)


def _empty_wasm_link_cache_metrics() -> dict[str, int | float]:
    metrics = {
        f"{prefix}_{suffix}": 0
        for prefix in ("runtime_tree_shake_cache", "split_app_optimize_cache")
        for suffix in _WASM_LINK_CACHE_METRIC_SUFFIXES
    }
    metrics.update(
        {
            f"split_app_optimize_cache_{suffix}": 0
            for suffix in _WASM_OPT_CACHE_METRIC_SUFFIXES
        }
    )
    return metrics


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
    status = attestation.get("status")
    if status == "timeout":
        _cache_metric_add(metrics, f"{prefix}_timeouts", 1)
    elif attestation.get("ok") is False:
        _cache_metric_add(metrics, f"{prefix}_failures", 1)
        if status == "identity-error":
            _cache_metric_add(metrics, f"{prefix}_identity_errors", 1)


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
    facts_authority_digest: str,
    wasm_opt_identity: tuple[str, str, str] | None = None,
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
        if wasm_opt_identity is None:
            return None
        _resolved_path, executable_sha256, _version = wasm_opt_identity
        hasher.update(b"\0wasm-opt-sha256\0")
        hasher.update(executable_sha256.encode("ascii"))
    hasher.update(b"\0tool\0")
    hasher.update(_wasm_link_transform_authority_digest().encode("ascii"))
    hasher.update(b"\0facts-authority\0")
    hasher.update(facts_authority_digest.encode("ascii"))
    return hasher.hexdigest()


@functools.lru_cache(maxsize=1)
def _wasm_link_transform_authority_digest() -> str:
    return _transform_authority_digest(_wasm_link_transform_authority_paths())


def _wasm_link_transform_authority_paths() -> tuple[Path, ...]:
    repo_root = TOOLS_ROOT.parent
    return tuple(
        repo_root / relative
        for relative in (
            "tools/artifact_publish.py",
            "tools/wasm_link.py",
            "tools/wasm_link_edit.py",
            "tools/wasm_link_facts.py",
            "tools/wasm_link_format.py",
            "tools/wasm_link_optimize.py",
            "tools/wasm_optimize.py",
            "src/molt/wasm_artifact.py",
            "src/molt/wasm_linking_symbols.py",
            "src/molt/wasm_optimization.py",
        )
    )


def _transform_authority_digest(paths: Sequence[Path]) -> str:
    hasher = hashlib.sha256()
    for path in paths:
        try:
            authority_name = path.resolve().relative_to(TOOLS_ROOT.parent).as_posix()
        except ValueError:
            authority_name = path.resolve().as_posix()
        hasher.update(authority_name.encode("utf-8"))
        hasher.update(b"\0")
        hasher.update(path.read_bytes())
        hasher.update(b"\0")
    return hasher.hexdigest()


def _wasm_facts_cache_authority_digest(
    facts_provider: Callable[[bytes], dict[str, object]],
    data: bytes,
) -> str:
    """Bind caches to both scanner custody and its facts for this input."""

    provider_identity = getattr(
        facts_provider,
        "_molt_wasm_facts_authority_digest",
        "fixture-or-legacy-provider",
    )
    facts = facts_provider(data)
    encoded_facts = json.dumps(
        facts,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    hasher = hashlib.sha256()
    hasher.update(str(provider_identity).encode("utf-8"))
    hasher.update(b"\0")
    hasher.update(encoded_facts)
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


def _file_stat_identity(stat: os.stat_result) -> tuple[int, int, int, int]:
    return (stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns, stat.st_ino)


@functools.lru_cache(maxsize=16)
def _stable_file_sha256_cached(
    resolved_path: str,
    stat_identity: tuple[int, int, int, int],
) -> str | None:
    path = Path(resolved_path)
    try:
        before = path.stat()
        if _file_stat_identity(before) != stat_identity:
            return None
        hasher = hashlib.sha256()
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                hasher.update(chunk)
        after = path.stat()
    except OSError:
        return None
    if _file_stat_identity(after) != stat_identity:
        return None
    return hasher.hexdigest()


def _wasm_opt_executable_identity(
    executable: str,
) -> tuple[str, str, str] | None:
    """Return immutable Binaryen custody identity or disable cache admission."""

    try:
        path = Path(executable).expanduser().resolve(strict=True)
        stat_identity = _file_stat_identity(path.stat())
    except OSError:
        return None
    digest = _stable_file_sha256_cached(os.fspath(path), stat_identity)
    if digest is None:
        return None
    return os.fspath(path), digest, _wasm_opt_version(os.fspath(path))


def _tree_shake_runtime_cache_key(
    *,
    runtime_data: bytes,
    normalized_required_exports: set[str],
    facts_authority_digest: str,
) -> str:
    hasher = hashlib.sha256()
    hasher.update(_TREE_SHAKE_RUNTIME_CACHE_SCHEMA.encode("ascii"))
    hasher.update(b"\0")
    hasher.update(runtime_data)
    hasher.update(b"\0exports\0")
    for name in sorted(normalized_required_exports):
        hasher.update(name.encode("utf-8"))
        hasher.update(b"\0")
    hasher.update(b"\0tool\0")
    hasher.update(_wasm_link_transform_authority_digest().encode("ascii"))
    hasher.update(b"\0facts-authority\0")
    hasher.update(facts_authority_digest.encode("ascii"))
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
    for symbol in parse_wasm_linking_symbols(data).data_symbols:
        if undefined != (not symbol.is_defined):
            continue
        yield symbol.name, symbol.data_offset, symbol.size


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
    """Map canonical data-symbol name -> runtime linear-memory address.

    wasm-ld exports a *defined data symbol* (requested via
    ``--export[-if-defined]``) as an immutable ``i32`` global whose init value
    is the symbol's absolute linear-memory address. Reading those exports from
    the deploy runtime is the authoritative source for the runtime's canonical
    singleton/type/exception addresses â€” the split app links against imported
    memory and shares those addresses at run time.

    Published runtimes may expose the address global under either its canonical
    link name (``PyType_Type``) or its split-runtime export name
    (``molt_PyType_Type``). Normalize the complete generated CPython-ABI data
    family here, once, so alias-object construction and post-link GOT retargeting
    cannot drift into competing name authorities. If both spellings are present
    they must identify the same address; disagreement is corruption, not a
    precedence choice.
    """
    cpython_data_symbols = frozenset(wasm_cpython_abi_data_symbol_names())
    globals_by_index = {
        global_.index: global_ for global_ in parse_wasm_defined_globals(runtime_data)
    }
    addresses: dict[str, int] = {}
    for export in parse_wasm_exports(runtime_data):
        canonical = wasm_split_runtime_import_name_for_export(export.name)
        if canonical not in cpython_data_symbols:
            canonical = export.name
        if canonical not in cpython_data_symbols:
            continue
        if export.kind != WASM_EXTERN_KIND_GLOBAL:
            raise ValueError(
                "runtime CPython-ABI data symbol must be exported as a global: "
                f"{export.name} has wasm export kind {export.kind}"
            )
        global_ = globals_by_index.get(export.index)
        if global_ is None:
            raise ValueError(
                "runtime CPython-ABI data symbol must reference a defined global: "
                f"{export.name} references global index {export.index}"
            )
        if global_.value_type != WASM_VALUE_TYPE_I32:
            raise ValueError(
                "runtime CPython-ABI data symbol address global must have i32 type: "
                f"{export.name} has value type 0x{global_.value_type:02x}"
            )
        if global_.mutable:
            raise ValueError(
                "runtime CPython-ABI data symbol address global must be immutable: "
                f"{export.name}"
            )
        if global_.initializer_opcode != 0x41 or global_.i32_const is None:
            raise ValueError(
                "runtime CPython-ABI data symbol address global must use a direct "
                f"i32.const initializer: {export.name}"
            )
        value = global_.i32_const
        previous = addresses.get(canonical)
        if previous is not None and previous != value:
            raise ValueError(
                "runtime exports conflicting addresses for canonical "
                f"CPython-ABI data symbol {canonical}: "
                f"{previous} != {value} (export {export.name})"
            )
        addresses[canonical] = value
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


def _resolve_deploy_runtime(deploy_runtime_override: Path | None) -> Path:
    """Resolve the deploy-ready (non-relocatable) runtime shared at run time.

    Mirrors the split-runtime publication path: honor
    ``MOLT_WASM_DEPLOY_RUNTIME`` / an explicit override. The returned artifact
    is the one whose linear-memory data addresses the split app must agree with.
    Trusted immutable members have content suffixes, so deriving one member from
    another member's filename is not an admissible custody authority.
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
    raise FileNotFoundError(
        "split deploy runtime requires one explicit trusted shared member"
    )


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


_APP_EXPORT_IDENTITY_PREFIX = "__molt_app_export_identity__"


def _app_export_identity_maps(
    adapter_symbol_map: Mapping[str, str],
    target_symbol_map: Mapping[str, str],
) -> tuple[dict[str, str], dict[str, str], dict[str, str]]:
    """Create optimizer-stable exports for exact adapter call identity.

    Binaryen may discard linker/name metadata and renumber functions. Temporary
    exports are WebAssembly semantic roots, so their post-optimizer indices are
    the durable identity channel. They are removed after exact validation and
    never enter a published artifact.
    """

    adapter_identity: dict[str, str] = {}
    target_identity: dict[str, str] = {}
    identity_exports: dict[str, str] = {}
    for public_name, adapter_symbol in adapter_symbol_map.items():
        token = hashlib.sha256(public_name.encode("utf-8")).hexdigest()
        adapter_export = f"{_APP_EXPORT_IDENTITY_PREFIX}adapter_{token}"
        target_export = f"{_APP_EXPORT_IDENTITY_PREFIX}target_{token}"
        target_symbol = target_symbol_map.get(public_name)
        if target_symbol is None:
            raise ValueError(f"app export {public_name!r} has no raw-target identity")
        adapter_identity[public_name] = adapter_export
        target_identity[public_name] = target_export
        identity_exports[adapter_export] = adapter_symbol
        identity_exports[target_export] = target_symbol
    return (
        adapter_identity,
        target_identity,
        identity_exports,
    )


def _strip_app_export_identity_markers(
    data: bytes,
    *,
    identity_exports: Mapping[str, str],
    preserve_exports: set[str],
) -> bytes:
    """Remove optimizer identity roots and reject any publication leak."""

    updated = _strip_internal_exports(data, preserve_exports=preserve_exports)
    stripped = data if updated is None else updated
    leaked = sorted(set(identity_exports) & set(_collect_function_exports(stripped)))
    if leaked:
        raise ValueError(
            "internal adapter identity exports leaked: " + ", ".join(leaked)
        )
    return stripped


def _publish_app_export_identity_markers(
    data: bytes,
    *,
    public_export_names: Sequence[str],
    adapter_symbol_map: Mapping[str, str],
    target_symbol_map: Mapping[str, str],
    identity_exports: Mapping[str, str],
) -> bytes:
    """Prove exact pre-optimizer identities, then publish durable markers."""

    _validate_app_export_adapters(
        data,
        public_export_names,
        adapter_symbol_map=adapter_symbol_map,
        target_symbol_map=target_symbol_map,
    )
    updated = _ensure_function_exports_by_symbol_names(
        data,
        dict(identity_exports),
    )
    marked = data if updated is None else updated
    missing = sorted(set(identity_exports) - set(_collect_function_exports(marked)))
    if missing:
        raise ValueError("optimizer identity exports are absent: " + ", ".join(missing))
    return marked


def _app_export_surface_error(
    data: bytes,
    contract: Mapping[str, object] | None,
    *,
    stage: str,
) -> str | None:
    if contract is None:
        return None
    exports = set(_collect_function_exports(data))
    expected = set(exported_app_symbols(contract))
    missing = sorted(expected - exports)
    forbidden = sorted(set(excluded_app_symbols(contract)) & exports)
    details: list[str] = []
    if missing:
        details.append("missing=" + ",".join(missing))
    if forbidden:
        details.append("excluded-exported=" + ",".join(forbidden))
    if not missing:
        try:
            call_abi = app_export_call_abi(contract)
            adapter = call_abi.get("adapter")
            if (
                isinstance(adapter, Mapping)
                and adapter.get("strategy") == "forward-owned-result"
            ):
                _validate_app_export_adapters(data, tuple(sorted(expected)))
        except ValueError as exc:
            details.append(f"adapter-invalid={exc}")
    if not details:
        return None
    return f"app callable export contract mismatch at {stage}: " + "; ".join(details)


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
    (plus memory/table/global exports which are always kept), then applies the
    linker's verified structural cleanup. Cargo/LLVM code generation owns the
    current shared-runtime body optimization. Any future Binaryen runtime pass
    belongs in runtime-generation custody; this app-link stage never runs a
    second whole-runtime Binaryen pipeline.
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
    # Host-facing publication roots have one generated authority in
    # ``output_export_policy.essential_exports``.  Keeping a second literal
    # list here previously let linked-result decoders lose ``molt_len`` and
    # ``molt_index`` while a superficially similar subset remained exported.
    normalized_required_exports.update(_ESSENTIAL_EXPORTS)
    raw_dynamic_exports = os.environ.get(
        "MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS", ""
    ).strip()
    if raw_dynamic_exports:
        normalized_required_exports.update(
            name.strip() for name in raw_dynamic_exports.split(",") if name.strip()
        )

    # The shared runtime is app-independent and retains the canonical public ABI.
    # Cargo/LLVM runtime generation owns body optimization. The linker owns only
    # export filtering and its verified structural post-link cleanup; the old
    # second Binaryen lane spent minutes reoptimizing the same full export graph
    # and could delete the public ABI.
    cache_started = time.perf_counter()
    metric_prefix = "runtime_tree_shake_cache"
    _cache_metric_add(operation_counts, f"{metric_prefix}_requests", 1)
    facts_authority_digest = _wasm_facts_cache_authority_digest(
        facts_provider,
        runtime_data,
    )
    cache_key = _tree_shake_runtime_cache_key(
        runtime_data=runtime_data,
        normalized_required_exports=normalized_required_exports,
        facts_authority_digest=facts_authority_digest,
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
            print(f"Runtime tree-shake cache hit: {cache_entry.root}", file=sys.stderr)
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

    with _locked_wasm_link_cache_entry(cache_entry) as lock_wait_ms:
        _cache_metric_add(
            operation_counts, f"{metric_prefix}_lock_wait_ms", lock_wait_ms
        )
        cached = _read_wasm_link_cache_entry(cache_entry)
        if cached.data is not None:
            _cache_metric_add(operation_counts, f"{metric_prefix}_hits", 1)
            _cache_metric_add(
                operation_counts, f"{metric_prefix}_bytes_read", cached.bytes_read
            )
            return cached.data
        _publish_wasm_link_cache_result(
            cache_entry,
            optimized_baseline,
            metrics=operation_counts,
            metric_prefix=metric_prefix,
            label="Runtime structural optimize",
            payload={
                "result_kind": "runtime-generation-plus-structural-link-cleanup",
                "kept_exports": kept_exports,
                "stripped_exports": stripped_exports,
            },
        )
    _cache_metric_add(
        operation_counts,
        f"{metric_prefix}_wall_ms",
        (time.perf_counter() - cache_started) * 1000.0,
    )
    return optimized_baseline


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
    cache_started = time.perf_counter()
    metric_prefix = "split_app_optimize_cache"
    _cache_metric_add(operation_counts, f"{metric_prefix}_requests", 1)
    facts_authority_digest = _wasm_facts_cache_authority_digest(
        facts_provider,
        app_data,
    )
    wasm_opt_identity = None
    if optimize:
        wasm_opt_path = find_wasm_opt()
        wasm_opt_identity = (
            _wasm_opt_executable_identity(wasm_opt_path)
            if wasm_opt_path is not None
            else None
        )
        if wasm_opt_identity is None:
            _cache_metric_add(operation_counts, f"{metric_prefix}_identity_errors", 1)
            _cache_metric_add(
                operation_counts,
                f"{metric_prefix}_wall_ms",
                (time.perf_counter() - cache_started) * 1000.0,
            )
            raise RuntimeError(
                "required split-app wasm optimization has no stable executable identity"
            )
    cache_key = _split_app_optimize_cache_key(
        app_data=app_data,
        reference_data=reference_data,
        optimize=optimize,
        optimize_level=optimize_level,
        contract_keep_set=contract_keep_set,
        facts_authority_digest=facts_authority_digest,
        wasm_opt_identity=wasm_opt_identity,
    )
    assert cache_key is not None
    cache_entry = _wasm_link_cache_entry(
        "split_app_optimize",
        _SPLIT_APP_OPTIMIZE_CACHE_SCHEMA,
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
            assert wasm_opt_identity is not None
            optimizer_policy = wasm_link_policy(optimize_level)
            with tempfile.TemporaryDirectory(prefix="molt-split-app-opt-") as tmp:
                app_path = Path(tmp) / "app_split_preopt.wasm"
                app_path.write_bytes(optimized)
                required_function_exports = (
                    set(_collect_function_exports(optimized)) & contract_keep_set
                )
                _cache_metric_add(operation_counts, "split_app_wasm_opt_runs", 1)
                optimizer_ok = _run_wasm_opt_via_optimize(
                    app_path,
                    level=optimizer_policy.level,
                    converge=optimizer_policy.converge,
                    required_exports=required_function_exports,
                    apply_level=optimizer_policy.apply_level,
                    extra_passes=optimizer_policy.extra_passes,
                    attestation=active_attestation,
                )
                _record_wasm_opt_attestation_cache_metrics(
                    operation_counts, metric_prefix, active_attestation
                )
                if optimizer_ok:
                    result = app_path.read_bytes()
                else:
                    failure = str(
                        active_attestation.get("error", "unknown optimizer failure")
                    )
                    raise RuntimeError(
                        f"required split-app wasm optimization failed: {failure}"
                    )
                if (
                    active_attestation.get("wasm_opt_path") != wasm_opt_identity[0]
                    or active_attestation.get("wasm_opt_sha256") != wasm_opt_identity[1]
                ):
                    raise RuntimeError(
                        "required split-app wasm optimization crossed executable identity"
                    )
        cache_payload = dict(active_attestation)
        cache_payload["cache_hit"] = False
        if optimize:
            assert wasm_opt_identity is not None
            cache_payload.update(
                {
                    "wasm_opt_path": wasm_opt_identity[0],
                    "wasm_opt_sha256": wasm_opt_identity[1],
                    "wasm_opt_version": wasm_opt_identity[2],
                }
            )
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


_link_validation.configure_api(globals())
_canonicalize_wasm_ld_output = _link_validation._canonicalize_wasm_ld_output
_validate_freestanding = _link_validation._validate_freestanding
_validate_wasm_structural = _link_validation._validate_wasm_structural
_validate_linked = _link_validation._validate_linked
_validate_split_runtime_outputs = _link_validation._validate_split_runtime_outputs


def _run_wasm_opt_via_optimize(
    linked: Path,
    level: str = "Oz",
    *,
    converge: bool | None = None,
    required_exports: set[str] | None = None,
    apply_level: bool | None = None,
    extra_passes: Sequence[str] | None = None,
    attestation: dict[str, object] | None = None,
) -> bool:
    """Run the canonical atomic optimizer and record its attestation."""

    policy = wasm_link_policy(level)
    resolved_converge = policy.converge if converge is None else converge
    resolved_apply_level = policy.apply_level if apply_level is None else apply_level
    resolved_extra_passes = (
        list(extra_passes) if extra_passes is not None else list(policy.extra_passes)
    )

    pre_size = linked.stat().st_size
    if required_exports is None:
        try:
            required_exports = set(_collect_function_exports(linked.read_bytes()))
        except (OSError, ValueError):
            required_exports = set()
    result = optimize_wasm(
        linked,
        output_path=linked,
        level=level,
        extra_passes=resolved_extra_passes,
        converge=resolved_converge,
        required_exports=required_exports,
        apply_level=resolved_apply_level,
    )

    if not result["ok"]:
        err = result.get("error", "unknown error")
        if attestation is not None:
            attestation.update(
                {
                    "ok": False,
                    "status": result.get("status", "failed"),
                    "error": err,
                    "pipeline": result.get("pipeline", []),
                    "wasm_opt_path": result.get("wasm_opt_path"),
                    "wasm_opt_sha256": result.get("wasm_opt_sha256"),
                    "wasm_opt_wall_ms": round(
                        float(result.get("elapsed_s", 0.0)) * 1000.0, 6
                    ),
                    "wasm_opt_peak_rss_kb": result.get("peak_rss_kb"),
                    "wasm_opt_peak_total_rss_kb": result.get("peak_total_rss_kb"),
                }
            )
        print(f"wasm-opt failed: {err}", file=sys.stderr)
        return False

    if attestation is not None:
        attestation.update(
            {
                "ok": True,
                "status": result.get("status", "success"),
                "binaryen_version": result.get("binaryen_version", ""),
                "wasm_opt_path": result.get("wasm_opt_path"),
                "wasm_opt_sha256": result.get("wasm_opt_sha256"),
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


_WASM_LINK_FACTS_SCHEMA_VERSION = _link_facts._WASM_LINK_FACTS_SCHEMA_VERSION
_decode_wasm_facts_response = _link_facts._decode_wasm_facts_response


def _make_rust_wasm_facts_provider(
    scanner: Path,
    scratch_root: Path,
    metrics: dict[str, float] | None = None,
) -> Callable[[bytes], dict[str, object]]:
    return _link_facts.make_rust_wasm_facts_provider(
        globals(), scanner, scratch_root, metrics
    )


def _publish_rust_wasm_link_facts(
    scanner: Path,
    artifact: Path,
    *,
    layout: CallableTableLayout | None = None,
    role: str = "monolithic",
) -> dict[str, object]:
    return _link_facts.publish_rust_wasm_link_facts(
        globals(), scanner, artifact, layout=layout, role=role
    )


_callable_layout_from_wasm_facts = _callable_table._callable_layout_from_wasm_facts
_reconcile_split_callable_layout = _callable_table._reconcile_split_callable_layout
_callable_entry_export_name = _callable_table._callable_entry_export_name
_callable_app_end = _callable_table._callable_app_end
_monolithic_linked_callable_growth_base = (
    _callable_table._monolithic_linked_callable_growth_base
)
_CallableTableEntryPlan = _callable_table._CallableTableEntryPlan
_resolve_callable_table_entry_plan = _callable_table._resolve_callable_table_entry_plan
_merge_linked_callable_table = _callable_table._merge_linked_callable_table
_write_varsint32 = _callable_table._write_varsint32


def _install_callable_table_layout(
    data: bytes,
    layout: CallableTableLayout,
    *,
    entry_symbol_names: Sequence[str] | None = None,
    include_fixed_prefix: bool = True,
    override_reserved_direct: bool = True,
    entry_plan: _CallableTableEntryPlan | None = None,
) -> bytes:
    return _callable_table._install_callable_table_layout(
        data,
        layout,
        entry_symbol_names=entry_symbol_names,
        include_fixed_prefix=include_fixed_prefix,
        override_reserved_direct=override_reserved_direct,
        entry_plan=entry_plan,
        _parse_sections=_parse_sections,
        _build_sections=_build_sections,
    )


def _run_wasm_ld_with_custodied_inputs(
    wasm_ld: str,
    runtime: Path,
    output: Path,
    linked: Path,
    *,
    runtime_role: RuntimeLinkInputRole,
    allowlist_override: Path | None = None,
    optimize: bool = False,
    optimize_level: str = "Oz",
    freestanding: bool = False,
    split_runtime: bool = False,
    split_output_dir: Path | None = None,
    deploy_runtime_override: Path | None = None,
    native_objects: Sequence[Path] = (),
    native_link_arguments: Sequence[str] = (),
    preserve_debug_sections: bool = False,
    phase_timings_file: Path | None = None,
    wasm_facts_scanner: Path,
    app_export_contract_path: Path | None = None,
) -> int:
    return _link_pipeline.run_wasm_ld_with_custodied_inputs(
        globals(),
        wasm_ld,
        runtime,
        output,
        linked,
        runtime_role=runtime_role,
        allowlist_override=allowlist_override,
        optimize=optimize,
        optimize_level=optimize_level,
        freestanding=freestanding,
        split_runtime=split_runtime,
        split_output_dir=split_output_dir,
        deploy_runtime_override=deploy_runtime_override,
        native_objects=native_objects,
        native_link_arguments=native_link_arguments,
        preserve_debug_sections=preserve_debug_sections,
        phase_timings_file=phase_timings_file,
        wasm_facts_scanner=wasm_facts_scanner,
        app_export_contract_path=app_export_contract_path,
    )


def _run_wasm_ld(
    wasm_ld: str,
    runtime: Path,
    output: Path,
    linked: Path,
    *,
    runtime_role: RuntimeLinkInputRole,
    allowlist_override: Path | None = None,
    optimize: bool = False,
    optimize_level: str = "Oz",
    freestanding: bool = False,
    split_runtime: bool = False,
    split_output_dir: Path | None = None,
    deploy_runtime_override: Path | None = None,
    native_objects: Sequence[Path] = (),
    native_link_arguments: Sequence[str] = (),
    preserve_debug_sections: bool = False,
    phase_timings_file: Path | None = None,
    wasm_facts_scanner: Path,
    app_export_contract_path: Path | None = None,
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
                if runtime_role == "reloc"
                else None,
                retry_delay_seconds=0.25,
            )
            runtime_snapshot = (
                runtime_snapshot_root / runtime_snapshot.parent.name / runtime.name
            )
            output_snapshot = _snapshot_link_input(
                output,
                snapshot_root,
                label="app",
                accept=_is_wasm_binary,
            )
            app_export_contract_snapshot = None
            if app_export_contract_path is not None:
                app_export_contract_snapshot = _snapshot_link_input(
                    app_export_contract_path,
                    snapshot_root,
                    label="app-export-contract",
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
            deploy_runtime_snapshot = None
            if split_runtime:
                deploy_runtime = _resolve_deploy_runtime(deploy_runtime_override)
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
                runtime_role=runtime_role,
                allowlist_override=allowlist_override,
                optimize=optimize,
                optimize_level=optimize_level,
                freestanding=freestanding,
                split_runtime=split_runtime,
                split_output_dir=split_output_dir,
                deploy_runtime_override=deploy_runtime_snapshot,
                native_objects=native_snapshots,
                native_link_arguments=native_link_arguments,
                preserve_debug_sections=preserve_debug_sections,
                phase_timings_file=phase_timings_file,
                wasm_facts_scanner=wasm_facts_scanner,
                app_export_contract_path=app_export_contract_snapshot,
            )
    except OSError as exc:
        print(f"Failed to establish wasm linker input custody: {exc}", file=sys.stderr)
        return 1


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Attempt to link Molt output/runtime into a single WASM module.",
    )
    parser.add_argument("--runtime", type=Path, default=_default_runtime_path())
    parser.add_argument("--runtime-shared", type=Path, required=True)
    parser.add_argument("--runtime-generation", type=Path, required=True)
    parser.add_argument("--runtime-expected-identity", type=Path, required=True)
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
        choices=WASM_OPT_LEVELS,
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
        "--native-link-arg",
        action="append",
        default=[],
        dest="native_link_arguments",
        help="Validated external source-extension final wasm link argument",
    )
    parser.add_argument(
        "--preserve-debug-sections",
        action="store_true",
        help="Preserve name and DWARF sections while still removing final-link metadata",
    )
    parser.add_argument("--phase-timings-file", type=Path, default=None)
    parser.add_argument("--wasm-facts-scanner", type=Path, required=True)
    parser.add_argument("--app-export-contract", type=Path, required=True)
    args = parser.parse_args()

    runtime = args.runtime
    output = args.input
    linked = args.output

    if not runtime.exists():
        print(f"Runtime wasm not found: {runtime}", file=sys.stderr)
        return 1
    generation = _verify_runtime_generation(
        reloc=runtime,
        shared=args.runtime_shared,
        generation_manifest=args.runtime_generation,
        expected_identity=args.runtime_expected_identity,
    )
    runtime = generation.reloc
    if args.deploy_runtime_override is not None and (
        args.deploy_runtime_override.resolve(strict=False)
        != generation.shared.resolve(strict=False)
    ):
        print(
            "Explicit deploy runtime is not the shared member selected by the "
            "trusted generation.",
            file=sys.stderr,
        )
        return 1
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
        runtime_role="reloc",
        optimize=args.optimize,
        optimize_level=args.optimize_level,
        freestanding=args.freestanding,
        split_runtime=args.split_runtime,
        split_output_dir=args.split_output_dir,
        deploy_runtime_override=generation.shared if args.split_runtime else None,
        native_objects=tuple(args.native_objects),
        native_link_arguments=tuple(args.native_link_arguments),
        preserve_debug_sections=args.preserve_debug_sections,
        phase_timings_file=args.phase_timings_file,
        wasm_facts_scanner=args.wasm_facts_scanner,
        app_export_contract_path=args.app_export_contract,
    )


if __name__ == "__main__":
    raise SystemExit(main())

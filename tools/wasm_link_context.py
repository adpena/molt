"""Typed dependency views owned by one WASM linker facade.

A context is the owning facade's live namespace, never a process-global
registration. Bound helper calls retain that exact view for their lifetime,
including when another facade is loaded or local dependencies are overridden.
Only the facade casts its namespace; helpers consume these explicit contracts.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from contextlib import AbstractContextManager
from dataclasses import dataclass
from pathlib import Path
import subprocess
from typing import TypedDict

from molt.cli.wasm_link_cache import WasmLinkCacheEntry, WasmLinkCacheRead
from molt.wasm_artifact import WasmImport
from molt.wasm_optimization import WasmOptPolicy
from wasm_link_format import WasmModuleFacts


@dataclass(frozen=True)
class SplitRuntimeExportContractEntry:
    artifact: str
    kind: int
    canonical_name: str
    accepted_names: tuple[str, ...]


class WasmBinaryContext(TypedDict):
    parse_wasm_module_facts: Callable[[bytes], WasmModuleFacts]
    _collect_function_exports: Callable[[bytes], dict[str, int]]
    _parse_sections: Callable[[bytes], list[tuple[int, bytes]]]
    _build_sections: Callable[[list[tuple[int, bytes]]], bytes]
    _read_varuint: Callable[[bytes, int], tuple[int, int]]
    _write_varuint: Callable[[int], bytes]
    _write_string: Callable[[str], bytes]


class WasmSplitContractContext(TypedDict):
    _split_runtime_export_contract: Callable[
        [str], tuple[SplitRuntimeExportContractEntry, ...]
    ]


class WasmValidationContext(WasmSplitContractContext):
    parse_wasm_module_facts: Callable[[bytes], WasmModuleFacts]
    _validate_wasm_structural: Callable[..., bool]
    _standard_section_order_error: Callable[[bytes], str | None]
    _strip_debug_sections: Callable[[bytes], bytes | None]
    _run_external_tool: Callable[..., subprocess.CompletedProcess[str]]
    is_call_indirect_import_name: Callable[[str], bool]
    _validate_linked_table_import_contract: Callable[
        [tuple[WasmImport, ...]], tuple[bool, str | None]
    ]
    _is_wasm_binary: Callable[[bytes], bool]
    wasm_split_runtime_export_name_for_import: Callable[[str], str | None]
    _ESSENTIAL_EXPORTS: frozenset[str]


class WasmExportContext(WasmBinaryContext, WasmSplitContractContext):
    _strip_internal_exports: Callable[..., bytes | None]
    _validate_app_export_adapters: Callable[..., None]
    _ensure_function_exports_by_symbol_names: Callable[..., bytes | None]
    exported_app_symbols: Callable[[Mapping[str, object]], tuple[str, ...]]
    excluded_app_symbols: Callable[[Mapping[str, object]], tuple[str, ...]]
    app_export_call_abi: Callable[[Mapping[str, object]], dict[str, object]]
    _rename_export_names: Callable[[bytes, dict[str, str]], bytes | None]
    _restore_output_export_aliases: Callable[[bytes], bytes | None]
    _collect_imports: Callable[[bytes], list[WasmImport]]
    _canonicalize_standard_section_order: Callable[[bytes], bytes | None]
    _ensure_export_by_index: Callable[..., bytes | None]
    _split_artifact_contract_function_symbols: Callable[..., dict[str, str]]
    _function_body_payloads_by_index: Callable[[bytes], dict[int, bytes]]
    _TRAP_FUNC_BODY: bytes
    _restore_public_output_exports: Callable[..., bytes]
    _import_index_for_kind: Callable[..., int | None]
    _split_artifact_contract_keep_set: Callable[..., set[str]]
    strip_wasm_publication_sections: Callable[..., bytes]
    _restore_split_runtime_contract_exports: Callable[..., bytes]
    _split_runtime_contract_export_names: Callable[[str], set[str]]


class WasmOptimizeResult(TypedDict, total=False):
    ok: bool
    error: str
    status: str
    pipeline: list[str]
    wasm_opt_path: str | None
    wasm_opt_sha256: str | None
    elapsed_s: float
    peak_rss_kb: int | None
    peak_total_rss_kb: int | None
    binaryen_version: str
    before: dict[str, object]
    after: dict[str, object]
    output_bytes: int


class WasmOptimizerContext(WasmBinaryContext):
    wasm_split_runtime_export_name_for_import: Callable[[str], str | None]
    _ESSENTIAL_EXPORTS: frozenset[str]
    _cache_metric_add: Callable[[dict[str, int | float] | None, str, int | float], None]
    _wasm_facts_cache_authority_digest: Callable[
        [Callable[[bytes], dict[str, object]], bytes], str
    ]
    _tree_shake_runtime_cache_key: Callable[..., str]
    _wasm_link_cache_entry: Callable[..., WasmLinkCacheEntry]
    _TREE_SHAKE_RUNTIME_CACHE_SCHEMA: str
    _wasm_link_cache_root: Callable[[], Path]
    _locked_wasm_link_cache_entry: Callable[
        [WasmLinkCacheEntry], AbstractContextManager[float]
    ]
    _read_wasm_link_cache_entry: Callable[[WasmLinkCacheEntry], WasmLinkCacheRead]
    _invalidate_wasm_link_cache_entry: Callable[[WasmLinkCacheEntry], None]
    _read_string: Callable[[bytes, int], tuple[str, int]]
    _post_link_optimize: Callable[..., bytes]
    _publish_wasm_link_cache_result: Callable[..., None]
    find_wasm_opt: Callable[[], str | None]
    _wasm_opt_executable_identity: Callable[[str], tuple[str, str, str] | None]
    _split_app_optimize_cache_key: Callable[..., str | None]
    _SPLIT_APP_OPTIMIZE_CACHE_SCHEMA: str
    _strip_unused_module_function_imports: Callable[..., bytes | None]
    wasm_link_policy: Callable[[str], WasmOptPolicy]
    _run_wasm_opt_via_optimize: Callable[..., bool]
    _record_wasm_opt_attestation_cache_metrics: Callable[
        [dict[str, int | float] | None, str, Mapping[str, object]], None
    ]
    optimize_wasm: Callable[..., WasmOptimizeResult]

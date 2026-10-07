"""Runtime-bound WASM layout shared by cold codegen and cached publication."""

from __future__ import annotations

import os
from dataclasses import dataclass

from molt.cli.runtime_wasm_generation import (
    RuntimeWasmCodegenBinding,
)


@dataclass(frozen=True)
class WasmCodegenLayout:
    data_base: int
    table_base: int
    split_app_table_base: int | None
    relocatable: bool

    def backend_environment(self) -> dict[str, str]:
        env = {
            "MOLT_WASM_DATA_BASE": str(self.data_base),
            "MOLT_WASM_TABLE_BASE": str(self.table_base),
        }
        if self.split_app_table_base is not None:
            env["MOLT_WASM_SPLIT_RUNTIME_APP_TABLE_BASE"] = str(
                self.split_app_table_base
            )
        if self.relocatable:
            env["MOLT_WASM_RELOCATABLE"] = "1"
        return env


def prepare_wasm_codegen_layout(
    binding: RuntimeWasmCodegenBinding | None,
    *,
    linked: bool,
    split_runtime: bool,
) -> WasmCodegenLayout:
    """Derive layout from the exact pair admitted before backend cache lookup.

    Cache reuse and backend execution consume the same facts. Ambient layout
    overrides and mutable runtime selection paths never own these addresses.
    """
    if binding is None:
        raise ValueError("Runtime WASM code generation lacks a bound pair")
    try:
        binding.verify()
    except (OSError, ValueError) as exc:
        raise ValueError(
            "The bound runtime WASM generation changed before code generation"
        ) from exc
    shared = binding.generation.facts()
    probe = (
        shared
        if split_runtime or not linked
        else binding.generation.facts(relocatable=True)
    )
    data_end = probe.data_end
    memory_min = probe.memory_min_bytes
    data_bounds = [value for value in (data_end, memory_min) if value is not None]
    if not data_bounds:
        raise ValueError("Runtime WASM memory layout is missing or invalid")
    # Preserve the shared-memory heap reservation: the runtime heap must not
    # grow into application constants during initialization of large modules.
    data_base = (max(data_bounds) + 64 * 1024 * 1024 + 7) & ~7
    split_app_table_base = None
    if split_runtime:
        layout = shared.split_callable_layout()
        table_base = layout.runtime_callable_base
        split_app_table_base = layout.runtime_table_min
    else:
        probe_table = probe.table_min
        shared_table = shared.table_min
        if probe_table is None or shared_table is None:
            raise ValueError("Runtime WASM callable-table layout is missing or invalid")
        table_base = max(probe_table, shared_table)
    return WasmCodegenLayout(
        data_base=data_base,
        table_base=table_base,
        split_app_table_base=split_app_table_base,
        relocatable=linked or os.environ.get("MOLT_WASM_RELOCATABLE") == "1",
    )

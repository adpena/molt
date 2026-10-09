"""Post-link canonicalization and validation authority."""

from __future__ import annotations

from pathlib import Path
import sys

from molt import _wasm_runtime_exports as _runtime_exports
from molt._wasm_abi_generated import WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS
from molt.wasm_artifact import flatten_wasm_plain_function_rec_groups
import wasm_link_edit as _edit
import wasm_link_export_contract as _export_contract
from wasm_link_fact_provider import WasmFactsProvider, WasmLinkFacts
import wasm_link_format as _format


def _validate_split_runtime_typed_edges(
    app_facts: WasmLinkFacts,
    runtime_facts: WasmLinkFacts,
) -> str | None:
    for import_fact in app_facts.imports:
        if import_fact.module == "env" and (
            _runtime_exports.wasm_split_runtime_export_name_for_import(import_fact.name)
            is not None
            or _format.wasm_runtime_export_name(import_fact.name) is not None
        ):
            return (
                "split-runtime app retains a runtime ABI import in env instead of "
                f"molt_runtime: {import_fact.name}"
            )
        if import_fact.module != "molt_runtime":
            continue
        import_name = import_fact.name
        export_name = _runtime_exports.wasm_split_runtime_export_name_for_import(
            import_name
        )
        if export_name is None:
            return (
                "split-runtime app import has no generated ABI export identity: "
                f"{import_name}"
            )
        export_fact = runtime_facts.exports.get(export_name)
        if export_fact is None:
            return (
                "split-runtime app import is absent from staged shared runtime: "
                f"{import_name} (expected {export_name})"
            )
        if _format.canonical_extern_type(
            import_fact.extern_type
        ) != _format.canonical_extern_type(export_fact.extern_type):
            return (
                "split-runtime ABI type mismatch for "
                f"{import_name} -> {export_name}: "
                f"app={dict(import_fact.extern_type)!r}, "
                f"runtime={dict(export_fact.extern_type)!r}"
            )
        try:
            canonical_import_name = (
                _runtime_exports.wasm_split_runtime_import_name_for_export(import_name)
                or import_name
            )
            if (
                WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS.get(canonical_import_name)
                == "data"
            ):
                generated_type = {
                    "kind": "global",
                    "value_type": (0x7F,),
                    "mutable": False,
                    "shared": False,
                }
            else:
                generated_type = _format.generated_function_type(import_name)
        except ValueError as exc:
            return str(exc)
        if generated_type is None:
            return (
                "split-runtime app import has no generated function signature: "
                f"{import_name}"
            )
        if _format.canonical_extern_type(
            import_fact.extern_type
        ) != _format.canonical_extern_type(generated_type):
            return (
                "split-runtime app import disagrees with generated ABI signature: "
                f"{import_name}: app={dict(import_fact.extern_type)!r}, "
                f"generated={generated_type!r}"
            )
    return None


def _canonicalize_wasm_ld_output(data: bytes, *, description: str) -> bytes:
    try:
        flattened = flatten_wasm_plain_function_rec_groups(data)
    except ValueError as exc:
        raise ValueError(
            f"Failed to flatten {description} wasm rec groups: {exc}"
        ) from exc
    return data if flattened is None else flattened


def _validate_freestanding(
    data: bytes,
    *,
    facts_provider: WasmFactsProvider,
) -> bool:
    """Validate a freestanding wasm binary has no prohibited imports.

    Returns True if valid, False if critical issues found.
    """
    facts = _validate_wasm_structural(
        data,
        description="Freestanding wasm",
        facts_provider=facts_provider,
    )
    if facts is None:
        return False

    wasi_imports = [
        (fact.module, fact.name)
        for fact in facts.imports
        if fact.module == "wasi_snapshot_preview1"
    ]
    if wasi_imports:
        for module, name in wasi_imports:
            print(
                f"Freestanding validation error: remaining WASI import {module}::{name}",
                file=sys.stderr,
            )
        return False

    runtime_imports = [
        (fact.module, fact.name)
        for fact in facts.imports
        if fact.module == "molt_runtime"
    ]
    if runtime_imports:
        for module, name in runtime_imports:
            print(
                f"Freestanding validation error: remaining molt_runtime import {module}::{name}",
                file=sys.stderr,
            )
        return False

    other_imports = [
        (fact.module, fact.name) for fact in facts.imports if fact.module != "env"
    ]
    for module, name in other_imports:
        print(
            f"Freestanding validation error: unexpected import {module}::{name}",
            file=sys.stderr,
        )
    if other_imports:
        return False

    return True


def _validate_wasm_structural(
    data: bytes,
    *,
    description: str,
    facts_provider: WasmFactsProvider,
) -> WasmLinkFacts | None:
    """Run the invocation-scoped, attested Rust structural validator."""
    section_order_error = _edit._standard_section_order_error(data)
    if section_order_error is not None:
        print(
            f"{description} failed canonical section-order validation: "
            f"{section_order_error}",
            file=sys.stderr,
        )
        return None
    try:
        return facts_provider(data)
    except Exception as exc:
        print(f"{description} failed structural validation: {exc}", file=sys.stderr)
        return None


def _validate_linked(
    linked: Path,
    *,
    facts_provider: WasmFactsProvider,
) -> bool:
    data = linked.read_bytes()
    facts = _validate_wasm_structural(
        data,
        description="Linked wasm",
        facts_provider=facts_provider,
    )
    if facts is None:
        return False
    imports = facts.imports
    if any(fact.module == "molt_runtime" for fact in imports):
        print(
            "Linked wasm still imports molt_runtime; link step incomplete.",
            file=sys.stderr,
        )
        return False
    call_indirect = [
        fact.name
        for fact in imports
        if fact.module == "env"
        and fact.kind == 0
        and _format.is_call_indirect_import_name(fact.name)
    ]
    if call_indirect:
        print(
            f"Linked wasm still imports {', '.join(sorted(call_indirect))}; "
            "remove JS call_indirect stubs.",
            file=sys.stderr,
        )
        return False
    table_imports = [fact for fact in imports if fact.kind == 1]
    if len(table_imports) > 1:
        names = ", ".join(f"{fact.module}::{fact.name}" for fact in table_imports)
        print(
            "Linked wasm table import validation failed: Linked wasm imports "
            f"multiple tables ({names}); only env::__indirect_function_table is "
            "supported.",
            file=sys.stderr,
        )
        return False
    if table_imports and (
        table_imports[0].module != "env"
        or table_imports[0].name != "__indirect_function_table"
    ):
        fact = table_imports[0]
        print(
            "Linked wasm table import validation failed: Linked wasm imports "
            f"unsupported table {fact.module}::{fact.name}; expected "
            "env::__indirect_function_table.",
            file=sys.stderr,
        )
        return False
    if table_imports:
        print(
            "Linked wasm retains env::__indirect_function_table under the "
            "host-table contract.",
            file=sys.stderr,
        )
    memory_imports = [fact for fact in imports if fact.kind == 2]
    if memory_imports:
        print("Linked wasm still imports memory.", file=sys.stderr)
        return False
    custom_names = facts.custom_section_names
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
    return True


def _validate_split_runtime_outputs(
    app_wasm: Path,
    rt_wasm: Path,
    *,
    facts_provider: WasmFactsProvider,
) -> bool:
    try:
        app_data = app_wasm.read_bytes()
        rt_data = rt_wasm.read_bytes()
    except OSError as exc:
        print(f"Failed to read split-runtime staged output: {exc}", file=sys.stderr)
        return False
    if not _format._is_wasm_binary(app_data):
        print(
            f"Split-runtime app output is not a wasm binary: {app_wasm}",
            file=sys.stderr,
        )
        return False
    if not _format._is_wasm_binary(rt_data):
        print(
            f"Split-runtime shared runtime output is not a wasm binary: {rt_wasm}",
            file=sys.stderr,
        )
        return False
    app_structural_facts = _validate_wasm_structural(
        app_data,
        description="Split-runtime app",
        facts_provider=facts_provider,
    )
    if app_structural_facts is None:
        return False
    runtime_structural_facts = _validate_wasm_structural(
        rt_data,
        description="Split-runtime shared runtime",
        facts_provider=facts_provider,
    )
    if runtime_structural_facts is None:
        return False
    try:
        app_memory_min = app_structural_facts.memory_import_minimum(
            module="env", name="memory"
        )
    except ValueError as exc:
        print(f"Failed to inspect split-runtime staged output: {exc}", file=sys.stderr)
        return False
    if app_memory_min is None:
        print(
            "Split-runtime app must import env.memory; a private app memory "
            "breaks pointer-bearing runtime ABI calls.",
            file=sys.stderr,
        )
        return False
    for entry in _export_contract._split_runtime_export_contract("app"):
        if any(
            app_structural_facts.exports.get(name) is not None
            and app_structural_facts.exports[name].kind == entry.kind
            for name in entry.accepted_names
        ):
            continue
        print(
            f"Split-runtime app missing contract export {entry.canonical_name} "
            f"(kind {entry.kind}).",
            file=sys.stderr,
        )
        return False
    typed_edge_error = _validate_split_runtime_typed_edges(
        app_structural_facts,
        runtime_structural_facts,
    )
    if typed_edge_error is not None:
        print(typed_edge_error, file=sys.stderr)
        return False
    return True

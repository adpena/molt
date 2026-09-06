"""Post-link canonicalization and validation authority."""

from __future__ import annotations

from pathlib import Path
import shutil
import sys
import tempfile

from molt.wasm_artifact import flatten_wasm_plain_function_rec_groups

from wasm_link_context import WasmValidationContext


def _canonicalize_wasm_ld_output(data: bytes, *, description: str) -> bytes:
    try:
        flattened = flatten_wasm_plain_function_rec_groups(data)
    except ValueError as exc:
        raise ValueError(
            f"Failed to flatten {description} wasm rec groups: {exc}"
        ) from exc
    return data if flattened is None else flattened


# Minimal function body: 0 locals, ``unreachable``, ``end``.


def _validate_freestanding(context: WasmValidationContext, data: bytes) -> bool:
    """Validate a freestanding wasm binary has no prohibited imports.

    Returns True if valid, False if critical issues found.
    """
    try:
        imports = context["parse_wasm_module_facts"](data).imports
    except ValueError as exc:
        print(f"Failed to parse freestanding wasm imports: {exc}", file=sys.stderr)
        return False

    wasi_imports = [
        (wasm_import.module, wasm_import.name)
        for wasm_import in imports
        if wasm_import.module == "wasi_snapshot_preview1"
    ]
    if wasi_imports:
        for module, name in wasi_imports:
            print(
                f"Freestanding validation error: remaining WASI import {module}::{name}",
                file=sys.stderr,
            )
        return False

    runtime_imports = [
        (wasm_import.module, wasm_import.name)
        for wasm_import in imports
        if wasm_import.module == "molt_runtime"
    ]
    if runtime_imports:
        for module, name in runtime_imports:
            print(
                f"Freestanding validation error: remaining molt_runtime import {module}::{name}",
                file=sys.stderr,
            )
        return False

    other_imports = [
        (wasm_import.module, wasm_import.name)
        for wasm_import in imports
        if wasm_import.module != "env"
    ]
    for module, name in other_imports:
        print(
            f"Freestanding validation warning: unexpected import {module}::{name}",
            file=sys.stderr,
        )

    return context["_validate_wasm_structural"](
        data,
        description="Freestanding wasm",
    )


def _validate_wasm_structural(
    context: WasmValidationContext, data: bytes, *, description: str
) -> bool:
    """Run the canonical wasm structural validator when available."""
    try:
        section_order_error = context["_standard_section_order_error"](data)
    except Exception as exc:
        print(
            f"{description} canonical section-order validation failed: {exc}",
            file=sys.stderr,
        )
        return False
    if section_order_error is not None:
        print(
            f"{description} failed canonical section-order validation: "
            f"{section_order_error}",
            file=sys.stderr,
        )
        return False
    exe = shutil.which("wasm-tools")
    if exe is None:
        print(
            f"{description} structural validation unavailable: wasm-tools not found",
            file=sys.stderr,
        )
        return False
    try:
        validate_data = context["_strip_debug_sections"](data) or data
    except ValueError as exc:
        print(
            f"{description} debug-section stripping warning: {exc}; "
            "validating original bytes",
            file=sys.stderr,
        )
        validate_data = data
    try:
        with tempfile.NamedTemporaryFile(suffix=".wasm", delete=False) as f:
            f.write(validate_data)
            f.flush()
            tmp_path = f.name
    except OSError as exc:
        print(
            f"{description} structural validation staging failed: {exc}",
            file=sys.stderr,
        )
        return False
    try:
        result = context["_run_external_tool"](
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
        print(f"{description} structural validation failed: {exc}", file=sys.stderr)
        return False
    finally:
        try:
            Path(tmp_path).unlink()
        except OSError:
            pass
    return True


def _validate_linked(context: WasmValidationContext, linked: Path) -> bool:
    data = linked.read_bytes()
    try:
        facts = context["parse_wasm_module_facts"](data)
    except ValueError as exc:
        print(f"Failed to parse linked wasm: {exc}", file=sys.stderr)
        return False
    imports = facts.imports
    if any(wasm_import.module == "molt_runtime" for wasm_import in imports):
        print(
            "Linked wasm still imports molt_runtime; link step incomplete.",
            file=sys.stderr,
        )
        return False
    call_indirect = [
        wasm_import.name
        for wasm_import in imports
        if wasm_import.module == "env"
        and wasm_import.kind == 0
        and context["is_call_indirect_import_name"](wasm_import.name)
    ]
    if call_indirect:
        print(
            f"Linked wasm still imports {', '.join(sorted(call_indirect))}; "
            "remove JS call_indirect stubs.",
            file=sys.stderr,
        )
        return False
    ok, err = context["_validate_linked_table_import_contract"](imports)
    if not ok:
        print(f"Linked wasm table import validation failed: {err}", file=sys.stderr)
        return False
    if any(wasm_import.kind == 1 for wasm_import in imports):
        print(
            "Linked wasm retains env::__indirect_function_table under the "
            "host-table contract.",
            file=sys.stderr,
        )
    memory_imports = [
        (wasm_import.module, wasm_import.name)
        for wasm_import in imports
        if wasm_import.kind == 2
    ]
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
    return context["_validate_wasm_structural"](data, description="Linked wasm")


def _validate_split_runtime_outputs(
    context: WasmValidationContext, app_wasm: Path, rt_wasm: Path
) -> bool:
    try:
        app_data = app_wasm.read_bytes()
        rt_data = rt_wasm.read_bytes()
    except OSError as exc:
        print(f"Failed to read split-runtime staged output: {exc}", file=sys.stderr)
        return False
    if not context["_is_wasm_binary"](app_data):
        print(
            f"Split-runtime app output is not a wasm binary: {app_wasm}",
            file=sys.stderr,
        )
        return False
    if not context["_is_wasm_binary"](rt_data):
        print(
            f"Split-runtime shared runtime output is not a wasm binary: {rt_wasm}",
            file=sys.stderr,
        )
        return False
    try:
        app_facts = context["parse_wasm_module_facts"](app_data)
        rt_facts = context["parse_wasm_module_facts"](rt_data)
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
    for entry in context["_split_runtime_export_contract"]("app"):
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
        export_name = context["wasm_split_runtime_export_name_for_import"](name)
        if export_name is not None and export_name in rt_exports:
            continue
        if export_name is None and name in rt_exports:
            continue
        if name in context["_ESSENTIAL_EXPORTS"]:
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
    if not context["_validate_wasm_structural"](
        app_data, description="Split-runtime app"
    ):
        return False
    if not context["_validate_wasm_structural"](
        rt_data, description="Split-runtime shared runtime"
    ):
        return False
    return True

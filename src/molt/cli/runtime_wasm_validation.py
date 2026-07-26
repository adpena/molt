from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
import shutil

from molt._wasm_runtime_exports import (
    wasm_runtime_missing_required_exports,
    wasm_runtime_required_export_symbol_kinds,
    wasm_split_runtime_missing_required_exports,
    wasm_split_runtime_required_export_symbol_kinds,
    wasm_split_runtime_import_name_for_export,
)
from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_FUNCTION_SIGNATURES,
    wasm_import_signature,
    wasm_runtime_import_name,
)
from molt.cli.command_runtime import _run_completed_command
from molt.wasm_artifact import (
    _wasm_import_minima,
    _read_wasm_memory_min_bytes,
    has_nonempty_wasm_code_section,
    inspect_wasm_binary,
    read_wasm_defined_globals,
    read_wasm_exports,
    _wasm_export_function_signatures,
    WASM_EXTERN_KIND_FUNCTION,
    WASM_EXTERN_KIND_GLOBAL,
    WASM_VALUE_TYPE_I32,
)


def _validate_wasm_structural(path: Path) -> str | None:
    exe = shutil.which("wasm-tools")
    if exe is None:
        return (
            "wasm-tools is required for deep structural validation; "
            "artifact reuse is disabled until the validator is provisioned"
        )
    try:
        resolved = path.resolve()
        result = _run_completed_command(
            [exe, "validate", str(resolved)],
            capture_output=True,
            timeout=60,
            env=None,
            cwd=resolved.parent,
            memory_guard_prefix="MOLT_BUILD",
        )
    except Exception as exc:
        return f"wasm-tools validate failed to run: {exc}"
    if result.returncode == 0:
        return None
    detail = (result.stderr or result.stdout).strip()
    return f"wasm-tools validate failed: {detail}"


def _reusable_wasm_artifact_validation_error(path: Path) -> str | None:
    state = inspect_wasm_binary(path)
    if state != "valid":
        return f"artifact is {state}"
    structural_error = _validate_wasm_structural(path)
    if structural_error is not None:
        return structural_error
    return None


def _runtime_wasm_artifact_validation_error(path: Path) -> str | None:
    artifact_error = _reusable_wasm_artifact_validation_error(path)
    if artifact_error is not None:
        return artifact_error
    if not has_nonempty_wasm_code_section(path):
        return "artifact has no non-empty code section"
    return None


def _shared_runtime_wasm_validation_error(path: Path) -> str | None:
    artifact_error = _runtime_wasm_artifact_validation_error(path)
    if artifact_error is not None:
        return artifact_error
    if not _runtime_wasm_has_shared_import_abi(path):
        return "artifact is missing the shared memory/table import ABI"
    return None


def _is_reusable_wasm_artifact(path: Path) -> bool:
    return _reusable_wasm_artifact_validation_error(path) is None


def _is_valid_runtime_wasm_artifact(path: Path) -> bool:
    return _runtime_wasm_artifact_validation_error(path) is None


def _runtime_wasm_has_shared_import_abi(path: Path) -> bool:
    try:
        memory_min, table_min = _wasm_import_minima(path)
    except (OSError, ValueError):
        return False
    return memory_min is not None and table_min is not None


def _is_valid_shared_runtime_wasm_artifact(path: Path) -> bool:
    return _shared_runtime_wasm_validation_error(path) is None


def _runtime_wasm_exports_satisfy(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> bool:
    return not _runtime_wasm_missing_exports(path, required_exports)


def _split_runtime_wasm_exports_satisfy(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> bool:
    return not _split_runtime_wasm_missing_exports(path, required_exports)


def _runtime_wasm_missing_exports(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> set[str]:
    available = _runtime_wasm_typed_export_names(
        path,
        wasm_runtime_required_export_symbol_kinds(required_exports),
    )
    if not available and required_exports:
        return wasm_runtime_missing_required_exports((), required_exports)
    return wasm_runtime_missing_required_exports(available, required_exports)


def _split_runtime_wasm_missing_exports(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
) -> set[str]:
    available = _runtime_wasm_typed_export_names(
        path,
        wasm_split_runtime_required_export_symbol_kinds(required_exports),
    )
    if not available and required_exports:
        return wasm_split_runtime_missing_required_exports((), required_exports)
    return wasm_split_runtime_missing_required_exports(available, required_exports)


def _runtime_wasm_typed_export_names(
    path: Path,
    expected_symbol_kinds: Mapping[str, str],
) -> set[str]:
    """Return only exports whose WebAssembly shape satisfies generated authority.

    Function obligations require function exports. Data obligations require a
    *defined*, immutable i32 global initialized directly by ``i32.const``; an
    imported, mutable, wrong-valtype, or ``global.get``-initialized global is
    not an address receipt and therefore cannot satisfy the contract.
    """
    try:
        exports = read_wasm_exports(path)
        globals_by_index = {
            global_.index: global_ for global_ in read_wasm_defined_globals(path)
        }
        function_names = {
            name for name, kind in expected_symbol_kinds.items() if kind == "function"
        }
        function_signatures = _wasm_export_function_signatures(
            path, export_names=function_names
        )
        memory_min_bytes = _read_wasm_memory_min_bytes(path)
    except (OSError, UnicodeDecodeError, ValueError, IndexError):
        return set()
    exports_by_name: dict[str, tuple[int, int] | None] = {}
    for export in exports:
        identity = (export.kind, export.index)
        previous = exports_by_name.get(export.name)
        if previous is not None and previous != identity:
            exports_by_name[export.name] = None
        elif export.name not in exports_by_name:
            exports_by_name[export.name] = identity
    available: set[str] = set()
    expected_data_identities: dict[tuple[int, int], str] = {}
    for name, expected_kind in expected_symbol_kinds.items():
        if expected_kind != "data":
            continue
        identity = exports_by_name.get(name)
        if identity is None:
            continue
        previous = expected_data_identities.get(identity)
        if previous is not None and previous != name:
            # Two public data names cannot silently project the same address;
            # canonical/split renames are exclusive publication modes.
            exports_by_name[previous] = None
            exports_by_name[name] = None
        else:
            expected_data_identities[identity] = name
    for name, expected_kind in expected_symbol_kinds.items():
        identity = exports_by_name.get(name)
        if identity is None:
            continue
        kind, index = identity
        if expected_kind == "function":
            canonical_name = (
                wasm_split_runtime_import_name_for_export(name)
                or wasm_runtime_import_name(name)
                or name
            )
            expected_signature = (
                WASM_EXTERNAL_NATIVE_LINK_IMPORT_FUNCTION_SIGNATURES.get(canonical_name)
            )
            if expected_signature is None:
                generated = wasm_import_signature(canonical_name)
                if generated is not None:
                    params, results = generated
                    expected_signature = {
                        "params": list(params),
                        "result": "nil" if not results else ", ".join(results),
                    }
            if (
                kind == WASM_EXTERN_KIND_FUNCTION
                and expected_signature is not None
                and function_signatures.get(name) == expected_signature
            ):
                available.add(name)
            continue
        if expected_kind != "data" or kind != WASM_EXTERN_KIND_GLOBAL:
            continue
        global_ = globals_by_index.get(index)
        address = (
            None
            if global_ is None or global_.i32_const is None
            else global_.i32_const & 0xFFFF_FFFF
        )
        if (
            global_ is not None
            and global_.value_type == WASM_VALUE_TYPE_I32
            and not global_.mutable
            and global_.initializer_opcode == 0x41
            and global_.i32_const_canonical
            and address is not None
            and address != 0
            and memory_min_bytes is not None
            and address < memory_min_bytes
        ):
            available.add(name)
    return available

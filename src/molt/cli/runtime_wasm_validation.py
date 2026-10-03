from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal

if TYPE_CHECKING:
    from molt.cli.runtime_wasm_generation import RuntimeWasmGeneration
from pathlib import Path

from molt._wasm_runtime_exports import (
    wasm_runtime_missing_required_exports,
    wasm_runtime_required_export_symbol_kinds,
    wasm_split_runtime_missing_required_exports,
    wasm_split_runtime_required_export_symbol_kinds,
    wasm_split_runtime_import_name_for_export,
)
from molt._wasm_abi_generated import (
    WASM_CPYTHON_ABI_LINK_IMPORT_FUNCTION_SIGNATURES,
    wasm_import_signature,
    wasm_runtime_import_name,
)
from molt.cli.command_runtime import _run_completed_command
from molt.tool_releases import run_pinned_tool
from molt.wasm_artifact import (
    WasmRuntimeFacts,
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
    try:
        resolved = path.resolve()
        result = run_pinned_tool(
            "wasm-tools",
            ["validate", str(resolved)],
            run=_run_completed_command,
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
    *,
    facts: WasmRuntimeFacts | None = None,
) -> set[str]:
    """Return only exports whose WebAssembly shape satisfies generated authority.

    Function obligations require function exports. Data obligations require a
    *defined*, immutable i32 global initialized directly by ``i32.const``; an
    imported, mutable, wrong-valtype, or ``global.get``-initialized global is
    not an address receipt and therefore cannot satisfy the contract.
    """
    try:
        exports = read_wasm_exports(path) if facts is None else facts.exports
        globals_by_index = {
            global_.index: global_
            for global_ in (
                read_wasm_defined_globals(path) if facts is None else facts.globals
            )
        }
        function_names = {
            name for name, kind in expected_symbol_kinds.items() if kind == "function"
        }
        function_signatures = (
            _wasm_export_function_signatures(path, export_names=function_names)
            if facts is None
            else {
                name: {"params": list(params), "result": result}
                for name, params, result in facts.function_signatures
                if name in function_names
            }
        )
        memory_min_bytes = (
            _read_wasm_memory_min_bytes(path)
            if facts is None
            else facts.memory_min_bytes
        )
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
            expected_signature = WASM_CPYTHON_ABI_LINK_IMPORT_FUNCTION_SIGNATURES.get(
                canonical_name
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


@dataclass(frozen=True, slots=True)
class RuntimeWasmAdmissionIssue:
    member: Literal["generation", "shared", "reloc"]
    reason: Literal["observation", "structure", "empty-code", "import-abi", "linking"]
    detail: str


@dataclass(frozen=True, slots=True)
class RuntimeWasmAdmissionReport:
    issues: tuple[RuntimeWasmAdmissionIssue, ...] = ()
    shared_missing_exports: tuple[str, ...] = ()
    reloc_missing_symbols: tuple[str, ...] = ()

    @property
    def accepted(self) -> bool:
        return not (
            self.issues or self.shared_missing_exports or self.reloc_missing_symbols
        )

    def details(self) -> dict[str, object]:
        return {
            "issues": [
                {"member": issue.member, "reason": issue.reason, "detail": issue.detail}
                for issue in self.issues
            ],
            "shared_missing_exports": list(self.shared_missing_exports),
            "reloc_missing_symbols": list(self.reloc_missing_symbols),
        }


def runtime_wasm_generation_admission(
    generation: RuntimeWasmGeneration,
    required_exports: set[str] | frozenset[str] | None,
) -> RuntimeWasmAdmissionReport:
    """One decision and diagnostic authority for every runtime-pair consumer.

    Required names are request policy. Successful member facts and structural
    checks belong to this physical generation; every reuse still checks its
    live stable-file fences. Failure reports preserve the failed observation
    instead of inspecting a later value of the mutable selection pointer.
    """
    issues: list[RuntimeWasmAdmissionIssue] = []
    shared_missing: tuple[str, ...] = ()
    reloc_missing: tuple[str, ...] = ()
    try:
        generation.verify_members()
        shared = generation.facts()
        reloc = generation.facts(relocatable=True)
    except (OSError, UnicodeError, ValueError, IndexError) as exc:
        return RuntimeWasmAdmissionReport(
            (RuntimeWasmAdmissionIssue("generation", "observation", str(exc)),)
        )
    for member, facts in (("shared", shared), ("reloc", reloc)):
        if facts.code_functions == 0:
            issues.append(
                RuntimeWasmAdmissionIssue(
                    member, "empty-code", "artifact has no non-empty code section"
                )
            )
    if not shared.shared_import_abi:
        issues.append(
            RuntimeWasmAdmissionIssue(
                "shared",
                "import-abi",
                "artifact is missing the shared memory/table import ABI",
            )
        )
    try:
        generation.validate_structure()
    except (OSError, ValueError) as exc:
        issues.append(RuntimeWasmAdmissionIssue("generation", "structure", str(exc)))
    shared_missing = tuple(
        sorted(
            wasm_split_runtime_missing_required_exports(
                _runtime_wasm_typed_export_names(
                    generation.shared,
                    wasm_split_runtime_required_export_symbol_kinds(required_exports),
                    facts=shared,
                ),
                required_exports,
            )
        )
    )
    try:
        available = generation.linking_names(
            wasm_runtime_required_export_symbol_kinds(required_exports)
        )
        reloc_missing = tuple(
            sorted(wasm_runtime_missing_required_exports(available, required_exports))
        )
    except (OSError, UnicodeError, ValueError) as exc:
        issues.append(RuntimeWasmAdmissionIssue("reloc", "linking", str(exc)))
    try:
        generation.verify_members()
    except (OSError, ValueError) as exc:
        issues.append(RuntimeWasmAdmissionIssue("generation", "observation", str(exc)))
    return RuntimeWasmAdmissionReport(tuple(issues), shared_missing, reloc_missing)

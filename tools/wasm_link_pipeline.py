"""Custodied monolithic and split-runtime WASM link orchestration."""

from __future__ import annotations

from wasm_link_fact_provider import WasmFactsProvider

from collections.abc import Mapping, Sequence
import contextlib
from dataclasses import dataclass
from pathlib import Path
import shlex
import subprocess
import sys
from molt.temporary_artifacts import OwnedTemporaryDirectory
import time

from molt import artifact_publication as artifact_publish
from molt._wasm_abi_generated import (
    WASM_OUTPUT_RUNTIME_EXPORT_ALIASES,
    WASM_RESERVED_RUNTIME_CALLABLES,
)
from molt._wasm_runtime_exports import wasm_split_runtime_export_name_for_import
from molt.cli.app_export_contract import (
    app_export_call_abi,
    exported_app_symbols,
    load_app_export_contract,
)
from molt.cli.source_extension_link_requirements import (
    render_source_extension_link_arguments,
    SourceExtensionLinkInput,
    SourceExtensionLinkRequirements,
    map_source_extension_link_inputs,
    merge_source_extension_link_requirements,
    source_extension_link_file,
)
from molt.exact_json import dumps_exact
from molt.link_outputs import wasm_link_output_paths
from molt.cli.link_fingerprints import FinalLinkReceiptRequest, publish_link_outputs
from molt.cli.link_selection_admission import (
    LinkSelectionAdmission,
    write_link_selection,
)
from molt.cli.wasm_link_args import wasm_link_output_arguments

from molt.wasm_artifact import read_wasm_split_runtime_callable_layout
from molt.wasm_optimizer_identity import (
    WasmOptimizerExecutableIdentity,
    WasmOptimizerIdentityError,
    build_wasm_optimizer_attestation,
    encode_wasm_optimizer_attestation,
    wasm_optimizer_attestation_path,
    wasm_optimizer_invocation_identity,
)
import wasm_link_callable_table as _callable_table
import wasm_link_command as _command
import wasm_link_edit as _edit
import wasm_link_export_contract as _export_contract
import wasm_link_fact_provider as _facts
import wasm_link_format as _format
from wasm_link_format import CallableTableLayout
import wasm_link_native_inputs as _native_inputs
import wasm_link_operations as _operations
import wasm_link_optimizer_policy as _optimizer
import wasm_link_optimize as _link_optimize
import wasm_link_runtime_data as _runtime_data
from wasm_link_transaction import WasmArtifactState
import wasm_link_validation as _validation
from wasm_metrics import wasm_metrics
from wasm_stub_wasi import stub_wasi_imports


TOOLS_ROOT = Path(__file__).resolve().parent


@dataclass(frozen=True, slots=True)
class _SplitPublication:
    app: WasmArtifactState
    runtime: WasmArtifactState
    app_destination: Path
    runtime_destination: Path
    size_attestation: dict[str, object]
    size_attestation_path: Path
    size_attestation_stage: Path
    callable_layout: CallableTableLayout
    optimizer_attestation: dict[str, object]
    optimizer_attestation_path: Path
    optimizer_attestation_stage: Path | None


@dataclass(frozen=True, slots=True)
class _SplitAppLinkPlan:
    command: tuple[str, ...]
    output_data: bytes
    planned_data_base: int
    callable_layout: CallableTableLayout
    callable_entry_symbol_names: tuple[str, ...]
    public_export_map: Mapping[str, str]
    required_native_direct_symbols: tuple[str, ...]
    runtime_exports: frozenset[str]
    provider_symbols: frozenset[str]
    data_alias_plan: _runtime_data.SplitRuntimeDataAliasPlan | None
    got_runtime_addresses: Mapping[str, int]
    failure_evidence_dir: Path


@dataclass(frozen=True, slots=True)
class _PreparedSplitApp:
    artifact: WasmArtifactState
    required_table_min: int


@dataclass(frozen=True, slots=True)
class _SplitAppLinkResult:
    returncode: int
    prepared: _PreparedSplitApp | None


@dataclass(frozen=True, slots=True)
class _SplitAppOptimizationPlan:
    output_data: bytes
    optimize: bool
    optimize_level: str
    contract_keep_set: frozenset[str]
    contract_app_exports: tuple[str, ...]
    adapter_symbol_map: Mapping[str, str]
    adapter_identity_map: Mapping[str, str]
    target_symbol_map: Mapping[str, str]
    target_identity_map: Mapping[str, str]
    identity_exports: Mapping[str, str]
    public_export_map: Mapping[str, str]
    required_native_direct_symbols: tuple[str, ...]
    output_memory_min: int | None
    preserve_debug: bool
    has_native_objects: bool


@dataclass(frozen=True, slots=True)
class _PreparedSplitRuntime:
    artifact: WasmArtifactState
    source_size: int


@dataclass(frozen=True, slots=True)
class _LinkedArtifactPlan:
    reference_data: bytes
    callable_layout: CallableTableLayout | None
    callable_entry_symbol_names: tuple[str, ...]
    preserved_output_exports: tuple[str, ...]
    export_symbol_map: Mapping[str, str]
    required_native_direct_symbols: tuple[str, ...]
    contract_app_exports: tuple[str, ...]
    adapter_symbol_map: Mapping[str, str]
    adapter_identity_map: Mapping[str, str]
    target_symbol_map: Mapping[str, str]
    target_identity_map: Mapping[str, str]
    identity_exports: Mapping[str, str]
    preserve_debug: bool
    optimize: bool
    optimize_level: str
    split_runtime: bool
    output_table_min: int | None
    output_memory_min: int | None
    freestanding: bool


@dataclass(frozen=True, slots=True)
class _PreparedLinkedArtifact:
    artifact: WasmArtifactState
    public_export_map: Mapping[str, str]
    split_app_contract_keep_set: frozenset[str]


@dataclass(frozen=True, slots=True)
class _LinkCommandInputs:
    wasm_ld: str
    runtime: Path
    allowlist: Path
    base_allowlist: Path
    output_callable_layout: CallableTableLayout | None
    split_callable_layout: CallableTableLayout | None
    force_exports: tuple[str, ...]
    required_native_direct_symbols: tuple[str, ...]
    user_export_symbol_names: tuple[str, ...]
    callable_entry_symbol_names: tuple[str, ...]
    reserved_runtime_link_exports: tuple[str, ...]
    work_linked: Path
    linked_rewritten_path: Path
    rewritten_path: Path
    linked_requirements: SourceExtensionLinkRequirements
    rewritten_requirements: SourceExtensionLinkRequirements
    native_objects: tuple[Path, ...]
    native_link_requirements: SourceExtensionLinkRequirements
    provider_paths: Mapping[str, Path]
    host_provider_symbols: frozenset[str]
    link_outputs: Mapping[str, Path]
    runtime_exports: frozenset[str]
    output_data: bytes
    split_runtime: bool
    deploy_runtime_path: Path | None
    temp_dir: OwnedTemporaryDirectory


@dataclass(frozen=True, slots=True)
class _PlannedSplitLink:
    command: tuple[str, ...]
    linked_path: Path
    data_base: int
    data_alias_plan: _runtime_data.SplitRuntimeDataAliasPlan | None
    got_runtime_addresses: Mapping[str, int]
    deploy_runtime_data: bytes


@dataclass(frozen=True, slots=True)
class _PlannedLinkCommands:
    monolithic: tuple[str, ...]
    split: _PlannedSplitLink | None


@dataclass(frozen=True, slots=True)
class _LoadedLinkContract:
    runtime_exports: frozenset[str]
    output_data: bytes
    app_export_contract: Mapping[str, object]
    app_call_abi: Mapping[str, object]
    facts_provider: _facts.WasmFactsProvider
    output_callable_layout: CallableTableLayout | None
    split_callable_layout: CallableTableLayout | None
    monolithic_callable_layout: CallableTableLayout | None
    deploy_runtime_path: Path | None
    output_memory_min: int | None
    output_table_min: int | None
    callable_entry_export_names: tuple[str, ...]
    reserved_runtime_link_exports: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class _PreparedLinkInputs:
    required_native_direct_symbols: tuple[str, ...]
    export_symbol_map: Mapping[str, str]
    callable_entry_symbol_names: tuple[str, ...]
    callable_entry_symbol_names_by_slot: tuple[str, ...]
    contract_app_exports: tuple[str, ...]
    app_target_symbol_map: Mapping[str, str]
    app_adapter_symbol_map: Mapping[str, str]
    app_adapter_identity_map: Mapping[str, str]
    app_target_identity_map: Mapping[str, str]
    app_identity_exports: Mapping[str, str]
    preserved_output_exports: tuple[str, ...]
    user_export_symbol_names: tuple[str, ...]
    rewritten_path: Path
    rewritten_requirements: SourceExtensionLinkRequirements
    force_exports: tuple[str, ...]
    base_allowlist: Path
    allowlist: Path
    linked_rewritten_path: Path
    linked_requirements: SourceExtensionLinkRequirements


def _publish_final_outputs_stage(
    linked_artifact: WasmArtifactState,
    linked_destination: Path,
    *,
    monolithic_callable_layout: CallableTableLayout | None,
    preserve_debug: bool,
    app_export_contract: Mapping[str, object],
    public_export_map: Mapping[str, str],
    required_native_direct_symbols: Sequence[str],
    optimize: bool,
    optimizer_attestation: dict[str, object],
    optimizer_attestation_path: Path,
    optimizer_attestation_stage: Path | None,
    split: _SplitPublication | None,
    phase_timings_ms: dict[str, float],
    link_receipt: FinalLinkReceiptRequest | None,
    link_outputs: Mapping[str, Path],
    selection_roles: Mapping[str, Mapping[str, object]],
    staged_outputs: list[Path],
    failure_evidence_dir: Path,
) -> bool:
    """Validate and atomically publish one fully transformed artifact family."""

    strip_started = time.perf_counter()
    linked_artifact.replace(
        _operations.strip_publication_sections(
            linked_artifact.data,
            final_artifact=True,
            preserve_debug=preserve_debug,
        )
    )
    canonical_sections = _edit._canonicalize_standard_section_order(
        linked_artifact.data
    )
    if canonical_sections is not None:
        linked_artifact.replace(canonical_sections)
    try:
        linked_artifact.apply_atomic_facts_publication(
            lambda path: linked_artifact.facts_provider.publish_in_place(
                path, layout=monolithic_callable_layout
            )
        )
    except ValueError as exc:
        print(f"Failed to attest final linked callable table: {exc}", file=sys.stderr)
        return False

    if split is not None:
        try:
            split.app.replace(
                _export_contract._strip_and_restore_split_artifact(
                    split.app.data,
                    artifact="app",
                    stage="publication-strip",
                    preserve_debug=preserve_debug,
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                    facts_provider=split.app.facts_provider,
                )
            )
            app_facts = split.app.apply_atomic_facts_publication(
                lambda path: split.app.facts_provider.publish_in_place(
                    path,
                    layout=split.callable_layout,
                    role="app",
                )
            )
            final_split_callable_layout = (
                _callable_table._callable_layout_from_wasm_facts(
                    app_facts,
                    artifact_role="app",
                )
            )
            if final_split_callable_layout is None:
                raise ValueError(
                    "final split app publication omitted callable-table layout"
                )
            split.runtime.apply_atomic_facts_publication(
                lambda path: split.runtime.facts_provider.publish_in_place(
                    path,
                    layout=final_split_callable_layout,
                    role="runtime",
                )
            )
        except ValueError as exc:
            print(
                f"Failed to attest final split callable table: {exc}", file=sys.stderr
            )
            return False
        if optimize:
            try:
                app_optimizer_attestation = build_wasm_optimizer_attestation(
                    split.optimizer_attestation,
                    published_output=split.app.data,
                )
                assert split.optimizer_attestation_stage is not None
                split.optimizer_attestation_stage.write_text(
                    encode_wasm_optimizer_attestation(app_optimizer_attestation),
                    encoding="utf-8",
                    newline="",
                )
            except (OSError, ValueError) as exc:
                print(
                    f"Failed to attest wasm optimizer execution: {exc}",
                    file=sys.stderr,
                )
                return False
            split.size_attestation["optimizer"] = app_optimizer_attestation
        split.size_attestation["published"] = {
            "app": wasm_metrics(split.app.data),
            "runtime": wasm_metrics(split.runtime.data),
        }
        split.size_attestation_stage.write_text(
            dumps_exact(split.size_attestation),
            encoding="utf-8",
        )
    phase_timings_ms["wasm_strip"] = round(
        max(0.0, (time.perf_counter() - strip_started) * 1000.0),
        6,
    )

    validation_started = time.perf_counter()
    app_export_error = _export_contract._app_export_surface_error(
        linked_artifact.data,
        app_export_contract,
        stage="linked-publication",
        facts_provider=linked_artifact.facts_provider,
    )
    if app_export_error is not None:
        print(app_export_error, file=sys.stderr)
        return False
    if split is not None:
        app_export_error = _export_contract._app_export_surface_error(
            split.app.data,
            app_export_contract,
            stage="split-app-publication",
            facts_provider=linked_artifact.facts_provider,
        )
        if app_export_error is not None:
            print(app_export_error, file=sys.stderr)
            return False

    if not _validation._validate_linked(
        linked_artifact.path,
        facts_provider=linked_artifact.facts_provider,
    ):
        try:
            failed_validation = _facts.preserve_rejected_wasm(
                linked_artifact.data, failure_evidence_dir, stage="linked-validation"
            )
        except (OSError, ValueError) as exc:
            print(
                f"Linked validation failed; failed to preserve rejected artifact: {exc}",
                file=sys.stderr,
            )
        else:
            print(
                f"Preserved failed linked validation artifact: {failed_validation}",
                file=sys.stderr,
            )
        if split is not None:
            print(
                "Linked wasm validation failed before split-runtime publication; "
                "failing because linked validation is the canonical "
                "table/memory/import guard.",
                file=sys.stderr,
            )
        return False

    if optimize:
        assert optimizer_attestation_stage is not None
        try:
            normalized_optimizer_attestation = build_wasm_optimizer_attestation(
                optimizer_attestation,
                published_output=linked_artifact.data,
            )
            optimizer_attestation_stage.write_text(
                encode_wasm_optimizer_attestation(normalized_optimizer_attestation),
                encoding="utf-8",
                newline="",
            )
        except (OSError, ValueError) as exc:
            print(f"Failed to attest wasm optimizer execution: {exc}", file=sys.stderr)
            return False

    publish_candidates = {"linked": (linked_artifact.path, linked_destination)}
    if optimizer_attestation_stage is not None:
        publish_candidates["optimizer"] = (
            optimizer_attestation_stage,
            optimizer_attestation_path,
        )
    if split is not None:
        if split.optimizer_attestation_stage is not None:
            publish_candidates["app_optimizer"] = (
                split.optimizer_attestation_stage,
                split.optimizer_attestation_path,
            )
        if not _validation._validate_split_runtime_outputs(
            split.app.path,
            split.runtime.path,
            facts_provider=split.app.facts_provider,
        ):
            return False
        publish_candidates.update(
            runtime=(split.runtime.path, split.runtime_destination),
            app=(split.app.path, split.app_destination),
            size_attestation=(
                split.size_attestation_stage,
                split.size_attestation_path,
            ),
        )
    if selection_roles:
        selection_stage = artifact_publish.staged_output_path(link_outputs["selection"])
        staged_outputs.append(selection_stage)
        write_link_selection(selection_stage, selection_roles)
        publish_candidates["selection"] = (selection_stage, link_outputs["selection"])
    try:
        publish_link_outputs(
            publish_candidates,
            receipt=link_receipt,
            removals=()
            if optimize
            else (
                optimizer_attestation_path,
                *((split.optimizer_attestation_path,) if split is not None else ()),
            ),
        )
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"Failed to publish wasm linker outputs: {exc}", file=sys.stderr)
        return False
    phase_timings_ms["fail_closed_validation"] = round(
        phase_timings_ms.get("fail_closed_validation", 0.0)
        + max(0.0, (time.perf_counter() - validation_started) * 1000.0),
        6,
    )
    return True


def _post_link_transform_stage(
    artifact: WasmArtifactState,
    *,
    reference_data: bytes | None,
    preserve_exports: set[str],
    preserve_debug: bool,
    optimize: bool,
    optimize_level: str,
    split_runtime: bool,
    optimizer_attestation: dict[str, object],
    optimizer_identity: WasmOptimizerExecutableIdentity | None,
    contract_app_exports: Sequence[str],
    app_adapter_identity_map: Mapping[str, str],
    app_target_identity_map: Mapping[str, str],
) -> bool:
    """Apply post-link transforms through one invalidating artifact authority."""

    pre_opt_size = len(artifact.data)
    artifact.replace(
        _link_optimize._post_link_optimize(
            artifact.data,
            reference_data=reference_data,
            preserve_exports=preserve_exports,
            preserve_debug=preserve_debug,
            facts_provider=artifact.facts_provider,
        )
    )
    post_opt_size = len(artifact.data)
    if post_opt_size < pre_opt_size:
        savings = pre_opt_size - post_opt_size
        print(
            f"Post-link optimization: stripped {savings:,} bytes "
            f"({savings / 1024:.1f} KB, "
            f"{savings / pre_opt_size * 100:.1f}% reduction)",
            file=sys.stderr,
        )
    if optimize:
        optimized = artifact.apply_atomic_path_mutation(
            lambda path: _optimizer._run_wasm_opt_via_optimize(
                path,
                level=optimize_level,
                converge=False,
                apply_level=not split_runtime,
                required_exports=(
                    set(artifact.facts().function_exports) & preserve_exports
                ),
                attestation=optimizer_attestation,
                optimizer_identity=optimizer_identity,
                preserve_debug=preserve_debug,
            )
        )
        if not optimized:
            print("Required linked WASM optimization failed.", file=sys.stderr)
            return False
    if app_adapter_identity_map:
        try:
            _edit._validate_app_export_adapters(
                artifact.data,
                contract_app_exports,
                adapter_symbol_map=app_adapter_identity_map,
                target_symbol_map=app_target_identity_map,
                facts_provider=artifact.facts_provider,
            )
        except ValueError as exc:
            print(
                f"Wasm link failed after post-link optimization: {exc}",
                file=sys.stderr,
            )
            return False
    return True


def _normalize_linked_resources_stage(
    artifact: WasmArtifactState,
    *,
    output_table_min: int | None,
    output_memory_min: int | None,
    split_runtime: bool,
    app_identity_exports: Sequence[str],
    preserved_output_exports: Sequence[str],
    freestanding: bool,
) -> bool:
    """Normalize linked table/memory/export state with facts invalidation."""

    required_table_min = _edit._required_linked_table_min(
        artifact.data,
        output_table_min,
        artifact.facts(),
        facts_provider=artifact.facts_provider,
    )
    if required_table_min is not None:
        try:
            updated = _edit._rewrite_table_import_min(artifact.data, required_table_min)
        except ValueError as exc:
            print(f"Failed to rewrite linked table min: {exc}", file=sys.stderr)
            return False
        if updated is not None:
            artifact.replace(updated)
    if output_memory_min is not None:
        try:
            updated = _edit._rewrite_memory_min(artifact.data, output_memory_min)
        except ValueError as exc:
            print(f"Failed to rewrite linked memory min: {exc}", file=sys.stderr)
            return False
        if updated is not None:
            artifact.replace(updated)
    try:
        updated = _format._ensure_table_export(
            artifact.data, facts_provider=artifact.facts_provider
        )
    except ValueError as exc:
        print(f"Failed to ensure table export: {exc}", file=sys.stderr)
        return False
    if updated is not None:
        artifact.replace(updated)
    linked_facts = artifact.facts()
    if not any(entry.kind == 2 for entry in linked_facts.imports):
        try:
            updated = _export_contract._ensure_defined_memory_export(
                artifact.data,
                facts=linked_facts,
            )
        except ValueError as exc:
            print(f"Failed to ensure memory export: {exc}", file=sys.stderr)
            return False
        if updated is not None:
            artifact.replace(updated)
    if not split_runtime:
        try:
            artifact.replace(
                _export_contract._strip_app_export_identity_markers(
                    artifact.data,
                    identity_exports=app_identity_exports,
                    preserve_exports=set(preserved_output_exports),
                    facts_provider=artifact.facts_provider,
                )
            )
        except ValueError as exc:
            print(f"Wasm link failed: {exc}", file=sys.stderr)
            return False
    if freestanding:
        try:
            freestanding_bytes, n_stubbed = stub_wasi_imports(artifact.data)
            if n_stubbed > 0:
                artifact.replace(freestanding_bytes)
                print(
                    f"Freestanding: stubbed {n_stubbed} WASI imports",
                    file=sys.stderr,
                )
        except ValueError as exc:
            print(f"Freestanding WASI stubbing failed: {exc}", file=sys.stderr)
            return False
    return True


def _load_link_contract_stage(
    runtime: Path,
    output: Path,
    *,
    app_export_contract_path: Path,
    wasm_facts_scanner: Path,
    wasm_facts_scanner_sha256: str | None,
    facts_provider: _facts.WasmFactsProvider | None,
    temp_dir: OwnedTemporaryDirectory,
    facts_metrics: dict[str, float],
    failure_evidence_dir: Path,
    split_runtime: bool,
    deploy_runtime_override: Path | None,
) -> _LoadedLinkContract | None:
    """Load and cross-check every immutable input to one link transaction."""

    try:
        if facts_provider is None:
            facts_provider = _facts.make_rust_wasm_facts_provider(
                wasm_facts_scanner,
                Path(temp_dir.name),
                facts_metrics,
                expected_sha256=wasm_facts_scanner_sha256,
                evidence_root=failure_evidence_dir,
            )
        runtime_data = runtime.read_bytes()
        runtime_facts = facts_provider(runtime_data)
        runtime_exports = frozenset(runtime_facts.linking_symbols.defined_names)
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        print(
            f"Failed to parse relocatable runtime symbols ({runtime}): {exc}",
            file=sys.stderr,
        )
        return None
    if not runtime_exports:
        print(
            f"Relocatable runtime linking definitions unavailable: {runtime}",
            file=sys.stderr,
        )
        return None
    try:
        output_data = output.read_bytes()
        app_export_contract = load_app_export_contract(app_export_contract_path)
        app_call_abi = app_export_call_abi(app_export_contract)
        output_facts = facts_provider(output_data)
        output_callable_layout = _callable_table._callable_layout_from_wasm_facts(
            output_facts,
            artifact_role="plan",
        )
        output_memory_min = _edit._memory_import_min(
            output_data, facts_provider=facts_provider
        )
        output_table_min = _edit._table_import_min(
            output_data, facts_provider=facts_provider
        )
    except (OSError, ValueError) as exc:
        print(
            f"Wasm link failed while loading its input contract: {exc}", file=sys.stderr
        )
        return None
    callable_entry_export_names = (
        tuple(
            _callable_table._callable_entry_export_name(slot)
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
    deploy_runtime_path: Path | None = None
    if split_runtime:
        if output_callable_layout is None:
            print(
                "Split app is missing explicit callable-table layout authority.",
                file=sys.stderr,
            )
            return None
        try:
            deploy_runtime_path = _runtime_data._resolve_deploy_runtime(
                deploy_runtime_override
            )
            runtime_callable_layout = read_wasm_split_runtime_callable_layout(
                deploy_runtime_path
            )
            split_callable_layout = _callable_table._reconcile_split_callable_layout(
                output_callable_layout,
                runtime_callable_layout,
            )
        except (OSError, ValueError) as exc:
            print(f"Split callable-table layout is invalid: {exc}", file=sys.stderr)
            return None
        expected_table_min = split_callable_layout.finalized_app_base + (
            split_callable_layout.app_entry_count
        )
        table_boundary_matches = output_table_min == expected_table_min or (
            expected_table_min == 0 and output_table_min is None
        )
        if expected_table_min > 0xFFFF_FFFF or not table_boundary_matches:
            print(
                "Split app table import boundary does not match explicit callable "
                f"layout: import_min={output_table_min}, "
                f"expected={expected_table_min}",
                file=sys.stderr,
            )
            return None
    return _LoadedLinkContract(
        runtime_exports=runtime_exports,
        output_data=output_data,
        app_export_contract=app_export_contract,
        app_call_abi=app_call_abi,
        facts_provider=facts_provider,
        output_callable_layout=output_callable_layout,
        split_callable_layout=split_callable_layout,
        monolithic_callable_layout=output_callable_layout,
        deploy_runtime_path=deploy_runtime_path,
        output_memory_min=output_memory_min,
        output_table_min=output_table_min,
        callable_entry_export_names=callable_entry_export_names,
        reserved_runtime_link_exports=reserved_runtime_link_exports,
    )


def _prepare_link_inputs_stage(
    output: Path,
    runtime: Path,
    *,
    output_data: bytes,
    runtime_exports: frozenset[str],
    app_export_contract: Mapping[str, object],
    app_call_abi: Mapping[str, object],
    callable_entry_export_names: tuple[str, ...],
    native_link_requirements: SourceExtensionLinkRequirements,
    provider_symbols: frozenset[str],
    host_provider_symbols: frozenset[str],
    split_runtime: bool,
    allowlist_override: Path | None,
    temp_dir: OwnedTemporaryDirectory,
    facts_provider: WasmFactsProvider,
) -> _PreparedLinkInputs | None:
    """Rewrite and attest all linker inputs before command construction."""
    native_objects = tuple(
        dict.fromkeys(Path(item.path) for item in native_link_requirements.inputs)
    )

    required_native_direct_symbols = tuple(
        sorted(
            set(
                _native_inputs._required_native_direct_symbols(
                    output_data, facts_provider=facts_provider
                )
            )
            | set(_native_inputs._sealed_native_init_symbols(native_objects))
        )
    )
    try:
        export_symbol_map = _edit._collect_output_export_symbol_map(
            output_data, facts_provider=facts_provider
        )
    except ValueError as exc:
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        return None
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
            "Wasm link failed: callable-table entry exports are missing linker "
            "symbols: " + ", ".join(missing_callable_exports),
            file=sys.stderr,
        )
        return None
    callable_entry_symbol_names = tuple(
        dict.fromkeys(callable_entry_export_map.values())
    )
    callable_entry_symbol_names_by_slot = tuple(
        callable_entry_export_map[name] for name in callable_entry_export_names
    )
    contract_app_exports = exported_app_symbols(app_export_contract)
    app_target_symbol_map = {
        name: export_symbol_map[name]
        for name in contract_app_exports
        if name in export_symbol_map
    }
    preserved_output_exports = tuple(
        dict.fromkeys(
            [
                *contract_app_exports,
                *(
                    name
                    for name in WASM_OUTPUT_RUNTIME_EXPORT_ALIASES
                    if name in export_symbol_map
                ),
                *(
                    entry.canonical_name
                    for entry in _export_contract._split_runtime_export_contract("app")
                    if entry.kind == 0 and entry.canonical_name in export_symbol_map
                ),
            ]
        )
    )
    missing_contract_exports = sorted(
        set(contract_app_exports) - export_symbol_map.keys()
    )
    if missing_contract_exports:
        print(
            "Wasm link failed: frontend app export contract names are absent from "
            "the relocatable app artifact: " + ", ".join(missing_contract_exports),
            file=sys.stderr,
        )
        return None
    rewritten = _edit._rewrite_output_imports(output, set(runtime_exports), temp_dir)
    if rewritten is None:
        return None
    rewritten_path, _returned_temp_dir, force_exports = rewritten
    if _returned_temp_dir is not temp_dir:
        raise ValueError("WASM import rewrite changed transaction workspace ownership")
    try:
        rewritten_path = _native_inputs._rewrite_required_native_direct_imports(
            rewritten_path,
            required_native_direct_symbols,
            temp_dir,
        )
        rewritten_path, app_adapter_symbol_map = _edit._inject_app_export_adapters(
            rewritten_path,
            temp_dir,
            public_export_names=contract_app_exports,
            call_abi=app_call_abi,
            facts_provider=facts_provider,
        )
        export_symbol_map.update(app_adapter_symbol_map)
        (
            app_adapter_identity_map,
            app_target_identity_map,
            app_identity_exports,
        ) = _export_contract._app_export_identity_maps(
            app_adapter_symbol_map,
            app_target_symbol_map,
        )
    except (OSError, ValueError) as exc:
        print(f"Failed to rewrite app linker inputs: {exc}", file=sys.stderr)
        return None
    user_export_symbol_names = tuple(
        export_symbol_map[name]
        for name in preserved_output_exports
        if name in export_symbol_map
    )
    native_link_inputs, native_force_exports = _edit._rewrite_native_runtime_imports(
        native_objects,
        set(runtime_exports),
        temp_dir,
        split_runtime=split_runtime,
        provider_symbols=provider_symbols,
    )
    force_exports.extend(native_force_exports)
    rewritten_by_source = {
        source: source_extension_link_file(rewritten)
        for source, rewritten in zip(native_objects, native_link_inputs, strict=True)
    }
    rewritten_requirements = map_source_extension_link_inputs(
        native_link_requirements,
        lambda item: SourceExtensionLinkInput(
            rewritten_by_source[Path(item.path)].path,
            rewritten_by_source[Path(item.path)].sha256,
            item.loading,
        ),
    )
    try:
        rewritten_path = _command._inject_call_indirect_alias(
            rewritten_path,
            runtime,
            temp_dir,
            facts_provider=facts_provider,
        )
    except (OSError, ValueError) as exc:
        print(f"Wasm call_indirect alias admission failed: {exc}", file=sys.stderr)
        return None
    base_allowlist = (
        allowlist_override
        if allowlist_override is not None
        else TOOLS_ROOT / "wasm_allowed_imports.txt"
    )
    if not base_allowlist.exists():
        print(f"Allowlist not found: {base_allowlist}", file=sys.stderr)
        return None
    allowlist = _native_inputs._compose_wasm_ld_allowlist(
        base_allowlist=base_allowlist,
        native_link_requirements=native_link_requirements,
        temp_dir=temp_dir,
        provider_symbols=host_provider_symbols,
    )
    linked_rewritten_path = rewritten_path
    linked_requirements = rewritten_requirements
    if split_runtime and native_objects:
        linked_rewrite = _edit._rewrite_runtime_import_module_namespace(
            rewritten_path,
            source_module="molt_runtime",
            target_module="env",
            runtime_exports=set(runtime_exports),
            temp_dir=temp_dir,
            filename="output_linked_runtime_imports.wasm",
            provider_symbols=provider_symbols,
        )
        if linked_rewrite is None:
            return None
        linked_rewritten_path, linked_force_exports = linked_rewrite
        force_exports.extend(linked_force_exports)
        linked_requirements = native_link_requirements
    return _PreparedLinkInputs(
        required_native_direct_symbols=required_native_direct_symbols,
        export_symbol_map=export_symbol_map,
        callable_entry_symbol_names=callable_entry_symbol_names,
        callable_entry_symbol_names_by_slot=callable_entry_symbol_names_by_slot,
        contract_app_exports=contract_app_exports,
        app_target_symbol_map=app_target_symbol_map,
        app_adapter_symbol_map=app_adapter_symbol_map,
        app_adapter_identity_map=app_adapter_identity_map,
        app_target_identity_map=app_target_identity_map,
        app_identity_exports=app_identity_exports,
        preserved_output_exports=preserved_output_exports,
        user_export_symbol_names=user_export_symbol_names,
        rewritten_path=rewritten_path,
        rewritten_requirements=rewritten_requirements,
        force_exports=tuple(force_exports),
        base_allowlist=base_allowlist,
        allowlist=allowlist,
        linked_rewritten_path=linked_rewritten_path,
        linked_requirements=linked_requirements,
    )


def _plan_link_commands_stage(
    inputs: _LinkCommandInputs,
    *,
    operation_counts: dict[str, int | float],
    facts_provider: WasmFactsProvider,
) -> _PlannedLinkCommands | None:
    """Construct the monolithic and optional split linker transactions."""

    command = [
        inputs.wasm_ld,
        "--no-entry",
        "--gc-sections",
        "--error-limit=0",
        f"--allow-undefined-file={inputs.allowlist}",
        "--import-table",
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
    linked_callable_growth_base = (
        _callable_table._monolithic_linked_callable_growth_base(
            inputs.output_callable_layout
        )
        if inputs.output_callable_layout is not None
        else None
    )
    if linked_callable_growth_base is not None:
        command.insert(
            command.index("--import-table") + 1,
            f"--table-base={linked_callable_growth_base}",
        )
    command.extend(
        _command._deduplicated_export_flags(
            (f"--export-if-defined={name}" for name in inputs.force_exports),
            (
                f"--export-if-defined={name}"
                for name in sorted(
                    _format._ESSENTIAL_EXPORTS
                    - {"__indirect_function_table", "memory", "molt_main"}
                )
            ),
            (f"--export={name}" for name in inputs.required_native_direct_symbols),
            (f"--export={name}" for name in inputs.user_export_symbol_names),
            (
                f"--export-if-defined={name}"
                for name in inputs.callable_entry_symbol_names
            ),
            (
                f"--export-if-defined={name}"
                for name in inputs.reserved_runtime_link_exports
            ),
        )
    )
    common_link_args = tuple(command)
    command.extend(
        (
            *wasm_link_output_arguments(
                inputs.link_outputs["linked"], staged_output=inputs.work_linked
            ),
            str(inputs.linked_rewritten_path),
            str(inputs.runtime),
            *render_source_extension_link_arguments(inputs.linked_requirements),
        )
    )
    if not inputs.split_runtime:
        return _PlannedLinkCommands(monolithic=tuple(command), split=None)

    if inputs.deploy_runtime_path is None or inputs.split_callable_layout is None:
        print("Split runtime link plan is incomplete.", file=sys.stderr)
        return None
    try:
        deploy_runtime_data = inputs.deploy_runtime_path.read_bytes()
    except OSError as exc:
        print(
            f"Split runtime is unreadable: {inputs.deploy_runtime_path}: {exc}",
            file=sys.stderr,
        )
        return None
    split_requirements = inputs.rewritten_requirements
    data_alias_plan: _runtime_data.SplitRuntimeDataAliasPlan | None = None
    got_runtime_addresses: Mapping[str, int] = {}
    if inputs.native_objects:
        try:
            data_alias_plan = _runtime_data._split_runtime_data_alias_object(
                native_link_requirements=inputs.rewritten_requirements,
                deploy_runtime=inputs.deploy_runtime_path,
                temp_dir=inputs.temp_dir,
                reloc_runtime=inputs.runtime,
                facts_provider=facts_provider,
            )
            got_runtime_addresses = (
                _runtime_data._runtime_exported_data_symbol_addresses(
                    deploy_runtime_data,
                    facts_provider=facts_provider,
                )
            )
        except (OSError, ValueError) as exc:
            print(str(exc), file=sys.stderr)
            return None
        if data_alias_plan is not None:
            split_requirements = merge_source_extension_link_requirements(
                (
                    split_requirements,
                    SourceExtensionLinkRequirements(
                        split_requirements.target_triple,
                        (source_extension_link_file(data_alias_plan.artifact),),
                    ),
                ),
                target_triple=split_requirements.target_triple,
            )
    split_allowlist = _native_inputs._compose_split_runtime_native_allowlist(
        base_allowlist=inputs.base_allowlist,
        native_link_requirements=split_requirements,
        provider_symbols=inputs.host_provider_symbols,
        split_runtime_exports=set(facts_provider(deploy_runtime_data).function_exports),
        temp_dir=inputs.temp_dir,
    )
    try:
        data_base = _runtime_data._split_app_global_base(inputs.output_data)
        split_app_link_args = _native_inputs._split_app_native_link_args(
            split_requirements, provider_paths=inputs.provider_paths
        )
    except ValueError as exc:
        print(f"WASM split app link plan is invalid: {exc}", file=sys.stderr)
        return None
    table_base = _callable_table._callable_app_end(inputs.split_callable_layout)
    prefix = [
        f"--allow-undefined-file={split_allowlist}"
        if part.startswith("--allow-undefined-file=")
        else "--no-stack-first"
        if part == "--stack-first"
        else part
        for part in common_link_args
        if part != "--export=molt_main" and not part.startswith("--table-base=")
    ]
    split_linked_path = Path(inputs.temp_dir.name) / "app_split_linked.wasm"
    split_command = (
        *prefix,
        "--emit-relocs",
        "--import-memory",
        f"--global-base={data_base}",
        f"--table-base={table_base}",
        *wasm_link_output_arguments(
            inputs.link_outputs["app"], staged_output=split_linked_path
        ),
        str(inputs.rewritten_path),
        *split_app_link_args,
    )
    operation_counts["split_app_data_base_bytes"] = data_base
    return _PlannedLinkCommands(
        monolithic=tuple(command),
        split=_PlannedSplitLink(
            command=tuple(split_command),
            linked_path=split_linked_path,
            data_base=data_base,
            data_alias_plan=data_alias_plan,
            got_runtime_addresses=got_runtime_addresses,
            deploy_runtime_data=deploy_runtime_data,
        ),
    )


def _prepare_linked_artifact_stage(
    path: Path,
    plan: _LinkedArtifactPlan,
    *,
    facts_provider: _facts.WasmFactsProvider,
    optimizer_attestation: dict[str, object],
    optimizer_identity: WasmOptimizerExecutableIdentity | None,
) -> _PreparedLinkedArtifact | None:
    """Admit, canonicalize, optimize, and normalize wasm-ld's main output."""

    if not path.exists():
        print(
            f"wasm-ld exited successfully but produced no linked output: {path}",
            file=sys.stderr,
        )
        return None
    artifact = WasmArtifactState.from_materialized_bytes(
        path,
        _command._read_wasm_bytes_with_retry(path),
        facts_provider=facts_provider,
    )
    if not _format._is_wasm_binary(artifact.data):
        print(
            "wasm-ld produced non-wasm linked output "
            f"({path}, size={len(artifact.data)} bytes)",
            file=sys.stderr,
        )
        return None
    try:
        artifact.replace(
            _validation._canonicalize_wasm_ld_output(
                artifact.data,
                description="linked",
            )
        )
        if plan.callable_layout is not None:
            raw_callable_entries = artifact.facts().get("callable_table_entries")
            entry_plan = _callable_table._resolve_callable_table_entry_plan(
                artifact.data,
                plan.callable_layout,
                entry_symbol_names=plan.callable_entry_symbol_names,
                include_fixed_prefix=True,
                override_reserved_direct=True,
                facts_provider=facts_provider,
            )
            _callable_table._merge_linked_callable_table(
                raw_callable_entries,
                plan.callable_layout,
                entry_plan,
            )
            artifact.replace(
                _callable_table._install_callable_table_layout(
                    artifact.data,
                    plan.callable_layout,
                    entry_symbol_names=plan.callable_entry_symbol_names,
                    entry_plan=entry_plan,
                    facts_provider=facts_provider,
                )
            )
    except ValueError as exc:
        print(f"Failed to canonicalize linked callable table: {exc}", file=sys.stderr)
        return None
    public_export_map = _export_contract._public_output_export_symbol_map(
        preserved_output_exports=plan.preserved_output_exports,
        export_symbol_map=plan.export_symbol_map,
    )
    artifact.replace(
        _export_contract._restore_public_output_exports(
            artifact.data,
            public_export_map,
            preserved_symbol_names=plan.required_native_direct_symbols,
            facts_provider=facts_provider,
        )
    )
    if plan.adapter_symbol_map:
        try:
            artifact.replace(
                _export_contract._publish_app_export_identity_markers(
                    artifact.data,
                    public_export_names=plan.contract_app_exports,
                    adapter_symbol_map=plan.adapter_symbol_map,
                    target_symbol_map=plan.target_symbol_map,
                    identity_exports=plan.identity_exports,
                    facts_provider=facts_provider,
                )
            )
        except ValueError as exc:
            print(f"Wasm link failed: {exc}", file=sys.stderr)
            return None
    try:
        native_link_error = _native_inputs._validate_required_native_direct_symbols(
            artifact.data,
            plan.required_native_direct_symbols,
            description="Wasm native link",
            facts_provider=facts_provider,
        )
    except ValueError as exc:
        print(f"Failed to inspect native direct symbols: {exc}", file=sys.stderr)
        return None
    if native_link_error is not None:
        print(native_link_error, file=sys.stderr)
        return None
    split_app_contract_keep_set = frozenset(
        _export_contract._split_artifact_contract_keep_set(
            "app",
            public_export_map=public_export_map,
            required_native_direct_symbols=plan.required_native_direct_symbols,
        )
    )
    post_link_preserve_exports = set(split_app_contract_keep_set)
    post_link_preserve_exports.update(plan.identity_exports)
    if not plan.split_runtime:
        post_link_preserve_exports.update(plan.preserved_output_exports)
    if not _post_link_transform_stage(
        artifact,
        reference_data=plan.reference_data,
        preserve_exports=post_link_preserve_exports,
        preserve_debug=plan.preserve_debug,
        optimize=plan.optimize,
        optimize_level=plan.optimize_level,
        split_runtime=plan.split_runtime,
        optimizer_attestation=optimizer_attestation,
        optimizer_identity=optimizer_identity,
        contract_app_exports=plan.contract_app_exports,
        app_adapter_identity_map=plan.adapter_identity_map,
        app_target_identity_map=plan.target_identity_map,
    ):
        return None
    if not _normalize_linked_resources_stage(
        artifact,
        output_table_min=plan.output_table_min,
        output_memory_min=plan.output_memory_min,
        split_runtime=plan.split_runtime,
        app_identity_exports=tuple(plan.identity_exports),
        preserved_output_exports=plan.preserved_output_exports,
        freestanding=plan.freestanding,
    ):
        return None
    return _PreparedLinkedArtifact(
        artifact=artifact,
        public_export_map=public_export_map,
        split_app_contract_keep_set=split_app_contract_keep_set,
    )


def _run_admitted_link(
    command: Sequence[str],
    *,
    selection: LinkSelectionAdmission | None,
    why_extract: Path,
    role: str,
    selection_roles: dict[str, Mapping[str, object]],
) -> subprocess.CompletedProcess[str]:
    arguments = list(command)
    if selection is not None and selection.lazy_archives:
        arguments.extend(("--trace", f"--why-extract={why_extract}"))
    result = _command._run_external_tool(arguments, capture_output=True, text=True)
    if result.returncode == 0 and selection is not None:
        selection_roles[role] = selection.admit(
            dialect="wasm",
            stdout=result.stdout,
            stderr=result.stderr,
            why_extract=why_extract.read_text(encoding="utf-8")
            if selection.lazy_archives
            else None,
        )
    return result


def _link_split_app_stage(
    plan: _SplitAppLinkPlan,
    linked_path: Path,
    *,
    facts_provider: _facts.WasmFactsProvider,
    size_attestation: dict[str, object],
    operation_counts: dict[str, int | float],
    selection: LinkSelectionAdmission | None,
    selection_roles: dict[str, Mapping[str, object]],
    why_extract: Path,
) -> _SplitAppLinkResult:
    """Run and admit the split-app native link as one typed transaction."""

    try:
        result = _run_admitted_link(
            plan.command,
            selection=selection,
            why_extract=why_extract,
            role="app",
            selection_roles=selection_roles,
        )
    except (OSError, ValueError) as exc:
        print(
            f"WASM split-app external member admission failed: {exc}", file=sys.stderr
        )
        return _SplitAppLinkResult(1, None)
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        if detail:
            print(detail, file=sys.stderr)
        return _SplitAppLinkResult(result.returncode, None)
    signature_mismatch = _command._wasm_ld_signature_mismatch_warning(result.stderr)
    if signature_mismatch is not None:
        print(signature_mismatch, file=sys.stderr)
        return _SplitAppLinkResult(1, None)
    if not linked_path.exists():
        print(
            "wasm-ld exited successfully but produced no split app linked output: "
            f"{linked_path}",
            file=sys.stderr,
        )
        return _SplitAppLinkResult(1, None)
    artifact = WasmArtifactState.from_materialized_bytes(
        linked_path,
        _command._read_wasm_bytes_with_retry(linked_path),
        facts_provider=facts_provider,
    )
    if not _format._is_wasm_binary(artifact.data):
        print(
            "wasm-ld produced non-wasm split app linked output "
            f"({linked_path}, size={len(artifact.data)} bytes)",
            file=sys.stderr,
        )
        return _SplitAppLinkResult(1, None)
    try:
        output_intervals, linked_intervals = (
            _runtime_data._validate_split_app_data_layout(
                plan.output_data,
                artifact.data,
                planned_base=plan.planned_data_base,
            )
        )
    except ValueError as exc:
        print(f"WASM split app memory layout is invalid: {exc}", file=sys.stderr)
        return _SplitAppLinkResult(1, None)
    operation_counts["split_app_output_data_segment_count"] = len(output_intervals)
    output_extent = (
        output_intervals[0][0],
        max(end for _start, end in output_intervals),
    )
    operation_counts["split_app_output_data_min_bytes"] = output_extent[0]
    operation_counts["split_app_output_data_end_bytes"] = output_extent[1]
    operation_counts["split_app_linked_data_segment_count"] = len(linked_intervals)
    linked_extent = (
        linked_intervals[0][0],
        max(end for _start, end in linked_intervals),
    )
    operation_counts["split_app_linked_data_min_bytes"] = linked_extent[0]
    operation_counts["split_app_linked_data_end_bytes"] = linked_extent[1]
    size_attestation["split_app_data_layout"] = {
        "alignment_bytes": 16,
        "planned_native_base": plan.planned_data_base,
        "output_active_intervals": output_intervals,
        "output_extent": output_extent,
        "linked_active_intervals": linked_intervals,
        "linked_extent": linked_extent,
    }
    try:
        artifact.replace(
            _validation._canonicalize_wasm_ld_output(
                artifact.data,
                description="split app linked",
            )
        )
        _normalize_split_app_runtime_imports(
            artifact, plan.runtime_exports, plan.provider_symbols
        )
        raw_entries = artifact.facts().get("callable_table_entries")
        entry_plan = _callable_table._resolve_callable_table_entry_plan(
            artifact.data,
            plan.callable_layout,
            entry_symbol_names=plan.callable_entry_symbol_names,
            include_fixed_prefix=False,
            override_reserved_direct=False,
            facts_provider=facts_provider,
        )
        required_table_min = _callable_table._merge_linked_callable_table(
            raw_entries,
            plan.callable_layout,
            entry_plan,
        )
        artifact.replace(
            _callable_table._install_callable_table_layout(
                artifact.data,
                plan.callable_layout,
                entry_symbol_names=plan.callable_entry_symbol_names,
                include_fixed_prefix=False,
                override_reserved_direct=False,
                entry_plan=entry_plan,
                facts_provider=facts_provider,
            )
        )
        artifact.replace(
            _export_contract._restore_split_runtime_contract_exports(
                artifact.data,
                artifact="app",
                stage="native-link",
                public_export_map=plan.public_export_map,
                required_native_direct_symbols=plan.required_native_direct_symbols,
                facts_provider=facts_provider,
            )
        )
        native_link_error = _native_inputs._validate_required_native_direct_symbols(
            artifact.data,
            plan.required_native_direct_symbols,
            description="Split-runtime native app link",
            facts_provider=facts_provider,
        )
    except ValueError as exc:
        print(
            f"Failed to finalize split-runtime native app link: {exc}", file=sys.stderr
        )
        return _SplitAppLinkResult(1, None)
    if native_link_error is not None:
        print(native_link_error, file=sys.stderr)
        try:
            evidence = _facts.preserve_rejected_wasm(
                artifact.data, plan.failure_evidence_dir, stage="split-native-link"
            )
        except (OSError, ValueError) as exc:
            print(
                f"Failed to preserve split-runtime native app rejection: {exc}",
                file=sys.stderr,
            )
        else:
            print(
                f"Split-runtime native app failure artifact: {evidence}",
                file=sys.stderr,
            )
        print(
            "Split-runtime native app linker argv: " + shlex.join(plan.command),
            file=sys.stderr,
        )
        return _SplitAppLinkResult(1, None)
    try:
        if plan.data_alias_plan is None:
            got_retargeted = 0
        else:
            retargeted_data, got_retargeted = (
                _runtime_data._rewrite_split_app_got_data_globals(
                    artifact.data,
                    runtime_addresses=plan.got_runtime_addresses,
                    alias_plan=plan.data_alias_plan,
                    wasm_facts=artifact.facts(),
                    description="Split-runtime native app link",
                )
            )
            if got_retargeted:
                artifact.replace(retargeted_data)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return _SplitAppLinkResult(1, None)
    if got_retargeted:
        print(
            "Split-runtime GOT data bridge: retargeted "
            f"{got_retargeted} CPython-ABI GOT data global(s) to the shared "
            "runtime's canonical addresses",
            file=sys.stderr,
        )
    return _SplitAppLinkResult(
        0,
        _PreparedSplitApp(
            artifact=artifact,
            required_table_min=required_table_min,
        ),
    )


def _normalize_split_app_runtime_imports(
    artifact: WasmArtifactState,
    runtime_exports: frozenset[str],
    provider_symbols: frozenset[str],
) -> None:
    """Route every final native/runtime ABI edge through one split namespace."""

    rewritten, _force_exports = _edit._rewrite_runtime_imports_in_module(
        artifact.data,
        source_module="env",
        target_module="molt_runtime",
        runtime_exports=set(runtime_exports),
        split_runtime=True,
        provider_symbols=provider_symbols,
    )
    if rewritten is not None:
        artifact.replace(rewritten)


def _optimize_split_app_stage(
    prepared: _PreparedSplitApp,
    destination: Path,
    plan: _SplitAppOptimizationPlan,
    *,
    optimizer_attestation: dict[str, object],
    operation_counts: dict[str, int | float],
    facts_provider: _facts.WasmFactsProvider,
    optimizer_identity: WasmOptimizerExecutableIdentity | None,
) -> WasmArtifactState | None:
    """Optimize and normalize the split app through one artifact owner."""

    source = prepared.artifact
    if plan.adapter_symbol_map:
        try:
            source.replace(
                _export_contract._publish_app_export_identity_markers(
                    source.data,
                    public_export_names=plan.contract_app_exports,
                    adapter_symbol_map=plan.adapter_symbol_map,
                    target_symbol_map=plan.target_symbol_map,
                    identity_exports=plan.identity_exports,
                    facts_provider=facts_provider,
                )
            )
        except ValueError as exc:
            print(f"Wasm split-app link failed: {exc}", file=sys.stderr)
            return None
    # Linker symbols have now been consumed and stable marker exports published.
    # No index-changing transform may see reloc.CODE or the linking symbol table.
    source.replace(
        _operations.strip_publication_sections(
            source.data, final_artifact=True, preserve_debug=plan.preserve_debug
        )
    )
    try:
        optimized = _optimizer._optimize_split_app_module(
            source.data,
            reference_data=plan.output_data,
            optimize=plan.optimize,
            optimize_level=plan.optimize_level,
            contract_keep_set=set(plan.contract_keep_set),
            attestation=optimizer_attestation,
            operation_counts=operation_counts,
            facts_provider=facts_provider,
            optimizer_identity=optimizer_identity,
            preserve_debug=plan.preserve_debug,
        )
    except RuntimeError as exc:
        print(str(exc), file=sys.stderr)
        return None
    artifact = WasmArtifactState.from_bytes(
        destination,
        optimized,
        facts_provider=facts_provider,
    )
    artifact.persist()
    try:
        updated = _edit._rewrite_table_import_min(
            artifact.data,
            prepared.required_table_min,
        )
        if updated is not None:
            artifact.replace(updated)
        if plan.output_memory_min is not None:
            updated = _edit._rewrite_memory_min(
                artifact.data,
                plan.output_memory_min,
            )
            if updated is not None:
                artifact.replace(updated)
        artifact.replace(
            _export_contract._restore_split_runtime_contract_exports(
                artifact.data,
                artifact="app",
                stage="optimized-app",
                public_export_map=plan.public_export_map,
                required_native_direct_symbols=plan.required_native_direct_symbols,
                facts_provider=facts_provider,
            )
        )
        if plan.adapter_identity_map:
            _edit._validate_app_export_adapters(
                artifact.data,
                plan.contract_app_exports,
                adapter_symbol_map=plan.adapter_identity_map,
                target_symbol_map=plan.target_identity_map,
                facts_provider=facts_provider,
            )
            artifact.replace(
                _export_contract._strip_app_export_identity_markers(
                    artifact.data,
                    identity_exports=plan.identity_exports,
                    preserve_exports=set(plan.contract_keep_set),
                    facts_provider=facts_provider,
                )
            )
    except ValueError as exc:
        print(f"Failed to normalize optimized split app: {exc}", file=sys.stderr)
        return None
    if plan.has_native_objects:
        native_imports = facts_provider(artifact.data).module_imports("molt_native")
        if native_imports:
            print(
                "Split-runtime native link left unresolved molt_native import(s): "
                + ", ".join(sorted(native_imports)),
                file=sys.stderr,
            )
            return None
    return artifact


def _prepare_split_runtime_stage(
    deploy_runtime_data: bytes,
    destination: Path,
    app_artifact: WasmArtifactState,
    *,
    runtime_imports: Sequence[str],
    size_attestation: dict[str, object],
    operation_counts: dict[str, int | float],
    facts_provider: _facts.WasmFactsProvider,
    preserve_debug: bool,
) -> _PreparedSplitRuntime | None:
    """Derive the app-independent shared runtime publication once."""

    try:
        source_size = len(deploy_runtime_data)
        size_attestation["runtime_before"] = wasm_metrics(deploy_runtime_data)
        canonical_required_exports = (
            _runtime_data._canonical_split_runtime_required_exports(
                deploy_runtime_data,
                runtime_imports=runtime_imports,
                facts_provider=facts_provider,
            )
        )
        app_imports = app_artifact.facts().module_imports("molt_runtime")
        runtime_function_exports = facts_provider(deploy_runtime_data).function_exports
        missing_runtime_imports: list[str] = []
        for name in app_imports:
            export_name = wasm_split_runtime_export_name_for_import(name)
            if export_name is not None and export_name in runtime_function_exports:
                continue
            if export_name is None and name in runtime_function_exports:
                continue
            missing_runtime_imports.append(name)
        missing_runtime_imports.sort()
        if missing_runtime_imports:
            raise ValueError(
                "split-runtime app imports runtime symbols absent from the "
                "canonical shared-runtime export surface: "
                f"{missing_runtime_imports}"
            )
        print(
            f"App imports {len(app_imports)} functions from molt_runtime; "
            f"shaking shared runtime against {len(canonical_required_exports)} "
            "canonical exports (app-independent, CDN-cacheable)",
            file=sys.stderr,
        )
        shaken_runtime = _optimizer._tree_shake_runtime(
            deploy_runtime_data,
            canonical_required_exports,
            facts_provider=facts_provider,
            operation_counts=operation_counts,
            preserve_debug=preserve_debug,
        )
        artifact = WasmArtifactState.from_bytes(
            destination,
            shaken_runtime,
            facts_provider=facts_provider,
        )
        artifact.persist()
    except Exception as exc:
        print(f"Required runtime tree-shake failed: {exc}", file=sys.stderr)
        return None
    return _PreparedSplitRuntime(artifact=artifact, source_size=source_size)


def run_wasm_ld_with_custodied_inputs(
    wasm_ld: str,
    runtime: Path,
    output: Path,
    linked: Path,
    *,
    failure_evidence_dir: Path,
    allowlist_override: Path | None = None,
    optimize: bool = False,
    optimize_level: str = "Oz",
    freestanding: bool = False,
    split_runtime: bool = False,
    split_output_dir: Path | None = None,
    deploy_runtime_override: Path | None = None,
    deploy_runtime_imports: Sequence[str] | None = None,
    native_link_requirements: SourceExtensionLinkRequirements | None = None,
    provider_paths: Mapping[str, Path] | None = None,
    provider_symbols: frozenset[str] = frozenset(),
    host_provider_symbols: frozenset[str] = frozenset(),
    preserve_debug_sections: bool = False,
    phase_timings_ms: dict[str, float] | None = None,
    wasm_facts_scanner: Path,
    wasm_facts_scanner_sha256: str | None = None,
    facts_provider: _facts.WasmFactsProvider | None = None,
    app_export_contract_path: Path | None,
    link_receipt: FinalLinkReceiptRequest | None = None,
) -> int:
    if phase_timings_ms is None:
        phase_timings_ms = {}
    facts_metrics: dict[str, float] = {}
    operation_counts: dict[str, int | float] = {
        "wasm_whole_artifact_section_walks": 0,
        "wasm_whole_artifact_reserializations": 0,
        "wasm_optimizer_identity_resolutions": 0,
        "wasm_optimizer_identity_wall_ms": 0.0,
        **_optimizer._empty_wasm_link_cache_metrics(),
    }
    total_start = time.perf_counter()
    optimizer_identity: WasmOptimizerExecutableIdentity | None = None
    if optimize:
        identity_started = time.perf_counter()
        try:
            optimizer_identity = wasm_optimizer_invocation_identity()
        except WasmOptimizerIdentityError as exc:
            print(
                f"Required linked WASM optimization has no valid Binaryen identity: {exc}",
                file=sys.stderr,
            )
            return 1
        operation_counts["wasm_optimizer_identity_resolutions"] = 1
        operation_counts["wasm_optimizer_identity_wall_ms"] = round(
            (time.perf_counter() - identity_started) * 1000.0,
            6,
        )
    # The finally block records partial-failure timing even when lld rejects an
    # input before split processing begins.
    split_runtime_start = total_start
    expected_target = "wasm32-unknown-unknown" if freestanding else "wasm32-wasip1"
    native_link_requirements = (
        native_link_requirements or SourceExtensionLinkRequirements(expected_target)
    )
    try:
        if native_link_requirements.target_triple != expected_target:
            raise ValueError(
                f"native WASM link requirements target mismatch: {native_link_requirements.target_triple} != {expected_target}"
            )
        if split_runtime and deploy_runtime_imports is None:
            raise ValueError(
                "split-runtime link requires the deploy runtime build's generated ABI"
            )
        link_outputs = wasm_link_output_paths(
            linked,
            external_selection=bool(native_link_requirements.items),
            optimize=optimize,
            split_output_dir=(split_output_dir or linked.parent)
            if split_runtime
            else None,
            inputs=(
                runtime,
                output,
                *((deploy_runtime_override,) if deploy_runtime_override else ()),
                *(Path(item.path) for item in native_link_requirements.inputs),
                *((app_export_contract_path,) if app_export_contract_path else ()),
            ),
        )
    except (OSError, ValueError) as exc:
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        return 1
    native_objects = tuple(
        dict.fromkeys(Path(item.path) for item in native_link_requirements.inputs)
    )
    if app_export_contract_path is None:
        print(
            "Wasm link failed: frontend app export contract is required",
            file=sys.stderr,
        )
        return 1
    temp_dir = OwnedTemporaryDirectory(prefix="molt-wasm-link-")
    staged_outputs: list[Path] = []
    whole_artifact_counts = contextlib.ExitStack()
    try:
        whole_artifact_counts.enter_context(
            _operations.bind_whole_artifact_operation_counts(operation_counts)
        )
        loaded_contract = _load_link_contract_stage(
            runtime,
            output,
            app_export_contract_path=app_export_contract_path,
            wasm_facts_scanner=wasm_facts_scanner,
            wasm_facts_scanner_sha256=wasm_facts_scanner_sha256,
            facts_provider=facts_provider,
            temp_dir=temp_dir,
            facts_metrics=facts_metrics,
            failure_evidence_dir=failure_evidence_dir,
            split_runtime=split_runtime,
            deploy_runtime_override=deploy_runtime_override,
        )
        if loaded_contract is None:
            return 1
        runtime_exports = loaded_contract.runtime_exports
        output_data = loaded_contract.output_data
        app_export_contract = loaded_contract.app_export_contract
        app_call_abi = loaded_contract.app_call_abi
        facts_provider = loaded_contract.facts_provider
        output_callable_layout = loaded_contract.output_callable_layout
        split_callable_layout = loaded_contract.split_callable_layout
        monolithic_callable_layout = loaded_contract.monolithic_callable_layout
        deploy_runtime_path = loaded_contract.deploy_runtime_path
        output_memory_min = loaded_contract.output_memory_min
        output_table_min = loaded_contract.output_table_min
        callable_entry_export_names = loaded_contract.callable_entry_export_names
        reserved_runtime_link_exports = loaded_contract.reserved_runtime_link_exports

        prepared_inputs = _prepare_link_inputs_stage(
            output,
            runtime,
            output_data=output_data,
            runtime_exports=runtime_exports,
            app_export_contract=app_export_contract,
            app_call_abi=app_call_abi,
            callable_entry_export_names=callable_entry_export_names,
            native_link_requirements=native_link_requirements,
            provider_symbols=provider_symbols,
            host_provider_symbols=host_provider_symbols,
            split_runtime=split_runtime,
            allowlist_override=allowlist_override,
            temp_dir=temp_dir,
            facts_provider=facts_provider,
        )
        if prepared_inputs is None:
            return 1
        required_native_direct_symbols = prepared_inputs.required_native_direct_symbols
        export_symbol_map = prepared_inputs.export_symbol_map
        callable_entry_symbol_names = prepared_inputs.callable_entry_symbol_names
        callable_entry_symbol_names_by_slot = (
            prepared_inputs.callable_entry_symbol_names_by_slot
        )
        contract_app_exports = prepared_inputs.contract_app_exports
        app_target_symbol_map = prepared_inputs.app_target_symbol_map
        app_adapter_symbol_map = prepared_inputs.app_adapter_symbol_map
        app_adapter_identity_map = prepared_inputs.app_adapter_identity_map
        app_target_identity_map = prepared_inputs.app_target_identity_map
        app_identity_exports = prepared_inputs.app_identity_exports
        preserved_output_exports = prepared_inputs.preserved_output_exports
        user_export_symbol_names = prepared_inputs.user_export_symbol_names
        rewritten_path = prepared_inputs.rewritten_path
        rewritten_requirements = prepared_inputs.rewritten_requirements
        force_exports = prepared_inputs.force_exports
        base_allowlist = prepared_inputs.base_allowlist
        allowlist = prepared_inputs.allowlist
        linked_rewritten_path = prepared_inputs.linked_rewritten_path
        linked_requirements = prepared_inputs.linked_requirements

        work_linked = artifact_publish.staged_output_path(linked)
        staged_outputs.append(work_linked)
        app_wasm: Path | None = None
        rt_wasm: Path | None = None
        app_stage: Path | None = None
        rt_stage: Path | None = None
        app_artifact_state: WasmArtifactState | None = None
        runtime_artifact_state: WasmArtifactState | None = None
        size_attestation: dict[str, object] = {}
        size_attestation_path: Path | None = None
        size_attestation_stage: Path | None = None
        optimizer_attestation: dict[str, object] = {}
        app_optimizer_attestation: dict[str, object] = {}
        app_optimizer_attestation_path: Path | None = None
        app_optimizer_attestation_stage: Path | None = None
        optimizer_attestation_path = wasm_optimizer_attestation_path(linked)
        optimizer_attestation_stage = (
            artifact_publish.staged_output_path(optimizer_attestation_path)
            if optimize
            else None
        )
        if optimizer_attestation_stage is not None:
            staged_outputs.append(optimizer_attestation_stage)

        planned_commands = _plan_link_commands_stage(
            _LinkCommandInputs(
                wasm_ld=wasm_ld,
                runtime=runtime,
                allowlist=allowlist,
                base_allowlist=base_allowlist,
                output_callable_layout=output_callable_layout,
                split_callable_layout=split_callable_layout,
                force_exports=force_exports,
                required_native_direct_symbols=required_native_direct_symbols,
                user_export_symbol_names=user_export_symbol_names,
                callable_entry_symbol_names=callable_entry_symbol_names,
                reserved_runtime_link_exports=reserved_runtime_link_exports,
                work_linked=work_linked,
                linked_rewritten_path=linked_rewritten_path,
                rewritten_path=rewritten_path,
                linked_requirements=linked_requirements,
                rewritten_requirements=rewritten_requirements,
                native_objects=native_objects,
                native_link_requirements=native_link_requirements,
                provider_paths=provider_paths or {},
                host_provider_symbols=host_provider_symbols,
                link_outputs=link_outputs,
                runtime_exports=runtime_exports,
                output_data=output_data,
                split_runtime=split_runtime,
                deploy_runtime_path=deploy_runtime_path,
                temp_dir=temp_dir,
            ),
            operation_counts=operation_counts,
            facts_provider=facts_provider,
        )
        if planned_commands is None:
            return 1

        selection_roles: dict[str, Mapping[str, object]] = {}
        try:
            selection = (
                LinkSelectionAdmission.capture(native_link_requirements)
                if native_link_requirements.items
                else None
            )
            res = _run_admitted_link(
                planned_commands.monolithic,
                selection=selection,
                why_extract=Path(temp_dir.name) / "linked.why-extract",
                role="linked",
                selection_roles=selection_roles,
            )
        except (OSError, ValueError) as exc:
            print(f"WASM external member admission failed: {exc}", file=sys.stderr)
            return 1
        if res.returncode != 0:
            err = res.stderr.strip() or res.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            return res.returncode
        signature_mismatch = _command._wasm_ld_signature_mismatch_warning(res.stderr)
        if signature_mismatch is not None:
            print(signature_mismatch, file=sys.stderr)
            return 1
        prepared_linked = _prepare_linked_artifact_stage(
            work_linked,
            _LinkedArtifactPlan(
                reference_data=output_data,
                callable_layout=output_callable_layout,
                callable_entry_symbol_names=callable_entry_symbol_names_by_slot,
                preserved_output_exports=preserved_output_exports,
                export_symbol_map=export_symbol_map,
                required_native_direct_symbols=required_native_direct_symbols,
                contract_app_exports=contract_app_exports,
                adapter_symbol_map=app_adapter_symbol_map,
                adapter_identity_map=app_adapter_identity_map,
                target_symbol_map=app_target_symbol_map,
                target_identity_map=app_target_identity_map,
                identity_exports=app_identity_exports,
                preserve_debug=preserve_debug_sections,
                optimize=optimize,
                optimize_level=optimize_level,
                split_runtime=split_runtime,
                output_table_min=output_table_min,
                output_memory_min=output_memory_min,
                freestanding=freestanding,
            ),
            facts_provider=facts_provider,
            optimizer_attestation=optimizer_attestation,
            optimizer_identity=optimizer_identity,
        )
        if prepared_linked is None:
            return 1
        linked_state = prepared_linked.artifact
        public_export_map = prepared_linked.public_export_map
        split_app_contract_keep_set = prepared_linked.split_app_contract_keep_set
        # -- Split-runtime: emit app.wasm + molt_runtime.wasm ---------------
        split_runtime_start = time.perf_counter()
        if split_runtime:
            out_dir = split_output_dir or linked.parent
            out_dir.mkdir(parents=True, exist_ok=True)

            app_wasm = link_outputs["app"]
            app_optimizer_attestation_path = wasm_optimizer_attestation_path(app_wasm)
            if optimize:
                app_optimizer_attestation_stage = artifact_publish.staged_output_path(
                    app_optimizer_attestation_path
                )
                staged_outputs.append(app_optimizer_attestation_stage)
            rt_wasm = link_outputs["runtime"]
            app_stage = artifact_publish.staged_output_path(app_wasm)
            rt_stage = artifact_publish.staged_output_path(rt_wasm)
            size_attestation_path = link_outputs["size_attestation"]
            size_attestation_stage = artifact_publish.staged_output_path(
                size_attestation_path
            )
            staged_outputs.extend([app_stage, rt_stage, size_attestation_stage])

            assert planned_commands.split is not None
            split_link = planned_commands.split
            assert split_callable_layout is not None
            link_result = _link_split_app_stage(
                _SplitAppLinkPlan(
                    command=split_link.command,
                    output_data=output_data,
                    planned_data_base=split_link.data_base,
                    callable_layout=split_callable_layout,
                    callable_entry_symbol_names=callable_entry_symbol_names_by_slot,
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                    runtime_exports=frozenset(runtime_exports),
                    provider_symbols=provider_symbols,
                    data_alias_plan=split_link.data_alias_plan,
                    got_runtime_addresses=split_link.got_runtime_addresses,
                    failure_evidence_dir=failure_evidence_dir,
                ),
                split_link.linked_path,
                facts_provider=facts_provider,
                size_attestation=size_attestation,
                operation_counts=operation_counts,
                selection=selection,
                selection_roles=selection_roles,
                why_extract=Path(temp_dir.name) / "app.why-extract",
            )
            if link_result.prepared is None:
                return link_result.returncode or 1
            app_artifact_state = _optimize_split_app_stage(
                link_result.prepared,
                app_stage,
                _SplitAppOptimizationPlan(
                    output_data=output_data,
                    optimize=optimize,
                    optimize_level=optimize_level,
                    contract_keep_set=split_app_contract_keep_set.union(
                        app_identity_exports
                    ),
                    contract_app_exports=contract_app_exports,
                    adapter_symbol_map=app_adapter_symbol_map,
                    adapter_identity_map=app_adapter_identity_map,
                    target_symbol_map=app_target_symbol_map,
                    target_identity_map=app_target_identity_map,
                    identity_exports=app_identity_exports,
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                    output_memory_min=output_memory_min,
                    preserve_debug=preserve_debug_sections,
                    has_native_objects=bool(native_objects),
                ),
                optimizer_attestation=app_optimizer_attestation,
                operation_counts=operation_counts,
                facts_provider=facts_provider,
                optimizer_identity=optimizer_identity,
            )
            if app_artifact_state is None:
                return 1
            assert deploy_runtime_imports is not None
            prepared_runtime = _prepare_split_runtime_stage(
                split_link.deploy_runtime_data,
                rt_stage,
                app_artifact_state,
                runtime_imports=deploy_runtime_imports,
                size_attestation=size_attestation,
                operation_counts=operation_counts,
                facts_provider=facts_provider,
                preserve_debug=preserve_debug_sections,
            )
            if prepared_runtime is None:
                return 1
            runtime_artifact_state = prepared_runtime.artifact

            app_size = len(app_artifact_state.data)
            rt_size = len(runtime_artifact_state.data)
            total = app_size + rt_size
            print(
                f"Split-runtime output: "
                f"{app_wasm.name} ({app_size:,} bytes, {app_size // 1024}KB) + "
                f"{rt_wasm.name} ({rt_size:,} bytes, {rt_size // 1024}KB) = "
                f"{total:,} bytes total "
                f"(runtime: {prepared_runtime.source_size:,} -> {rt_size:,}, "
                f"{(1 - rt_size / prepared_runtime.source_size) * 100:.0f}% reduction)",
                file=sys.stderr,
            )
        if split_runtime:
            phase_timings_ms["split_runtime_processing"] = round(
                max(0.0, (time.perf_counter() - split_runtime_start) * 1000.0), 6
            )

        validation_start = time.perf_counter()
        if freestanding:
            if not _validation._validate_freestanding(
                linked_state.data,
                facts_provider=facts_provider,
            ):
                return 1
        phase_timings_ms["fail_closed_validation"] = round(
            max(0.0, (time.perf_counter() - validation_start) * 1000.0), 6
        )
        split_publication: _SplitPublication | None = None
        if split_runtime:
            assert app_stage is not None
            assert rt_stage is not None
            assert app_artifact_state is not None
            assert runtime_artifact_state is not None
            assert app_wasm is not None
            assert rt_wasm is not None
            assert size_attestation_path is not None
            assert size_attestation_stage is not None
            assert split_callable_layout is not None
            assert app_optimizer_attestation_path is not None
            split_publication = _SplitPublication(
                app=app_artifact_state,
                runtime=runtime_artifact_state,
                app_destination=app_wasm,
                runtime_destination=rt_wasm,
                size_attestation=size_attestation,
                size_attestation_path=size_attestation_path,
                size_attestation_stage=size_attestation_stage,
                callable_layout=split_callable_layout,
                optimizer_attestation=app_optimizer_attestation,
                optimizer_attestation_path=app_optimizer_attestation_path,
                optimizer_attestation_stage=app_optimizer_attestation_stage,
            )
        if not _publish_final_outputs_stage(
            linked_state,
            linked,
            monolithic_callable_layout=monolithic_callable_layout,
            preserve_debug=preserve_debug_sections,
            app_export_contract=app_export_contract,
            public_export_map=public_export_map,
            required_native_direct_symbols=required_native_direct_symbols,
            optimize=optimize,
            optimizer_attestation=optimizer_attestation,
            optimizer_attestation_path=optimizer_attestation_path,
            optimizer_attestation_stage=optimizer_attestation_stage,
            split=split_publication,
            phase_timings_ms=phase_timings_ms,
            link_receipt=link_receipt,
            link_outputs=link_outputs,
            selection_roles=selection_roles,
            staged_outputs=staged_outputs,
            failure_evidence_dir=failure_evidence_dir,
        ):
            return 1

        return 0
    finally:
        try:
            if split_runtime and "split_runtime_processing" not in phase_timings_ms:
                phase_timings_ms["split_runtime_processing"] = round(
                    max(0.0, (time.perf_counter() - split_runtime_start) * 1000.0),
                    6,
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
        finally:
            whole_artifact_counts.close()
            for staged_output in staged_outputs:
                with contextlib.suppress(OSError):
                    staged_output.unlink()
            temp_dir.cleanup()

from __future__ import annotations

from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_import_scan import SourceImportPurpose

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

from dataclasses import dataclass

import ast
import contextlib
import hashlib
import json
import os
from collections.abc import Collection, Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any, cast

from molt.frontend import MoltValue, SimpleTIRGenerator
from molt.frontend import BoundMethodHint, ClassId, ClassInfo, DeclaredClass
from molt.frontend.module_publication import (
    parse_function_metadata,
    inspect_source_module_publication,
)
from molt.frontend.sema import collect_module_func_kinds
from molt.frontend.sema.funcmeta import collect_module_func_defaults
from molt.compiler_analysis.native_support_slice import canonical_native_support_roots
from molt.type_facts import TypeFacts

from molt.cli.cache_fingerprints import (
    _frontend_semantic_tooling_fingerprint,
    _source_tree_fingerprint_transaction,
)
from molt.cli.cache_keys import (
    _cache_json_value,
    _decode_class_identity,
    _encode_class_identity,
    _json_ir_default,
)
from molt.opcode_literal_payloads import integer_literal_value
from molt.cli.function_references import (
    format_function_reference_edges,
    missing_local_function_references,
)
from molt.cli.json_cache import (
    SOURCE_ANALYSIS_CACHE_MAX_BYTES,
)
from molt.cli.build_request import TypeHintPolicy
from molt.cli.models import (
    _RuntimeImportScanCustody,
    ImportScanMode,
    ModuleExecutionKind,
    ModuleSourceRole,
    _ModuleGraphMetadata,
    _ScopedLoweringInputView,
    _ScopedLoweringInputs,
)
from molt.cli.module_dependencies import _module_dependency_closure
from molt.cli.module_graph_discovery import _load_module_import_scan
from molt.cli.source_record_store import shared_source_records
from molt.cli.function_records import (
    FunctionDigestSeed,
    encode_lowering_record,
    decode_lowering_record,
    decode_function_records,
)
from molt.cli.module_graph_cache import (
    _resolved_module_cache_key,
    _namespace_premise_cache_identity,
)
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import (
    PythonSourceSnapshot,
    _ModuleSourceLease,
    _payload_source_matches,
)
from molt.target_python import TargetPythonVersion, _DEFAULT_TARGET_PYTHON_VERSION


@dataclass(frozen=True)
class _ModuleLoweringMetadataView:
    logical_source_path: str
    is_package: bool
    module_execution_kind: ModuleExecutionKind
    module_source_role: ModuleSourceRole
    namespace_premise: PythonNamespacePremise | None


@dataclass(frozen=True)
class _ModuleLoweringExecutionView:
    metadata: _ModuleLoweringMetadataView
    scoped_inputs: _ScopedLoweringInputView
    scoped_known_classes: dict[ClassId, ClassInfo]


def _collect_func_defaults(tree: ast.AST) -> dict[str, dict[str, Any]]:
    if not isinstance(tree, ast.Module):
        return {}
    return collect_module_func_defaults(tree)


def _collect_func_kinds(tree: ast.AST) -> dict[str, str]:
    if not isinstance(tree, ast.Module):
        return {}
    return {name: kind.value for name, kind in collect_module_func_kinds(tree).items()}


def _scoped_known_func_defaults(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> dict[str, dict[str, dict[str, Any]]]:
    scoped_names = module_dep_closures.get(module_name) if module_dep_closures else None
    if scoped_names is None:
        scoped_names = _module_dependency_closure(module_name, module_deps)
    return {
        name: known_func_defaults[name]
        for name in sorted(scoped_names)
        if name in known_func_defaults
    }


def _scoped_known_func_kinds(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_func_kinds: dict[str, dict[str, str]],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> dict[str, dict[str, str]]:
    scoped_names = module_dep_closures.get(module_name) if module_dep_closures else None
    if scoped_names is None:
        scoped_names = _module_dependency_closure(module_name, module_deps)
    return {
        name: known_func_kinds[name]
        for name in sorted(scoped_names)
        if name in known_func_kinds
    }


def _scoped_known_modules(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_modules: Collection[str],
    always_known_modules: Collection[str] = (),
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> tuple[str, ...]:
    scoped_names = module_dep_closures.get(module_name) if module_dep_closures else None
    if scoped_names is None:
        scoped_names = _module_dependency_closure(module_name, module_deps)
    known_modules_set = set(known_modules)
    always_known_modules_set = set(always_known_modules)
    return tuple(
        sorted(
            {
                name
                for name in scoped_names
                if name == module_name or name in known_modules_set
            }
            | always_known_modules_set
        )
    )


def _scoped_direct_call_modules(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    direct_call_modules: Collection[str],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> tuple[str, ...]:
    scoped_names = module_dep_closures.get(module_name) if module_dep_closures else None
    if scoped_names is None:
        scoped_names = _module_dependency_closure(module_name, module_deps)
    direct_call_modules_set = set(direct_call_modules)
    return tuple(
        sorted(name for name in scoped_names if name in direct_call_modules_set)
    )


def _always_known_modules(
    known_modules: Collection[str],
    source_modules: Collection[str],
) -> frozenset[str]:
    return frozenset(set(known_modules) - set(source_modules))


def _scoped_known_classes(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_classes: Mapping[ClassId, ClassInfo],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> dict[ClassId, ClassInfo]:
    scoped_modules = (
        module_dep_closures.get(module_name) if module_dep_closures else None
    )
    if scoped_modules is None:
        scoped_modules = _module_dependency_closure(module_name, module_deps)
    return {
        class_id: class_info
        for class_id, class_info in known_classes.items()
        if class_id.module in scoped_modules
    }


def _scoped_type_facts(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    type_facts: TypeFacts | None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> TypeFacts | None:
    if type_facts is None:
        return None
    scoped_modules = (
        module_dep_closures.get(module_name) if module_dep_closures else None
    )
    if scoped_modules is None:
        scoped_modules = _module_dependency_closure(module_name, module_deps)
    modules = getattr(type_facts, "modules", None)
    if not isinstance(modules, dict):
        return type_facts
    filtered_modules = {
        name: module for name, module in modules.items() if name in scoped_modules
    }
    if len(filtered_modules) == len(modules):
        return type_facts
    return TypeFacts(
        schema_version=type_facts.schema_version,
        created_at=type_facts.created_at,
        tool=type_facts.tool,
        strict=type_facts.strict,
        modules=filtered_modules,
    )


def _native_callable_export_module(
    qualified_name: str,
    spec: Mapping[str, Any],
) -> str:
    module = spec.get("module")
    if isinstance(module, str) and module.strip():
        return module.strip()
    name = spec.get("name")
    if isinstance(name, str) and qualified_name.endswith("." + name):
        return qualified_name[: -(len(name) + 1)]
    return qualified_name.rsplit(".", 1)[0]


def _native_python_export_module(qualified_name: str) -> str:
    return qualified_name.rsplit(".", 1)[0]


def _native_callable_exports_payload(
    native_callable_exports: Mapping[str, Mapping[str, Any]],
) -> dict[str, dict[str, Any]]:
    return {
        qualified_name: dict(native_callable_exports[qualified_name])
        for qualified_name in sorted(native_callable_exports)
    }


def _scoped_native_callable_exports(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    native_callable_exports: Mapping[str, Mapping[str, Any]],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> dict[str, dict[str, Any]]:
    if not native_callable_exports:
        return {}
    scoped_modules = (
        module_dep_closures.get(module_name) if module_dep_closures else None
    )
    if scoped_modules is None:
        scoped_modules = _module_dependency_closure(module_name, module_deps)
    scoped: dict[str, dict[str, Any]] = {}
    for qualified_name, spec in sorted(native_callable_exports.items()):
        export_module = _native_callable_export_module(qualified_name, spec)
        provider_module = spec.get("provider_module")
        provider_module_name = (
            provider_module.strip()
            if isinstance(provider_module, str) and provider_module.strip()
            else None
        )
        if export_module in scoped_modules or provider_module_name in scoped_modules:
            scoped[qualified_name] = dict(spec)
    return scoped


def _scoped_native_python_exports(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    native_python_exports: Collection[str],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> tuple[str, ...]:
    if not native_python_exports:
        return ()
    scoped_modules = (
        module_dep_closures.get(module_name) if module_dep_closures else None
    )
    if scoped_modules is None:
        scoped_modules = _module_dependency_closure(module_name, module_deps)
    return tuple(
        sorted(
            qualified_name
            for qualified_name in native_python_exports
            if _native_python_export_module(qualified_name) in scoped_modules
        )
    )


def _scoped_native_support_function_roots(
    module_name: str,
    roots_by_module: Mapping[str, Sequence[str]] | None,
) -> tuple[str, ...]:
    if not roots_by_module:
        return ()
    roots = roots_by_module.get(module_name)
    if roots is None:
        return ()
    return tuple(sorted({root for root in roots if isinstance(root, str) and root}))


def _build_scoped_lowering_inputs(
    module_names: Collection[str],
    *,
    module_deps: dict[str, set[str]],
    module_dep_closures: dict[str, frozenset[str]],
    known_modules: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    pgo_hot_function_names: Collection[str],
    type_facts: TypeFacts | None,
    direct_call_modules: Collection[str] | None = None,
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] | None = None,
    native_support_function_roots_by_module: Mapping[str, Sequence[str]] | None = None,
) -> _ScopedLoweringInputs:
    scoped_known_modules_by_module: dict[str, tuple[str, ...]] = {}
    scoped_direct_call_modules_by_module: dict[str, tuple[str, ...]] = {}
    scoped_known_func_defaults_by_module: dict[str, dict[str, dict[str, Any]]] = {}
    scoped_known_func_kinds_by_module: dict[str, dict[str, dict[str, str]]] = {}
    scoped_native_callable_exports_by_module: dict[str, dict[str, dict[str, Any]]] = {}
    scoped_native_python_exports_by_module: dict[str, tuple[str, ...]] = {}
    scoped_native_support_function_roots_by_module: dict[str, tuple[str, ...]] = {}
    scoped_pgo_hot_function_names_by_module: dict[str, tuple[str, ...]] = {}
    scoped_type_facts_by_module: dict[str, TypeFacts | None] = {}
    source_modules = frozenset(module_names)
    always_known_modules = _always_known_modules(known_modules, source_modules)
    direct_call_scope_source = (
        source_modules if direct_call_modules is None else direct_call_modules
    )
    native_callable_scope_source = native_callable_exports or {}
    native_python_scope_source = native_python_exports or ()
    for module_name in sorted(module_names):
        scoped_known_modules_by_module[module_name] = _scoped_known_modules(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules,
            always_known_modules=always_known_modules,
            module_dep_closures=module_dep_closures,
        )
        scoped_direct_call_modules_by_module[module_name] = _scoped_direct_call_modules(
            module_name,
            module_deps=module_deps,
            direct_call_modules=direct_call_scope_source,
            module_dep_closures=module_dep_closures,
        )
        scoped_known_func_defaults_by_module[module_name] = _scoped_known_func_defaults(
            module_name,
            module_deps=module_deps,
            known_func_defaults=known_func_defaults,
            module_dep_closures=module_dep_closures,
        )
        scoped_known_func_kinds_by_module[module_name] = _scoped_known_func_kinds(
            module_name,
            module_deps=module_deps,
            known_func_kinds=known_func_kinds,
            module_dep_closures=module_dep_closures,
        )
        scoped_native_callable_exports_by_module[module_name] = (
            _scoped_native_callable_exports(
                module_name,
                module_deps=module_deps,
                native_callable_exports=native_callable_scope_source,
                module_dep_closures=module_dep_closures,
            )
        )
        scoped_native_python_exports_by_module[module_name] = (
            _scoped_native_python_exports(
                module_name,
                module_deps=module_deps,
                native_python_exports=native_python_scope_source,
                module_dep_closures=module_dep_closures,
            )
        )
        scoped_native_support_function_roots_by_module[module_name] = (
            _scoped_native_support_function_roots(
                module_name,
                native_support_function_roots_by_module,
            )
        )
        scoped_pgo_hot_function_names_by_module[module_name] = (
            _scoped_pgo_hot_function_names(module_name, pgo_hot_function_names)
        )
        scoped_type_facts_by_module[module_name] = _scoped_type_facts(
            module_name,
            module_deps=module_deps,
            type_facts=type_facts,
            module_dep_closures=module_dep_closures,
        )
    return _ScopedLoweringInputs(
        known_modules_by_module=scoped_known_modules_by_module,
        direct_call_modules_by_module=scoped_direct_call_modules_by_module,
        known_func_defaults_by_module=scoped_known_func_defaults_by_module,
        known_func_kinds_by_module=scoped_known_func_kinds_by_module,
        native_callable_exports_by_module=scoped_native_callable_exports_by_module,
        native_python_exports_by_module=scoped_native_python_exports_by_module,
        native_support_function_roots_by_module=(
            scoped_native_support_function_roots_by_module
        ),
        pgo_hot_function_names_by_module=scoped_pgo_hot_function_names_by_module,
        type_facts_by_module=scoped_type_facts_by_module,
    )


def _build_scoped_known_classes_snapshot(
    module_names: Collection[str],
    *,
    module_deps: dict[str, set[str]],
    module_dep_closures: dict[str, frozenset[str]],
    known_classes_snapshot: Mapping[ClassId, ClassInfo],
) -> dict[str, dict[ClassId, ClassInfo]]:
    scoped_known_classes_by_module: dict[str, dict[ClassId, ClassInfo]] = {}
    for module_name in sorted(module_names):
        scoped_known_classes_by_module[module_name] = _scoped_known_classes(
            module_name,
            module_deps=module_deps,
            known_classes=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
        )
    return scoped_known_classes_by_module


def _scoped_known_classes_view(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_classes_snapshot: Mapping[ClassId, ClassInfo],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[ClassId, ClassInfo]]
    | None = None,
) -> dict[ClassId, ClassInfo]:
    if (
        scoped_known_classes_by_module is not None
        and module_name in scoped_known_classes_by_module
    ):
        return scoped_known_classes_by_module[module_name]
    return _scoped_known_classes(
        module_name,
        module_deps=module_deps,
        known_classes=known_classes_snapshot,
        module_dep_closures=module_dep_closures,
    )


def _scoped_lowering_input_view(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_modules: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    pgo_hot_function_names: Collection[str],
    type_facts: TypeFacts | None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    known_modules_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    source_modules: Collection[str] | None = None,
    direct_call_modules: Collection[str] | None = None,
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] | None = None,
    native_support_function_roots_by_module: Mapping[str, Sequence[str]] | None = None,
) -> _ScopedLoweringInputView:
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.known_modules_by_module
    ):
        scoped_known_modules = scoped_lowering_inputs.known_modules_by_module[
            module_name
        ]
    else:
        known_modules_scope_source: Collection[str]
        if known_modules_sorted is None:
            known_modules_scope_source = known_modules
        else:
            known_modules_scope_source = known_modules_sorted
        always_known_modules: Collection[str] = ()
        if source_modules is not None:
            always_known_modules = _always_known_modules(
                known_modules_scope_source,
                source_modules,
            )
        scoped_known_modules = _scoped_known_modules(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules_scope_source,
            always_known_modules=always_known_modules,
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.direct_call_modules_by_module
    ):
        scoped_direct_call_modules = (
            scoped_lowering_inputs.direct_call_modules_by_module[module_name]
        )
    else:
        direct_call_scope_source = (
            (source_modules if source_modules is not None else (module_name,))
            if direct_call_modules is None
            else direct_call_modules
        )
        scoped_direct_call_modules = _scoped_direct_call_modules(
            module_name,
            module_deps=module_deps,
            direct_call_modules=direct_call_scope_source,
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.known_func_defaults_by_module
    ):
        scoped_known_func_defaults = (
            scoped_lowering_inputs.known_func_defaults_by_module[module_name]
        )
    else:
        scoped_known_func_defaults = _scoped_known_func_defaults(
            module_name,
            module_deps=module_deps,
            known_func_defaults=known_func_defaults,
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.known_func_kinds_by_module
    ):
        scoped_known_func_kinds = scoped_lowering_inputs.known_func_kinds_by_module[
            module_name
        ]
    else:
        scoped_known_func_kinds = _scoped_known_func_kinds(
            module_name,
            module_deps=module_deps,
            known_func_kinds=known_func_kinds,
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.native_callable_exports_by_module
    ):
        scoped_native_callable_exports = (
            scoped_lowering_inputs.native_callable_exports_by_module[module_name]
        )
    else:
        scoped_native_callable_exports = _scoped_native_callable_exports(
            module_name,
            module_deps=module_deps,
            native_callable_exports=native_callable_exports or {},
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.native_python_exports_by_module
    ):
        scoped_native_python_exports = (
            scoped_lowering_inputs.native_python_exports_by_module[module_name]
        )
    else:
        scoped_native_python_exports = _scoped_native_python_exports(
            module_name,
            module_deps=module_deps,
            native_python_exports=native_python_exports or (),
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name
        in scoped_lowering_inputs.native_support_function_roots_by_module
    ):
        scoped_native_support_function_roots = (
            scoped_lowering_inputs.native_support_function_roots_by_module[module_name]
        )
    else:
        scoped_native_support_function_roots = _scoped_native_support_function_roots(
            module_name,
            native_support_function_roots_by_module,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.pgo_hot_function_names_by_module
    ):
        scoped_pgo_hot_function_names = (
            scoped_lowering_inputs.pgo_hot_function_names_by_module[module_name]
        )
    else:
        pgo_hot_functions_scope_source: Collection[str]
        if pgo_hot_function_names_sorted is None:
            pgo_hot_functions_scope_source = pgo_hot_function_names
        else:
            pgo_hot_functions_scope_source = pgo_hot_function_names_sorted
        scoped_pgo_hot_function_names = _scoped_pgo_hot_function_names(
            module_name,
            pgo_hot_functions_scope_source,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.type_facts_by_module
    ):
        scoped_type_facts = scoped_lowering_inputs.type_facts_by_module[module_name]
    else:
        scoped_type_facts = _scoped_type_facts(
            module_name,
            module_deps=module_deps,
            type_facts=type_facts,
            module_dep_closures=module_dep_closures,
        )
    return _ScopedLoweringInputView(
        known_modules=scoped_known_modules,
        direct_call_modules=scoped_direct_call_modules,
        known_func_defaults=scoped_known_func_defaults,
        known_func_kinds=scoped_known_func_kinds,
        native_callable_exports=scoped_native_callable_exports,
        native_python_exports=scoped_native_python_exports,
        body_selection=ModuleBodySelection.from_requested_roots(
            scoped_native_support_function_roots
        ),
        pgo_hot_function_names=scoped_pgo_hot_function_names,
        type_facts=scoped_type_facts,
        known_modules_payload=list(scoped_known_modules),
        known_modules_set=frozenset(scoped_known_modules),
        direct_call_modules_payload=list(scoped_direct_call_modules),
        direct_call_modules_set=frozenset(scoped_direct_call_modules),
        native_callable_exports_payload=_native_callable_exports_payload(
            scoped_native_callable_exports
        ),
        native_python_exports_payload=list(scoped_native_python_exports),
        native_python_exports_set=frozenset(scoped_native_python_exports),
        pgo_hot_function_names_payload=list(scoped_pgo_hot_function_names),
        pgo_hot_function_names_set=frozenset(scoped_pgo_hot_function_names),
    )


def _scoped_pgo_hot_function_names(
    module_name: str,
    pgo_hot_function_names: Collection[str],
) -> tuple[str, ...]:
    if not pgo_hot_function_names:
        return ()
    module_prefix_a = f"{module_name}::"
    module_prefix_b = f"{module_name}."
    init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
    scoped = {
        symbol
        for symbol in pgo_hot_function_names
        if symbol.startswith(module_prefix_a)
        or symbol.startswith(module_prefix_b)
        or symbol == init_symbol
        or symbol == f"{module_name}::{init_symbol}"
        or symbol == f"{module_name}.{init_symbol}"
    }
    return tuple(sorted(scoped))


def _module_lowering_metadata_view(
    module_name: str,
    *,
    module_graph_metadata: _ModuleGraphMetadata,
) -> _ModuleLoweringMetadataView:
    return _ModuleLoweringMetadataView(
        namespace_premise=(
            module_graph_metadata.scan_authority.by_module[
                module_name
            ].namespace_premise
            if module_graph_metadata.scan_authority is not None
            else None
        ),
        module_source_role=module_graph_metadata.module_source_role_by_module[
            module_name
        ],
        logical_source_path=module_graph_metadata.logical_source_path_by_module[
            module_name
        ],
        is_package=module_graph_metadata.module_is_package_by_module[module_name],
        module_execution_kind=module_graph_metadata.module_execution_kind_by_module[
            module_name
        ],
    )


def _module_lowering_execution_view(
    module_name: str,
    *,
    module_graph_metadata: _ModuleGraphMetadata,
    module_deps: dict[str, set[str]],
    known_modules: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    pgo_hot_function_names: Collection[str],
    type_facts: TypeFacts | None,
    known_classes_snapshot: Mapping[ClassId, ClassInfo],
    module_dep_closures: dict[str, frozenset[str]],
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    known_modules_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[ClassId, ClassInfo]]
    | None = None,
    source_modules: Collection[str] | None = None,
    direct_call_modules: Collection[str] | None = None,
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] | None = None,
) -> _ModuleLoweringExecutionView:
    metadata = _module_lowering_metadata_view(
        module_name,
        module_graph_metadata=module_graph_metadata,
    )
    scoped_inputs = _scoped_lowering_input_view(
        module_name,
        module_deps=module_deps,
        known_modules=known_modules,
        direct_call_modules=direct_call_modules,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        native_callable_exports=native_callable_exports,
        native_python_exports=native_python_exports,
        pgo_hot_function_names=pgo_hot_function_names,
        type_facts=type_facts,
        module_dep_closures=module_dep_closures,
        scoped_lowering_inputs=scoped_lowering_inputs,
        known_modules_sorted=known_modules_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        source_modules=source_modules,
    )
    scoped_known_classes = _scoped_known_classes_view(
        module_name,
        module_deps=module_deps,
        known_classes_snapshot=known_classes_snapshot,
        module_dep_closures=module_dep_closures,
        scoped_known_classes_by_module=scoped_known_classes_by_module,
    )
    return _ModuleLoweringExecutionView(
        metadata=metadata,
        scoped_inputs=scoped_inputs,
        scoped_known_classes=scoped_known_classes,
    )


def _frontend_lowering_cache_disabled() -> bool:
    """Disable all persistent frontend result reads and publication."""
    return os.environ.get(
        "MOLT_DISABLE_FRONTEND_LOWERING_CACHE", ""
    ).strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }


_MODULE_ANALYSIS_CACHE_SCHEMA_VERSION = 13


_MODULE_LOWERING_CACHE_SCHEMA_VERSION = 10


_MODULE_ANALYSIS_FUNC_KINDS = frozenset({"sync", "async", "gen", "asyncgen"})


def _module_analysis_record_key(
    path: Path,
    *,
    module_name: str,
    is_package: bool | None,
    import_scan_mode: ImportScanMode,
    target_python: TargetPythonVersion,
    native_support_function_roots: tuple[str, ...] = (),
) -> str:
    """Source/request identity; source bytes are qualified by the reader."""
    package_kind = "pkg" if is_package else "mod" if is_package is not None else "-"
    key_parts = [
        module_name,
        package_kind,
        import_scan_mode,
        repr(ModuleBodySelection.from_requested_roots(native_support_function_roots)),
        "module-analysis",
        target_python.tag,
        _frontend_semantic_tooling_fingerprint(),
    ]
    return _resolved_module_cache_key(
        os.fspath(path),
        *key_parts,
    )


def _validate_persisted_module_analysis_payload(
    payload: Mapping[str, Any] | None,
    path: Path,
    *,
    import_scan_mode: ImportScanMode,
    path_stat: os.stat_result | None,
    snapshot: PythonSourceSnapshot | None = None,
    native_support_function_roots: tuple[str, ...] = (),
) -> tuple[dict[str, dict[str, Any]], dict[str, str]] | None:
    """Admit analysis only after schema, semantic and source checks pass."""
    if payload is None:
        return None
    if (
        payload.get("version") != _MODULE_ANALYSIS_CACHE_SCHEMA_VERSION
        or payload.get("compiler_fingerprint")
        != _frontend_semantic_tooling_fingerprint()
        or payload.get("import_scan_mode") != import_scan_mode
        or payload.get("body_selection")
        != ModuleBodySelection.from_requested_roots(
            native_support_function_roots
        ).payload()
        or "imports" in payload
    ):
        return None
    try:
        raw_defaults = _decode_cached_json_value(payload.get("func_defaults"))
        raw_kinds = _decode_cached_json_value(payload.get("func_kinds"))
    except ValueError:
        return None
    if not isinstance(raw_defaults, dict):
        return None
    if not isinstance(raw_kinds, dict) or not all(
        isinstance(name, str)
        and isinstance(kind, str)
        and kind in _MODULE_ANALYSIS_FUNC_KINDS
        for name, kind in raw_kinds.items()
    ):
        return None
    if snapshot is not None:
        if (
            snapshot.path.resolve() != path.resolve()
            or payload.get("size") != len(snapshot.content)
            or payload.get("source_sha256") != snapshot.sha256
        ):
            return None
    else:
        if path_stat is None:
            try:
                path_stat = path.stat()
            except OSError:
                return None
        if not _payload_source_matches(payload, path, path_stat):
            return None
    normalized: dict[str, dict[str, Any]] = {}
    for func_name, func_payload in raw_defaults.items():
        if not isinstance(func_name, str) or not isinstance(func_payload, dict):
            return None
        if not _validate_module_func_default_payload(func_payload):
            return None
        normalized[func_name] = func_payload
    return normalized, dict(cast(dict[str, str], raw_kinds))


@_source_tree_fingerprint_transaction()
def _read_persisted_module_analysis(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    path_stat: os.stat_result | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    snapshot: PythonSourceSnapshot | None = None,
    native_support_function_roots: tuple[str, ...] = (),
) -> tuple[dict[str, dict[str, Any]], dict[str, str]] | None:
    if _frontend_lowering_cache_disabled():
        return None
    key = _module_analysis_record_key(
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        native_support_function_roots=native_support_function_roots,
    )
    record = shared_source_records().get(
        "module-analysis",
        key,
        generation=_frontend_semantic_tooling_fingerprint(),
        max_bytes=SOURCE_ANALYSIS_CACHE_MAX_BYTES,
    )
    if record is None:
        return None
    try:
        payload = json.loads(record.payload)
    except (ValueError, RecursionError):
        return None
    if not isinstance(payload, dict):
        return None
    result = _validate_persisted_module_analysis_payload(
        payload,
        path,
        import_scan_mode=import_scan_mode,
        path_stat=path_stat,
        snapshot=snapshot,
        native_support_function_roots=native_support_function_roots,
    )
    if result is not None:
        record.refresh()
    return result


@_source_tree_fingerprint_transaction()
def _write_persisted_module_analysis(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    func_defaults: dict[str, dict[str, Any]],
    func_kinds: dict[str, str],
    snapshot: PythonSourceSnapshot,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    native_support_function_roots: tuple[str, ...] = (),
) -> None:
    if _frontend_lowering_cache_disabled():
        return
    stat = path.stat()
    if snapshot.path.resolve() != path.resolve() or not _payload_source_matches(
        {"size": len(snapshot.content), "source_sha256": snapshot.sha256}, path, stat
    ):
        return
    key = _module_analysis_record_key(
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        native_support_function_roots=native_support_function_roots,
    )
    payload = {
        "version": _MODULE_ANALYSIS_CACHE_SCHEMA_VERSION,
        "compiler_fingerprint": _frontend_semantic_tooling_fingerprint(),
        "module_name": module_name,
        "is_package": is_package,
        "import_scan_mode": import_scan_mode,
        "body_selection": ModuleBodySelection.from_requested_roots(
            native_support_function_roots
        ).payload(),
        "target_python": target_python.tag,
        "size": len(snapshot.content),
        "mtime_ns": stat.st_mtime_ns,
        "source_sha256": snapshot.sha256,
        "func_defaults": func_defaults,
        "func_kinds": func_kinds,
    }
    shared_source_records().put(
        "module-analysis",
        key,
        json.dumps(
            _cache_json_value(payload), separators=(",", ":"), default=_json_ir_default
        ).encode(),
        generation=_frontend_semantic_tooling_fingerprint(),
    )


def _validate_module_func_default_payload(payload: dict[str, Any]) -> bool:
    kind = payload.get("kind")
    if kind not in _MODULE_ANALYSIS_FUNC_KINDS:
        return False
    if not isinstance(payload.get("has_decorators"), bool):
        return False
    has_vararg = payload.get("has_vararg", False)
    if not isinstance(has_vararg, bool):
        return False
    if has_vararg:
        return True
    if not isinstance(payload.get("params"), int):
        return False
    if not isinstance(payload.get("defaults"), list):
        return False
    if not isinstance(payload.get("posonly"), int):
        return False
    if not isinstance(payload.get("kwonly"), int):
        return False
    return True


def _decode_cached_json_value(value: Any) -> Any:
    """Decode one cache value under the validators' malformed-value protocol."""
    try:
        return _decode_cached_json_value_inner(value)
    except (TypeError, OverflowError, RecursionError) as exc:
        raise ValueError("invalid cached JSON value") from exc


def _decode_cached_json_value_inner(value: Any) -> Any:
    if isinstance(value, list):
        return [_decode_cached_json_value_inner(item) for item in value]
    if isinstance(value, dict):
        if "__dict__" in value and len(value) == 1:
            entries = value["__dict__"]
            if not isinstance(entries, list) or not all(
                isinstance(entry, list) and len(entry) == 2 for entry in entries
            ):
                raise ValueError("invalid cached dictionary entries")
            result = {}
            for raw_key, raw_value in entries:
                key = _decode_cached_json_value_inner(raw_key)
                child = _decode_cached_json_value_inner(raw_value)
                if key in result:
                    raise ValueError("duplicate cached dictionary key")
                result[key] = child
            return result
        if "__integer_bytes__" in value and len(value) == 1:
            raw = value["__integer_bytes__"]
            if not isinstance(raw, list) or not all(
                type(item) is int and 0 <= item <= 255 for item in raw
            ):
                raise ValueError("invalid cached integer byte payload")
            return integer_literal_value(bytes(raw))
        if value.get("__ellipsis__") is True and len(value) == 1:
            return Ellipsis
        if "__bytes__" in value and isinstance(value["__bytes__"], list):
            raw = value["__bytes__"]
            if all(isinstance(item, int) and 0 <= item <= 255 for item in raw):
                return bytes(raw)
        if "__complex__" in value and isinstance(value["__complex__"], list):
            real_imag = value["__complex__"]
            if len(real_imag) == 2:
                return complex(real_imag[0], real_imag[1])
        if "__tuple__" in value and isinstance(value["__tuple__"], list):
            return tuple(
                _decode_cached_json_value_inner(item) for item in value["__tuple__"]
            )
        if "__ast__" in value and isinstance(value["__ast__"], str):
            return value["__ast__"]
        if "__set__" in value and isinstance(value["__set__"], list):
            return set(
                _decode_cached_json_value_inner(item) for item in value["__set__"]
            )
        if "__frozenset__" in value and isinstance(value["__frozenset__"], list):
            return frozenset(
                _decode_cached_json_value_inner(item) for item in value["__frozenset__"]
            )
        class_identity = _decode_class_identity(value)
        if class_identity is not None:
            return class_identity
        if "__molt_value__" in value and isinstance(value["__molt_value__"], dict):
            payload = value["__molt_value__"]
            name = payload.get("name")
            raw_hint = payload.get("type_hint", "Unknown")
            type_hint = (
                _decode_class_identity(raw_hint)
                if isinstance(raw_hint, dict)
                else raw_hint
            )
            if isinstance(name, str) and isinstance(
                type_hint, (str, DeclaredClass, BoundMethodHint)
            ):
                return MoltValue(name=name, type_hint=type_hint)
        return {
            str(key): _decode_cached_json_value_inner(item)
            for key, item in value.items()
        }
    return value


def _known_classes_from_payload(value: object) -> dict[ClassId, ClassInfo]:
    """Admit a pickled or cache-decoded class table only with ClassId keys."""
    if not isinstance(value, dict) or not all(
        isinstance(class_id, ClassId) and isinstance(class_info, dict)
        for class_id, class_info in value.items()
    ):
        raise ValueError("class tables are keyed by ClassId")
    return cast(dict[ClassId, ClassInfo], value)


@_source_tree_fingerprint_transaction()
def _load_module_analysis(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    namespace_premise: PythonNamespacePremise | None,
    source: str | None,
    source_snapshot: PythonSourceSnapshot | None = None,
    logical_source_path: str,
    resolution_cache: _ModuleResolutionCache,
    project_root: Path | None,
    path_stat: os.stat_result | None = None,
    retain_source: bool = True,
    retain_tree: bool = True,
    roots: Sequence[Path] | None = None,
    stdlib_root: Path | None = None,
    stdlib_allowlist: set[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_import_custody: _RuntimeImportScanCustody | None = None,
    native_support_function_roots: tuple[str, ...] = (),
) -> tuple[
    ast.AST | None,
    tuple[str, ...],
    dict[str, dict[str, Any]],
    dict[str, str],
    str | None,
    bool,
    os.stat_result | None,
]:
    if native_support_function_roots:
        native_support_function_roots = canonical_native_support_roots(
            native_support_function_roots
        )
    if source is not None:
        # Caller-supplied text is an operation-local generation, not proof that
        # the path still contains the bytes which produced it.
        project_root = None
    if runtime_import_custody is not None and not runtime_import_custody.owns(
        module_name, path.resolve()
    ):
        runtime_import_custody = None
    # An owner's scan is source-pure and its completion applies the live catalog,
    # so owner analysis persists like any other; function metadata never
    # depended on the catalog.
    if path_stat is None and project_root is not None:
        with contextlib.suppress(OSError):
            path_stat = resolution_cache.path_stat(path)
    # Function metadata can be persisted; resolved import edges cannot. Every
    # analysis request completes its source-pure scan against the live graph.
    loaded_scan = _load_module_import_scan(
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        purpose=SourceImportPurpose.INVENTORY,
        namespace_premise=namespace_premise,
        resolution_cache=resolution_cache,
        project_root=project_root,
        source=source,
        source_snapshot=source_snapshot,
        source_filename=logical_source_path,
        retain_source=retain_source,
        retain_tree=retain_tree,
        roots=roots,
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        target_python=target_python,
        runtime_import_custody=runtime_import_custody,
        native_support_function_roots=native_support_function_roots,
    )
    imports = loaded_scan.scan.imports
    snapshot = loaded_scan.snapshot
    if source is None:
        source = loaded_scan.source
    persisted_analysis = (
        _read_persisted_module_analysis(
            path,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            path_stat=path_stat,
            target_python=target_python,
            snapshot=snapshot,
            native_support_function_roots=native_support_function_roots,
        )
        if project_root is not None
        else None
    )
    persisted_defaults = (
        persisted_analysis[0] if persisted_analysis is not None else None
    )
    persisted_kinds = persisted_analysis[1] if persisted_analysis is not None else None
    if persisted_defaults is not None and persisted_kinds is not None:
        return (
            loaded_scan.tree if retain_tree else None,
            imports,
            persisted_defaults,
            persisted_kinds,
            source if retain_source else None,
            loaded_scan.cache_hit,
            path_stat,
        )

    if source is None:
        source = (
            snapshot.text_for_filename(logical_source_path)
            if snapshot is not None
            else resolution_cache.read_module_source(path, retain=retain_source)
        )

    tree = loaded_scan.tree
    if tree is None:
        parsed = resolution_cache.parse_module_ast(
            path,
            source,
            filename=logical_source_path,
            retain=retain_tree,
            target_python=target_python,
        )
        tree = parsed.tree
        if native_support_function_roots:
            if not isinstance(tree, ast.Module):
                raise ValueError("native support selection requires a module AST")
            selected, _source_pruned = resolution_cache.native_support_view(
                path,
                tree,
                ModuleBodySelection.from_requested_roots(native_support_function_roots),
                ast_digest_admission=parsed.admission,
                target_python=target_python,
            )
            tree = selected.tree
    func_defaults = persisted_defaults
    if func_defaults is None:
        func_defaults = _collect_func_defaults(tree)
    func_kinds = persisted_kinds
    if func_kinds is None:
        func_kinds = _collect_func_kinds(tree)
    if persisted_defaults is None or persisted_kinds is None:
        if project_root is not None and snapshot is not None:
            with contextlib.suppress(OSError):
                _write_persisted_module_analysis(
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    import_scan_mode=import_scan_mode,
                    func_defaults=func_defaults,
                    func_kinds=func_kinds,
                    snapshot=snapshot,
                    target_python=target_python,
                    native_support_function_roots=native_support_function_roots,
                )
    return (
        tree if retain_tree else None,
        imports,
        func_defaults,
        func_kinds,
        source if retain_source else None,
        False,
        path_stat,
    )


def _normalize_backend_ir_functions(
    functions: Sequence[dict[str, Any]],
) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    for func in functions:
        params = func.get("params")
        if isinstance(params, list) and params:
            raw_param_types = func.get("param_types")
            if isinstance(raw_param_types, list) and len(raw_param_types) >= len(
                params
            ):
                normalized.append(func)
                continue
            param_types = (
                list(raw_param_types) if isinstance(raw_param_types, list) else []
            )
            param_types.extend(["i64"] * (len(params) - len(param_types)))
            normalized.append({**func, "param_types": param_types})
        else:
            normalized.append(func)
    return normalized


def _module_lowering_local_reference_issue(
    module_name: str,
    functions: Sequence[dict[str, Any]],
    *,
    known_modules: Iterable[str] = (),
) -> str | None:
    try:
        missing = missing_local_function_references(
            module_name, functions, known_modules=known_modules
        )
    except ValueError as exc:
        return str(exc)
    if not missing:
        return None
    return (
        "module lowering payload has local SimpleIR function references without "
        f"function bodies: {format_function_reference_edges(missing)}"
    )


def _module_lowering_context_payload(
    module_name: str,
    module_path: Path,
    *,
    source_lease: _ModuleSourceLease,
    logical_source_path: str,
    module_execution_kind: ModuleExecutionKind,
    module_source_role: ModuleSourceRole,
    namespace_premise: PythonNamespacePremise | None,
    known_classes_snapshot: Mapping[ClassId, ClassInfo],
    type_hint_policy: TypeHintPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    direct_call_modules: Collection[str] | None = None,
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] | None = None,
    native_support_function_roots_by_module: Mapping[str, Sequence[str]] | None = None,
    module_deps: dict[str, set[str]],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...] | None = None,
    stdlib_allowlist_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    source_modules: Collection[str] | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[ClassId, ClassInfo]]
    | None = None,
    scoped_known_classes: dict[ClassId, ClassInfo] | None = None,
    is_package: bool | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    target_sys_platform: str | None = None,
) -> dict[str, Any] | None:
    source_lease.validate_path(module_path)
    if scoped_inputs is None:
        scoped_inputs = _scoped_lowering_input_view(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules,
            direct_call_modules=direct_call_modules,
            known_func_defaults=known_func_defaults,
            known_func_kinds=known_func_kinds,
            native_callable_exports=native_callable_exports,
            native_python_exports=native_python_exports,
            native_support_function_roots_by_module=(
                native_support_function_roots_by_module
            ),
            pgo_hot_function_names=pgo_hot_function_names,
            type_facts=type_facts,
            module_dep_closures=module_dep_closures,
            scoped_lowering_inputs=scoped_lowering_inputs,
            known_modules_sorted=known_modules_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            source_modules=source_modules,
        )
    known_modules_sorted = scoped_inputs.known_modules
    direct_call_modules_sorted = scoped_inputs.direct_call_modules
    if stdlib_allowlist_sorted is None:
        stdlib_allowlist_sorted = tuple(sorted(stdlib_allowlist))
    pgo_hot_function_names_sorted = scoped_inputs.pgo_hot_function_names
    scoped_known_func_defaults = scoped_inputs.known_func_defaults
    scoped_known_func_kinds = scoped_inputs.known_func_kinds
    scoped_native_callable_exports = scoped_inputs.native_callable_exports
    scoped_native_python_exports = scoped_inputs.native_python_exports
    if scoped_known_classes is None:
        scoped_known_classes = _scoped_known_classes_view(
            module_name,
            module_deps=module_deps,
            known_classes_snapshot=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
        )
    scoped_type_facts = scoped_inputs.type_facts
    if is_package is None:
        is_package = module_path.name == "__init__.py"
    return {
        # The context owns the captured bytes, not a later file stat/hash.
        # Inline text and raw path bytes are distinct source generations.
        "version": 7,
        "module_name": module_name,
        "logical_source_path": logical_source_path,
        "is_package": is_package,
        "module_execution_kind": module_execution_kind,
        "module_source_role": module_source_role,
        "namespace_premise": _namespace_premise_cache_identity(namespace_premise),
        "compiler_fingerprint": _frontend_semantic_tooling_fingerprint(),
        "target_python": target_python.tag,
        "target_sys_platform": target_sys_platform,
        "size": source_lease.source_size,
        "source_sha256": source_lease.source_sha256,
        "source_kind": source_lease.source_kind,
        "type_hint_policy": type_hint_policy,
        "type_facts": _type_facts_cache_payload(scoped_type_facts),
        "enable_phi": enable_phi,
        "known_modules": known_modules_sorted,
        "direct_call_modules": direct_call_modules_sorted,
        # ClassInfo is frozen after its class statement lowers; lowering-time
        # derived facts live in generator side tables, never in this payload.
        "known_classes": scoped_known_classes,
        "stdlib_allowlist": stdlib_allowlist_sorted,
        "known_func_defaults": scoped_known_func_defaults,
        "known_func_kinds": scoped_known_func_kinds,
        "native_callable_exports": scoped_native_callable_exports,
        "native_python_exports": scoped_native_python_exports,
        "body_selection": scoped_inputs.body_selection.payload(),
        "module_chunking": module_chunking,
        "module_chunk_max_ops": module_chunk_max_ops,
        "optimization_profile": optimization_profile,
        "pgo_hot_functions": pgo_hot_function_names_sorted,
    }


# A module's scoped class view repeats the class statements of its whole
# dependency closure: encoding the views cost 41 MB and 0.68 s of a 92-module
# edit-rebuild. Each class statement is digested once and the view digest is
# a Merkle combination of (class identity, class digest) pairs.
_CLASS_INFO_DIGESTS: dict[int, tuple[ClassInfo, bytes]] = {}
_CLASS_ID_BYTES: dict[ClassId, bytes] = {}
_CLASS_DIGEST_LIMIT = 65536


def _canonical_json_bytes(value: Any) -> bytes:
    return json.dumps(
        _cache_json_value(value),
        sort_keys=True,
        separators=(",", ":"),
        default=_json_ir_default,
    ).encode("utf-8")


def _bounded_put(memo: dict[Any, Any], key: Any, value: Any) -> None:
    if len(memo) >= _CLASS_DIGEST_LIMIT:
        memo.pop(next(iter(memo)))
    memo[key] = value


def _class_info_digest(info: ClassInfo) -> bytes:
    # visit_ClassDef builds a ClassInfo and nothing writes it afterwards, so a
    # digest stays valid for the object's lifetime; the memo holds the object,
    # so its id cannot name a different object while the entry exists.
    cached = _CLASS_INFO_DIGESTS.get(id(info))
    if cached is not None and cached[0] is info:
        return cached[1]
    digest = hashlib.sha256(_canonical_json_bytes(info)).digest()
    _bounded_put(_CLASS_INFO_DIGESTS, id(info), (info, digest))
    return digest


def _class_id_bytes(class_id: ClassId) -> bytes:
    cached = _CLASS_ID_BYTES.get(class_id)
    if cached is None:
        cached = _canonical_json_bytes(_encode_class_identity(class_id))
        _bounded_put(_CLASS_ID_BYTES, class_id, cached)
    return cached


def _class_view_sort_key(class_id: object) -> tuple[str, str, int, int, int]:
    if not isinstance(class_id, ClassId):
        raise TypeError(f"a scoped class view is keyed by ClassId, not {class_id!r}")
    return class_id.sort_key()


def _known_classes_digest(classes: Mapping[ClassId, ClassInfo]) -> str:
    hasher = hashlib.sha256(b"molt.lowering-known-classes.v1\0")
    for class_id in sorted(classes, key=_class_view_sort_key):
        identity = _class_id_bytes(class_id)
        hasher.update(len(identity).to_bytes(4, "little"))
        hasher.update(identity)
        hasher.update(_class_info_digest(classes[class_id]))
    return hasher.hexdigest()


def _module_lowering_context_digest(payload: dict[str, Any]) -> str | None:
    try:
        if "known_classes" in payload:
            payload = dict(
                payload, known_classes=_known_classes_digest(payload["known_classes"])
            )
        encoded = _canonical_json_bytes(payload)
    except (TypeError, ValueError):
        return None
    digest = hashlib.sha256(encoded).hexdigest()
    _trace_lowering_context_digest(payload, digest)
    return digest


def _trace_lowering_context_digest(payload: dict[str, Any], digest: str) -> None:
    """Append this module's context digest + per-field digests when tracing.

    Gated on ``MOLT_TRACE_LOWERING_CTX=<path>``: emits one JSON line per module
    (module name, full context digest, and a sha256 per top-level payload field).
    This is the standing profiler for the frontend-lowering-cache hit rate: the
    shared per-module lowering tier hits a fresh session only when this digest is
    identical to the one a prior session wrote, so diffing two runs' traces
    localizes any run-varying context input to a specific field. No-op (a single
    ``os.environ`` read) unless the env var is set, so it never affects a build.
    ``tools/witness_iter.py`` sets it for the warm witness attestation.
    """
    dump_path = os.environ.get("MOLT_TRACE_LOWERING_CTX")
    if not dump_path:
        return
    try:
        field_digests: dict[str, str] = {}
        for key, value in payload.items():
            try:
                blob = json.dumps(
                    _cache_json_value(value),
                    sort_keys=True,
                    separators=(",", ":"),
                    default=_json_ir_default,
                ).encode("utf-8")
            except (TypeError, ValueError):
                blob = repr(_cache_json_value(value)).encode("utf-8", "replace")
            field_digests[key] = hashlib.sha256(blob).hexdigest()[:16]
        line = json.dumps(
            {
                "module_name": payload.get("module_name"),
                "digest": digest,
                "field_digests": field_digests,
            },
            sort_keys=True,
        )
        with open(dump_path, "a", encoding="utf-8") as handle:
            handle.write(line + "\n")
    except OSError:
        return


def _type_facts_cache_payload(type_facts: Any) -> Any:
    if not isinstance(type_facts, TypeFacts):
        return type_facts
    payload = type_facts.to_dict()
    return {
        "schema_version": payload.get("schema_version", 1),
        "strict": bool(payload.get("strict", False)),
        "modules": payload.get("modules", {}),
    }


@_source_tree_fingerprint_transaction()
def _module_lowering_context_digest_for_module(
    module_name: str,
    module_path: Path,
    *,
    source_lease: _ModuleSourceLease,
    logical_source_path: str,
    module_execution_kind: ModuleExecutionKind,
    module_source_role: ModuleSourceRole,
    namespace_premise: PythonNamespacePremise | None,
    known_classes_snapshot: Mapping[ClassId, ClassInfo],
    type_hint_policy: TypeHintPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    direct_call_modules: Collection[str] | None = None,
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] | None = None,
    native_support_function_roots_by_module: Mapping[str, Sequence[str]] | None = None,
    module_deps: dict[str, set[str]],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...] | None = None,
    stdlib_allowlist_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    source_modules: Collection[str] | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[ClassId, ClassInfo]]
    | None = None,
    scoped_known_classes: dict[ClassId, ClassInfo] | None = None,
    is_package: bool | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    target_sys_platform: str | None = None,
) -> str | None:
    context_payload = _module_lowering_context_payload(
        module_name,
        module_path,
        source_lease=source_lease,
        logical_source_path=logical_source_path,
        module_execution_kind=module_execution_kind,
        module_source_role=module_source_role,
        namespace_premise=namespace_premise,
        known_classes_snapshot=known_classes_snapshot,
        type_hint_policy=type_hint_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        direct_call_modules=direct_call_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        native_callable_exports=native_callable_exports,
        native_python_exports=native_python_exports,
        native_support_function_roots_by_module=(
            native_support_function_roots_by_module
        ),
        module_deps=module_deps,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_function_names=pgo_hot_function_names,
        known_modules_sorted=known_modules_sorted,
        stdlib_allowlist_sorted=stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        module_dep_closures=module_dep_closures,
        scoped_lowering_inputs=scoped_lowering_inputs,
        scoped_inputs=scoped_inputs,
        source_modules=source_modules,
        scoped_known_classes_by_module=scoped_known_classes_by_module,
        scoped_known_classes=scoped_known_classes,
        is_package=is_package,
        target_python=target_python,
        target_sys_platform=target_sys_platform,
    )
    if context_payload is None:
        return None
    return _module_lowering_context_digest(context_payload)


def _module_lowering_payload_identity_matches(
    payload: Mapping[str, Any],
    path: Path,
    *,
    source_lease: _ModuleSourceLease,
    context_digest: str,
) -> bool:
    if (
        payload.get("version") != _MODULE_LOWERING_CACHE_SCHEMA_VERSION
        or payload.get("context_digest") != context_digest
    ):
        return False
    source_lease.validate_path(path)
    return (
        payload.get("source_sha256") == source_lease.source_sha256
        and payload.get("size") == source_lease.source_size
        and payload.get("source_kind") == source_lease.source_kind
    )


def _validate_persisted_module_lowering_payload(
    payload: Mapping[str, Any] | None,
    path: Path,
    *,
    source_lease: _ModuleSourceLease,
    module_name: str,
    context_digest: str,
    body_selection: ModuleBodySelection,
    known_modules: Iterable[str] = (),
) -> dict[str, Any] | None:
    """Admit schema/context, source custody and local reference closure."""
    if not isinstance(payload, dict) or not _module_lowering_payload_identity_matches(
        payload, path, source_lease=source_lease, context_digest=context_digest
    ):
        return None
    raw_result = payload.get("result")
    if not isinstance(raw_result, dict):
        return None
    try:
        if (
            ModuleBodySelection.from_payload(raw_result.get("body_selection"))
            != body_selection
        ):
            return None
    except ValueError:
        return None
    raw_functions = raw_result.get("functions")
    if not isinstance(raw_functions, list):
        return None
    try:
        result = _decode_cached_json_value(
            {key: value for key, value in raw_result.items() if key != "functions"}
        )
    except ValueError:
        return None
    if not isinstance(result, dict):
        return None
    result["functions"] = raw_functions
    by_symbol: dict[str, dict[str, Any]] = {}
    for function in raw_functions:
        if not isinstance(function, dict):
            return None
        symbol = function.get("name")
        if not isinstance(symbol, str) or symbol in by_symbol:
            return None
        by_symbol[symbol] = function
        params = function.get("params")
        param_types = function.get("param_types")
        if not isinstance(params, list) or (
            params
            and (not isinstance(param_types, list) or len(param_types) < len(params))
        ):
            return None
    try:
        metadata = parse_function_metadata(
            result.get("function_metadata"), function_symbols=by_symbol.keys()
        )
        for symbol, row in metadata.items():
            inspect_source_module_publication(by_symbol[symbol], row)
    except ValueError:
        return None
    if (
        _module_lowering_local_reference_issue(
            module_name, raw_functions, known_modules=known_modules
        )
        is not None
    ):
        return None
    source_lease.validate()
    return result


def _read_persisted_module_lowering(
    path: Path,
    *,
    source_lease: _ModuleSourceLease,
    module_name: str,
    context_digest: str,
    body_selection: ModuleBodySelection,
    known_modules: Iterable[str] = (),
) -> dict[str, Any] | None:
    if _frontend_lowering_cache_disabled():
        return None
    record = shared_source_records().get(
        "module-lowering",
        context_digest,
        generation=_frontend_semantic_tooling_fingerprint(),
        max_bytes=None,
    )
    if record is None:
        return None
    try:
        view = decode_lowering_record(record.payload)
        payload = json.loads(view.metadata)
    except (ValueError, TypeError, RecursionError):
        return None
    # Source custody failures are hard errors, outside optional codec rejection.
    if not isinstance(payload, dict) or not _module_lowering_payload_identity_matches(
        payload, path, source_lease=source_lease, context_digest=context_digest
    ):
        return None
    raw_result = payload.get("result")
    if not isinstance(raw_result, dict) or (
        "functions" in raw_result or "function_digest_seeds" in raw_result
    ):
        return None
    try:
        if (
            ModuleBodySelection.from_payload(raw_result.get("body_selection"))
            != body_selection
        ):
            return None
    except ValueError:
        return None
    try:
        functions, seeds = decode_function_records(view)
    except (ValueError, TypeError, RecursionError):
        return None
    payload["result"] = {**raw_result, "functions": functions}
    result = _validate_persisted_module_lowering_payload(
        payload,
        path,
        source_lease=source_lease,
        module_name=module_name,
        context_digest=context_digest,
        body_selection=body_selection,
        known_modules=known_modules,
    )
    if result is not None:
        result["function_digest_seeds"] = seeds
        record.refresh()
    return result


def _write_persisted_module_lowering(
    path: Path,
    *,
    source_lease: _ModuleSourceLease,
    context_digest: str,
    result: dict[str, Any],
) -> tuple[FunctionDigestSeed, ...]:
    if _frontend_lowering_cache_disabled():
        return ()
    source_lease.validate_path(path)
    source_lease.validate()
    # A producer must state which source body it lowered. Missing selection is
    # not evidence of complete source and cannot be written as a current record.
    ModuleBodySelection.from_payload(result.get("body_selection"))
    functions = result["functions"]
    metadata = {
        "version": _MODULE_LOWERING_CACHE_SCHEMA_VERSION,
        "context_digest": context_digest,
        "size": source_lease.source_size,
        "source_sha256": source_lease.source_sha256,
        "source_kind": source_lease.source_kind,
        "result": {
            key: value
            for key, value in result.items()
            if key not in {"functions", "function_digest_seeds", "ok"}
        },
    }
    encoded, seeds = encode_lowering_record(
        json.dumps(
            _cache_json_value(metadata), separators=(",", ":"), default=_json_ir_default
        ).encode(),
        functions,
    )
    shared_source_records().put(
        "module-lowering",
        context_digest,
        encoded,
        generation=_frontend_semantic_tooling_fingerprint(),
    )
    return seeds


@_source_tree_fingerprint_transaction()
def _load_cached_module_lowering_result(
    project_root: Path | None,
    module_name: str,
    module_path: Path,
    *,
    source_lease: _ModuleSourceLease,
    logical_source_path: str,
    module_execution_kind: ModuleExecutionKind,
    module_source_role: ModuleSourceRole,
    namespace_premise: PythonNamespacePremise | None,
    is_package: bool,
    known_classes_snapshot: dict[ClassId, ClassInfo],
    type_hint_policy: TypeHintPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    direct_call_modules: Collection[str] | None = None,
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] | None = None,
    native_support_function_roots_by_module: Mapping[str, Sequence[str]] | None = None,
    module_deps: dict[str, set[str]],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...] | None = None,
    stdlib_allowlist_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[ClassId, ClassInfo]]
    | None = None,
    scoped_known_classes: dict[ClassId, ClassInfo] | None = None,
    context_digest: str | None = None,
    source_modules: Collection[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    target_sys_platform: str | None = None,
) -> dict[str, Any] | None:
    if project_root is None:
        return None
    if context_digest is None:
        context_digest = _module_lowering_context_digest_for_module(
            module_name,
            module_path,
            source_lease=source_lease,
            logical_source_path=logical_source_path,
            module_execution_kind=module_execution_kind,
            module_source_role=module_source_role,
            namespace_premise=namespace_premise,
            known_classes_snapshot=known_classes_snapshot,
            type_hint_policy=type_hint_policy,
            type_facts=type_facts,
            enable_phi=enable_phi,
            known_modules=known_modules,
            direct_call_modules=direct_call_modules,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
            known_func_kinds=known_func_kinds,
            native_callable_exports=native_callable_exports,
            native_python_exports=native_python_exports,
            native_support_function_roots_by_module=(
                native_support_function_roots_by_module
            ),
            module_deps=module_deps,
            module_chunking=module_chunking,
            module_chunk_max_ops=module_chunk_max_ops,
            optimization_profile=optimization_profile,
            pgo_hot_function_names=pgo_hot_function_names,
            known_modules_sorted=known_modules_sorted,
            stdlib_allowlist_sorted=stdlib_allowlist_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            module_dep_closures=module_dep_closures,
            scoped_lowering_inputs=scoped_lowering_inputs,
            scoped_inputs=scoped_inputs,
            source_modules=source_modules,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
            scoped_known_classes=scoped_known_classes,
            is_package=is_package,
            target_python=target_python,
            target_sys_platform=target_sys_platform,
        )
        if context_digest is None:
            return None
    return _read_persisted_module_lowering(
        module_path,
        source_lease=source_lease,
        module_name=module_name,
        context_digest=context_digest,
        body_selection=(
            scoped_inputs.body_selection
            if scoped_inputs is not None
            else ModuleBodySelection.from_requested_roots(
                scoped_lowering_inputs.native_support_function_roots_by_module[
                    module_name
                ]
                if scoped_lowering_inputs is not None
                and module_name
                in scoped_lowering_inputs.native_support_function_roots_by_module
                else _scoped_native_support_function_roots(
                    module_name, native_support_function_roots_by_module
                )
            )
        ),
        known_modules=known_modules,
    )


def _module_worker_payload(
    module_name: str,
    *,
    module_path: Path,
    logical_source_path: str,
    source_lease: _ModuleSourceLease,
    type_hint_policy: TypeHintPolicy,
    module_execution_kind: ModuleExecutionKind,
    module_source_role: ModuleSourceRole,
    namespace_premise: PythonNamespacePremise | None,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    direct_call_modules: Collection[str] | None = None,
    known_classes_snapshot: dict[ClassId, ClassInfo],
    stdlib_allowlist_sorted: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] | None = None,
    native_support_function_roots_by_module: Mapping[str, Sequence[str]] | None = None,
    module_deps: dict[str, set[str]],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    module_dep_closures: dict[str, frozenset[str]],
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    source_modules: Collection[str] | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[ClassId, ClassInfo]]
    | None = None,
    scoped_known_classes: dict[ClassId, ClassInfo] | None = None,
    stdlib_allowlist_payload: list[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    target_sys_platform: str | None = None,
    frontend_phase_timeout: float | None = None,
) -> dict[str, Any]:
    source_lease.validate_path(module_path)
    if scoped_inputs is None:
        scoped_inputs = _scoped_lowering_input_view(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules,
            direct_call_modules=direct_call_modules,
            known_func_defaults=known_func_defaults,
            known_func_kinds=known_func_kinds,
            native_callable_exports=native_callable_exports,
            native_python_exports=native_python_exports,
            native_support_function_roots_by_module=(
                native_support_function_roots_by_module
            ),
            pgo_hot_function_names=pgo_hot_function_names,
            type_facts=type_facts,
            module_dep_closures=module_dep_closures,
            scoped_lowering_inputs=scoped_lowering_inputs,
            known_modules_sorted=tuple(known_modules),
            pgo_hot_function_names_sorted=tuple(pgo_hot_function_names),
            source_modules=source_modules,
        )
    if stdlib_allowlist_payload is None:
        stdlib_allowlist_payload = list(stdlib_allowlist_sorted)
    if scoped_known_classes is None:
        scoped_known_classes = _scoped_known_classes_view(
            module_name,
            module_deps=module_deps,
            known_classes_snapshot=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
        )
    return {
        "module_name": module_name,
        "module_path": str(module_path),
        "logical_source_path": logical_source_path,
        "source_lease": source_lease.worker_payload(),
        "type_hint_policy": type_hint_policy,
        "module_execution_kind": module_execution_kind,
        "module_source_role": module_source_role,
        "namespace_premise": _namespace_premise_cache_identity(namespace_premise),
        "enable_phi": enable_phi,
        "known_modules": scoped_inputs.known_modules_payload,
        "direct_call_modules": scoped_inputs.direct_call_modules_payload,
        "known_classes": scoped_known_classes,
        "stdlib_allowlist": stdlib_allowlist_payload,
        "known_func_defaults": scoped_inputs.known_func_defaults,
        "known_func_kinds": scoped_inputs.known_func_kinds,
        "native_callable_exports": scoped_inputs.native_callable_exports_payload,
        "native_python_exports": scoped_inputs.native_python_exports_payload,
        "body_selection": scoped_inputs.body_selection.payload(),
        "module_chunking": module_chunking,
        "module_chunk_max_ops": module_chunk_max_ops,
        "optimization_profile": optimization_profile,
        "pgo_hot_functions": scoped_inputs.pgo_hot_function_names_payload,
        "type_facts": scoped_inputs.type_facts,
        "target_python": target_python.short,
        "target_sys_platform": target_sys_platform,
        "frontend_phase_timeout": frontend_phase_timeout,
    }

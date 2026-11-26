from __future__ import annotations
from molt import compiled_symbols

from molt.frontend.lowering.op_kinds_generated import ExceptionEdgeKind

from molt.compilation_target import ArtifactTarget, Target, TargetKind

from dataclasses import asdict, dataclass, field, replace

from collections import Counter
import hashlib
import json
from pathlib import Path
from typing import Any, Callable, Collection, Mapping, Sequence, cast, TYPE_CHECKING

from molt.frontend._types import ClassId, SimpleIROp
from molt.frontend.class_model import ClassModel
from molt.frontend.lowering.op_kinds_generated import WireOpKind
from molt.frontend.lowering.function_metadata import (
    FunctionMetadataEmitter,
    MaterializedFunctionMetadata,
    emit_materialized_function_metadata,
)
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_FUNCTION_REFERENCE_KINDS,
    SIMPLEIR_STRUCTURAL_KINDS,
)
from molt.frontend.module_publication import (
    CallableDeclaration,
    FunctionAssemblyMetadata,
    inspect_source_module_publication,
    parse_function_metadata,
    reject_embedded_function_metadata,
)
from molt.compiler_analysis.python_function_kind import FunctionKind
from molt.compiler_analysis.python_import_redirects import ImportRedirects
from molt.native_callable_abi import (
    NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
    native_callable_uses_callargs,
)
from molt.native_callable_exports import NativeCallableExport

from molt import file_publication
from molt.cli.cache_keys import _json_ir_default
from molt.cli.cache_fingerprints import (
    _frontend_semantic_tooling_fingerprint,
    _source_tree_fingerprint_transaction,
)
from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE
from molt.cli.frontend_integration import (
    _register_global_code_id_with_state,
    _validate_class_definition_references_with_state,
    stamp_foreign_globals,
    stamp_global_slots,
)
from molt.cli.models import (
    _FrontendIntegrationState,
    ModuleSourceRole,
    _ExternalNativeModuleInitSpec,
    _ExternalPackageNativeArtifactPlan,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
)
from molt.cli.module_cache import _normalize_backend_ir_functions
from molt.cli.module_graph_cache import _namespace_premise_cache_identity
from molt.compiler_analysis.native_support_slice import ModuleBodySelection
from molt.compiler_analysis.python_body_materialization import BodyMaterializationMode
from molt.compiler_analysis.python_source_keys import PythonSourceKey
from molt.cli.closed_world_bindings import bind_closed_world_calls
from molt.cli.static_module_cells import (
    FunctionCells,
    StaticCellPlan,
    apply_function_cells,
    plan_module_cells,
    propagate_init_once_values,
    prove_init_once_calls,
)
from molt.cli.module_graph import ENTRY_OVERRIDE_SPAWN
from molt.cli.module_registry import (
    MODULE_FLAG_PACKAGE,
    ModuleRegistry,
    ModuleRegistryEntry,
    build_module_registry,
    runtime_builtin_kind,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.module_resolution import STDLIB_SUBMODULE_ALIASES
from molt.cli import required_features as _required_features
from molt.cli.function_references import reached_intrinsic_symbols
from molt.cli import runtime_features as _runtime_features
from molt.target_python import TargetPythonVersion

if TYPE_CHECKING:
    from molt.cli.function_records import FunctionDigestSeed
    from molt.cli.frontend_pipeline import _PreparedFrontendPipelineBundle
    from molt.cli.models import PgoProfileSummary, RuntimeFeedbackSummary


@dataclass(frozen=True)
class FrontendDeclarationTable:
    callables: Mapping[str, CallableDeclaration]
    classes: Mapping[ClassId, ClassModel]
    module_layouts: Mapping[str, tuple[str, ...]]
    declaration_only: frozenset[str]


@dataclass(frozen=True)
class _PreparedBackendIR:
    ir: dict[str, Any]
    function_metadata: dict[str, FunctionAssemblyMetadata]
    function_digest_seeds: tuple[FunctionDigestSeed, ...] = ()
    required_link_features: frozenset[str] = frozenset()
    # Import bedrock registry (molt.cli.module_registry.ModuleRegistry) on the
    # native lane; None on lanes that keep the legacy isolate dispatch.
    module_registry: Any | None = None
    # One line per qualifying module attribute call that keeps the generic
    # path, naming what blocked its closed-world binding.
    binding_diagnostics: tuple[str, ...] = ()
    # Static module cells (call_anatomy.md 4.5a): eligible modules, their
    # cells' InitOnce facts and why every other module keeps its dictionary.
    static_cell_diagnostics: dict[str, object] = field(default_factory=dict)
    declarations: FrontendDeclarationTable | None = None
    plan_seal: str | None = None


def _build_version_info_ops(
    *,
    target_python: TargetPythonVersion,
) -> list[SimpleIROp]:
    major = target_python.major
    minor = target_python.minor
    micro = target_python.micro
    version_release = target_python.release
    version_serial = target_python.serial
    return [
        {"kind": "const", "value": major, "out": "v3_raw"},
        {"kind": "box", "args": ["v3_raw"], "out": "v3"},
        {"kind": "const", "value": minor, "out": "v4_raw"},
        {"kind": "box", "args": ["v4_raw"], "out": "v4"},
        {"kind": "const", "value": micro, "out": "v5_raw"},
        {"kind": "box", "args": ["v5_raw"], "out": "v5"},
        {"kind": "const_str", "s_value": version_release, "out": "v6"},
        {"kind": "const", "value": version_serial, "out": "v7_raw"},
        {"kind": "box", "args": ["v7_raw"], "out": "v7"},
        {
            "kind": "call",
            "s_value": "molt_sys_set_version_info",
            "args": ["v3", "v4", "v5", "v6", "v7"],
            "out": "v8",
        },
    ]


def _build_entry_main_ops(
    *,
    entry_init: str,
    version_ops: Sequence[SimpleIROp],
    code_slot_count: int,
) -> list[SimpleIROp]:
    # The host has already initialized and attached the runtime. This body
    # owns only application initialization in that attached context.
    return [
        {"kind": "code_slots_init", "value": code_slot_count},
        *version_ops,
        # Pending startup errors are preserved. The completed wrapper is guarded
        # after all late code-slot, sys-init, and spawn-override insertions.
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": "v1",
        },
        {"kind": "ret_void"},
    ]


def _entry_call_index(entry_ops: Sequence[Mapping[str, Any]], entry_init: str) -> int:
    return next(
        idx
        for idx, op in enumerate(entry_ops)
        if op.get("kind") == "call" and op.get("s_value") == entry_init
    )


def _next_tir_var_index(ops: Sequence[Mapping[str, Any]]) -> int:
    used_vars: set[int] = set()
    for op in ops:
        out = op.get("out")
        if isinstance(out, str) and out.startswith("v"):
            try:
                used_vars.add(int(out[1:]))
            except ValueError:
                continue
    return max(used_vars, default=-1) + 1


def _append_entry_sys_init_op(
    entry_ops: list[SimpleIROp],
    *,
    entry_init: str,
    next_var: int,
    lazy: bool = False,
) -> int:
    sys_init = compiled_symbols.module_init_symbol("sys")
    if lazy:
        # Lazy sys init: check module cache first, only call the sys init body if
        # the module is not yet initialised.  This mirrors the pattern emitted
        # by _emit_module_load in the frontend so that sys initialisation is
        # deferred until the first real import.
        name_var = f"v{next_var}"
        next_var += 1
        cache_var = f"v{next_var}"
        next_var += 1
        none_var = f"v{next_var}"
        next_var += 1
        is_none_var = f"v{next_var}"
        next_var += 1
        sys_out_var = f"v{next_var}"
        next_var += 1
        entry_call_idx = _entry_call_index(entry_ops, entry_init)
        entry_ops[entry_call_idx:entry_call_idx] = [
            {"kind": "const_str", "s_value": "sys", "out": name_var},
            {"kind": "module_cache_get", "args": [name_var], "out": cache_var},
            {"kind": "const_none", "out": none_var},
            {"kind": "is", "args": [cache_var, none_var], "out": is_none_var},
            {"kind": "if", "args": [is_none_var]},
            {
                "kind": "call",
                "s_value": sys_init,
                "args": [],
                "out": sys_out_var,
            },
            {"kind": "end_if"},
        ]
    else:
        sys_out_var = f"v{next_var}"
        next_var += 1
        entry_call_idx = _entry_call_index(entry_ops, entry_init)
        entry_ops[entry_call_idx:entry_call_idx] = [
            {
                "kind": "call",
                "s_value": sys_init,
                "args": [],
                "out": sys_out_var,
            }
        ]
    return next_var


def _replace_entry_call_with_spawn_override(
    entry_ops: list[SimpleIROp],
    *,
    entry_init: str,
    next_var: int,
) -> int:
    spawn_init = compiled_symbols.module_init_symbol(ENTRY_OVERRIDE_SPAWN)
    entry_call_idx = _entry_call_index(entry_ops, entry_init)
    spawn_eq_var = f"v{next_var}"
    next_var += 1
    spawn_out_var = f"v{next_var}"
    next_var += 1
    entry_out_var = f"v{next_var}"
    next_var += 1
    entry_ops[entry_call_idx : entry_call_idx + 1] = [
        {
            "kind": "call",
            "s_value": "molt_startup_entry_is_spawn",
            "args": [],
            "out": spawn_eq_var,
            "type_hint": "bool",
        },
        {"kind": "if", "args": [spawn_eq_var]},
        {
            "kind": "call",
            "s_value": spawn_init,
            "args": [],
            "out": spawn_out_var,
        },
        {"kind": "else"},
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": entry_out_var,
        },
        {"kind": "end_if"},
    ]
    return next_var


def _guard_initialization_ops(
    ops: Sequence[SimpleIROp],
    *,
    failure_label: int | None = None,
    check_at_entry: bool = False,
) -> list[SimpleIROp]:
    """Preserve pending setup failure before any subsequent initialization work.

    Check every initialization operation rather than maintaining a second
    may-throw classifier. The existing operation-effect authority can eliminate
    redundant checks. Structural control markers, terminators, and existing checks do not execute
    initialization work and need no additional edge.
    """
    owns_failure_exit = failure_label is None
    if failure_label is None:
        failure_label = 1 + max(
            (int(op["value"]) for op in ops if op.get("kind") == "label"),
            default=0,
        )
    check: SimpleIROp = {
        "kind": "check_exception",
        "exception_edge": ExceptionEdgeKind.ORIGIN,
        "value": failure_label,
    }
    guarded: list[SimpleIROp] = [check.copy()] if check_at_entry else []
    for op in ops:
        if op == check and guarded and guarded[-1] == check:
            continue
        guarded.append(op.copy())
        kind = op.get("kind")
        if kind != "check_exception" and kind not in SIMPLEIR_STRUCTURAL_KINDS:
            guarded.append(check.copy())
    if owns_failure_exit:
        guarded.append({"kind": "label", "value": failure_label})
        guarded.append({"kind": "ret_void", "unwind": True})
    return guarded


def _build_isolate_bootstrap_ops(
    *,
    code_slot_count: int,
    version_ops: Sequence[SimpleIROp],
) -> list[SimpleIROp]:
    ops: list[SimpleIROp] = [
        {"kind": "code_slots_init", "value": code_slot_count},
        *version_ops,
    ]
    # Native runtime and WASM host imports share () -> owned Molt object.
    # The generated function declares its value return ABI independently of
    # these return payloads, including failure exits with no explicit payload.
    result = f"v{_next_tir_var_index(ops)}"
    ops.extend(
        [
            {"kind": "const_none", "out": result},
            {"kind": "ret", "args": [result]},
        ]
    )
    return _guard_initialization_ops(ops)


def _build_isolate_import_ops(
    *,
    code_slot_count: int,
    version_ops: Sequence[SimpleIROp],
    module_init_symbols: Sequence[tuple[str, str]],
) -> list[SimpleIROp]:
    # Runtime-side module imports can reach this function before either
    # `molt_main` or `molt_isolate_bootstrap` has initialized the global
    # code-slot table or committed the compiler target. Establish both before
    # accessing a module; each body owns its CODE_NEW/CODE_SLOT_SET sequence.
    import_ops: list[SimpleIROp] = [
        {"kind": "code_slots_init", "value": code_slot_count},
        *version_ops,
    ]
    # Module-frame code metadata has one authority: each module init body emits
    # its own CODE_NEW/CODE_SLOT_SET immediately before TRACE_ENTER_SLOT.  The
    # import dispatcher must not replay a second synthetic code object for the
    # same symbol, because that turns code slots into replaceable roots and makes
    # frame/trace ownership harder to reason about.
    import_var_idx = _next_tir_var_index(import_ops)

    def import_var() -> str:
        nonlocal import_var_idx
        name = f"v{import_var_idx}"
        import_var_idx += 1
        return name

    import_failed_label = 1
    name_var = "p0"
    module_var = import_var()
    import_ops.append(
        {"kind": "module_cache_get", "args": [name_var], "out": module_var}
    )
    none_var = import_var()
    import_ops.append({"kind": "const_none", "out": none_var})
    is_none_var = import_var()
    import_ops.append(
        {"kind": "is", "args": [module_var, none_var], "out": is_none_var}
    )
    import_ops.append({"kind": "if", "args": [is_none_var]})
    if module_init_symbols:
        for idx, (module_name, init_symbol) in enumerate(module_init_symbols):
            match_name_var = import_var()
            import_ops.append(
                {"kind": "const_str", "s_value": module_name, "out": match_name_var}
            )
            match_var = import_var()
            import_ops.append(
                {
                    "kind": "string_eq",
                    "args": [name_var, match_name_var],
                    "out": match_var,
                }
            )
            import_ops.append({"kind": "if", "args": [match_var]})
            # Never erase a failure from code-slot/cache/name setup before
            # entering module initialization.
            init_out = import_var()
            import_ops.append(
                {
                    "kind": "call",
                    "s_value": init_symbol,
                    "args": [],
                    "out": init_out,
                }
            )
            import_ops.append(
                {
                    "kind": "check_exception",
                    "exception_edge": ExceptionEdgeKind.ORIGIN,
                    "value": import_failed_label,
                }
            )
            if idx < len(module_init_symbols) - 1:
                import_ops.append({"kind": "else"})
        import_ops.extend({"kind": "end_if"} for _ in module_init_symbols)
    import_ops.append({"kind": "end_if"})
    loaded_var = import_var()
    import_ops.append(
        {"kind": "module_cache_get", "args": [name_var], "out": loaded_var}
    )
    import_ops.append({"kind": "ret", "args": [loaded_var]})
    import_ops = _guard_initialization_ops(
        import_ops, failure_label=import_failed_label
    )
    import_ops.append({"kind": "label", "value": import_failed_label})
    import_ops.append({"kind": "ret_void", "unwind": True})
    return import_ops


_MODULE_ENSURE_SYMBOL = "molt_module_ensure"


def _module_registry_target_enabled(target: Target) -> bool:
    """Select the binary representation of the shared finite import catalog.

    Native projects it as a relocated init table; WASM projects the same rows
    as its app-owned import export. Source emitters do not yet ship the Molt
    runtime/catalog ABI and retain their textual dispatch shape. This is not
    source-scan custody or admission of runtime import semantics.
    """
    if target is TargetKind.MLIR:
        return False
    return True


def _build_module_registry(
    *,
    entry_module: str,
    module_graph: Mapping[str, Path],
    logical_source_path_by_module: Mapping[str, str],
    namespace_portions_by_module: Mapping[str, tuple[str, ...]],
    module_source_role_by_module: Mapping[str, ModuleSourceRole],
    module_order: Sequence[str],
    runtime_import_dispatch_roots: Collection[str],
    native_module_init_specs: Sequence[_ExternalNativeModuleInitSpec],
    generated_native_init_modules: Collection[str],
    spawn_enabled: bool,
    global_layouts: Mapping[str, tuple[str, ...]],
    import_redirects: ImportRedirects,
) -> ModuleRegistry:
    """Derive the per-build ModuleRegistry from the binary-image closure plan.

    Runtime builtin kinds own their initialization independently of explicit
    imports. Other rows acquire an init lane from the admitted import closure
    or generated entry roots. Rows without an init lane fail closed inside
    ``molt_module_ensure`` with a diagnostic naming the admission channel
    (invariant I11). A row is Fixed when ``import_redirects`` names no writer
    of it (bedrock §13 D1, M4): only then may the runtime set its ready word.
    """
    # The entry module, __main__, and the spawn entry override are
    # ensure()-reached from generated code even when no explicit import names
    # them; they always own an init lane.
    forced_init = {entry_module, "__main__"}
    if spawn_enabled:
        forced_init.add(ENTRY_OVERRIDE_SPAWN)

    entries: list[ModuleRegistryEntry] = []
    module_order_set = set(module_order)
    for name in module_order:
        kind = runtime_builtin_kind(name)
        has_init = (
            kind == "runtime_builtin"
            or name in runtime_import_dispatch_roots
            or name in forced_init
        )
        entries.append(
            ModuleRegistryEntry(
                name=name,
                kind=kind,
                has_location=True,
                is_stdlib_provider=module_source_role_by_module[name] == "stdlib",
                init_symbol=(
                    compiled_symbols.module_init_symbol(name) if has_init else ""
                ),
                is_package=module_graph[name].name == "__init__.py",
                # The origin is the module's own reported path (HF-372).
                origin=logical_source_path_by_module[name],
                globals=global_layouts.get(name, ()),
                fixed=not import_redirects.closed_writers(name),
            )
        )
    for alias, target in sorted(STDLIB_SUBMODULE_ALIASES.items()):
        if target in module_order_set and alias not in module_order_set:
            entries.append(
                ModuleRegistryEntry(
                    name=alias,
                    kind="alias",
                    has_location=False,
                    is_stdlib_provider=True,
                    alias_of=target,
                    origin=f"<static:{alias}>",
                    fixed=not import_redirects.closed_writers(alias),
                )
            )
    for name, portions in sorted(namespace_portions_by_module.items()):
        entries.append(
            ModuleRegistryEntry(
                name=name,
                kind="namespace_parent",
                has_location=False,
                is_stdlib_provider=False,
                is_package=True,
                portions=portions,
                fixed=not import_redirects.closed_writers(name),
            )
        )
    generated = set(generated_native_init_modules)
    external_package_names = {
        spec.module
        for spec in native_module_init_specs
        if any(
            other.module.startswith(f"{spec.module}.")
            for other in native_module_init_specs
        )
    }
    for spec in native_module_init_specs:
        if spec.module in module_order_set or spec.module not in generated:
            # The module is compiled from source (or another spec already
            # owns the symbol); the source row above is the authority.
            continue
        if spec.is_alias:
            entries.append(
                ModuleRegistryEntry(
                    name=spec.module,
                    kind="alias",
                    has_location=False,
                    is_stdlib_provider=False,
                    alias_of=spec.alias_of,
                    origin=f"<static:{spec.module}>",
                    fixed=not import_redirects.closed_writers(spec.module),
                )
            )
            continue
        entries.append(
            ModuleRegistryEntry(
                name=spec.module,
                kind="extension" if spec.is_extension else "source",
                has_location=False,
                is_stdlib_provider=False,
                init_symbol=compiled_symbols.module_init_symbol(spec.module),
                is_package=spec.module in external_package_names,
                origin=f"<static:{spec.module}>",
                deps=tuple(
                    sorted(
                        {export.provider_module for export in spec.module_attr_exports}
                    )
                ),
                fixed=not import_redirects.closed_writers(spec.module),
            )
        )
    if entry_module != "__main__":
        entries.append(
            ModuleRegistryEntry(
                name="__main__",
                kind="source",
                has_location=True,
                # This fixed row is the explicit program entry capability,
                # not an advertised named stdlib provider. The original named
                # entry row retains its source provider role for imports.
                is_stdlib_provider=False,
                # __main__ shares the entry module's init body (the frontend
                # compiles the entry module with __main__ semantics); there is
                # no separate trampoline init on the registry lane.
                init_symbol=compiled_symbols.module_init_symbol(entry_module),
                origin=logical_source_path_by_module[entry_module],
                globals=global_layouts.get(entry_module, ()),
                fixed=not import_redirects.closed_writers("__main__"),
            )
        )
    return build_module_registry(entries)


def _module_ensure_call_ops(
    module_name: str,
    *,
    id_var: str,
    boxed_var: str,
    out_var: str,
) -> list[SimpleIROp]:
    """Resolve a symbolic module slot at final emission, then ensure its ModuleId.

    Runtime-call arguments are NaN-boxed values (the same convention every
    other `call` op uses, e.g. ``molt_sys_set_version_info``), so the resolved
    module id is boxed explicitly; ``molt_module_ensure`` decodes the boxed
    int at the ABI boundary.  Both vars MUST be canonical fresh ``v<N>``
    names: the TIR round-trip numbers values from the numeric suffix, so a
    non-numeric name silently aliases a neighbor.
    """
    return [
        {"kind": "module_slot", "s_value": module_name, "out": id_var},
        {"kind": "box", "args": [id_var], "out": boxed_var},
        {
            "kind": "call",
            "s_value": _MODULE_ENSURE_SYMBOL,
            "args": [boxed_var],
            "out": out_var,
        },
    ]


def _registry_init_symbol_rewrite_map(registry: ModuleRegistry) -> dict[str, str]:
    """Map generated init symbols to the registry row an ensure call targets.

    The entry module and ``__main__`` share one init body; direct calls to
    either symbol mean "run the program entry", so the ``__main__`` row wins
    ties deterministically.
    """
    rewrite: dict[str, str] = {}
    for row in registry.rows:
        if not row.init_symbol:
            continue
        existing = rewrite.get(row.init_symbol)
        if existing is None or row.name == "__main__":
            if existing == "__main__":
                continue
            rewrite[row.init_symbol] = row.name
    main_id = registry.id_of("__main__")
    if main_id is not None:
        # molt_main / molt_host_init / the spawn override call the entry via
        # the __main__ init symbol; on the registry lane that symbol may have
        # no function of its own (the trampoline is deleted), so alias it to
        # the __main__ row explicitly.
        rewrite[compiled_symbols.module_init_symbol("__main__")] = "__main__"
    return rewrite


def _is_module_init_body(function: Mapping[str, Any]) -> bool:
    """Whether ``function`` is the body that runs its owning module."""
    module = function.get("module")
    return isinstance(module, str) and function.get(
        "name"
    ) == compiled_symbols.module_init_symbol(module)


def _has_import_lane_site(ops: list[Any], init_symbols: Collection[str]) -> bool:
    """Whether ``ops`` hold a direct initializer call, the one site
    ``_rewrite_native_import_lanes`` reads."""
    return any(
        isinstance(op, dict)
        and op.get("kind") == "call"
        and op.get("s_value") in init_symbols
        for op in ops
    )


def _rewrite_native_import_lanes(
    functions: list[dict[str, Any]],
    registry: ModuleRegistry,
) -> str | None:
    """Bind internal initializer dispatch to symbolic catalog slots.

    Entry/sys/spawn/static-provider initializer calls enter the same dense
    initialization authority as imports (``molt_module_ensure``). Unknown
    direct initializer symbols fail closed. Changed function records retain
    copy-on-write custody.
    """
    rewrite_map = _registry_init_symbol_rewrite_map(registry)
    # A direct call that enters a module body: a registry row's initializer,
    # or the init body of a module the registry does not own.
    init_symbols = set(rewrite_map)
    init_symbols.update(
        func["name"] for func in functions if _is_module_init_body(func)
    )
    for function_index, func in enumerate(functions):
        ops = func.get("ops")
        if not isinstance(ops, list) or not _has_import_lane_site(ops, init_symbols):
            continue
        next_var = _next_tir_var_index([op for op in ops if isinstance(op, dict)])
        new_ops: list[SimpleIROp] = []
        changed = False
        for op in ops:
            if not isinstance(op, dict):
                new_ops.append(op)
                continue
            kind = op.get("kind")
            symbol = op.get("s_value") if kind == "call" else None
            if isinstance(symbol, str) and symbol in init_symbols:
                module_name = rewrite_map.get(symbol)
                if module_name is None:
                    return (
                        "direct module-init call targets a symbol outside "
                        f"the module registry: {symbol} in "
                        f"{func.get('name')!r}; init bodies are reachable "
                        "only through MODULE_INIT_TABLE"
                    )
                id_var = f"v{next_var}"
                boxed_var = f"v{next_var + 1}"
                next_var += 2
                out_var = op.get("out")
                out_name = out_var if isinstance(out_var, str) else f"v{next_var}"
                if not isinstance(out_var, str):
                    next_var += 1
                new_ops.extend(
                    _module_ensure_call_ops(
                        module_name,
                        id_var=id_var,
                        boxed_var=boxed_var,
                        out_var=out_name,
                    )
                )
                changed = True
                continue
            new_ops.append(op)
        if changed:
            functions[function_index] = {**func, "ops": new_ops}
    return None


# The protocol call each ``import_name`` form keeps when a row it names has a
# writer; its operands are exactly the import op's (bedrock §13 D3).
_IMPORT_PROTOCOL_SYMBOL: dict[str, str] = {
    "dunder": "molt_importlib_import_transaction",
    "module": "molt_importlib_import_module",
    "root": "molt_importlib_import_module",
}
# A statement that keeps the protocol in a program that may rebind
# ``builtins.__import__`` reads the frame's ``__import__`` first, as CPython's
# ``IMPORT_NAME`` does (D3); the operands stay the same.
_IMPORT_STATEMENT_HOOK_SYMBOL = "molt_importlib_import_statement"
# The pure constants an ``import_name`` prelude builds for its protocol call;
# a closed site drops each one no other op reads.
_IMPORT_PRELUDE_KINDS = frozenset({"const_str", "const_none", "const", "tuple_new"})


def _closed_import_ops(
    module_name: str, *, out: str, next_var: int
) -> tuple[list[SimpleIROp], int]:
    """``module_import_closed`` of one row, its id bound at final emission."""
    id_var, boxed_var = f"v{next_var}", f"v{next_var + 1}"
    return [
        {"kind": "module_slot", "s_value": module_name, "out": id_var},
        {"kind": "box", "args": [id_var], "out": boxed_var},
        {"kind": "module_import_closed", "args": [boxed_var], "out": out},
    ], next_var + 2


def _closed_import_rows(
    op: Mapping[str, Any], registry: ModuleRegistry | None
) -> tuple[str, ...] | None:
    """The rows a closed lowering of ``op`` ensures, result row last; None
    when the site keeps the protocol (IC3).

    A ``root`` form needs only an init lane (M9): ensure owns it, and the
    runtime sets its word only for a Fixed row. Every other form needs each
    row it names to be Fixed (``MODULE_FLAG_FIXED``: no redirect and no spec
    writer). A relative ``dunder`` import keeps the protocol, which resolves
    its anchor from the executing frame's globals.
    """
    if registry is None:
        return None
    name = op.get("s_value")
    form = op.get("import_form")
    if not isinstance(name, str) or not name:
        return None
    if form == "root":
        return (name,) if registry.ensure_lane_id(name) is not None else None
    if form == "module":
        return (name,) if registry.closed_import_id(name) is not None else None
    if op.get("value") != 0:
        return None
    root = name.split(".", 1)[0]
    rows = (name,) if op.get("fromlist") or root == name else (name, root)
    if any(registry.closed_import_id(row) is None for row in rows):
        return None
    return rows


def _modules_storing_path(
    module_functions: Mapping[str, Sequence[Mapping[str, Any]]],
) -> frozenset[str]:
    """Modules whose own code stores ``__path__`` (``os.py`` makes itself a
    package this way): CPython's ``_handle_fromlist`` then runs for them."""
    found: set[str] = set()
    for module, funcs in module_functions.items():
        for func in funcs:
            constants = {
                op.get("out"): op.get("s_value")
                for op in func.get("ops", ())
                if op.get("kind") == "const_str"
            }
            if any(
                op.get("kind") == "module_set_attr"
                and len(op.get("args") or ()) == 3
                and constants.get(op["args"][1]) == "__path__"
                for op in func.get("ops", ())
            ):
                found.add(module)
                break
    return frozenset(found)


def lower_import_names(
    functions: list[dict[str, Any]],
    registry: ModuleRegistry | None,
    import_redirects: ImportRedirects,
    path_modules: Collection[str] = (),
) -> list[str]:
    """Lower every ``import_name`` (bedrock §13 D2, D3, IC3, IC9).

    The frontend emits one typed import per statement; whole-program Fixed
    status exists only once the registry does, so the choice is made here. A
    site whose rows are all Fixed becomes ``module_import_closed`` per row
    (``import a.b.c``: the leaf, then the bound root), and a package
    from-import keeps the transaction's fromlist preparation. Every other
    site becomes its protocol call with the op's own operands. Returns one
    diagnostic per protocol-kept registry row that names its writer.
    """
    kept: dict[str, str] = {}
    dunder_symbol = (
        _IMPORT_STATEMENT_HOOK_SYMBOL
        if import_redirects.import_hook_writers
        else _IMPORT_PROTOCOL_SYMBOL["dunder"]
    )
    for function_index, func in enumerate(functions):
        ops = func.get("ops")
        if not isinstance(ops, list) or not any(
            isinstance(op, dict) and op.get("kind") == "import_name" for op in ops
        ):
            continue
        uses: Counter[str] = Counter()
        definition: dict[str, dict[str, Any]] = {}
        for op in ops:
            if not isinstance(op, dict):
                continue
            for arg in op.get("args") or ():
                if isinstance(arg, str):
                    uses[arg] += 1
            if isinstance(op.get("var"), str):
                uses[op["var"]] += 1
            if isinstance(op.get("out"), str):
                definition[op["out"]] = op
        next_var = _next_tir_var_index([op for op in ops if isinstance(op, dict)])
        dropped: set[int] = set()
        replacement: dict[int, list[SimpleIROp]] = {}

        def drop_prelude(var: str) -> None:
            source = definition.get(var)
            if (
                source is None
                or uses[var] != 0
                or source.get("kind") not in _IMPORT_PRELUDE_KINDS
            ):
                return
            dropped.add(id(source))
            for inner in source.get("args") or ():
                if isinstance(inner, str):
                    uses[inner] -= 1
                    drop_prelude(inner)

        for op in ops:
            if not isinstance(op, dict) or op.get("kind") != "import_name":
                continue
            args = list(op.get("args") or ())
            out = op["out"]
            rows = _closed_import_rows(op, registry)
            if rows is None:
                form = op.get("import_form", "dunder")
                replacement[id(op)] = [
                    {
                        "kind": "call",
                        "s_value": dunder_symbol
                        if form == "dunder"
                        else _IMPORT_PROTOCOL_SYMBOL[form],
                        "args": args,
                        "out": out,
                    }
                ]
                name = op.get("s_value")
                if (
                    registry is not None
                    and isinstance(name, str)
                    and registry.id_of(name) is not None
                    and name not in kept
                ):
                    writers = import_redirects.closed_writers(name)
                    reason = (
                        str(writers[0])
                        if writers
                        else "a relative import"
                        if op.get("value")
                        else "no init lane"
                    )
                    kept[name] = (
                        f"{func.get('name')}: import {name}: protocol: {reason}"
                    )
                continue
            package_fromlist = bool(op.get("fromlist")) and (
                rows[-1] in path_modules
                or bool(
                    registry is not None
                    and (row := registry.row_of(rows[-1])) is not None
                    and row.flags & MODULE_FLAG_PACKAGE
                )
            )
            lowered: list[SimpleIROp] = []
            for index, row_name in enumerate(rows):
                last = index == len(rows) - 1
                target = out if last else f"v{next_var}"
                if not last:
                    next_var += 1
                row_ops, next_var = _closed_import_ops(
                    row_name, out=target, next_var=next_var
                )
                lowered.extend(row_ops)
            if package_fromlist:
                # CPython's _handle_fromlist: a package imports each listed
                # child it lacks; operands are the prelude's name and tuple.
                lowered.append(
                    {
                        "kind": "call",
                        "s_value": "molt_importlib_prepare_fromlist",
                        "args": [args[0], out, args[3]],
                        "out": f"v{next_var}",
                    }
                )
                next_var += 1
                kept_args = {args[0], args[3]}
            else:
                kept_args = set()
            for arg in args:
                if not isinstance(arg, str):
                    continue
                uses[arg] -= 1
                if arg not in kept_args:
                    drop_prelude(arg)
                else:
                    uses[arg] += 1
            replacement[id(op)] = lowered
        new_ops: list[Any] = []
        for op in ops:
            if id(op) in dropped:
                continue
            new_ops.extend(replacement.get(id(op), (op,)))
        functions[function_index] = {**func, "ops": new_ops}
    return [kept[name] for name in sorted(kept)]


_STATIC_NATIVE_PYINIT_EXPORT_PREFIX = "__molt_static_pyinit__."


def _parent_module_parts(module_name: str) -> tuple[str, str] | None:
    if "." not in module_name:
        return None
    parent_name, attr_name = module_name.rsplit(".", 1)
    if not parent_name or not attr_name:
        return None
    return parent_name, attr_name


def _append_static_native_module_metadata_ops(
    ops: list[SimpleIROp],
    *,
    module_var: str,
    module_name: str,
    is_extension: bool,
    next_var: int,
) -> int:
    package_name = module_name
    if is_extension and "." in module_name:
        package_name = module_name.rsplit(".", 1)[0]
    package_attr_var = f"v{next_var}"
    next_var += 1
    package_name_var = f"v{next_var}"
    next_var += 1
    package_set_var = f"v{next_var}"
    next_var += 1
    ops.extend(
        [
            {"kind": "const_str", "s_value": "__package__", "out": package_attr_var},
            {"kind": "const_str", "s_value": package_name, "out": package_name_var},
            {
                "kind": "module_set_attr",
                "args": [module_var, package_attr_var, package_name_var],
                "out": package_set_var,
            },
        ]
    )
    return next_var


def _append_static_native_parent_binding_ops(
    ops: list[SimpleIROp],
    *,
    module_var: str,
    module_name: str,
    next_var: int,
) -> int:
    parent_parts = _parent_module_parts(module_name)
    if parent_parts is None:
        return next_var
    parent_name, attr_name = parent_parts
    parent_name_var = f"v{next_var}"
    next_var += 1
    parent_var = f"v{next_var}"
    next_var += 1
    none_var = f"v{next_var}"
    next_var += 1
    parent_missing_var = f"v{next_var}"
    next_var += 1
    attr_name_var = f"v{next_var}"
    next_var += 1
    parent_set_var = f"v{next_var}"
    next_var += 1
    ops.extend(
        [
            {"kind": "const_str", "s_value": parent_name, "out": parent_name_var},
            {"kind": "module_cache_get", "args": [parent_name_var], "out": parent_var},
            {"kind": "const_none", "out": none_var},
            {"kind": "is", "args": [parent_var, none_var], "out": parent_missing_var},
            {"kind": "if", "args": [parent_missing_var]},
            {"kind": "else"},
            {"kind": "const_str", "s_value": attr_name, "out": attr_name_var},
            {
                "kind": "module_set_attr",
                "args": [parent_var, attr_name_var, module_var],
                "out": parent_set_var,
            },
            {"kind": "end_if"},
        ]
    )
    return next_var


def _native_callable_wrapper_symbol(export: NativeCallableExport) -> str:
    identity = "\0".join(
        (
            export.qualified_name,
            export.abi,
            export.symbol or "",
            str(export.wrapper_payload_arity),
        )
    )
    digest = hashlib.sha256(identity.encode("utf-8")).hexdigest()[:16]
    return compiled_symbols.native_wrapper_symbol(export.qualified_name, digest)


def _native_callable_wrapper_function(
    export: NativeCallableExport,
) -> dict[str, Any]:
    symbol = _native_callable_wrapper_symbol(export)
    uses_callargs = native_callable_uses_callargs(export.abi)
    if uses_callargs:
        params = ["args", "kwargs"]
        ops: list[SimpleIROp] = [
            {"kind": "callargs_new", "out": "v0"},
            {
                "kind": "check_exception",
                "exception_edge": ExceptionEdgeKind.ORIGIN,
                "value": 2,
            },
            {
                "kind": "callargs_expand_star",
                "args": ["v0", "args"],
                "out": "v1",
            },
            {
                "kind": "check_exception",
                "exception_edge": ExceptionEdgeKind.ORIGIN,
                "value": 1,
            },
            {
                "kind": "callargs_expand_kwstar",
                "args": ["v0", "kwargs"],
                "out": "v2",
            },
            {
                "kind": "check_exception",
                "exception_edge": ExceptionEdgeKind.ORIGIN,
                "value": 1,
            },
        ]
        payload = ["v0"]
        result = "v3"
    else:
        params = [f"p{index}" for index in range(export.wrapper_payload_arity)]
        ops: list[SimpleIROp] = []
        payload = list(params)
        result = "v0"
    ops.append(
        {
            "kind": "invoke_ffi",
            "args": payload,
            "out": result,
            "native_callable_export": export.qualified_name,
            "native_callable_binding": "direct_symbol",
            "native_callable_abi": export.abi,
            "native_callable_symbol": export.symbol,
        }
    )
    ops.append(
        {
            "kind": "check_exception",
            "exception_edge": ExceptionEdgeKind.ORIGIN,
            "value": 1,
        }
    )
    if uses_callargs:
        ops.append({"kind": "release", "args": ["v0"]})
    ops.append({"kind": "ret", "args": [result]})
    ops.append({"kind": "label", "value": 1})
    if uses_callargs:
        ops.append({"kind": "release", "args": ["v0"]})
    ops.append({"kind": "ret_void", "unwind": True})
    if uses_callargs:
        ops.extend(
            [
                {"kind": "label", "value": 2},
                {"kind": "ret_void", "unwind": True},
            ]
        )
    return {
        "name": symbol,
        "module": export.module,
        "params": params,
        "return_abi": "value",
        "ops": ops,
    }


class _SimpleIRFunctionMetadataEmitter(FunctionMetadataEmitter[str]):
    """Canonical metadata primitives for post-frontend generated functions."""

    def __init__(
        self,
        ops: list[SimpleIROp],
        *,
        module_var: str,
        register_global_code_id: Callable[[str], int],
        next_var: int,
    ) -> None:
        self.ops = ops
        self.module_var = module_var
        self.register_global_code_id = register_global_code_id
        self.next_var = next_var

    def alloc(self) -> str:
        value = f"v{self.next_var}"
        self.next_var += 1
        return value

    def _result(self, kind: WireOpKind, **fields: Any) -> str:
        result = self.alloc()
        self.ops.append({"kind": kind, **fields, "out": result})
        return result

    def const_str(self, value: str) -> str:
        return self._result("const_str", s_value=value)

    def const_int(self, value: int) -> str:
        return self._result("const", value=value)

    def const_none(self) -> str:
        return self._result("const_none")

    def tuple_new(self, values: list[str]) -> str:
        return self._result("tuple_new", args=list(values))

    def code_new(self, values: list[str]) -> str:
        return self._result("code_new", args=list(values))

    def code_slot_set(self, code_symbol: str, code: str) -> None:
        self.register_global_code_id(code_symbol)
        module_dict_name = self.const_str("__dict__")
        globals_dict = self._result(
            "module_get_attr",
            args=[self.module_var, module_dict_name],
        )
        self.ops.append(
            {
                "kind": "code_slot_set",
                "s_value": code_symbol,
                "args": [code, globals_dict],
            }
        )

    def init_metadata(
        self,
        function: str,
        metadata: str,
        code: str,
        bind_kind: str,
    ) -> None:
        self.ops.append(
            {
                "kind": "call",
                "s_value": "molt_function_init_metadata_packed",
                "args": [function, metadata, code, bind_kind],
                "out": self.alloc(),
            }
        )


def _append_native_callable_function_metadata_ops(
    ops: list[SimpleIROp],
    *,
    module_var: str,
    export: NativeCallableExport,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> int:
    wrapper_symbol = _native_callable_wrapper_symbol(export)
    uses_callargs = native_callable_uses_callargs(export.abi)
    if uses_callargs:
        bound_params = ["args", "kwargs"]
        positional_names: tuple[str, ...] = ()
        vararg_name = "args"
        varkw_name = "kwargs"
    else:
        bound_params = [f"p{index}" for index in range(export.wrapper_payload_arity)]
        positional_names = tuple(bound_params)
        vararg_name = None
        varkw_name = None

    func_var = f"v{next_var}"
    next_var += 1
    ops.append(
        {
            "kind": "func_new",
            "s_value": wrapper_symbol,
            "value": len(bound_params),
            "out": func_var,
        }
    )
    emitter = _SimpleIRFunctionMetadataEmitter(
        ops,
        module_var=module_var,
        register_global_code_id=register_global_code_id,
        next_var=next_var,
    )

    def materialize_empty_defaults(function: str) -> tuple[str, str, str]:
        return function, emitter.const_none(), emitter.const_none()

    emit_materialized_function_metadata(
        emitter,
        function=func_var,
        materialize_defaults=materialize_empty_defaults,
        metadata=MaterializedFunctionMetadata(
            name=export.name,
            qualname=export.name,
            module=export.module,
            posonly_params=positional_names,
            pos_or_kw_params=(),
            kwonly_params=(),
            vararg=vararg_name,
            varkw=varkw_name,
            docstring=None,
            execution_kind=FunctionKind.SYNC,
            bind_kind=None,
            code_symbol=wrapper_symbol,
            trace_filename=f"<native callable export {export.qualified_name}>",
            trace_lineno=0,
            varnames=tuple(bound_params),
            code_names=(),
            freevars=(),
            cellvars=(),
        ),
    )
    attr_name_var = emitter.const_str(export.name)
    ops.append(
        {
            "kind": "module_set_attr",
            "args": [module_var, attr_name_var, func_var],
            "out": emitter.alloc(),
        }
    )
    return emitter.next_var


def _append_static_native_direct_symbol_export_ops(
    ops: list[SimpleIROp],
    *,
    module_var: str,
    spec: _ExternalNativeModuleInitSpec,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> int:
    for export in spec.direct_symbol_exports:
        next_var = _append_native_callable_function_metadata_ops(
            ops,
            module_var=module_var,
            export=export,
            register_global_code_id=register_global_code_id,
            next_var=next_var,
        )
    return next_var


def _append_static_native_callable_wrapper_functions(
    functions: list[dict[str, Any]],
    *,
    specs: Sequence[_ExternalNativeModuleInitSpec],
    register_global_code_id: Callable[[str], int],
) -> None:
    existing = {
        func.get("name")
        for func in functions
        if isinstance(func, Mapping) and isinstance(func.get("name"), str)
    }
    for spec in specs:
        for export in spec.direct_symbol_exports:
            symbol = _native_callable_wrapper_symbol(export)
            if symbol in existing:
                raise ValueError(
                    f"native callable wrapper {symbol!r} for "
                    f"{export.qualified_name!r} collides with an existing function"
                )
            register_global_code_id(symbol)
            functions.append(_native_callable_wrapper_function(export))
            existing.add(symbol)


def _append_static_native_module_attr_export_ops(
    ops: list[SimpleIROp],
    *,
    module_var: str,
    spec: _ExternalNativeModuleInitSpec,
    next_var: int,
    failure_label: int = 1,
) -> int:
    exports_by_provider: dict[str, list[str]] = {}
    for export in spec.module_attr_exports:
        exports_by_provider.setdefault(export.provider_module, []).append(export.attr)
    for provider_module in sorted(exports_by_provider):
        provider_init = compiled_symbols.module_init_symbol(provider_module)
        provider_init_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "call",
                    "s_value": provider_init,
                    "args": [],
                    "out": provider_init_var,
                },
                {
                    "kind": "check_exception",
                    "exception_edge": ExceptionEdgeKind.ORIGIN,
                    "value": failure_label,
                },
            ]
        )
        provider_name_var = f"v{next_var}"
        next_var += 1
        provider_module_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "const_str",
                    "s_value": provider_module,
                    "out": provider_name_var,
                },
                {
                    "kind": "module_cache_get",
                    "args": [provider_name_var],
                    "out": provider_module_var,
                },
            ]
        )
        for attr in sorted(set(exports_by_provider[provider_module])):
            attr_name_var = f"v{next_var}"
            next_var += 1
            attr_value_var = f"v{next_var}"
            next_var += 1
            attr_set_var = f"v{next_var}"
            next_var += 1
            ops.extend(
                [
                    {"kind": "const_str", "s_value": attr, "out": attr_name_var},
                    {
                        "kind": "module_get_attr",
                        "args": [provider_module_var, attr_name_var],
                        "out": attr_value_var,
                    },
                    {
                        "kind": "check_exception",
                        "exception_edge": ExceptionEdgeKind.ORIGIN,
                        "value": failure_label,
                    },
                    {
                        "kind": "module_set_attr",
                        "args": [module_var, attr_name_var, attr_value_var],
                        "out": attr_set_var,
                    },
                ]
            )
    return next_var


def _append_static_native_callable_publication_ops(
    ops: list[SimpleIROp],
    *,
    module_var: str,
    spec: _ExternalNativeModuleInitSpec,
    register_global_code_id: Callable[[str], int],
    next_var: int,
    failure_label: int = 1,
) -> int:
    """Publish local callable identities before executing provider callbacks."""
    next_var = _append_static_native_direct_symbol_export_ops(
        ops,
        module_var=module_var,
        spec=spec,
        register_global_code_id=register_global_code_id,
        next_var=next_var,
    )
    return _append_static_native_module_attr_export_ops(
        ops,
        module_var=module_var,
        spec=spec,
        next_var=next_var,
        failure_label=failure_label,
    )


def _guard_generated_native_module_init_ops(
    before_publication: Sequence[SimpleIROp],
    after_publication: Sequence[SimpleIROp],
    *,
    module_name_var: str,
) -> list[SimpleIROp]:
    """Separate pre-publication failures from an owned cache transaction.

    Namespace bodies publish through ``module_cache_set``; static extensions
    publish inside the runtime PyInit transaction. Once either succeeds, later
    body failures unwind the name through the canonical cache deletion primitive,
    which also unpublishes the registry slot and preserves the pending error.
    Cleanup is intentionally unguarded: a pending error must not loop back into
    its own handler. No name/value allocated by a failed prologue is read there.
    """
    return [
        *_guard_initialization_ops(before_publication, failure_label=1),
        *_guard_initialization_ops(after_publication, failure_label=2),
        {"kind": "label", "value": 2},
        {"kind": "module_cache_del", "args": [module_name_var]},
        {"kind": "ret_void", "unwind": True},
        {"kind": "label", "value": 1},
        {"kind": "ret_void", "unwind": True},
    ]


def _build_registry_native_module_init_ops(
    spec: _ExternalNativeModuleInitSpec,
    *,
    register_global_code_id: Callable[[str], int],
) -> list[SimpleIROp]:
    """Static-native module init body for the registry (import bedrock) lane.

    Pure body executor: no cache-probe preamble (init-exactly-once is the
    ``molt_module_ensure`` CAS, invariant I5), no parent binding (ensure's
    ``bind_parent_attr`` owns it from the registry parent edge), and no alias
    bodies (``kind=Alias`` rows resolve inside ensure).  Provider modules for
    capsule/attr exports are ordinary registry dependencies reached through
    direct calls of the provider init body here, which the
    ``_rewrite_native_import_lanes`` pass lowers onto ensure calls.
    """
    if spec.is_alias:
        raise ValueError(
            "alias modules own no init body on the registry lane: "
            f"{spec.module} -> {spec.alias_of}"
        )
    next_var = 0
    module_name_var = f"v{next_var}"
    next_var += 1
    ops: list[SimpleIROp] = [
        {"kind": "const_str", "s_value": spec.module, "out": module_name_var},
    ]
    module_var: str
    if spec.is_extension:
        module_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "invoke_ffi",
                    "args": [module_name_var],
                    "out": module_var,
                    "native_callable_export": (
                        f"{_STATIC_NATIVE_PYINIT_EXPORT_PREFIX}{spec.module}"
                    ),
                    "native_callable_binding": "direct_symbol",
                    "native_callable_abi": NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
                    "native_callable_symbol": spec.init_symbol,
                },
                {
                    "kind": "check_exception",
                    "exception_edge": ExceptionEdgeKind.ORIGIN,
                    "value": 1,
                },
            ]
        )
    else:
        module_var = f"v{next_var}"
        next_var += 1
        ops.append({"kind": "module_new", "args": [module_name_var], "out": module_var})
    before_publication = ops
    ops: list[SimpleIROp] = []
    if not spec.is_extension:
        cache_set_var = f"v{next_var}"
        next_var += 1
        ops.append(
            {
                "kind": "module_cache_set",
                "args": [module_name_var, module_var],
                "out": cache_set_var,
            }
        )
    # The runtime publishes static extensions before Py_mod_exec and rolls
    # back a failed PyInit. Re-publishing here would split that transaction.
    next_var = _append_static_native_module_metadata_ops(
        ops,
        module_var=module_var,
        module_name=spec.module,
        is_extension=spec.is_extension,
        next_var=next_var,
    )
    next_var = _append_static_native_callable_publication_ops(
        ops,
        module_var=module_var,
        spec=spec,
        register_global_code_id=register_global_code_id,
        next_var=next_var,
        failure_label=2,
    )
    ops.append({"kind": "ret_void"})
    return _guard_generated_native_module_init_ops(
        before_publication, ops, module_name_var=module_name_var
    )


def _build_static_native_module_init_ops(
    spec: _ExternalNativeModuleInitSpec,
    *,
    register_global_code_id: Callable[[str], int],
) -> list[SimpleIROp]:
    next_var = 0
    module_name_var = f"v{next_var}"
    next_var += 1
    ops: list[SimpleIROp] = [
        {"kind": "const_str", "s_value": spec.module, "out": module_name_var},
    ]
    # The init function owns the init-exactly-once invariant: every call
    # path (isolate-import dispatch, capsule alias providers) re-enters
    # here, and re-running a static extension PyInit trips CPython-side
    # reload guards ("cannot load module more than once per process")
    # while re-running a plain init would replace the cached module
    # object and break module identity.
    cached_var = f"v{next_var}"
    next_var += 1
    none_var = f"v{next_var}"
    next_var += 1
    is_missing_var = f"v{next_var}"
    next_var += 1
    ops.extend(
        [
            {
                "kind": "module_cache_get",
                "args": [module_name_var],
                "out": cached_var,
            },
            {"kind": "const_none", "out": none_var},
            {"kind": "is", "args": [cached_var, none_var], "out": is_missing_var},
            {"kind": "if", "args": [is_missing_var]},
        ]
    )
    module_var: str
    if spec.is_alias:
        provider_init = compiled_symbols.module_init_symbol(spec.alias_of)
        provider_init_var = f"v{next_var}"
        next_var += 1
        provider_name_var = f"v{next_var}"
        next_var += 1
        module_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "call",
                    "s_value": provider_init,
                    "args": [],
                    "out": provider_init_var,
                },
                {
                    "kind": "check_exception",
                    "exception_edge": ExceptionEdgeKind.ORIGIN,
                    "value": 1,
                },
                {
                    "kind": "const_str",
                    "s_value": spec.alias_of,
                    "out": provider_name_var,
                },
                {
                    "kind": "module_cache_get",
                    "args": [provider_name_var],
                    "out": module_var,
                },
            ]
        )
    elif spec.is_extension:
        module_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "invoke_ffi",
                    "args": [module_name_var],
                    "out": module_var,
                    "native_callable_export": (
                        f"{_STATIC_NATIVE_PYINIT_EXPORT_PREFIX}{spec.module}"
                    ),
                    "native_callable_binding": "direct_symbol",
                    "native_callable_abi": NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
                    "native_callable_symbol": spec.init_symbol,
                },
                {
                    "kind": "check_exception",
                    "exception_edge": ExceptionEdgeKind.ORIGIN,
                    "value": 1,
                },
            ]
        )
    else:
        module_var = f"v{next_var}"
        next_var += 1
        ops.append({"kind": "module_new", "args": [module_name_var], "out": module_var})
    before_publication = ops
    ops: list[SimpleIROp] = []
    if not spec.is_extension:
        cache_set_var = f"v{next_var}"
        next_var += 1
        ops.append(
            {
                "kind": "module_cache_set",
                "args": [module_name_var, module_var],
                "out": cache_set_var,
            }
        )
    if not spec.is_alias:
        next_var = _append_static_native_module_metadata_ops(
            ops,
            module_var=module_var,
            module_name=spec.module,
            is_extension=spec.is_extension,
            next_var=next_var,
        )
    next_var = _append_static_native_callable_publication_ops(
        ops,
        module_var=module_var,
        spec=spec,
        register_global_code_id=register_global_code_id,
        next_var=next_var,
        failure_label=2,
    )
    # Providers may have replaced the cache entry. Bind the parent to the
    # canonical result, but only while this invocation owns initialization.
    # A cached import preserves any subsequent user attribute rebinding.
    canonical_module_var = f"v{next_var}"
    next_var += 1
    ops.append(
        {
            "kind": "module_cache_get",
            "args": [module_name_var],
            "out": canonical_module_var,
        }
    )
    next_var = _append_static_native_parent_binding_ops(
        ops,
        module_var=canonical_module_var,
        module_name=spec.module,
        next_var=next_var,
    )
    ops.append({"kind": "end_if"})
    ops.append({"kind": "ret_void"})
    return _guard_generated_native_module_init_ops(
        before_publication, ops, module_name_var=module_name_var
    )


def _attach_static_native_publication_to_source_init(
    function: dict[str, Any],
    *,
    function_metadata: Mapping[str, Any],
    spec: _ExternalNativeModuleInitSpec,
    register_global_code_id: Callable[[str], int],
) -> dict[str, Any]:
    """Attach native callable publication to the canonical source init body.

    A public Python module and a native artifact may intentionally share the
    same module init symbol.  The source body remains the only init body;
    callable publication runs at the declared post-frame/pre-body boundary,
    reusing the source body's module object and exception cleanup. Source code
    may therefore capture the published callable and later rebind it normally.
    Return the original record on no change, otherwise a replacement sharing
    every untouched operation with the borrowed source body.
    """

    init_symbol = compiled_symbols.module_init_symbol(spec.module)
    if spec.is_extension or spec.is_alias:
        authority = "extension" if spec.is_extension else "alias"
        raise ValueError(
            f"existing source init {init_symbol!r} collides with native {authority} "
            f"module authority for {spec.module!r}"
        )
    if not spec.module_attr_exports and not spec.direct_symbol_exports:
        return function
    if function.get("params") != []:
        raise ValueError(
            f"existing source init {init_symbol!r} cannot publish native callables: "
            "module init must have no parameters"
        )
    ops = function.get("ops")
    if not isinstance(ops, list) or not all(isinstance(op, dict) for op in ops):
        raise ValueError(
            f"existing source init {init_symbol!r} cannot publish native callables: "
            "ops must be a mutable list of operation objects"
        )
    publication_envelope = inspect_source_module_publication(
        function, function_metadata
    )
    if publication_envelope is None:
        raise ValueError(
            f"existing source init {init_symbol!r} cannot publish native callables: "
            "missing canonical source-module publication metadata"
        )
    source_publication = publication_envelope.publication
    if source_publication["module_name"] != spec.module:
        raise ValueError(
            f"source module identity {source_publication['module_name']!r} "
            f"does not own native module {spec.module!r}"
        )
    module_var = str(source_publication["module_value"])
    failure_label = int(source_publication["failure_label"])
    boundary_index = publication_envelope.boundary_index

    wrapper_symbols = {
        _native_callable_wrapper_symbol(export) for export in spec.direct_symbol_exports
    }
    if any(
        op.get("kind") == "func_new" and op.get("s_value") in wrapper_symbols
        for op in ops
    ):
        raise ValueError(
            f"existing source init {init_symbol!r} already contains native callable "
            "publication"
        )

    publication_ops: list[SimpleIROp] = []
    _append_static_native_callable_publication_ops(
        publication_ops,
        module_var=module_var,
        spec=spec,
        register_global_code_id=register_global_code_id,
        next_var=_next_tir_var_index(ops),
        failure_label=failure_label,
    )
    guarded_publication = _guard_initialization_ops(
        publication_ops,
        failure_label=failure_label,
    )
    # Bind locals before any provider can observe this frame, and propagate a
    # prologue failure before executing publication side effects. The boundary
    # is a frontend fact; an optimized-away adjacent check is not an authority.
    insertion_index = boundary_index + 1
    return {
        **function,
        "ops": [
            *ops[:insertion_index],
            {
                "kind": "check_exception",
                "exception_edge": ExceptionEdgeKind.ORIGIN,
                "value": failure_label,
            },
            *guarded_publication,
            *ops[insertion_index:],
        ],
    }


def _append_static_native_module_init_functions(
    functions: list[dict[str, Any]],
    *,
    function_metadata: Mapping[str, FunctionAssemblyMetadata],
    specs: Sequence[_ExternalNativeModuleInitSpec],
    register_global_code_id: Callable[[str], int],
    registry_lane: bool,
) -> tuple[str, ...]:
    """Append generated static-native init bodies.

    Returns the module names this call owns in the registry (generated init
    bodies plus, on the registry lane, alias rows that own no body at all).
    On the registry lane (native), alias specs generate NO function — the
    ``kind=Alias`` registry row resolves inside ``molt_module_ensure`` — and
    init bodies are pure body executors without cache-guard preambles.
    """
    existing: dict[str, list[dict[str, Any]]] = {}
    function_indices: dict[str, int] = {}
    source_inits: dict[str, int] = {}
    for function_index, func in enumerate(functions):
        if not isinstance(func, dict):
            continue
        # This admission boundary must also refuse legacy bodies without a
        # native spec; no later consumer can reinterpret an embedded carrier.
        reject_embedded_function_metadata(func)
        if not isinstance(func.get("name"), str):
            continue
        existing.setdefault(func["name"], []).append(func)
        function_indices[func["name"]] = function_index
    metadata = parse_function_metadata(
        function_metadata, function_symbols=existing.keys()
    )
    for symbol, row in metadata.items():
        if row.get("declaration_only"):
            continue
        if len(existing[symbol]) != 1:
            raise ValueError(
                f"multiple executable bodies own metadata symbol {symbol!r}"
            )
        func = existing[symbol][0]
        publication_envelope = inspect_source_module_publication(func, row)
        if publication_envelope is not None:
            publication = publication_envelope.publication
            module = publication["module_name"]
            if (
                symbol != compiled_symbols.module_init_symbol(module)
                or func.get("module") != module
            ):
                raise ValueError(
                    f"source module identity {module!r} does not own init "
                    f"symbol {symbol!r}"
                )
            if module in source_inits:
                raise ValueError(f"multiple source init bodies own module {module!r}")
            source_inits[module] = function_indices[symbol]
    for symbol, bodies in existing.items():
        if len(bodies) > 1 and (
            any(_is_module_init_body(body) for body in bodies)
            or "source_module_publication" in function_metadata.get(symbol, {})
        ):
            raise ValueError(f"multiple existing init bodies own {symbol!r}")

    # Preflight the complete ownership set before publishing code IDs or editing
    # any source body. A lowered symbol is a linkage address, never Python module
    # identity. Registry aliases own a name/target edge but no function symbol.
    native_names: set[str] = set()
    body_owners: dict[str, str] = {
        functions[index]["name"]: module for module, index in source_inits.items()
    }
    for spec in specs:
        if spec.module in native_names:
            raise ValueError(f"multiple native init specs own module {spec.module!r}")
        native_names.add(spec.module)
        for export in spec.direct_symbol_exports:
            if export.module != spec.module:
                raise ValueError(
                    f"native export {export.qualified_name!r} does not belong to "
                    f"module {spec.module!r}"
                )
        symbol = compiled_symbols.module_init_symbol(spec.module)
        if spec.module in source_inits:
            if spec.is_alias or spec.is_extension:
                authority = "extension" if spec.is_extension else "alias"
                raise ValueError(
                    f"existing source init {symbol!r} collides with native "
                    f"{authority} module authority for {spec.module!r}"
                )
            continue
        if registry_lane and spec.is_alias:
            continue
        owner = body_owners.get(symbol)
        if owner is not None:
            raise ValueError(
                f"module init symbol collision: {owner!r} and {spec.module!r} "
                f"both require {symbol!r}"
            )
        if symbol in existing:
            # A legacy/unstructured function cannot be silently treated as the
            # source initializer merely because its spelling happens to match.
            raise ValueError(
                f"existing init {symbol!r} for {spec.module!r}: "
                "missing canonical source-module publication metadata"
            )
        body_owners[symbol] = spec.module

    owned_modules: list[str] = []
    for spec in specs:
        init_symbol = compiled_symbols.module_init_symbol(spec.module)
        source_index = source_inits.get(spec.module)
        if source_index is not None:
            functions[source_index] = _attach_static_native_publication_to_source_init(
                functions[source_index],
                function_metadata=function_metadata[init_symbol],
                spec=spec,
                register_global_code_id=register_global_code_id,
            )
            continue
        if registry_lane and spec.is_alias:
            owned_modules.append(spec.module)
            continue
        register_global_code_id(init_symbol)
        build_ops = (
            _build_registry_native_module_init_ops
            if registry_lane
            else _build_static_native_module_init_ops
        )
        function = {
            "name": init_symbol,
            "module": spec.module,
            "params": [],
            "return_abi": "void",
            "ops": build_ops(
                spec,
                register_global_code_id=register_global_code_id,
            ),
        }
        functions.append(function)
        owned_modules.append(spec.module)
        existing[init_symbol] = [function]
    return tuple(owned_modules)


def _finalize_backend_ir(
    *,
    functions: Sequence[dict[str, Any]],
    pgo_profile_summary: Any | None,
    runtime_feedback_summary: Any | None,
) -> dict[str, Any]:
    ir: dict[str, Any] = {"functions": _normalize_backend_ir_functions(functions)}
    if pgo_profile_summary is not None:
        profile_data: dict[str, Any] = {
            "version": pgo_profile_summary.version,
            "hash": pgo_profile_summary.hash,
            "hot_functions": pgo_profile_summary.hot_functions,
        }
        if pgo_profile_summary.branch_counts:
            profile_data["branch_counts"] = pgo_profile_summary.branch_counts
        if pgo_profile_summary.call_counts:
            profile_data["call_counts"] = pgo_profile_summary.call_counts
        if pgo_profile_summary.loop_counts:
            profile_data["loop_counts"] = pgo_profile_summary.loop_counts
        ir["profile"] = profile_data
    if runtime_feedback_summary is not None:
        ir["runtime_feedback"] = {
            "schema_version": runtime_feedback_summary.schema_version,
            "hash": runtime_feedback_summary.hash,
            "hot_functions": runtime_feedback_summary.hot_functions,
        }
    return ir


def _write_emitted_ir(
    emit_ir_path: Path | None,
    ir: Mapping[str, Any],
    *,
    validate_inputs: Callable[[], None],
) -> str | None:
    if emit_ir_path is None:
        return None
    try:
        # Emit exactly the backend input. Label, handler and saved-state IDs
        # share typed cross-references; a diagnostic-only partial renumbering
        # corrupts replay and can make evidence describe a different program.
        def publish(staged: Path, destination: Path) -> None:
            validate_inputs()
            file_publication.durable_replace(staged, destination)

        payload = json.dumps(ir, indent=2, default=_json_ir_default, allow_nan=False)
        file_publication.atomic_write_bytes(
            emit_ir_path, (payload + "\n").encode("utf-8"), replace=publish
        )
    except (OSError, ValueError) as exc:
        return f"Failed to write IR: {exc}"
    return None


def _bind_declaration_only_entries(
    functions: list[dict[str, Any]],
    declarations: Collection[str],
) -> None:
    """Bind each original constructor to the shared unreachable entry.

    Planners have already consumed the constructor and its original identity.
    The constructor's code object and code slot remain source-owned.
    """
    if not declarations:
        return
    published: set[str] = set()
    for position, function in enumerate(functions):
        original = function["ops"]
        replacements: dict[int, tuple[SimpleIROp, SimpleIROp]] = {}
        next_var: int | None = None
        for index, operation in enumerate(original):
            target = operation.get("s_value")
            if not isinstance(target, str) or target not in declarations:
                continue
            kind = operation["kind"]
            if kind == "func_new":
                arity = operation.get("value")
                if type(arity) is not int or arity < 0 or target in published:
                    raise ValueError(
                        f"invalid declaration-only constructor for {target!r}"
                    )
                published.add(target)
                if next_var is None:
                    next_var = _next_tir_var_index(original)
                value = f"v{next_var}"
                next_var += 1
                replacements[index] = (
                    {"kind": "const", "value": arity, "out": value},
                    {
                        "kind": "call",
                        "s_value": "molt_function_new_unreachable",
                        "args": [value],
                        "out": operation["out"],
                    },
                )
            elif (
                kind in SIMPLEIR_FUNCTION_REFERENCE_KINDS or kind == "trace_enter_slot"
            ):
                raise ValueError(
                    f"{function['name']} has a {kind} of declaration-only function {target!r}"
                )
        if replacements:
            functions[position] = {
                **function,
                "ops": [
                    replacement
                    for index, operation in enumerate(original)
                    for replacement in replacements.get(index, (operation,))
                ],
            }
    if missing := set(declarations) - published:
        raise ValueError(
            f"missing declaration-only constructors for {', '.join(sorted(missing))}"
        )


def _static_backend_ir_compiled_call_closure_issue(
    ir: Mapping[str, Any],
) -> str | None:
    """Fail early when backend IR directly calls compiled code it lacks.

    A ``call`` names either a runtime symbol or compiled code. A compiled
    target with no body in the program (a module outside the graph, say)
    would otherwise surface only as an undefined symbol at link time.
    """
    functions = ir.get("functions")
    if not isinstance(functions, list):
        return None
    defined = {
        name
        for func in functions
        if isinstance(func, Mapping) and isinstance((name := func.get("name")), str)
    }
    missing: list[str] = []
    seen: set[str] = set()
    for func in functions:
        if not isinstance(func, Mapping):
            continue
        func_name = func.get("name")
        ops = func.get("ops")
        if not isinstance(ops, list):
            continue
        for index, op in enumerate(ops):
            if not isinstance(op, Mapping) or op.get("kind") != "call":
                continue
            target = op.get("s_value")
            if (
                isinstance(target, str)
                and target not in defined
                and target not in seen
                and compiled_symbols.is_compiled_symbol(target)
            ):
                seen.add(target)
                missing.append(f"{target} at {func_name}[{index}]")
    if not missing:
        return None
    suffix = "" if len(missing) <= 8 else ", ..."
    return (
        "backend IR contains direct calls to compiled functions with no body "
        f"in the program: {', '.join(missing[:8])}{suffix}"
    )


def _reachability_feature_refusal(
    reached_symbols: frozenset[str],
    *,
    stdlib_profile: str | None,
    facts_target: ArtifactTarget,
) -> str | None:
    """Refuse when the reached SimpleIR needs a feature the profile excludes.

    Compares the requirements of ``reached_symbols`` (the finalized merged
    backend IR's ``ReachedIntrinsics``) against the selected profile's
    per-target link-affecting feature
    ceiling (``runtime_features.profile_link_features``). Returns a truthful,
    reached-intrinsic refusal message when a required feature is excluded, or
    ``None`` to proceed. The ceiling is computed for the build target's triple so
    a WASM build is checked against the WASM feature surface and a native build
    against the native surface. ``RequiredLinkFeatures`` itself is
    target-independent (a property of the reached IR).
    """
    # The ceiling is the SAME per-target available-feature authority the build
    # uses to select the staticlib (``_runtime_builtin_features_for_profile``):
    # the Cargo ladder plus explicit target exclusions. Using this keeps the
    # reachability ceiling aligned with the features the linked archive provides.
    profile_features = frozenset(
        _runtime_features._runtime_builtin_features_for_profile(
            stdlib_profile or DEFAULT_STDLIB_PROFILE,
            target=facts_target,
        )
    )
    return _required_features.reachability_profile_feature_refusal(
        reached_symbols,
        profile_name=stdlib_profile or DEFAULT_STDLIB_PROFILE,
        profile_features=profile_features,
        target=facts_target,
    )


def _finalize_gpu_descriptors(
    functions: list[dict[str, Any]],
    *,
    function_metadata: Mapping[str, FunctionAssemblyMetadata],
    target_python: TargetPythonVersion,
    global_code_ids: Mapping[str, int],
) -> None:
    """Bind compiler-authored descriptors after module/cache integration.

    Source origin is a semantic-content comparison over the *lowered* AST,
    never a reopen of application source or a path-name assertion. Both cache
    tiers preserve these optional facts under source+compilation-context keys.
    Foreign IR/native ABI authors remain trusted compiler inputs.
    """
    from molt.frontend.lowering.gpu_kernel_descriptor import descriptor_publications

    publications = [
        (index, constants)
        for index, function in enumerate(functions)
        if (constants := descriptor_publications(function["ops"]))
    ]
    if not publications:
        # No descriptor consumer: reference-body origins remain unvalidated.
        return

    from molt.cli.cache_fingerprints import _frontend_semantic_tooling_snapshot
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        GPU_PYTHON_BODY_REFERENCES,
        body_origin_evidence,
    )
    from molt.python_canonical_ast import canonical_python_module
    from molt.target_python import _parse_source_for_target

    snapshot = _frontend_semantic_tooling_snapshot()
    expected: dict[str, dict[str, Any]] = {}
    for module_name, (relative, _) in GPU_PYTHON_BODY_REFERENCES.items():
        captured = snapshot.reference_source(relative)
        tree = canonical_python_module(
            _parse_source_for_target(
                captured.content,
                filename=str(captured.path),
                target_python=target_python,
            )
        )
        for evidence in body_origin_evidence(
            module_name, tree, target_python.feature_version
        ).values():
            expected[evidence["role"]] = evidence

    admitted: dict[str, dict[str, Any]] = {}
    ambiguous: set[str] = set()
    for function in functions:
        evidence = function_metadata.get(function["name"], {}).get("gpu_body_origin")
        if not isinstance(evidence, dict):
            continue
        role = evidence.get("role")
        if not isinstance(role, str) or evidence != expected.get(role):
            continue
        if role in admitted:
            ambiguous.add(role)
        admitted[role] = {
            "symbol": function["name"],
            "arity": len(function["params"]),
            "defaults": evidence["defaults"],
        }
    missing = sorted((set(expected) - admitted.keys()) | ambiguous)
    for function_index, constants in publications:
        replacements: dict[int, dict[str, Any]] = {}
        for constant in constants:
            descriptor = json.loads(constant["s_value"])
            symbol = descriptor.get("symbol")
            if symbol not in global_code_ids:
                raise ValueError(
                    "GPU metadata publication has no integrated code owner"
                )
            descriptor["python_bodies"] = admitted if not missing else {}
            if missing:
                descriptor["unsupported"] = (
                    "canonical Python buffer body origin unavailable: "
                    + ", ".join(missing)
                )
            serialized = json.dumps(descriptor, sort_keys=True, separators=(",", ":"))
            if serialized != constant["s_value"]:
                replacements[id(constant)] = {**constant, "s_value": serialized}
        if replacements:
            function = functions[function_index]
            functions[function_index] = {
                **function,
                "ops": [replacements.get(id(op), op) for op in function["ops"]],
            }


def _seal_frontend_body_plan(
    frontend: _PreparedFrontendPipelineBundle,
    *,
    entry_module: str,
    mode: BodyMaterializationMode,
    target: Target,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None,
) -> str:
    context = frontend.prepared_frontend_run_ticket.frontend_layer_execution_context
    metadata = context.module_graph_metadata
    scan_authority = metadata.scan_authority
    authorities = scan_authority.by_module if scan_authority is not None else {}
    redirects = frontend.integration_state.import_redirects
    selections: dict[str, tuple[PythonSourceKey, ...]] = {}
    premises = {}
    for module, lease in sorted(frontend.module_source_catalog.leases.items()):
        authority = authorities.get(module)
        premise = authority.namespace_premise if authority is not None else None
        premises[module] = _namespace_premise_cache_identity(premise)
        names = frontend.namespace_declarations.get(module)
        if (
            mode != "demanded"
            or names is None
            or authority is None
            or authority.source_sha256 != lease.source_sha256
            or authority.body_selection.kind != "complete_source"
            or authority.namespace_own_line != 0
            or premise is None
            or premise.foreign_writes is None
            or premise.namespace_reflected
            or premise.code_rebinding
            or premise.opaque_activation_sources is None
            or frontend.native_artifact_plan.artifacts
            or module in frontend.runtime_import_dispatch_roots
            or redirects.redirect(module)
        ):
            continue
        read = (
            authority.read_callables
            | {key for key, _arguments in authority.deferred_owners}
            | set(premise.opaque_activation_sources)
        )
        selection = tuple(
            key
            for key, name in names.callable_candidates
            if key not in read
            and (module != entry_module or name.startswith("_"))
            and name not in premise.foreign_writes
        )
        if selection:
            selections[module] = selection
    seal = hashlib.sha256(
        json.dumps(
            {
                "producer": _frontend_semantic_tooling_fingerprint(),
                "target": str(target),
                "python": asdict(target_python),
                "platform": context.target_sys_platform,
                "stdlib_profile": stdlib_profile,
                "entry": entry_module,
                "mode": mode,
                "spawn": frontend.spawn_enabled,
                "runtime_import_roots": sorted(frontend.runtime_import_dispatch_roots),
                "import_redirects": asdict(redirects),
                "native_artifacts": frontend.native_artifact_plan.digest_payload(),
                "sources": {
                    module: [
                        lease.source_sha256,
                        premises[module],
                        metadata.logical_source_path_by_module.get(module),
                        metadata.module_execution_kind_by_module.get(module),
                        metadata.module_source_role_by_module.get(module),
                        metadata.module_is_package_by_module.get(module),
                        sorted(context.module_deps.get(module, ())),
                        (
                            authority.body_selection.payload()
                            if (authority := authorities.get(module)) is not None
                            else None
                        ),
                    ]
                    for module, lease in frontend.module_source_catalog.leases.items()
                },
                "declaration_only": selections,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode(),
    ).hexdigest()
    if context.scoped_lowering_inputs is not None:
        context.scoped_lowering_inputs.body_selections.update(
            {
                module: ModuleBodySelection("complete_source", declaration_only=keys)
                for module, keys in selections.items()
            },
        )
    elif selections:
        raise ValueError(
            "declaration-only body selection requires scoped lowering inputs"
        )
    return seal


def materialize_tir_closure(
    frontend: _PreparedFrontendPipelineBundle,
    *,
    entry_module: str,
    target: Target,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None,
    pgo_profile_summary: PgoProfileSummary | None,
    runtime_feedback_summary: RuntimeFeedbackSummary | None,
    fail: Callable[..., _CliFailure],
    json_output: bool,
    mode: BodyMaterializationMode = "eager",
) -> tuple[_PreparedBackendIR | None, _CliFailure | None]:
    """Lower and integrate the source closure under a single sealed plan."""
    state = frontend.materialization
    request = (
        entry_module,
        target,
        target_python,
        stdlib_profile,
        mode,
        pgo_profile_summary,
        runtime_feedback_summary,
        json_output,
    )
    with state.lock:
        producer = state.request is None
        if producer:
            state.request = request
        elif state.request != request:
            return None, fail(
                "frontend source closure already has a materialization request",
                json_output,
                command="build",
            )
    if producer:
        try:
            outcome = _run_tir_materialization(
                frontend,
                entry_module=entry_module,
                target=target,
                target_python=target_python,
                stdlib_profile=stdlib_profile,
                pgo_profile_summary=pgo_profile_summary,
                runtime_feedback_summary=runtime_feedback_summary,
                fail=fail,
                json_output=json_output,
                mode=mode,
            )
        except BaseException as exc:
            state.outcome.set_exception(exc)
        else:
            state.outcome.set_result(outcome)
    return state.outcome.result()


@_source_tree_fingerprint_transaction()
def _run_tir_materialization(
    frontend: _PreparedFrontendPipelineBundle,
    *,
    entry_module: str,
    target: Target,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None,
    pgo_profile_summary: PgoProfileSummary | None,
    runtime_feedback_summary: RuntimeFeedbackSummary | None,
    fail: Callable[..., _CliFailure],
    json_output: bool,
    mode: BodyMaterializationMode,
) -> tuple[_PreparedBackendIR | None, _CliFailure | None]:
    from time import perf_counter

    from molt.cli.frontend_execution import _run_frontend_pipeline

    ticket = frontend.prepared_frontend_run_ticket
    stages = ticket.frontend_parallel_details.setdefault("pipeline_stage_ms", {})
    started = perf_counter()
    seal = _seal_frontend_body_plan(
        frontend,
        entry_module=entry_module,
        mode=mode,
        target=target,
        target_python=target_python,
        stdlib_profile=stdlib_profile,
    )
    frontend_error = _run_frontend_pipeline(prepared_frontend_run_ticket=ticket)
    stages["run_frontend_pipeline"] = round((perf_counter() - started) * 1000.0, 6)
    if frontend_error is not None:
        return None, frontend_error
    started = perf_counter()
    metadata = ticket.frontend_layer_execution_context.module_graph_metadata
    prepared, issue = _prepare_backend_ir(
        entry_module=entry_module,
        module_graph=frontend.module_graph,
        logical_source_path_by_module=metadata.logical_source_path_by_module,
        namespace_portions_by_module=metadata.namespace_portions_by_module,
        module_source_role_by_module=metadata.module_source_role_by_module,
        known_modules=frontend.known_modules,
        stdlib_allowlist=frontend.stdlib_allowlist,
        integration_state=frontend.integration_state,
        fail=fail,
        json_output=json_output,
        module_order=frontend.module_order,
        runtime_import_dispatch_roots=frontend.runtime_import_dispatch_roots,
        spawn_enabled=frontend.spawn_enabled,
        pgo_profile_summary=pgo_profile_summary,
        runtime_feedback_summary=runtime_feedback_summary,
        emit_ir_path=frontend.output_layout.emit_ir_path,
        validate_inputs=frontend.module_source_catalog.validate,
        target_python=target_python,
        stdlib_profile=stdlib_profile,
        target=target,
        facts_target=frontend.output_layout.facts_target,
        native_artifact_plan=frontend.native_artifact_plan,
    )
    stages["prepare_backend_ir"] = round((perf_counter() - started) * 1000.0, 6)
    return (replace(prepared, plan_seal=seal) if prepared is not None else None), issue


def _prepare_backend_ir(
    *,
    entry_module: str,
    module_graph: Mapping[str, Path],
    logical_source_path_by_module: Mapping[str, str],
    namespace_portions_by_module: Mapping[str, tuple[str, ...]],
    module_source_role_by_module: Mapping[str, ModuleSourceRole],
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    integration_state: _FrontendIntegrationState,
    fail: Callable[..., _CliFailure],
    json_output: bool,
    module_order: Sequence[str],
    runtime_import_dispatch_roots: Collection[str],
    spawn_enabled: bool,
    pgo_profile_summary: Any | None,
    runtime_feedback_summary: Any | None,
    emit_ir_path: Path | None,
    validate_inputs: Callable[[], None],
    target_python: TargetPythonVersion,
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
    target: Target = TargetKind.NATIVE,
    facts_target: ArtifactTarget,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
) -> tuple[_PreparedBackendIR | None, _CliFailure | None]:
    catalog_lane = _module_registry_target_enabled(target)
    module_table_lane = catalog_lane
    native_callable_conflicts = (
        native_artifact_plan.native_callable_contract_conflicts()
    )
    if native_callable_conflicts:
        return None, fail(
            "static native callable export contract conflicts:\n"
            + "\n".join(native_callable_conflicts),
            json_output,
            command="build",
        )

    if entry_module != "__main__":
        if entry_module not in module_graph:
            return None, fail(
                f"Entry module not found: {entry_module}",
                json_output,
                command="build",
            )
        if not module_table_lane:
            # Dedup: the entry module is already compiled with entry_module=
            # entry_module (giving it __main__ semantics — dynamic __name__,
            # MODULE_CACHE_SET for "__main__", etc.).  Emit a thin trampoline
            # __main__ init body that delegates to the real init instead of
            # re-compiling the entire module.  On the registry lane no
            # trampoline exists: the __main__ registry row points at the entry
            # module's init body directly (one init table entry, two rows).
            _entry_real_init = compiled_symbols.module_init_symbol(entry_module)
            _main_init = compiled_symbols.module_init_symbol("__main__")
            integration_state.functions.append(
                {
                    "name": _main_init,
                    "module": "__main__",
                    "params": [],
                    "return_abi": "void",
                    "ops": [
                        {
                            "kind": "call",
                            "s_value": _entry_real_init,
                            "args": [],
                            "out": "v0",
                        },
                        {"kind": "ret_void"},
                    ],
                }
            )

    functions = integration_state.functions
    global_code_ids = integration_state.global_code_ids
    class_definition_issue = _validate_class_definition_references_with_state(
        integration_state
    )
    if class_definition_issue is not None:
        return None, fail(class_definition_issue, json_output, command="build")

    def register_global_code_id(symbol: str) -> int:
        return _register_global_code_id_with_state(integration_state, symbol)

    missing_static_init_symbols = sorted(
        artifact.module
        for artifact in native_artifact_plan.artifacts
        if artifact.runtime_linkage == "static_link" and not artifact.init_symbol
    )
    if missing_static_init_symbols:
        preview = ", ".join(missing_static_init_symbols[:8])
        suffix = "" if len(missing_static_init_symbols) <= 8 else ", ..."
        return None, fail(
            "static native artifacts require executable init_symbol custody: "
            f"{preview}{suffix}",
            json_output,
            command="build",
        )

    native_module_init_specs = native_artifact_plan.native_module_init_specs()
    try:
        generated_native_init_modules = _append_static_native_module_init_functions(
            functions,
            function_metadata=integration_state.function_metadata,
            specs=native_module_init_specs,
            register_global_code_id=register_global_code_id,
            registry_lane=module_table_lane,
        )
        _append_static_native_callable_wrapper_functions(
            functions,
            specs=native_module_init_specs,
            register_global_code_id=register_global_code_id,
        )
    except ValueError as exc:
        return None, fail(
            f"static native module publication assembly failed: {exc}",
            json_output,
            command="build",
        )
    entry_init_name = "__main__" if entry_module != "__main__" else entry_module
    entry_init = compiled_symbols.module_init_symbol(entry_init_name)
    version_ops = _build_version_info_ops(
        target_python=target_python,
    )
    entry_ops = _build_entry_main_ops(
        entry_init=entry_init,
        version_ops=version_ops,
        code_slot_count=len(global_code_ids),
    )
    next_var = _next_tir_var_index(entry_ops)
    # Determine whether to inject a sys init call into molt_main.
    #
    # With micro profile, sys initialisation is handled entirely by the
    # frontend's _emit_module_load: the first module that does
    # ``import sys`` will run the sys init body lazily.  Injecting it
    # unconditionally in molt_main forces the dead-function-elimination
    # pass to keep the entire sys + builtins + _intrinsics transitive
    # closure (~260 functions), which inflates the binary by ~25 MB and
    # adds ~300 ms of cold-cache startup on macOS.
    #
    # For the full profile, we keep the eager inject for backwards
    # compatibility with code that expects sys to be available before
    # the first explicit import.
    _inject_sys_init = "sys" in module_graph and stdlib_profile == "full"
    if _inject_sys_init:
        next_var = _append_entry_sys_init_op(
            entry_ops,
            entry_init=entry_init,
            next_var=next_var,
            lazy=False,
        )
    # Both attached entries install the same application state. The program
    # entry alone can replace its module invocation with a spawn override.
    host_init_ops = _guard_initialization_ops(entry_ops, check_at_entry=True)
    if spawn_enabled:
        _replace_entry_call_with_spawn_override(
            entry_ops,
            entry_init=entry_init,
            next_var=next_var,
        )
    entry_ops = _guard_initialization_ops(entry_ops, check_at_entry=True)
    functions.append(
        {
            "name": "molt_host_init",
            "params": [],
            "return_abi": "void",
            "ops": host_init_ops,
        }
    )
    functions.append(
        {"name": "molt_main", "params": [], "return_abi": "void", "ops": entry_ops}
    )
    isolate_bootstrap_ops = _build_isolate_bootstrap_ops(
        code_slot_count=len(global_code_ids),
        version_ops=version_ops,
    )
    functions.append(
        {
            "name": "molt_isolate_bootstrap",
            "params": [],
            "return_abi": "value",
            "ops": isolate_bootstrap_ops,
        }
    )
    # Closed-world bindings (call_anatomy.md 4.5): proven module attribute
    # calls become direct intrinsic calls; every frontend module guess is
    # consumed here, on every lane.
    binding_diagnostics, initializers = bind_closed_world_calls(
        functions,
        integration_state.module_functions,
        integration_state.module_namespace_facts,
        integration_state.module_dep_closures,
        integration_state.import_redirects,
    )
    # Class calls the frontend could not allocate inline name their refused
    # fact here, beside the other closed-world fallbacks.
    binding_diagnostics.extend(integration_state.constructor_fallbacks)
    module_registry: ModuleRegistry | None = None
    static_cells = StaticCellPlan()
    function_cells = FunctionCells()
    if catalog_lane:
        # Import bedrock (design doc 69, PR1): the ModuleRegistry replaces the
        # molt_isolate_import string_eq dispatch chain.  Cold dispatch is the
        # MODULE_INIT_TABLE relocation column of the registry blob; literal
        # import sites retain symbolic module_slot owners until final emission.
        try:
            module_registry = _build_module_registry(
                entry_module=entry_module,
                module_graph=module_graph,
                logical_source_path_by_module=logical_source_path_by_module,
                namespace_portions_by_module=namespace_portions_by_module,
                module_source_role_by_module=module_source_role_by_module,
                module_order=module_order,
                runtime_import_dispatch_roots=runtime_import_dispatch_roots,
                native_module_init_specs=native_module_init_specs,
                generated_native_init_modules=generated_native_init_modules,
                spawn_enabled=spawn_enabled,
                global_layouts=integration_state.global_layouts,
                import_redirects=integration_state.import_redirects,
            )
        except ValueError as exc:
            return None, fail(
                f"module registry construction failed: {exc}",
                json_output,
                command="build",
            )
        # Compiled global slot sites (call_anatomy.md 4.5): the registry row
        # carries each module's layout, so only registered modules are stamped.
        # Cell-eligible modules keep their globals in static cells (4.5a).
        registered = {row.name for row in module_registry.rows if row.init_symbol}
        static_cells, function_cells = plan_module_cells(
            integration_state.module_functions,
            integration_state.module_namespace_facts,
            integration_state.global_layouts,
            registered,
            integration_state.import_redirects,
            {row.name for row in module_registry.rows if row.kind == "extension"},
            entry_module=entry_module,
            dep_closures=integration_state.module_dep_closures,
            program_functions=functions,
            initializers=initializers,
        )
        binding_diagnostics.extend(function_cells.diagnostics)
        stamp_global_slots(
            functions,
            integration_state.module_functions,
            integration_state.global_layouts,
            {row.name for row in module_registry.rows},
            static_cells.modules,
            static_cells.guard_masks,
            static_cells.init_dominated,
            function_cells.counts(),
        )
        apply_function_cells(functions, function_cells)
        if module_table_lane:
            rewrite_issue = _rewrite_native_import_lanes(functions, module_registry)
            if rewrite_issue is not None:
                return None, fail(rewrite_issue, json_output, command="build")
        binding_diagnostics.extend(
            lower_import_names(
                functions,
                module_registry,
                integration_state.import_redirects,
                _modules_storing_path(integration_state.module_functions),
            )
        )
    else:
        binding_diagnostics.extend(
            lower_import_names(functions, None, integration_state.import_redirects)
        )
        native_dispatch = {spec.module: None for spec in native_module_init_specs}
        import_ops = _build_isolate_import_ops(
            code_slot_count=len(global_code_ids),
            version_ops=version_ops,
            module_init_symbols=tuple(
                (name, compiled_symbols.module_init_symbol(name))
                for name in dict.fromkeys((*module_order, *native_dispatch))
                if name in runtime_import_dispatch_roots or name in native_dispatch
            ),
        )
        functions.append(
            {
                "name": "molt_isolate_import",
                "params": ["p0"],
                "return_abi": "value",
                "ops": import_ops,
            }
        )
    # Proven foreign globals (HF-845): `m.f` reads M's InitOnce cell when M
    # keeps one; every lane consumes the binder's foreign_global records here.
    binding_diagnostics.extend(
        stamp_foreign_globals(
            functions,
            integration_state.global_layouts,
            static_cells.modules,
            static_cells.init_once,
            static_cells.ineligible,
            module_functions=integration_state.module_functions,
            dep_closures=integration_state.module_dep_closures,
        )
    )
    # Proven direct calls of InitOnce functions (HF-846 slice 2); every lane
    # consumes the frontend's func_new shape facts here.
    binding_diagnostics.extend(
        prove_init_once_calls(
            functions,
            integration_state.module_functions,
            static_cells,
            initializers=initializers,
            namespaces=integration_state.module_namespace_facts,
            dependencies=integration_state.module_dep_closures,
        )
    )
    # A dominated read carries the value published by the cell (4.5a).
    propagate_init_once_values(
        functions, integration_state.module_functions, static_cells
    )
    try:
        _finalize_gpu_descriptors(
            functions,
            function_metadata=integration_state.function_metadata,
            target_python=target_python,
            global_code_ids=global_code_ids,
        )
    except ValueError as exc:
        return None, fail(
            f"GPU metadata assembly failed: {exc}", json_output, command="build"
        )
    declaration_only = frozenset(
        symbol
        for symbol, row in integration_state.function_metadata.items()
        if row.get("declaration_only")
    )
    try:
        _bind_declaration_only_entries(functions, declaration_only)
    except ValueError as exc:
        return None, fail(str(exc), json_output, command="build")
    ir = _finalize_backend_ir(
        functions=functions,
        pgo_profile_summary=pgo_profile_summary,
        runtime_feedback_summary=runtime_feedback_summary,
    )
    ir["code_slots"] = dict(global_code_ids)
    if module_registry is not None:
        registry_payload = module_registry.backend_ir_payload()
        # The backend emits one cell block per eligible module, sized to its
        # layout: every layout name owns a word, deleted-only names included.
        registry_rows = registry_payload["rows"]
        assert isinstance(registry_rows, list)
        for row in registry_rows:
            if row["name"] in static_cells.modules:
                row["static_cells"] = len(
                    integration_state.global_layouts[row["name"]]
                ) + function_cells.counts().get(row["name"], 0)
        ir["module_registry"] = registry_payload
    # ModuleRegistry init bodies are reached through relocation slots in
    # MODULE_INIT_TABLE, not through SimpleIR call edges.  They must therefore
    # enter the same reachability authority as explicit roots before runtime
    # feature selection.  Omitting them let the layout probe compile a narrow
    # runtime, then backend emission exposed the table-reachable imports and
    # forced a second Cargo compile with a larger feature plan.  The registry is the canonical relocation-root
    # authority; do not revive module-presence or source-text feature scans.
    runtime_feature_roots = (
        module_registry.init_symbols() if module_registry is not None else ()
    )
    # Reachability-driven runtime-feature requirement / refusal (Option b,
    # docs/design/foundation/feature_reachability_tree_shaking.md). ``ir`` is the
    # finalized merged backend IR (exactly the function list the native/WASM
    # backends dead-strip), so ``required_features`` computes the link-affecting
    # features the REACHED code actually needs and refuses BEFORE codegen/link
    # (with a truthful, reached-intrinsic message) when the selected profile's
    # ceiling omits one. This is the only requirement fact; stdlib source scans
    # are classifiers/audits, never link-feature authority. The closure runs
    # once; the refusal and the link features both read its symbols.
    functions_for_features = ir.get("functions")
    reached_symbols = (
        reached_intrinsic_symbols(
            cast(Sequence[Mapping[str, object]], functions_for_features),
            extra_roots=runtime_feature_roots,
        )
        if isinstance(functions_for_features, list)
        else frozenset()
    )
    feature_refusal = _reachability_feature_refusal(
        reached_symbols,
        stdlib_profile=stdlib_profile,
        facts_target=facts_target,
    )
    if feature_refusal is not None:
        return None, fail(feature_refusal, json_output, command="build")
    module_call_issue = _static_backend_ir_compiled_call_closure_issue(ir)
    if module_call_issue is not None:
        return None, fail(module_call_issue, json_output, command="build")
    required_link_features = _required_features.required_link_features(reached_symbols)
    emit_ir_error = _write_emitted_ir(emit_ir_path, ir, validate_inputs=validate_inputs)
    if emit_ir_error is not None:
        return None, fail(emit_ir_error, json_output, command="build")
    final_function_ids = {id(function) for function in ir.get("functions", ())}
    surviving_seeds = tuple(
        seed
        for seed in integration_state.function_digest_seeds
        if id(seed.function) in final_function_ids
    )
    return _PreparedBackendIR(
        ir=ir,
        declarations=FrontendDeclarationTable(
            callables={
                symbol: row["callable_declaration"]
                for symbol, row in integration_state.function_metadata.items()
                if "callable_declaration" in row
            },
            classes=integration_state.known_classes,
            module_layouts=integration_state.global_layouts,
            declaration_only=declaration_only,
        ),
        function_digest_seeds=surviving_seeds,
        function_metadata={
            symbol: {"app_callable_bindings": row["app_callable_bindings"]}
            for symbol, row in integration_state.function_metadata.items()
            if "app_callable_bindings" in row
        },
        required_link_features=required_link_features,
        module_registry=module_registry,
        binding_diagnostics=tuple(binding_diagnostics),
        static_cell_diagnostics=static_cells.diagnostics(),
    ), None

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable, Collection, Mapping, Sequence, cast

from molt.frontend import SimpleTIRGenerator
from molt.native_callable_abi import NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1
from molt.type_facts import TypeFacts

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.backend_cache import (
    _emitted_name_matches_module_symbol,
    _module_symbol_name,
)
from molt.cli.cache_keys import _json_ir_default
from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE, ENTRY_OVERRIDE_ENV
from molt.cli.frontend_integration import _register_global_code_id_with_state
from molt.cli.models import (
    FallbackPolicy,
    ParseCodec,
    TypeHintPolicy,
    _FrontendIntegrationState,
    _ExternalNativeModuleInitSpec,
    _ExternalPackageNativeArtifactPlan,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _MidendDiagnosticsState,
    _PreparedBackendIR,
)
from molt.cli.module_cache import _normalize_backend_ir_functions
from molt.cli.module_graph import ENTRY_OVERRIDE_SPAWN
from molt.cli.module_registry import (
    ModuleRegistry,
    ModuleRegistryEntry,
    build_module_registry,
    runtime_builtin_kind,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli import required_features as _required_features
from molt.cli import runtime_features as _runtime_features
from molt.cli.target_python import TargetPythonVersion


def _append_module_code_slot_ops(
    ops: list[dict[str, Any]],
    *,
    logical_source_path: str,
    code_id: int,
    next_var: int,
) -> int:
    file_var = f"v{next_var}"
    next_var += 1
    name_var = f"v{next_var}"
    next_var += 1
    line_var = f"v{next_var}"
    next_var += 1
    linetable_var = f"v{next_var}"
    next_var += 1
    varnames_var = f"v{next_var}"
    next_var += 1
    names_var = f"v{next_var}"
    next_var += 1
    argcount_var = f"v{next_var}"
    next_var += 1
    posonly_var = f"v{next_var}"
    next_var += 1
    kwonly_var = f"v{next_var}"
    next_var += 1
    code_var = f"v{next_var}"
    next_var += 1
    ops.extend(
        [
            {
                "kind": "const_str",
                "s_value": logical_source_path,
                "out": file_var,
            },
            {"kind": "const_str", "s_value": "<module>", "out": name_var},
            {"kind": "const", "value": 1, "out": line_var},
            {"kind": "const_none", "out": linetable_var},
            {"kind": "tuple_new", "args": [], "out": varnames_var},
            {"kind": "tuple_new", "args": [], "out": names_var},
            {"kind": "const", "value": 0, "out": argcount_var},
            {"kind": "const", "value": 0, "out": posonly_var},
            {"kind": "const", "value": 0, "out": kwonly_var},
            {
                "kind": "code_new",
                "args": [
                    file_var,
                    name_var,
                    line_var,
                    linetable_var,
                    varnames_var,
                    names_var,
                    argcount_var,
                    posonly_var,
                    kwonly_var,
                ],
                "out": code_var,
            },
            {
                "kind": "code_slot_set",
                "value": code_id,
                "args": [code_var],
            },
        ]
    )
    return next_var


def _python_version_display(
    target_python: TargetPythonVersion,
) -> tuple[str, str, int]:
    # Use Molt's selected target version, not the host Python version, so
    # compiled binaries report the semantics they were parsed and lowered for.
    major = target_python.major
    minor = target_python.minor
    micro = target_python.micro
    release = target_python.release
    serial = target_python.serial
    version_suffix = ""
    if release == "alpha":
        version_suffix = f"a{serial}"
    elif release == "beta":
        version_suffix = f"b{serial}"
    elif release == "candidate":
        version_suffix = f"rc{serial}"
    elif release != "final":
        version_suffix = f"{release}{serial}"
    version_str = f"{major}.{minor}.{micro}{version_suffix} (molt)"
    return release, version_str, serial


def _build_version_info_ops(
    *,
    register_global_code_id: Callable[[str], int],
    target_python: TargetPythonVersion,
) -> list[dict[str, Any]]:
    major = target_python.major
    minor = target_python.minor
    micro = target_python.micro
    version_release = target_python.release
    version_serial = target_python.serial
    _, version_str, _ = _python_version_display(target_python)
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
        {"kind": "const_str", "s_value": version_str, "out": "v8"},
        {
            "kind": "call",
            "s_value": "molt_sys_set_version_info",
            "args": ["v3", "v4", "v5", "v6", "v7", "v8"],
            "out": "v9",
            "value": register_global_code_id("molt_sys_set_version_info"),
        },
    ]


def _build_entry_main_ops(
    *,
    entry_init: str,
    version_ops: Sequence[dict[str, Any]],
    register_global_code_id: Callable[[str], int],
) -> list[dict[str, Any]]:
    # NOTE: molt_runtime_init is called here for WASM targets where
    # there is no C main stub.  For native targets the C stub calls
    # molt_runtime_init before molt_main, so this is a harmless no-op
    # (idempotent — returns immediately if already initialised).
    #
    # molt_runtime_shutdown is NOT called here.  For native targets the
    # C stub's molt_finish() handles shutdown + _exit().  For WASM
    # targets the JS host runner handles cleanup.  Previously this
    # function emitted a molt_runtime_shutdown call which tore down the
    # runtime while the C stub still needed it (e.g. to check for
    # pending exceptions), and the subsequent TLS/atexit destructor
    # phase would hang or crash on exit.
    return [
        {
            "kind": "call",
            "s_value": "molt_runtime_init",
            "args": [],
            "out": "v0",
            "value": register_global_code_id("molt_runtime_init"),
        },
        *version_ops,
        # Clear any stale exception flag left by startup helpers. Without
        # this, the first check_exception inside the entry init function
        # sees the leftover flag and jumps straight to the error handler,
        # skipping module_cache_set and leaving the module unavailable.
        {"kind": "exception_clear"},
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": "v1",
            "value": register_global_code_id(entry_init),
        },
        {"kind": "ret_void"},
    ]


def _entry_call_index(entry_ops: Sequence[dict[str, Any]], entry_init: str) -> int:
    return next(
        idx
        for idx, op in enumerate(entry_ops)
        if op.get("kind") == "call" and op.get("s_value") == entry_init
    )


def _next_tir_var_index(ops: Sequence[dict[str, Any]]) -> int:
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
    entry_ops: list[dict[str, Any]],
    *,
    entry_init: str,
    register_global_code_id: Callable[[str], int],
    next_var: int,
    lazy: bool = False,
) -> int:
    sys_init = SimpleTIRGenerator.module_init_symbol("sys")
    if lazy:
        # Lazy sys init: check module cache first, only call molt_init_sys if
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
                "value": register_global_code_id(sys_init),
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
                "value": register_global_code_id(sys_init),
            }
        ]
    return next_var


def _build_module_code_ops(
    *,
    module_order: Sequence[str],
    module_graph: Mapping[str, Path],
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    entry_path: Path | None,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> tuple[list[dict[str, Any]], int, dict[str, list[dict[str, Any]]]]:
    """Build code-slot setup ops for each module.

    Returns ``(all_ops, next_var, per_module_ops)`` where *per_module_ops*
    maps each module name to its individual code-slot setup ops.  The flat
    *all_ops* list is the concatenation of all per-module ops (kept for
    backwards compatibility with ``molt_isolate_bootstrap``).
    """
    module_code_ops: list[dict[str, Any]] = []
    per_module_ops: dict[str, list[dict[str, Any]]] = {}
    for module_name in module_order:
        module_path = module_graph[module_name]
        logical_source_path = generated_module_source_paths.get(
            module_name, module_path.as_posix()
        )
        init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
        code_id = register_global_code_id(init_symbol)
        this_module_ops: list[dict[str, Any]] = []
        next_var = _append_module_code_slot_ops(
            this_module_ops,
            logical_source_path=logical_source_path,
            code_id=code_id,
            next_var=next_var,
        )
        per_module_ops[module_name] = this_module_ops
        module_code_ops.extend(this_module_ops)
    if entry_module != "__main__" and entry_path is not None:
        init_symbol = SimpleTIRGenerator.module_init_symbol("__main__")
        code_id = register_global_code_id(init_symbol)
        main_ops: list[dict[str, Any]] = []
        next_var = _append_module_code_slot_ops(
            main_ops,
            logical_source_path=entry_path.as_posix(),
            code_id=code_id,
            next_var=next_var,
        )
        per_module_ops["__main__"] = main_ops
        module_code_ops.extend(main_ops)
    return module_code_ops, next_var, per_module_ops


def _replace_entry_call_with_spawn_override(
    entry_ops: list[dict[str, Any]],
    *,
    entry_init: str,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> int:
    spawn_init = SimpleTIRGenerator.module_init_symbol(ENTRY_OVERRIDE_SPAWN)
    spawn_code_id = register_global_code_id(spawn_init)
    entry_call_idx = _entry_call_index(entry_ops, entry_init)
    entry_code_id = register_global_code_id(entry_init)
    env_key_var = f"v{next_var}"
    next_var += 1
    env_default_var = f"v{next_var}"
    next_var += 1
    env_value_var = f"v{next_var}"
    next_var += 1
    spawn_name_var = f"v{next_var}"
    next_var += 1
    spawn_eq_var = f"v{next_var}"
    next_var += 1
    spawn_out_var = f"v{next_var}"
    next_var += 1
    entry_out_var = f"v{next_var}"
    next_var += 1
    entry_ops[entry_call_idx : entry_call_idx + 1] = [
        {"kind": "const_str", "s_value": ENTRY_OVERRIDE_ENV, "out": env_key_var},
        {"kind": "const_str", "s_value": "", "out": env_default_var},
        {
            "kind": "env_get",
            "args": [env_key_var, env_default_var],
            "out": env_value_var,
        },
        {
            "kind": "const_str",
            "s_value": ENTRY_OVERRIDE_SPAWN,
            "out": spawn_name_var,
        },
        {
            "kind": "string_eq",
            "args": [env_value_var, spawn_name_var],
            "out": spawn_eq_var,
        },
        {"kind": "if", "args": [spawn_eq_var]},
        {
            "kind": "call",
            "s_value": spawn_init,
            "args": [],
            "out": spawn_out_var,
            "value": spawn_code_id,
        },
        {"kind": "else"},
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": entry_out_var,
            "value": entry_code_id,
        },
        {"kind": "end_if"},
    ]
    return next_var


def _build_isolate_bootstrap_ops(
    *,
    code_slot_count: int,
    version_ops: Sequence[dict[str, Any]],
    module_code_ops: Sequence[dict[str, Any]],
) -> list[dict[str, Any]]:
    return [
        {"kind": "code_slots_init", "value": code_slot_count},
        *version_ops,
        # Match `molt_main`: version/code-slot setup can leave a stale pending
        # exception bit that must not leak into the first lazily imported module.
        {"kind": "exception_clear"},
        *module_code_ops,
        {"kind": "ret_void"},
    ]


def _build_isolate_import_ops(
    *,
    code_slot_count: int,
    module_order: Sequence[str],
    register_global_code_id: Callable[[str], int],
) -> list[dict[str, Any]]:
    # Runtime-side module imports can reach this function before either
    # `molt_main` or `molt_isolate_bootstrap` has initialized the global
    # code-slot table. The import dispatcher owns table allocation only; each
    # module init body owns its module-frame CODE_NEW/CODE_SLOT_SET sequence.
    import_ops: list[dict[str, Any]] = [
        {"kind": "code_slots_init", "value": code_slot_count},
    ]
    # Module-frame code metadata has one authority: each `molt_init_*` body emits
    # its own CODE_NEW/CODE_SLOT_SET immediately before TRACE_ENTER_SLOT.  The
    # import dispatcher must not replay a second synthetic code object for the
    # same symbol, because that turns code slots into replaceable roots and makes
    # frame/trace ownership harder to reason about.
    import_var_idx = 0

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
    if module_order:
        for idx, module_name in enumerate(module_order):
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
            # Match `molt_main` and `molt_isolate_bootstrap`: startup helpers
            # can leave stale pending exception bits. Imported module init
            # functions start with check_exception-based error paths and often
            # predeclare globals to None, so entering them with a stale flag
            # corrupts module publication instead of executing the real body.
            import_ops.append({"kind": "exception_clear"})
            init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
            init_out = import_var()
            import_ops.append(
                {
                    "kind": "call",
                    "s_value": init_symbol,
                    "args": [],
                    "out": init_out,
                    "value": register_global_code_id(init_symbol),
                }
            )
            import_ops.append({"kind": "check_exception", "value": import_failed_label})
            if idx < len(module_order) - 1:
                import_ops.append({"kind": "else"})
        import_ops.extend({"kind": "end_if"} for _ in module_order)
    import_ops.append({"kind": "end_if"})
    loaded_var = import_var()
    import_ops.append(
        {"kind": "module_cache_get", "args": [name_var], "out": loaded_var}
    )
    import_ops.append({"kind": "ret", "args": [loaded_var]})
    failed_var = import_var()
    import_ops.append({"kind": "label", "value": import_failed_label})
    import_ops.append({"kind": "const_none", "out": failed_var})
    import_ops.append({"kind": "ret", "args": [failed_var]})
    return import_ops


def _isolate_import_module_order(
    module_order: Sequence[str],
    runtime_import_dispatch_roots: Collection[str],
    native_module_order: Sequence[str] = (),
) -> list[str]:
    if not runtime_import_dispatch_roots:
        return []
    runtime_roots = frozenset(runtime_import_dispatch_roots)
    import_roots: set[str] = set()
    for module_name in runtime_roots:
        parts = module_name.split(".")
        import_roots.update(".".join(parts[:idx]) for idx in range(1, len(parts) + 1))
    ordered = [
        module_name for module_name in module_order if module_name in import_roots
    ]
    seen = set(ordered)
    for module_name in native_module_order:
        if module_name in seen:
            continue
        if module_name in import_roots or any(
            root == module_name
            or root.startswith(f"{module_name}.")
            or module_name.startswith(f"{root}.")
            for root in runtime_roots
        ):
            ordered.append(module_name)
            seen.add(module_name)
    return ordered


_MODULE_ENSURE_SYMBOL = "molt_module_ensure"


def _module_registry_target_enabled(target: str) -> bool:
    """The import-bedrock registry lane is the native lane (design doc 69 PR1).

    WASM keeps the legacy isolate dispatch until PR3 unifies its projections;
    the luau/rust/mlir emit lanes keep the legacy shape because their emitted
    artifacts do not carry the native registry blob.
    """
    if target in {"wasm", "wasm-freestanding", "luau", "rust", "mlir"}:
        return False
    return not target.startswith("wasm32")


def _build_native_module_registry(
    *,
    entry_module: str,
    module_order: Sequence[str],
    runtime_import_dispatch_roots: Collection[str],
    native_module_init_specs: Sequence[_ExternalNativeModuleInitSpec],
    generated_native_init_modules: Collection[str],
    spawn_enabled: bool,
) -> ModuleRegistry:
    """Derive the per-build ModuleRegistry from the binary-image closure plan.

    Init-lane membership (rows whose ``MODULE_INIT_TABLE`` entry is non-null)
    is exactly the legacy isolate-dispatch membership
    (``_isolate_import_module_order``), so runtime importability is preserved
    one-to-one while the string_eq dispatch chain itself is deleted.  Rows
    without an init lane fail closed inside ``molt_module_ensure`` with a
    diagnostic naming the admission channel (invariant I11).
    """
    native_module_order = [spec.module for spec in native_module_init_specs]
    dispatchable = set(
        _isolate_import_module_order(
            module_order,
            runtime_import_dispatch_roots,
            native_module_order=native_module_order,
        )
    )
    # The entry module, __main__, sys, and the spawn entry override are
    # ensure()-reached from generated code even when no explicit import names
    # them; they always own an init lane.
    forced_init = {entry_module, "__main__", "sys"}
    if spawn_enabled:
        forced_init.add(ENTRY_OVERRIDE_SPAWN)

    entries: list[ModuleRegistryEntry] = []
    module_order_set = set(module_order)
    for name in module_order:
        has_init = name in dispatchable or name in forced_init
        entries.append(
            ModuleRegistryEntry(
                name=name,
                kind=runtime_builtin_kind(name),
                init_symbol=(
                    SimpleTIRGenerator.module_init_symbol(name) if has_init else ""
                ),
            )
        )
    generated = set(generated_native_init_modules)
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
                    alias_of=spec.alias_of,
                )
            )
            continue
        entries.append(
            ModuleRegistryEntry(
                name=spec.module,
                kind="extension" if spec.is_extension else "source",
                init_symbol=SimpleTIRGenerator.module_init_symbol(spec.module),
                deps=tuple(
                    sorted(
                        {export.provider_module for export in spec.module_attr_exports}
                    )
                ),
            )
        )
    if entry_module != "__main__":
        entries.append(
            ModuleRegistryEntry(
                name="__main__",
                kind="source",
                # __main__ shares the entry module's init body (the frontend
                # compiles the entry module with __main__ semantics); there is
                # no separate trampoline init on the registry lane.
                init_symbol=SimpleTIRGenerator.module_init_symbol(entry_module),
            )
        )
    return build_module_registry(entries)


def _module_ensure_call_ops(
    module_id: int,
    *,
    id_var: str,
    boxed_var: str,
    out_var: str,
) -> list[dict[str, Any]]:
    """`ensure(const ModuleId)` — the compiled literal import site shape.

    Runtime-call arguments are NaN-boxed values (the same convention every
    other `call` op uses, e.g. ``molt_sys_set_version_info``), so the constant
    module id is boxed explicitly; ``molt_module_ensure`` decodes the boxed
    int at the ABI boundary.  Both vars MUST be canonical fresh ``v<N>``
    names: the TIR round-trip numbers values from the numeric suffix, so a
    non-numeric name silently aliases a neighbor.
    """
    return [
        {"kind": "const", "value": module_id, "out": id_var},
        {"kind": "box", "args": [id_var], "out": boxed_var},
        {
            "kind": "call",
            "s_value": _MODULE_ENSURE_SYMBOL,
            "args": [boxed_var],
            "out": out_var,
        },
    ]


def _registry_init_symbol_rewrite_map(registry: ModuleRegistry) -> dict[str, int]:
    """Map generated init symbols to the registry row an ensure call targets.

    The entry module and ``__main__`` share one init body; direct calls to
    either symbol mean "run the program entry", so the ``__main__`` row wins
    ties deterministically.
    """
    rewrite: dict[str, int] = {}
    for row in registry.rows:
        if not row.init_symbol:
            continue
        existing = rewrite.get(row.init_symbol)
        if existing is None or row.name == "__main__":
            if existing is not None and registry.rows[existing].name == "__main__":
                continue
            rewrite[row.init_symbol] = row.id
    main_id = registry.id_of("__main__")
    if main_id is not None:
        # molt_main / molt_host_init / the spawn override call the entry via
        # the __main__ init symbol; on the registry lane that symbol may have
        # no function of its own (the trampoline is deleted), so alias it to
        # the __main__ row explicitly.
        rewrite[SimpleTIRGenerator.module_init_symbol("__main__")] = main_id
    return rewrite


_IMPORT_TRANSACTION_SYMBOL = "molt_importlib_import_transaction"


def _literal_plain_import_transaction_name(
    op: Mapping[str, Any],
    *,
    const_str_by_var: Mapping[str, str],
    builtin_func_by_var: Mapping[str, str],
    const_none_vars: Collection[str],
    empty_tuple_vars: Collection[str],
    const_int_by_var: Mapping[str, int],
) -> str | None:
    """Match the frontend's plain-import transaction shape and return the
    literal absolute module name.

    The shape (``_emit_import_transaction`` with empty fromlist, level 0):
    ``call_func [<transaction builtin>, <const_str name>, <globals>,
    <const_none locals>, <empty tuple_new fromlist>, <const 0 level>]``.
    Anything else (fromlist imports, relative levels, computed names) keeps
    the CPython-parity transaction lane.
    """
    if op.get("kind") != "call_func":
        return None
    args = op.get("args")
    if not isinstance(args, list) or len(args) != 6:
        return None
    if builtin_func_by_var.get(args[0]) != _IMPORT_TRANSACTION_SYMBOL:
        return None
    name = const_str_by_var.get(args[1])
    if name is None:
        return None
    if args[3] not in const_none_vars:
        return None
    if args[4] not in empty_tuple_vars:
        return None
    if const_int_by_var.get(args[5]) != 0:
        return None
    return name


def _rewrite_native_import_lanes(
    functions: Sequence[dict[str, Any]],
    registry: ModuleRegistry,
) -> str | None:
    """Lower literal import sites onto `molt_module_ensure(const ModuleId)`.

    Rewrites, in every prepared backend-IR function:

    * direct ``call molt_init_*`` ops (entry dispatch, eager sys init, spawn
      override, static-native provider inits) into ensure calls — after this
      pass init bodies are reachable ONLY through ``MODULE_INIT_TABLE``
      (invariant I5, gate G3);
    * ``module_import`` ops whose literal const name owns a registry init
      lane into ensure calls;
    * plain-import transaction calls (``import x`` / ``import a.b`` — empty
      fromlist, level 0, literal absolute name with a registry init lane)
      into ``ensure(full_id)`` followed by ``ensure(top_id)`` (the import
      statement binds the top-level module).  This is design §4.2/§10.1
      verbatim: compiled literal import sites carry ``const ModuleId`` and
      never route a string argument through the dynamic call lane, which is
      the incident-1 corruption surface.

    Names without a registry init lane keep their dynamic ops so the
    importlib fallback ladder (runtime-root spec imports, extension loaders)
    is preserved.  Returns an error message when a direct init call targets a
    symbol outside the registry (fail closed — that call could bypass
    init-exactly-once).
    """
    rewrite_map = _registry_init_symbol_rewrite_map(registry)
    for func in functions:
        ops = func.get("ops")
        if not isinstance(ops, list):
            continue
        const_str_by_var: dict[str, str] = {}
        builtin_func_by_var: dict[str, str] = {}
        const_none_vars: set[str] = set()
        empty_tuple_vars: set[str] = set()
        const_int_by_var: dict[str, int] = {}
        for op in ops:
            if not isinstance(op, Mapping) or not isinstance(op.get("out"), str):
                continue
            kind = op.get("kind")
            out = op["out"]
            if kind == "const_str" and isinstance(op.get("s_value"), str):
                const_str_by_var[out] = op["s_value"]
            elif kind == "builtin_func" and isinstance(op.get("s_value"), str):
                builtin_func_by_var[out] = op["s_value"]
            elif kind == "const_none":
                const_none_vars.add(out)
            elif kind == "tuple_new" and not op.get("args"):
                empty_tuple_vars.add(out)
            elif kind == "const" and isinstance(op.get("value"), int):
                const_int_by_var[out] = op["value"]
        next_var = _next_tir_var_index([op for op in ops if isinstance(op, Mapping)])
        new_ops: list[dict[str, Any]] = []
        changed = False
        for op in ops:
            if not isinstance(op, Mapping):
                new_ops.append(op)
                continue
            kind = op.get("kind")
            if kind == "call":
                symbol = op.get("s_value")
                if isinstance(symbol, str) and symbol.startswith("molt_init_"):
                    module_id = rewrite_map.get(symbol)
                    if module_id is None:
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
                            module_id,
                            id_var=id_var,
                            boxed_var=boxed_var,
                            out_var=out_name,
                        )
                    )
                    changed = True
                    continue
            elif kind == "module_import":
                args = op.get("args")
                name = (
                    const_str_by_var.get(args[0])
                    if isinstance(args, list) and len(args) == 1
                    else None
                )
                module_id = (
                    registry.ensure_lane_id(name) if isinstance(name, str) else None
                )
                if module_id is not None:
                    id_var = f"v{next_var}"
                    boxed_var = f"v{next_var + 1}"
                    next_var += 2
                    out_var = op.get("out")
                    out_name = out_var if isinstance(out_var, str) else f"v{next_var}"
                    if not isinstance(out_var, str):
                        next_var += 1
                    new_ops.extend(
                        _module_ensure_call_ops(
                            module_id,
                            id_var=id_var,
                            boxed_var=boxed_var,
                            out_var=out_name,
                        )
                    )
                    changed = True
                    continue
            elif kind == "call_func":
                name = _literal_plain_import_transaction_name(
                    op,
                    const_str_by_var=const_str_by_var,
                    builtin_func_by_var=builtin_func_by_var,
                    const_none_vars=const_none_vars,
                    empty_tuple_vars=empty_tuple_vars,
                    const_int_by_var=const_int_by_var,
                )
                full_id = (
                    registry.ensure_lane_id(name) if isinstance(name, str) else None
                )
                top_id = (
                    registry.ensure_lane_id(name.split(".", 1)[0])
                    if isinstance(name, str) and full_id is not None
                    else None
                )
                if full_id is not None and top_id is not None:
                    out_var = op.get("out")
                    out_name = out_var if isinstance(out_var, str) else f"v{next_var}"
                    if not isinstance(out_var, str):
                        next_var += 1
                    if top_id == full_id:
                        id_var = f"v{next_var}"
                        boxed_var = f"v{next_var + 1}"
                        next_var += 2
                        new_ops.extend(
                            _module_ensure_call_ops(
                                full_id,
                                id_var=id_var,
                                boxed_var=boxed_var,
                                out_var=out_name,
                            )
                        )
                    else:
                        # `import a.b` initializes a.b (parent-first inside
                        # ensure) and binds the top-level module.  A failure
                        # in the first ensure leaves the pending exception
                        # set; ensure propagates it without running another
                        # init, and the site's existing check_exception fires.
                        full_id_var = f"v{next_var}"
                        full_boxed_var = f"v{next_var + 1}"
                        full_out_var = f"v{next_var + 2}"
                        top_id_var = f"v{next_var + 3}"
                        top_boxed_var = f"v{next_var + 4}"
                        next_var += 5
                        new_ops.extend(
                            _module_ensure_call_ops(
                                full_id,
                                id_var=full_id_var,
                                boxed_var=full_boxed_var,
                                out_var=full_out_var,
                            )
                        )
                        new_ops.extend(
                            _module_ensure_call_ops(
                                top_id,
                                id_var=top_id_var,
                                boxed_var=top_boxed_var,
                                out_var=out_name,
                            )
                        )
                    changed = True
                    continue
            new_ops.append(op if isinstance(op, dict) else op)
        if changed:
            func["ops"] = new_ops
    return None


_STATIC_NATIVE_PREPARE_SYMBOL = "molt_cpython_abi_prepare_static_extension"
_STATIC_NATIVE_PYINIT_TO_BITS_SYMBOL = "molt_cpython_abi_pyinit_module_to_bits"
_STATIC_NATIVE_PYINIT_EXPORT_PREFIX = "__molt_static_pyinit__."


def _parent_module_parts(module_name: str) -> tuple[str, str] | None:
    if "." not in module_name:
        return None
    parent_name, attr_name = module_name.rsplit(".", 1)
    if not parent_name or not attr_name:
        return None
    return parent_name, attr_name


def _append_static_native_module_metadata_ops(
    ops: list[dict[str, Any]],
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
    ops: list[dict[str, Any]],
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


def _append_static_native_module_attr_export_ops(
    ops: list[dict[str, Any]],
    *,
    module_var: str,
    spec: _ExternalNativeModuleInitSpec,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> int:
    exports_by_provider: dict[str, list[str]] = {}
    for export in spec.module_attr_exports:
        exports_by_provider.setdefault(export.provider_module, []).append(export.attr)
    for provider_module in sorted(exports_by_provider):
        provider_init = SimpleTIRGenerator.module_init_symbol(provider_module)
        provider_init_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "call",
                    "s_value": provider_init,
                    "args": [],
                    "out": provider_init_var,
                    "value": register_global_code_id(provider_init),
                },
                {"kind": "check_exception", "value": 1},
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
                    {"kind": "check_exception", "value": 1},
                    {
                        "kind": "module_set_attr",
                        "args": [module_var, attr_name_var, attr_value_var],
                        "out": attr_set_var,
                    },
                ]
            )
    return next_var


def _build_registry_native_module_init_ops(
    spec: _ExternalNativeModuleInitSpec,
    *,
    register_global_code_id: Callable[[str], int],
) -> list[dict[str, Any]]:
    """Static-native module init body for the registry (import bedrock) lane.

    Pure body executor: no cache-probe preamble (init-exactly-once is the
    ``molt_module_ensure`` CAS, invariant I5), no parent binding (ensure's
    ``bind_parent_attr`` owns it from the registry parent edge), and no alias
    bodies (``kind=Alias`` rows resolve inside ensure).  Provider modules for
    capsule/attr exports are ordinary registry dependencies reached through
    direct ``call molt_init_<provider>`` ops here, which the
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
    ops: list[dict[str, Any]] = [
        {"kind": "const_str", "s_value": spec.module, "out": module_name_var},
    ]
    module_var: str
    if spec.is_extension:
        prepare_var = f"v{next_var}"
        next_var += 1
        pyobj_var = f"v{next_var}"
        next_var += 1
        module_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "call",
                    "s_value": _STATIC_NATIVE_PREPARE_SYMBOL,
                    "args": [],
                    "out": prepare_var,
                    "value": register_global_code_id(_STATIC_NATIVE_PREPARE_SYMBOL),
                },
                {
                    "kind": "invoke_ffi",
                    "args": [],
                    "out": pyobj_var,
                    "native_callable_export": (
                        f"{_STATIC_NATIVE_PYINIT_EXPORT_PREFIX}{spec.module}"
                    ),
                    "native_callable_binding": "direct_symbol",
                    "native_callable_abi": NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
                    "native_callable_symbol": spec.init_symbol,
                },
                {
                    "kind": "call",
                    "s_value": _STATIC_NATIVE_PYINIT_TO_BITS_SYMBOL,
                    "args": [pyobj_var],
                    "out": module_var,
                    "value": register_global_code_id(
                        _STATIC_NATIVE_PYINIT_TO_BITS_SYMBOL
                    ),
                },
                {"kind": "check_exception", "value": 1},
            ]
        )
    else:
        module_var = f"v{next_var}"
        next_var += 1
        ops.append({"kind": "module_new", "args": [module_name_var], "out": module_var})
    cache_set_var = f"v{next_var}"
    next_var += 1
    # Publication (module_cache_set) stays in the body so the module is
    # visible in the store for the entirety of body execution — the same
    # publish-before-exec shape frontend-compiled module bodies have
    # (invariant I6); the runtime mirrors it into the ModuleTable slot while
    # this ensure transaction is open.
    ops.append(
        {
            "kind": "module_cache_set",
            "args": [module_name_var, module_var],
            "out": cache_set_var,
        }
    )
    next_var = _append_static_native_module_metadata_ops(
        ops,
        module_var=module_var,
        module_name=spec.module,
        is_extension=spec.is_extension,
        next_var=next_var,
    )
    next_var = _append_static_native_module_attr_export_ops(
        ops,
        module_var=module_var,
        spec=spec,
        register_global_code_id=register_global_code_id,
        next_var=next_var,
    )
    ops.append({"kind": "ret_void"})
    if spec.is_extension or spec.module_attr_exports:
        ops.extend(({"kind": "label", "value": 1}, {"kind": "ret_void"}))
    return ops


def _build_static_native_module_init_ops(
    spec: _ExternalNativeModuleInitSpec,
    *,
    register_global_code_id: Callable[[str], int],
) -> list[dict[str, Any]]:
    next_var = 0
    module_name_var = f"v{next_var}"
    next_var += 1
    ops: list[dict[str, Any]] = [
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
        provider_init = SimpleTIRGenerator.module_init_symbol(spec.alias_of)
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
                    "value": register_global_code_id(provider_init),
                },
                {"kind": "check_exception", "value": 1},
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
        prepare_var = f"v{next_var}"
        next_var += 1
        pyobj_var = f"v{next_var}"
        next_var += 1
        module_var = f"v{next_var}"
        next_var += 1
        ops.extend(
            [
                {
                    "kind": "call",
                    "s_value": _STATIC_NATIVE_PREPARE_SYMBOL,
                    "args": [],
                    "out": prepare_var,
                    "value": register_global_code_id(_STATIC_NATIVE_PREPARE_SYMBOL),
                },
                {
                    "kind": "invoke_ffi",
                    "args": [],
                    "out": pyobj_var,
                    "native_callable_export": (
                        f"{_STATIC_NATIVE_PYINIT_EXPORT_PREFIX}{spec.module}"
                    ),
                    "native_callable_binding": "direct_symbol",
                    "native_callable_abi": NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
                    "native_callable_symbol": spec.init_symbol,
                },
                {
                    "kind": "call",
                    "s_value": _STATIC_NATIVE_PYINIT_TO_BITS_SYMBOL,
                    "args": [pyobj_var],
                    "out": module_var,
                    "value": register_global_code_id(
                        _STATIC_NATIVE_PYINIT_TO_BITS_SYMBOL
                    ),
                },
                {"kind": "check_exception", "value": 1},
            ]
        )
    else:
        module_var = f"v{next_var}"
        next_var += 1
        ops.append({"kind": "module_new", "args": [module_name_var], "out": module_var})
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
    ops.append({"kind": "end_if"})
    # Re-load through the cache so parent binding and attr publication use
    # the canonical module object on both the first-init and cached paths.
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
    next_var = _append_static_native_module_attr_export_ops(
        ops,
        module_var=canonical_module_var,
        spec=spec,
        register_global_code_id=register_global_code_id,
        next_var=next_var,
    )
    ops.append({"kind": "ret_void"})
    if spec.is_extension or spec.is_alias or spec.module_attr_exports:
        ops.extend(({"kind": "label", "value": 1}, {"kind": "ret_void"}))
    return ops


def _append_static_native_module_init_functions(
    functions: list[dict[str, Any]],
    *,
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
    existing = {
        func.get("name")
        for func in functions
        if isinstance(func, Mapping) and isinstance(func.get("name"), str)
    }
    owned_modules: list[str] = []
    for spec in specs:
        init_symbol = SimpleTIRGenerator.module_init_symbol(spec.module)
        if init_symbol in existing:
            continue
        if registry_lane and spec.is_alias:
            owned_modules.append(spec.module)
            existing.add(init_symbol)
            continue
        register_global_code_id(init_symbol)
        build_ops = (
            _build_registry_native_module_init_ops
            if registry_lane
            else _build_static_native_module_init_ops
        )
        functions.append(
            {
                "name": init_symbol,
                "params": [],
                "ops": build_ops(
                    spec,
                    register_global_code_id=register_global_code_id,
                ),
            }
        )
        owned_modules.append(spec.module)
        existing.add(init_symbol)
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


def _normalize_ir_labels(ir: Mapping[str, Any]) -> dict[str, Any]:
    """Remap label/state IDs in emitted IR to sequential values per function.

    Different backends compile different sets of stdlib initialization functions
    before user code, which shifts the global label counter.  Normalizing labels
    makes the emitted IR deterministic regardless of backend, ensuring parity
    tests compare semantic content rather than implementation-specific counters.
    """
    normalized: dict[str, Any] = dict(ir)
    functions = normalized.get("functions")
    if not isinstance(functions, list):
        return normalized

    # Keys in an op whose integer value is a label/state ID.
    _LABEL_KEYS = frozenset({"value"})
    # Op kinds that define or reference labels.
    _LABEL_OPS = frozenset(
        {
            "label",
            "jump",
            "br_if",
            "check_exception",
            "for_iter_next",
        }
    )

    new_functions = []
    for func in functions:
        if not isinstance(func, dict):
            new_functions.append(func)
            continue
        ops = func.get("ops")
        if not isinstance(ops, list):
            new_functions.append(func)
            continue

        # First pass: collect all label IDs in order of appearance.
        label_map: dict[int, int] = {}
        next_id = 1
        for op in ops:
            if not isinstance(op, dict):
                continue
            kind = op.get("kind", "")
            if kind not in _LABEL_OPS:
                continue
            val = op.get("value")
            if isinstance(val, int) and val not in label_map:
                label_map[val] = next_id
                next_id += 1

        # Second pass: rewrite label IDs.
        new_ops = []
        for op in ops:
            if not isinstance(op, dict):
                new_ops.append(op)
                continue
            kind = op.get("kind", "")
            if kind in _LABEL_OPS and "value" in op:
                val = op["value"]
                if isinstance(val, int) and val in label_map:
                    op = {**op, "value": label_map[val]}
            new_ops.append(op)

        new_func = {**func, "ops": new_ops}
        new_functions.append(new_func)

    normalized["functions"] = new_functions
    return normalized


def _write_emitted_ir(emit_ir_path: Path | None, ir: Mapping[str, Any]) -> str | None:
    if emit_ir_path is None:
        return None
    try:
        normalized = _normalize_ir_labels(ir)
        _atomic_write_json(emit_ir_path, normalized, indent=2, default=_json_ir_default)
    except OSError as exc:
        return f"Failed to write IR: {exc}"
    return None


def _module_owned_symbol_map(module_names: Collection[str]) -> dict[str, str]:
    module_by_symbol: dict[str, str] = {}
    for module_name in module_names:
        if not isinstance(module_name, str) or not module_name:
            continue
        module_symbol = _module_symbol_name(module_name)
        existing = module_by_symbol.get(module_symbol)
        if existing is None or module_name.count(".") > existing.count("."):
            module_by_symbol[module_symbol] = module_name
    return dict(
        sorted(module_by_symbol.items(), key=lambda item: len(item[0]), reverse=True)
    )


def _module_owned_symbol_name(
    symbol_name: str,
    module_by_symbol: Mapping[str, str],
) -> str | None:
    for module_symbol, module_name in module_by_symbol.items():
        if _emitted_name_matches_module_symbol(symbol_name, module_symbol):
            return module_name
    return None


def _static_backend_ir_module_call_targets(
    ir: Mapping[str, Any],
    module_names: Collection[str],
) -> tuple[tuple[str, str, str, int], ...]:
    """Return statically known direct module-symbol calls from backend-facing IR."""
    module_by_symbol = _module_owned_symbol_map(module_names)
    if not module_by_symbol:
        return ()
    targets: list[tuple[str, str, str, int]] = []
    functions = ir.get("functions")
    if not isinstance(functions, list):
        return ()
    for func in functions:
        if not isinstance(func, Mapping):
            continue
        func_map = cast(Mapping[str, object], func)
        func_name = func_map.get("name")
        if not isinstance(func_name, str) or not func_name:
            func_name = "<unknown>"
        ops = func_map.get("ops")
        if not isinstance(ops, list):
            continue
        for index, op in enumerate(ops):
            if not isinstance(op, Mapping):
                continue
            op_map = cast(Mapping[str, object], op)
            if op_map.get("kind") != "call":
                continue
            symbol_name = op_map.get("s_value")
            if not isinstance(symbol_name, str) or symbol_name.startswith("molt_"):
                continue
            module_name = _module_owned_symbol_name(symbol_name, module_by_symbol)
            if module_name is not None:
                targets.append((module_name, symbol_name, func_name, index))
    return tuple(targets)


def _static_backend_ir_module_call_closure_issue(
    ir: Mapping[str, Any],
    module_graph: Mapping[str, Path],
    module_names: Collection[str],
) -> str | None:
    """Fail early when backend IR directly calls symbols from absent modules."""
    graph_modules = set(module_graph)
    missing: list[tuple[str, str, str, int]] = []
    seen: set[tuple[str, str]] = set()
    for (
        module_name,
        symbol_name,
        func_name,
        index,
    ) in _static_backend_ir_module_call_targets(ir, module_names):
        if module_name in graph_modules or (module_name, symbol_name) in seen:
            continue
        seen.add((module_name, symbol_name))
        missing.append((module_name, symbol_name, func_name, index))
    if not missing:
        return None
    preview = ", ".join(
        f"{symbol_name} ({module_name}) at {func_name}[{index}]"
        for module_name, symbol_name, func_name, index in missing[:8]
    )
    suffix = "" if len(missing) <= 8 else ", ..."
    return (
        "backend IR contains direct calls to module symbols outside the module graph: "
        f"{preview}{suffix}; graph_modules={len(graph_modules)}"
    )


def _reachability_feature_refusal(
    ir: Mapping[str, Any],
    *,
    stdlib_profile: str | None,
    target: str,
) -> str | None:
    """Refuse when the reached SimpleIR needs a feature the profile excludes.

    Computes ``RequiredLinkFeatures`` from the finalized merged backend IR and
    compares it against the selected profile's per-target link-affecting feature
    ceiling (``runtime_features.profile_link_features``). Returns a truthful,
    reached-intrinsic refusal message when a required feature is excluded, or
    ``None`` to proceed. The ceiling is computed for the build target's triple so
    a WASM build is checked against the WASM feature surface and a native build
    against the native surface. ``RequiredLinkFeatures`` itself is
    target-independent (a property of the reached IR).
    """
    functions = ir.get("functions")
    if not isinstance(functions, list):
        return None
    is_wasm = target in {"wasm", "wasm-freestanding"} or target.startswith("wasm32")
    target_triple = "wasm32-wasip1" if is_wasm else None
    # The ceiling is the SAME per-target available-feature authority the build
    # uses to select the staticlib (``_runtime_builtin_features_for_profile``):
    # the Cargo ladder plus explicit target exclusions. Using this keeps the
    # reachability ceiling aligned with the features the linked archive provides.
    profile_features = frozenset(
        _runtime_features._runtime_builtin_features_for_profile(
            stdlib_profile or DEFAULT_STDLIB_PROFILE,
            target_triple=target_triple,
        )
    )
    return _required_features.reachability_profile_feature_refusal(
        functions,
        profile_name=stdlib_profile or DEFAULT_STDLIB_PROFILE,
        profile_features=profile_features,
    )


def _prepare_backend_ir(
    *,
    entry_module: str,
    module_graph: Mapping[str, Path],
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    known_classes: Mapping[str, Any],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    frontend_phase_timeout: float | None,
    integration_state: _FrontendIntegrationState,
    diagnostics_state: _MidendDiagnosticsState,
    record_frontend_timing: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
    module_order: Sequence[str],
    runtime_import_dispatch_roots: Collection[str],
    generated_module_source_paths: Mapping[str, str],
    spawn_enabled: bool,
    pgo_profile_summary: Any | None,
    runtime_feedback_summary: Any | None,
    emit_ir_path: Path | None,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
    target: str = "native",
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
) -> tuple[_PreparedBackendIR | None, _CliFailure | None]:
    registry_lane = _module_registry_target_enabled(target)
    entry_path: Path | None = None
    if entry_module != "__main__":
        entry_path = module_graph.get(entry_module)
        if entry_path is None:
            return None, fail(
                f"Entry module not found: {entry_module}",
                json_output,
                command="build",
            )
        if not registry_lane:
            # Dedup: the entry module is already compiled with entry_module=
            # entry_module (giving it __main__ semantics — dynamic __name__,
            # MODULE_CACHE_SET for "__main__", etc.).  Emit a thin trampoline
            # molt_init___main__ that delegates to the real init instead of
            # re-compiling the entire module.  On the registry lane no
            # trampoline exists: the __main__ registry row points at the entry
            # module's init body directly (one init table entry, two rows).
            _entry_real_init = SimpleTIRGenerator.module_init_symbol(entry_module)
            _main_init = SimpleTIRGenerator.module_init_symbol("__main__")
            _trampoline_code_id = _register_global_code_id_with_state(
                integration_state, _entry_real_init
            )
            integration_state.functions.append(
                {
                    "name": _main_init,
                    "params": [],
                    "ops": [
                        {
                            "kind": "call",
                            "s_value": _entry_real_init,
                            "args": [],
                            "out": "v0",
                            "value": _trampoline_code_id,
                        },
                        {"kind": "ret_void"},
                    ],
                }
            )

    functions = integration_state.functions
    global_code_ids = integration_state.global_code_ids

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
    generated_native_init_modules = _append_static_native_module_init_functions(
        functions,
        specs=native_module_init_specs,
        register_global_code_id=register_global_code_id,
        registry_lane=registry_lane,
    )
    native_module_order = [spec.module for spec in native_module_init_specs]
    native_runtime_import_dispatch_roots = set(runtime_import_dispatch_roots)
    native_runtime_import_dispatch_roots.update(native_module_order)

    entry_init_name = "__main__" if entry_module != "__main__" else entry_module
    entry_init = SimpleTIRGenerator.module_init_symbol(entry_init_name)
    version_ops = _build_version_info_ops(
        register_global_code_id=register_global_code_id,
        target_python=target_python,
    )
    entry_ops = _build_entry_main_ops(
        entry_init=entry_init,
        version_ops=version_ops,
        register_global_code_id=register_global_code_id,
    )
    entry_call_idx = _entry_call_index(entry_ops, entry_init)
    next_var = _next_tir_var_index(entry_ops)
    # Determine whether to inject a sys init call into molt_main.
    #
    # With micro profile, sys initialisation is handled entirely by the
    # frontend's _emit_module_load: the first module that does
    # ``import sys`` will trigger molt_init_sys lazily.  Injecting it
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
            register_global_code_id=register_global_code_id,
            next_var=next_var,
            lazy=False,
        )
        entry_call_idx = _entry_call_index(entry_ops, entry_init)
    module_code_ops, next_var, per_module_ops = _build_module_code_ops(
        module_order=module_order,
        module_graph=module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        entry_path=entry_path,
        register_global_code_id=register_global_code_id,
        next_var=next_var,
    )

    # ── Lazy stdlib initialisation ──
    #
    # Previously *all* module code-slot metadata was set up eagerly in
    # ``molt_main`` before any module init ran.  This meant that even modules
    # the program never imports paid the cost of allocating a code object at
    # startup.
    #
    # Each module's own ``molt_init_*`` function already contains a
    # ``CODE_SLOT_SET`` emitted by the frontend, so it is safe to omit the
    # duplicate setup from ``molt_main`` for modules that are not needed at
    # startup.  Only the entry module (and ``sys`` when eagerly injected in
    # the ``full`` profile) keep their code-slot ops in ``molt_main``.
    eager_modules: set[str] = {entry_module}
    if entry_module != "__main__":
        eager_modules.add("__main__")
    # When sys is eagerly injected (full profile), its code-slot metadata
    # must also be set up before the init call runs.
    if _inject_sys_init:
        eager_modules.add("sys")

    # Collect only the eager module code-slot ops for molt_main.
    entry_module_code_ops: list[dict[str, Any]] = []
    for mod_name in eager_modules:
        entry_module_code_ops.extend(per_module_ops.get(mod_name, []))

    entry_call_idx = _entry_call_index(entry_ops, entry_init)
    entry_ops[entry_call_idx:entry_call_idx] = entry_module_code_ops
    host_init_ops = _build_entry_main_ops(
        entry_init=entry_init,
        version_ops=version_ops,
        register_global_code_id=register_global_code_id,
    )
    if _inject_sys_init:
        _append_entry_sys_init_op(
            host_init_ops,
            entry_init=entry_init,
            register_global_code_id=register_global_code_id,
            next_var=next_var,
            lazy=False,
        )
    host_init_call_idx = _entry_call_index(host_init_ops, entry_init)
    host_init_ops[host_init_call_idx:host_init_call_idx] = entry_module_code_ops
    host_init_ops.insert(1, {"kind": "code_slots_init", "value": len(global_code_ids)})
    if spawn_enabled:
        _replace_entry_call_with_spawn_override(
            entry_ops,
            entry_init=entry_init,
            register_global_code_id=register_global_code_id,
            next_var=next_var,
        )
    entry_ops.insert(1, {"kind": "code_slots_init", "value": len(global_code_ids)})
    functions.append({"name": "molt_host_init", "params": [], "ops": host_init_ops})
    functions.append({"name": "molt_main", "params": [], "ops": entry_ops})
    isolate_bootstrap_ops = _build_isolate_bootstrap_ops(
        code_slot_count=len(global_code_ids),
        version_ops=version_ops,
        module_code_ops=entry_module_code_ops,
    )
    functions.append(
        {"name": "molt_isolate_bootstrap", "params": [], "ops": isolate_bootstrap_ops}
    )
    module_registry: ModuleRegistry | None = None
    if registry_lane:
        # Import bedrock (design doc 69, PR1): the ModuleRegistry replaces the
        # molt_isolate_import string_eq dispatch chain.  Cold dispatch is the
        # MODULE_INIT_TABLE relocation column of the registry blob; literal
        # import sites lower onto molt_module_ensure(const ModuleId).
        try:
            module_registry = _build_native_module_registry(
                entry_module=entry_module,
                module_order=module_order,
                runtime_import_dispatch_roots=native_runtime_import_dispatch_roots,
                native_module_init_specs=native_module_init_specs,
                generated_native_init_modules=generated_native_init_modules,
                spawn_enabled=spawn_enabled,
            )
        except ValueError as exc:
            return None, fail(
                f"module registry construction failed: {exc}",
                json_output,
                command="build",
            )
        rewrite_issue = _rewrite_native_import_lanes(functions, module_registry)
        if rewrite_issue is not None:
            return None, fail(rewrite_issue, json_output, command="build")
    else:
        import_ops = _build_isolate_import_ops(
            code_slot_count=len(global_code_ids),
            module_order=_isolate_import_module_order(
                module_order,
                native_runtime_import_dispatch_roots,
                native_module_order=native_module_order,
            ),
            register_global_code_id=register_global_code_id,
        )
        functions.append(
            {"name": "molt_isolate_import", "params": ["p0"], "ops": import_ops}
        )
    ir = _finalize_backend_ir(
        functions=functions,
        pgo_profile_summary=pgo_profile_summary,
        runtime_feedback_summary=runtime_feedback_summary,
    )
    if module_registry is not None:
        ir["module_registry"] = module_registry.backend_ir_payload()
    # Reachability-driven runtime-feature requirement / refusal (Option b,
    # docs/design/foundation/feature_reachability_tree_shaking.md). ``ir`` is the
    # finalized merged backend IR (exactly the function list the native/WASM
    # backends dead-strip), so ``required_features`` computes the link-affecting
    # features the REACHED code actually needs and refuses BEFORE codegen/link
    # (with a truthful, reached-intrinsic message) when the selected profile's
    # ceiling omits one. This is the only requirement fact; stdlib source scans
    # are classifiers/audits, never link-feature authority.
    feature_refusal = _reachability_feature_refusal(
        ir,
        stdlib_profile=stdlib_profile,
        target=target,
    )
    if feature_refusal is not None:
        return None, fail(feature_refusal, json_output, command="build")
    module_call_issue = _static_backend_ir_module_call_closure_issue(
        ir,
        module_graph,
        set(module_graph) | set(known_modules) | set(stdlib_allowlist),
    )
    if module_call_issue is not None:
        return None, fail(module_call_issue, json_output, command="build")
    functions_for_features = ir.get("functions")
    required_link_features = (
        _required_features.required_link_features(
            cast(Sequence[Mapping[str, object]], functions_for_features)
        )
        if isinstance(functions_for_features, list)
        else frozenset()
    )
    emit_ir_error = _write_emitted_ir(emit_ir_path, ir)
    if emit_ir_error is not None:
        return None, fail(emit_ir_error, json_output, command="build")
    return _PreparedBackendIR(
        ir=ir,
        required_link_features=required_link_features,
        module_registry=module_registry,
    ), None

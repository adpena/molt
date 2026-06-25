from __future__ import annotations

import ast
import contextlib
import functools
import os
import signal
import sys
import threading
import time
from concurrent.futures import ProcessPoolExecutor
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Callable, Collection, Mapping, Sequence, cast

from molt.compat import CompatibilityError
from molt.frontend import SimpleTIRGenerator
from molt.type_facts import TypeFacts

from molt.cli import frontend_parallel as _frontend_parallel
from molt.cli.models import (
    BuildProfile,
    FallbackPolicy,
    ParseCodec,
    TypeHintPolicy,
    _EntryFrontendLoweringContext,
    _FrontendIntegrationState,
    _FrontendLayerExecutionContext,
    _FrontendLayerPlan,
    _FrontendLayerRunResult,
    _FrontendLayerRuntimeHooks,
    _FrontendModuleResultTimings,
    _FrontendParallelConfig,
    _FrontendParallelLayerState,
    _MidendDiagnosticsState,
    _ModuleGraphMetadata,
    _ModuleLowerError,
    _ParallelWorkerSubmission,
    _PreparedFrontendRunTicket,
    _ScopedLoweringInputView,
    _ScopedLoweringInputs,
    _SerialFrontendLoweringContext,
    _SerialFrontendLoweringHooks,
)
from molt.cli.module_graph import ModuleSyntaxErrorInfo
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import (
    _ModuleSourceCatalog,
    _build_module_source_catalog,
    _read_module_source,
)
from molt.cli.module_cache import (
    _build_scoped_known_classes_snapshot,
    _load_cached_module_lowering_result,
    _module_lowering_context_digest_for_module,
    _module_lowering_execution_view,
    _module_worker_payload,
    _read_persisted_module_lowering,
    _write_persisted_module_lowering,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.target_python import (
    TargetPythonVersion,
    _parse_source_for_target,
    _parse_target_python_version,
)

def _format_syntax_error_message(info: ModuleSyntaxErrorInfo) -> str:
    if info.lineno is None:
        return info.message
    filename = Path(info.filename).name if info.filename else "<unknown>"
    return f"{info.message} ({filename}, line {info.lineno})"


def _syntax_error_stub_ast(info: ModuleSyntaxErrorInfo) -> ast.Module:
    msg = _format_syntax_error_message(info)
    err_name = ast.Name(id="err", ctx=ast.Store())
    err_value = ast.Name(id="err", ctx=ast.Load())
    stmts: list[ast.stmt] = [
        ast.Assign(
            targets=[err_name],
            value=ast.Call(
                func=ast.Name(id="SyntaxError", ctx=ast.Load()),
                args=[ast.Constant(msg)],
                keywords=[],
            ),
        )
    ]
    attr_values = [
        ("lineno", info.lineno),
        ("offset", info.offset),
        ("filename", Path(info.filename).name if info.filename else None),
        ("text", info.text),
    ]
    for attr_name, value in attr_values:
        if value is None:
            continue
        stmts.append(
            ast.Assign(
                targets=[
                    ast.Attribute(
                        value=err_value,
                        attr=attr_name,
                        ctx=ast.Store(),
                    )
                ],
                value=ast.Constant(value),
            )
        )
    stmts.append(ast.Raise(exc=err_value, cause=None))
    module = ast.Module(body=stmts, type_ignores=[])
    return ast.fix_missing_locations(module)


def _read_worker_source_lease(raw_lease: object) -> str:
    if not isinstance(raw_lease, Mapping):
        raise ValueError("missing source lease")
    lease = cast(Mapping[str, object], raw_lease)
    kind = lease.get("kind")
    if kind == "inline":
        source = lease.get("source")
        if not isinstance(source, str):
            raise ValueError("inline source lease is missing source text")
        return source
    if kind != "path":
        raise ValueError(f"unsupported source lease kind: {kind!r}")
    raw_path = lease.get("path")
    if not isinstance(raw_path, str) or not raw_path:
        raise ValueError("path source lease is missing path")
    path = Path(raw_path)
    expected_size = lease.get("source_size")
    expected_mtime_ns = lease.get("mtime_ns")
    if expected_size is not None or expected_mtime_ns is not None:
        stat = path.stat()
        if isinstance(expected_size, int) and stat.st_size != expected_size:
            raise OSError(f"Source lease for {path} changed size during compile")
        if isinstance(expected_mtime_ns, int) and stat.st_mtime_ns != expected_mtime_ns:
            raise OSError(f"Source lease for {path} changed mtime during compile")
    return _read_module_source(path)


def _frontend_lower_module_worker(payload: dict[str, Any]) -> dict[str, Any]:
    worker_started_ns = time.time_ns()
    worker_pid = os.getpid()
    module_name = str(payload["module_name"])
    module_path = str(payload["module_path"])
    logical_source_path = str(payload.get("logical_source_path") or module_path)
    try:
        source = _read_worker_source_lease(payload["source_lease"])
    except (OSError, UnicodeDecodeError, SyntaxError, ValueError) as exc:
        worker_finished_ns = time.time_ns()
        return {
            "ok": False,
            "error": f"Failed to read module {module_path}: {exc}",
            "timings": {
                "visit_s": 0.0,
                "lower_s": 0.0,
                "total_s": 0.0,
            },
            "worker": {
                "pid": worker_pid,
                "started_ns": worker_started_ns,
                "finished_ns": worker_finished_ns,
            },
        }
    parse_codec = cast(ParseCodec, payload["parse_codec"])
    type_hint_policy = cast(TypeHintPolicy, payload["type_hint_policy"])
    fallback_policy = cast(FallbackPolicy, payload["fallback_policy"])
    module_is_namespace = bool(payload["module_is_namespace"])
    entry_module = cast(str | None, payload["entry_module"])
    enable_phi = bool(payload["enable_phi"])
    known_modules = set(cast(list[str], payload["known_modules"]))
    known_classes = cast(dict[str, Any], payload["known_classes"])
    stdlib_allowlist = set(cast(list[str], payload["stdlib_allowlist"]))
    known_func_defaults = cast(
        dict[str, dict[str, dict[str, Any]]], payload["known_func_defaults"]
    )
    known_func_kinds = cast(dict[str, dict[str, str]], payload["known_func_kinds"])
    module_chunking = bool(payload["module_chunking"])
    module_chunk_max_ops = int(payload["module_chunk_max_ops"])
    optimization_profile = cast(BuildProfile, payload["optimization_profile"])
    target_python = _parse_target_python_version(
        cast(str | None, payload.get("target_python"))
    )
    pgo_hot_functions = {
        symbol.strip()
        for symbol in cast(list[str], payload.get("pgo_hot_functions", []))
        if isinstance(symbol, str) and symbol.strip()
    }
    module_frontend_start = time.perf_counter()
    visit_s = 0.0
    lower_s = 0.0
    try:
        tree = _parse_source_for_target(
            source,
            filename=logical_source_path,
            target_python=target_python,
        )
    except SyntaxError as exc:
        worker_finished_ns = time.time_ns()
        return {
            "ok": False,
            "error": f"Syntax error in {module_path}: {exc}",
            "timings": {
                "visit_s": visit_s,
                "lower_s": lower_s,
                "total_s": time.perf_counter() - module_frontend_start,
            },
            "worker": {
                "pid": worker_pid,
                "started_ns": worker_started_ns,
                "finished_ns": worker_finished_ns,
            },
        }
    gen = SimpleTIRGenerator(
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        source_path=logical_source_path,
        module_name=module_name,
        module_is_namespace=module_is_namespace,
        entry_module=entry_module,
        enable_phi=enable_phi,
        known_modules=known_modules,
        known_classes=known_classes,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_functions=pgo_hot_functions,
    )
    try:
        visit_start = time.perf_counter()
        gen.visit(tree)
        visit_s = time.perf_counter() - visit_start
        lower_start = time.perf_counter()
        ir = gen.to_json()
        lower_s = time.perf_counter() - lower_start
    except CompatibilityError as exc:
        worker_finished_ns = time.time_ns()
        return {
            "ok": False,
            "error": str(exc),
            "timings": {
                "visit_s": visit_s,
                "lower_s": lower_s,
                "total_s": time.perf_counter() - module_frontend_start,
            },
            "worker": {
                "pid": worker_pid,
                "started_ns": worker_started_ns,
                "finished_ns": worker_finished_ns,
            },
        }
    worker_finished_ns = time.time_ns()
    return {
        "ok": True,
        "functions": ir["functions"],
        "func_code_ids": dict(gen.func_code_ids),
        "local_class_names": sorted(gen.local_class_names),
        "local_classes": {
            class_name: gen.classes[class_name]
            for class_name in sorted(gen.local_class_names)
        },
        "midend_policy_outcomes_by_function": dict(
            gen.midend_policy_outcomes_by_function
        ),
        "midend_pass_stats_by_function": dict(gen.midend_pass_stats_by_function),
        "timings": {
            "visit_s": visit_s,
            "lower_s": lower_s,
            "total_s": time.perf_counter() - module_frontend_start,
        },
        "worker": {
            "pid": worker_pid,
            "started_ns": worker_started_ns,
            "finished_ns": worker_finished_ns,
        },
    }


def _module_frontend_payload(
    gen: SimpleTIRGenerator,
    ir: dict[str, Any],
    *,
    visit_s: float,
    lower_s: float,
    total_s: float,
) -> dict[str, Any]:
    return {
        "functions": ir["functions"],
        "func_code_ids": dict(gen.func_code_ids),
        "local_class_names": sorted(gen.local_class_names),
        "local_classes": {
            class_name: gen.classes[class_name]
            for class_name in sorted(gen.local_class_names)
        },
        "midend_policy_outcomes_by_function": dict(
            gen.midend_policy_outcomes_by_function
        ),
        "midend_pass_stats_by_function": dict(gen.midend_pass_stats_by_function),
        "timings": {
            "visit_s": visit_s,
            "lower_s": lower_s,
            "total_s": total_s,
        },
    }


def _module_frontend_generator(
    *,
    module_name: str,
    logical_source_path: str,
    entry_override: str | None,
    module_is_namespace: bool,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    enable_phi: bool,
    stdlib_allowlist: Collection[str],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    scoped_inputs: _ScopedLoweringInputView,
    scoped_known_classes: dict[str, Any],
) -> SimpleTIRGenerator:
    return SimpleTIRGenerator(
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        source_path=logical_source_path,
        type_facts=scoped_inputs.type_facts,
        module_name=module_name,
        module_is_namespace=module_is_namespace,
        entry_module=entry_override,
        enable_phi=enable_phi,
        known_modules=set(scoped_inputs.known_modules_set),
        known_classes=scoped_known_classes,
        stdlib_allowlist=set(stdlib_allowlist),
        known_func_defaults=scoped_inputs.known_func_defaults,
        known_func_kinds=scoped_inputs.known_func_kinds,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=cast(BuildProfile, optimization_profile),
        pgo_hot_functions=set(scoped_inputs.pgo_hot_function_names_set),
    )


def _known_classes_snapshot_copy(known_classes: Mapping[str, Any]) -> dict[str, Any]:
    if not known_classes:
        return {}
    return dict(known_classes)


def _resolve_tree_for_serial_frontend_module(
    module_name: str,
    module_path: Path,
    *,
    lowering_context: _SerialFrontendLoweringContext,
) -> ast.AST:
    if module_name in lowering_context.syntax_error_modules:
        return _syntax_error_stub_ast(
            lowering_context.syntax_error_modules[module_name]
        )
    tree = lowering_context.module_trees.get(module_name)
    if tree is not None:
        return tree
    try:
        source = lowering_context.module_source_catalog.read_source(
            module_name,
            module_path,
            lowering_context.module_resolution_cache,
        )
    except (SyntaxError, UnicodeDecodeError) as exc:
        raise _ModuleLowerError(f"Syntax error in {module_path}: {exc}") from exc
    except OSError as exc:
        raise _ModuleLowerError(f"Failed to read module {module_path}: {exc}") from exc
    logical_source_path = lowering_context.generated_module_source_paths.get(
        module_name, str(module_path)
    )
    try:
        return lowering_context.module_resolution_cache.parse_module_ast(
            module_path,
            source,
            filename=logical_source_path,
            retain=False,
            target_python=lowering_context.target_python,
        )
    except SyntaxError as exc:
        raise _ModuleLowerError(f"Syntax error in {module_path}: {exc}") from exc


def _lower_module_serial_with_context(
    module_name: str,
    module_path: Path,
    *,
    lowering_context: _SerialFrontendLoweringContext,
) -> tuple[dict[str, Any], float, float, float]:
    execution_view = _module_lowering_execution_view(
        module_name,
        module_path=module_path,
        module_graph_metadata=lowering_context.module_graph_metadata,
        module_deps=lowering_context.module_deps,
        known_modules=lowering_context.known_modules,
        known_func_defaults=lowering_context.known_func_defaults,
        known_func_kinds=lowering_context.known_func_kinds,
        pgo_hot_function_names=lowering_context.pgo_hot_function_names,
        type_facts=lowering_context.type_facts,
        known_classes_snapshot=lowering_context.known_classes,
        module_dep_closures=lowering_context.module_dep_closures,
        path_stat_by_module=lowering_context.module_path_stats,
        scoped_lowering_inputs=lowering_context.scoped_lowering_inputs,
        known_modules_sorted=lowering_context.known_modules_sorted,
        pgo_hot_function_names_sorted=lowering_context.pgo_hot_function_names_sorted,
    )
    metadata_view = execution_view.metadata
    scoped_inputs = execution_view.scoped_inputs
    logical_source_path = metadata_view.logical_source_path
    entry_override = metadata_view.entry_override
    is_package = metadata_view.is_package
    module_is_namespace = metadata_view.module_is_namespace
    path_stat = metadata_view.path_stat
    if path_stat is None:
        with contextlib.suppress(OSError):
            path_stat = lowering_context.module_resolution_cache.path_stat(module_path)
    scoped_known_classes = execution_view.scoped_known_classes
    context_digest: str | None = None
    if lowering_context.project_root is not None:
        context_digest = _module_lowering_context_digest_for_module(
            module_name,
            module_path,
            logical_source_path=logical_source_path,
            entry_override=entry_override,
            known_classes_snapshot=lowering_context.known_classes,
            parse_codec=lowering_context.parse_codec,
            type_hint_policy=lowering_context.type_hint_policy,
            fallback_policy=lowering_context.fallback_policy,
            type_facts=lowering_context.type_facts,
            enable_phi=lowering_context.enable_phi,
            known_modules=lowering_context.known_modules,
            stdlib_allowlist=lowering_context.stdlib_allowlist,
            known_func_defaults=lowering_context.known_func_defaults,
            known_func_kinds=lowering_context.known_func_kinds,
            module_deps=lowering_context.module_deps,
            module_is_namespace=module_is_namespace,
            module_chunking=lowering_context.module_chunking,
            module_chunk_max_ops=lowering_context.module_chunk_max_ops,
            optimization_profile=lowering_context.optimization_profile,
            pgo_hot_function_names=lowering_context.pgo_hot_function_names,
            known_modules_sorted=lowering_context.known_modules_sorted,
            stdlib_allowlist_sorted=lowering_context.stdlib_allowlist_sorted,
            pgo_hot_function_names_sorted=lowering_context.pgo_hot_function_names_sorted,
            module_dep_closures=lowering_context.module_dep_closures,
            scoped_lowering_inputs=lowering_context.scoped_lowering_inputs,
            scoped_inputs=scoped_inputs,
            scoped_known_classes=scoped_known_classes,
            is_package=is_package,
            path_stat=path_stat,
            target_python=lowering_context.target_python,
        )
        if (
            context_digest is not None
            and module_name not in lowering_context.dirty_lowering_modules
        ):
            cached_payload = _read_persisted_module_lowering(
                lowering_context.project_root,
                module_path,
                module_name=module_name,
                is_package=is_package,
                context_digest=context_digest,
                path_stat=path_stat,
                target_python=lowering_context.target_python,
            )
            if cached_payload is not None:
                return cached_payload, 0.0, 0.0, 0.0

    tree = _resolve_tree_for_serial_frontend_module(
        module_name,
        module_path,
        lowering_context=lowering_context,
    )
    gen = _module_frontend_generator(
        module_name=module_name,
        logical_source_path=logical_source_path,
        entry_override=entry_override,
        module_is_namespace=module_is_namespace,
        parse_codec=lowering_context.parse_codec,
        type_hint_policy=lowering_context.type_hint_policy,
        fallback_policy=lowering_context.fallback_policy,
        enable_phi=lowering_context.enable_phi,
        stdlib_allowlist=lowering_context.stdlib_allowlist,
        module_chunking=lowering_context.module_chunking,
        module_chunk_max_ops=lowering_context.module_chunk_max_ops,
        optimization_profile=lowering_context.optimization_profile,
        scoped_inputs=scoped_inputs,
        scoped_known_classes=scoped_known_classes,
    )
    module_frontend_start = time.perf_counter()
    visit_s = 0.0
    lower_s = 0.0
    try:
        visit_start = time.perf_counter()
        # Increase recursion limit for deeply nested ASTs (e.g., networkx large
        # dict/list literals).  Restore the original limit afterward to maintain
        # safety guarantees for the rest of the pipeline.
        _prev_recursion_limit = sys.getrecursionlimit()
        if _prev_recursion_limit < 8000:
            sys.setrecursionlimit(8000)
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name=f"frontend visit ({module_name})",
        ):
            gen.visit(tree)
        sys.setrecursionlimit(_prev_recursion_limit)
        visit_s = time.perf_counter() - visit_start
        lower_start = time.perf_counter()
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name=f"frontend IR lowering ({module_name})",
        ):
            ir = gen.to_json()
        lower_s = time.perf_counter() - lower_start
    except TimeoutError as exc:
        raise _ModuleLowerError(str(exc), timed_out=True) from exc
    except CompatibilityError as exc:
        raise _ModuleLowerError(str(exc)) from exc
    except NotImplementedError as exc:
        raise _ModuleLowerError(f"NotImplementedError in {module_name}: {exc}") from exc
    except SyntaxError as exc:
        # Format SyntaxError to match CPython's compile-time output exactly.
        # We manually format because traceback.format_exception_only produces
        # slightly different caret counts when text is set vs None.
        parts: list[str] = []
        fname = exc.filename or (str(module_path) if module_path else "<unknown>")
        parts.append(f'  File "{fname}", line {exc.lineno}')
        if exc.text:
            raw = exc.text.rstrip("\n")
            stripped = raw.lstrip()
            indent_removed = len(raw) - len(stripped)
            parts.append(f"    {stripped}")
            if exc.offset and exc.end_offset:
                adj_start = max(0, exc.offset - 1 - indent_removed)
                adj_end = max(adj_start, exc.end_offset - 1 - indent_removed)
                parts.append(" " * (adj_start + 4) + "^" * max(1, adj_end - adj_start))
        parts.append(f"SyntaxError: {exc.msg}")
        raise _ModuleLowerError("\n".join(parts)) from exc
    total_s = time.perf_counter() - module_frontend_start
    payload = _module_frontend_payload(
        gen,
        ir,
        visit_s=visit_s,
        lower_s=lower_s,
        total_s=total_s,
    )
    if lowering_context.project_root is not None and context_digest is not None:
        with contextlib.suppress(OSError):
            _write_persisted_module_lowering(
                lowering_context.project_root,
                module_path,
                module_name=module_name,
                is_package=is_package,
                context_digest=context_digest,
                result=payload,
                target_python=lowering_context.target_python,
            )
    return payload, visit_s, lower_s, total_s


def _run_serial_frontend_lower_with_context(
    module_name: str,
    module_path: Path,
    *,
    lowering_context: _SerialFrontendLoweringContext,
    lowering_hooks: _SerialFrontendLoweringHooks,
) -> tuple[
    dict[str, Any] | None, _FrontendModuleResultTimings | None, _CliFailure | None
]:
    try:
        result, visit_s, lower_s, total_s = _lower_module_serial_with_context(
            module_name,
            module_path,
            lowering_context=lowering_context,
        )
    except _ModuleLowerError as exc:
        lowering_hooks.record_frontend_timing(
            module_name=module_name,
            module_path=module_path,
            visit_s=0.0,
            lower_s=0.0,
            total_s=0.0,
            timed_out=exc.timed_out,
            detail=str(exc),
        )
        return (
            None,
            None,
            lowering_hooks.fail(str(exc), lowering_hooks.json_output, command="build"),
        )
    result_timings = _FrontendModuleResultTimings(
        visit_s=visit_s,
        lower_s=lower_s,
        total_s=total_s,
    )
    lowering_hooks.record_frontend_timing(
        module_name=module_name,
        module_path=module_path,
        visit_s=result_timings.visit_s,
        lower_s=result_timings.lower_s,
        total_s=result_timings.total_s,
    )
    return result, result_timings, None


def _register_global_code_id_with_state(
    integration_state: _FrontendIntegrationState,
    symbol: str,
) -> int:
    code_id = integration_state.global_code_ids.get(symbol)
    if code_id is None:
        code_id = integration_state.global_code_id_counter
        integration_state.global_code_ids[symbol] = code_id
        integration_state.global_code_id_counter += 1
    return code_id


def _remap_module_code_ops_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    funcs: list[dict[str, Any]],
    local_id_to_symbol: dict[int, str],
) -> None:
    for func in funcs:
        ops = func.get("ops", [])
        remapped_ops: list[dict[str, Any]] = []
        for op in ops:
            kind = op.get("kind")
            if kind == "code_slots_init":
                continue
            if kind in {"call", "call_internal"}:
                symbol = op.get("s_value")
                if symbol:
                    op["value"] = _register_global_code_id_with_state(
                        integration_state, symbol
                    )
            elif kind == "code_slot_set":
                local_id = op.get("value")
                symbol = local_id_to_symbol.get(local_id)
                if symbol is None:
                    raise ValueError(
                        f"Missing code symbol for id {local_id} in module {module_name}"
                    )
                op["value"] = _register_global_code_id_with_state(
                    integration_state, symbol
                )
            elif kind == "trace_enter_slot":
                local_id = op.get("value")
                symbol = local_id_to_symbol.get(local_id)
                if symbol is None:
                    raise ValueError(
                        f"Missing code symbol for id {local_id} in module {module_name}"
                    )
                op["value"] = _register_global_code_id_with_state(
                    integration_state, symbol
                )
            remapped_ops.append(op)
        func["ops"] = remapped_ops


def _accumulate_midend_diagnostics_with_state(
    diagnostics_state: _MidendDiagnosticsState,
    module_name: str,
    *,
    policy_outcomes_by_func: dict[str, dict[str, Any]],
    pass_stats_by_func: dict[str, dict[str, dict[str, Any]]],
) -> None:
    def normalize_function_name(function_name: str) -> str:
        if function_name == "molt_main":
            return SimpleTIRGenerator.module_init_symbol(module_name)
        return function_name

    for function_name in sorted(policy_outcomes_by_func):
        normalized_name = normalize_function_name(function_name)
        combined_name = f"{module_name}::{normalized_name}"
        outcome = policy_outcomes_by_func[function_name]
        copied_events: list[dict[str, Any]] = []
        for event in outcome.get("degrade_events", []):
            if isinstance(event, dict):
                copied_events.append(dict(event))
        copied_outcome = dict(outcome)
        copied_outcome["degrade_events"] = copied_events
        diagnostics_state.policy_outcomes_by_function[combined_name] = copied_outcome
    for function_name in sorted(pass_stats_by_func):
        normalized_name = normalize_function_name(function_name)
        combined_name = f"{module_name}::{normalized_name}"
        per_pass = pass_stats_by_func[function_name]
        copied_per_pass: dict[str, dict[str, Any]] = {}
        for pass_name in sorted(per_pass):
            copied_stats = dict(per_pass[pass_name])
            samples = copied_stats.get("samples_ms")
            if isinstance(samples, list):
                copied_stats["samples_ms"] = list(samples)
            copied_per_pass[pass_name] = copied_stats
        diagnostics_state.pass_stats_by_function[combined_name] = copied_per_pass


def _integrate_module_frontend_result_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    *,
    ir_functions: list[dict[str, Any]],
    func_code_ids: dict[str, int],
    local_class_names: list[str],
    local_classes: dict[str, Any],
) -> str | None:
    init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
    local_code_ids = dict(func_code_ids)
    if "molt_main" in local_code_ids:
        local_code_ids[init_symbol] = local_code_ids.pop("molt_main")
    local_id_to_symbol = {code_id: symbol for symbol, code_id in local_code_ids.items()}
    try:
        _remap_module_code_ops_with_state(
            integration_state,
            module_name,
            ir_functions,
            local_id_to_symbol,
        )
    except ValueError as exc:
        return str(exc)
    for func in ir_functions:
        if func["name"] == "molt_main":
            func["name"] = init_symbol
    integration_state.functions.extend(ir_functions)
    for class_name in local_class_names:
        class_info = local_classes.get(class_name)
        if class_info is not None:
            integration_state.known_classes[class_name] = class_info
    return None


def _lower_entry_module_as_main(
    *,
    lowering_context: _EntryFrontendLoweringContext,
    integration_state: _FrontendIntegrationState,
    diagnostics_state: _MidendDiagnosticsState,
    record_frontend_timing: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
) -> _CliFailure | None:
    try:
        source = _read_module_source(lowering_context.entry_path)
    except (SyntaxError, UnicodeDecodeError) as exc:
        return fail(
            f"Syntax error in {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )
    except OSError as exc:
        return fail(
            f"Failed to read module {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )
    try:
        tree = _parse_source_for_target(
            source,
            filename=str(lowering_context.entry_path),
            target_python=lowering_context.target_python,
        )
    except SyntaxError as exc:
        return fail(
            f"Syntax error in {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )

    main_gen = SimpleTIRGenerator(
        parse_codec=lowering_context.parse_codec,
        type_hint_policy=lowering_context.type_hint_policy,
        fallback_policy=lowering_context.fallback_policy,
        source_path=str(lowering_context.entry_path),
        type_facts=lowering_context.type_facts,
        type_facts_module=lowering_context.entry_module,
        module_name="__main__",
        module_spec_name=lowering_context.entry_module,
        entry_module=None,
        enable_phi=lowering_context.enable_phi,
        known_modules=set(lowering_context.known_modules),
        known_classes=cast(Any, lowering_context.known_classes),
        stdlib_allowlist=set(lowering_context.stdlib_allowlist),
        known_func_defaults=lowering_context.known_func_defaults,
        known_func_kinds=lowering_context.known_func_kinds,
        module_chunking=lowering_context.module_chunking,
        module_chunk_max_ops=lowering_context.module_chunk_max_ops,
        optimization_profile=cast(BuildProfile, lowering_context.optimization_profile),
        pgo_hot_functions=set(lowering_context.pgo_hot_function_names),
    )
    main_frontend_start = time.perf_counter()
    main_visit_s = 0.0
    main_lower_s = 0.0
    try:
        main_visit_start = time.perf_counter()
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name="frontend visit (__main__)",
        ):
            main_gen.visit(tree)
        main_visit_s = time.perf_counter() - main_visit_start
        main_lower_start = time.perf_counter()
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name="frontend IR lowering (__main__)",
        ):
            main_ir = main_gen.to_json()
        main_lower_s = time.perf_counter() - main_lower_start
    except TimeoutError as exc:
        record_frontend_timing(
            module_name="__main__",
            module_path=lowering_context.entry_path,
            visit_s=main_visit_s,
            lower_s=main_lower_s,
            total_s=time.perf_counter() - main_frontend_start,
            timed_out=True,
            detail=str(exc),
        )
        return fail(str(exc), json_output, command="build")
    except CompatibilityError as exc:
        return fail(str(exc), json_output, command="build")

    record_frontend_timing(
        module_name="__main__",
        module_path=lowering_context.entry_path,
        visit_s=main_visit_s,
        lower_s=main_lower_s,
        total_s=time.perf_counter() - main_frontend_start,
    )
    main_init = SimpleTIRGenerator.module_init_symbol("__main__")
    local_code_ids = dict(main_gen.func_code_ids)
    if "molt_main" in local_code_ids:
        local_code_ids[main_init] = local_code_ids.pop("molt_main")
    local_id_to_symbol = {code_id: symbol for symbol, code_id in local_code_ids.items()}
    try:
        _remap_module_code_ops_with_state(
            integration_state,
            "__main__",
            main_ir["functions"],
            local_id_to_symbol,
        )
    except ValueError as exc:
        return fail(str(exc), json_output, command="build")
    for func in main_ir["functions"]:
        if func["name"] == "molt_main":
            func["name"] = main_init
    integration_state.functions.extend(main_ir["functions"])
    _accumulate_midend_diagnostics_with_state(
        diagnostics_state,
        "__main__",
        policy_outcomes_by_func=dict(main_gen.midend_policy_outcomes_by_function),
        pass_stats_by_func=dict(main_gen.midend_pass_stats_by_function),
    )
    return None


def _prepare_frontend_execution(
    *,
    syntax_error_modules: dict[str, "ModuleSyntaxErrorInfo"],
    module_graph: Mapping[str, Path],
    module_source_catalog: _ModuleSourceCatalog,
    project_root: Path,
    module_resolution_cache: "_ModuleResolutionCache",
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    module_deps: dict[str, set[str]],
    module_chunk_max_ops: int,
    optimization_profile: BuildProfile,
    pgo_hot_function_names: set[str],
    known_modules_sorted: tuple[str, ...],
    stdlib_allowlist_sorted: tuple[str, ...],
    pgo_hot_function_names_sorted: tuple[str, ...],
    module_dep_closures: dict[str, frozenset[str]],
    module_graph_metadata: _ModuleGraphMetadata,
    module_path_stats: dict[str, os.stat_result | None],
    module_chunking: bool,
    scoped_lowering_inputs: _ScopedLoweringInputs,
    dirty_lowering_modules: set[str],
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
    known_classes: dict[str, Any],
    module_trees: dict[str, ast.AST],
    generated_module_source_paths: Mapping[str, str],
    frontend_phase_timeout: float | None,
    record_frontend_timing: Callable[..., None],
    fail: Callable[..., int],
    json_output: bool,
    warnings: list[str],
    frontend_parallel_details: dict[str, Any],
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    midend_policy_outcomes_by_function: dict[str, dict[str, Any]],
    midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]],
    target_python: TargetPythonVersion,
) -> tuple[
    _FrontendLayerExecutionContext,
    _FrontendLayerRuntimeHooks,
    _FrontendIntegrationState,
    _MidendDiagnosticsState,
]:
    frontend_layer_execution_context = _FrontendLayerExecutionContext(
        syntax_error_modules=syntax_error_modules,
        module_graph=module_graph,
        module_source_catalog=module_source_catalog,
        project_root=project_root,
        module_resolution_cache=module_resolution_cache,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        module_deps=module_deps,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_function_names=pgo_hot_function_names,
        known_modules_sorted=known_modules_sorted,
        stdlib_allowlist_sorted=stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        module_dep_closures=module_dep_closures,
        module_graph_metadata=module_graph_metadata,
        path_stat_by_module=module_path_stats,
        module_chunking=module_chunking,
        scoped_lowering_inputs=scoped_lowering_inputs,
        dirty_lowering_modules=dirty_lowering_modules,
        frontend_module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
        known_classes=known_classes,
        target_python=target_python,
    )
    serial_frontend_lowering_context = _SerialFrontendLoweringContext(
        syntax_error_modules=syntax_error_modules,
        module_trees=module_trees,
        module_source_catalog=module_source_catalog,
        generated_module_source_paths=generated_module_source_paths,
        module_resolution_cache=module_resolution_cache,
        project_root=project_root,
        dirty_lowering_modules=dirty_lowering_modules,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
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
        module_graph_metadata=module_graph_metadata,
        module_path_stats=module_path_stats,
        known_classes=known_classes,
        frontend_phase_timeout=frontend_phase_timeout,
        target_python=target_python,
    )
    serial_frontend_lowering_hooks = _SerialFrontendLoweringHooks(
        record_frontend_timing=record_frontend_timing,
        fail=fail,
        json_output=json_output,
    )
    integration_state = _FrontendIntegrationState(
        functions=[],
        known_classes=known_classes,
    )
    midend_diagnostics_state = _MidendDiagnosticsState(
        policy_outcomes_by_function=midend_policy_outcomes_by_function,
        pass_stats_by_function=midend_pass_stats_by_function,
    )

    def _run_serial_frontend_lower(
        module_name: str,
        module_path: Path,
    ) -> tuple[
        dict[str, Any] | None,
        _FrontendModuleResultTimings | None,
        _CliFailure | None,
    ]:
        return _run_serial_frontend_lower_with_context(
            module_name,
            module_path,
            lowering_context=serial_frontend_lowering_context,
            lowering_hooks=serial_frontend_lowering_hooks,
        )

    frontend_layer_runtime_hooks = _FrontendLayerRuntimeHooks(
        warnings=warnings,
        frontend_parallel_details=frontend_parallel_details,
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        record_frontend_timing=record_frontend_timing,
        integrate_module_frontend_result=functools.partial(
            _integrate_module_frontend_result_with_state,
            integration_state,
        ),
        accumulate_midend_diagnostics=functools.partial(
            _accumulate_midend_diagnostics_with_state,
            midend_diagnostics_state,
        ),
        fail=fail,
        json_output=json_output,
        run_serial_frontend_lower=_run_serial_frontend_lower,
    )
    return (
        frontend_layer_execution_context,
        frontend_layer_runtime_hooks,
        integration_state,
        midend_diagnostics_state,
    )


def _run_frontend_parallel_enabled_layers(
    module_layers: Sequence[Sequence[str]],
    *,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_config: _FrontendParallelConfig,
    frontend_parallel_layers: list[dict[str, Any]],
) -> _CliFailure | None:
    parallel_pool_usable = True
    with ProcessPoolExecutor(max_workers=frontend_parallel_config.workers) as executor:
        for layer_index, layer in enumerate(module_layers):
            layer_started_ns = time.time_ns()
            layer_run_result, layer_error = _run_frontend_layer(
                layer,
                layer_index=layer_index,
                executor=executor,
                execution_context=execution_context,
                runtime_hooks=runtime_hooks,
                frontend_parallel_config=frontend_parallel_config,
                parallel_pool_usable=parallel_pool_usable,
            )
            if layer_error is not None:
                return layer_error
            assert layer_run_result is not None
            layer_state = layer_run_result.layer_state
            layer_plan = layer_run_result.layer_plan
            parallel_pool_usable = layer_run_result.parallel_pool_usable
            _frontend_parallel._append_frontend_parallel_layer_detail(
                frontend_parallel_layers,
                layer_index=layer_index,
                layer_mode=layer_plan.mode,
                layer_policy_reason=layer_plan.policy_reason,
                module_names=layer,
                candidate_count=len(layer_plan.candidates),
                workers=layer_plan.workers,
                timing_items=layer_state.recorded_worker_timings,
                predicted_cost_total=layer_plan.predicted_cost_total,
                effective_min_predicted_cost=layer_plan.effective_min_predicted_cost,
                stdlib_candidates=layer_plan.stdlib_candidates,
                target_cost_per_worker=frontend_parallel_config.target_cost_per_worker,
                started_ns=layer_started_ns,
                finished_ns=time.time_ns(),
                fallback_reason=layer_state.fallback_reason,
            )
    return None


def _run_frontend_pipeline(
    *,
    prepared_frontend_run_ticket: _PreparedFrontendRunTicket,
) -> _CliFailure | None:
    frontend_parallel_config = prepared_frontend_run_ticket.frontend_parallel_config
    frontend_parallel_layers = prepared_frontend_run_ticket.frontend_parallel_layers
    frontend_layer_execution_context = (
        prepared_frontend_run_ticket.frontend_layer_execution_context
    )
    frontend_layer_runtime_hooks = (
        prepared_frontend_run_ticket.frontend_layer_runtime_hooks
    )
    if frontend_parallel_config.enabled:
        frontend_layer_error = _run_frontend_parallel_enabled_layers(
            prepared_frontend_run_ticket.module_layers,
            execution_context=frontend_layer_execution_context,
            runtime_hooks=frontend_layer_runtime_hooks,
            frontend_parallel_config=frontend_parallel_config,
            frontend_parallel_layers=frontend_parallel_layers,
        )
    else:
        frontend_layer_error = _run_frontend_serial_disabled_layers(
            prepared_frontend_run_ticket.module_order,
            execution_context=frontend_layer_execution_context,
            runtime_hooks=frontend_layer_runtime_hooks,
            frontend_parallel_layers=frontend_parallel_layers,
            frontend_parallel_config=frontend_parallel_config,
        )
    if frontend_layer_error is not None:
        return frontend_layer_error
    _frontend_parallel._summarize_frontend_parallel_worker_timings(
        prepared_frontend_run_ticket.frontend_parallel_details,
        prepared_frontend_run_ticket.frontend_parallel_worker_timings,
    )
    return None


def _run_frontend_serial_disabled_layers(
    module_order: Sequence[str],
    *,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_layers: list[dict[str, Any]],
    frontend_parallel_config: _FrontendParallelConfig,
) -> _CliFailure | None:
    serial_layer_started_ns = time.time_ns()
    serial_layer_state = _frontend_parallel._fresh_frontend_parallel_layer_state()
    serial_error = _run_frontend_serial_layer_modules(
        module_order,
        module_graph=execution_context.module_graph,
        run_serial_frontend_lower=runtime_hooks.run_serial_frontend_lower,
        record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
        integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
        accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
        fail=runtime_hooks.fail,
        json_output=runtime_hooks.json_output,
        layer_state=serial_layer_state,
        layer_index=0,
        serial_mode="serial_disabled",
    )
    if serial_error is not None:
        return serial_error
    _frontend_parallel._append_frontend_serial_disabled_layer_detail(
        frontend_parallel_layers,
        module_order=module_order,
        serial_layer_state=serial_layer_state,
        frontend_module_costs=execution_context.frontend_module_costs,
        stdlib_like_by_module=execution_context.stdlib_like_by_module,
        frontend_parallel_config=frontend_parallel_config,
        serial_layer_started_ns=serial_layer_started_ns,
    )
    return None


def _run_frontend_parallel_layer_batches(
    candidates: Sequence[str],
    *,
    layer_workers: int,
    executor: Any,
    known_classes_snapshot_source: Mapping[str, Any],
    module_graph: Mapping[str, Path],
    module_source_catalog: _ModuleSourceCatalog,
    project_root: Path | None,
    module_resolution_cache: _ModuleResolutionCache,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    module_deps: dict[str, set[str]],
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...],
    stdlib_allowlist_sorted: tuple[str, ...],
    pgo_hot_function_names_sorted: tuple[str, ...],
    module_dep_closures: dict[str, frozenset[str]],
    module_graph_metadata: _ModuleGraphMetadata,
    path_stat_by_module: Mapping[str, os.stat_result | None] | None,
    module_chunking: bool,
    scoped_lowering_inputs: _ScopedLoweringInputs | None,
    dirty_lowering_modules: Collection[str],
    target_python: TargetPythonVersion,
) -> tuple[_FrontendParallelLayerState, str | None, str | None]:
    layer_state = _frontend_parallel._fresh_frontend_parallel_layer_state()
    known_classes_snapshot = _known_classes_snapshot_copy(known_classes_snapshot_source)
    scoped_known_classes_by_module = _build_scoped_known_classes_snapshot(
        candidates,
        module_deps=module_deps,
        module_dep_closures=module_dep_closures,
        known_classes_snapshot=known_classes_snapshot,
    )
    for batch_start in range(0, len(candidates), layer_workers):
        batch = list(candidates[batch_start : batch_start + layer_workers])
        worker_submissions: list[_ParallelWorkerSubmission] = []
        (
            cached_results,
            worker_payloads,
            context_digest_by_module,
            batch_error,
        ) = _prepare_frontend_parallel_batch(
            batch,
            module_graph=module_graph,
            module_source_catalog=module_source_catalog,
            project_root=project_root,
            known_classes_snapshot=known_classes_snapshot,
            module_resolution_cache=module_resolution_cache,
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            type_facts=type_facts,
            enable_phi=enable_phi,
            known_modules=known_modules,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
            known_func_kinds=known_func_kinds,
            module_deps=module_deps,
            module_chunk_max_ops=module_chunk_max_ops,
            optimization_profile=optimization_profile,
            pgo_hot_function_names=pgo_hot_function_names,
            known_modules_sorted=known_modules_sorted,
            stdlib_allowlist_sorted=stdlib_allowlist_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            module_dep_closures=module_dep_closures,
            module_graph_metadata=module_graph_metadata,
            path_stat_by_module=path_stat_by_module,
            module_chunking=module_chunking,
            scoped_lowering_inputs=scoped_lowering_inputs,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
            dirty_lowering_modules=dirty_lowering_modules,
            target_python=target_python,
        )
        if batch_error is not None:
            return layer_state, batch_error, None
        layer_state.context_digests.update(context_digest_by_module)
        for module_name, cached_result in cached_results.items():
            _frontend_parallel._record_parallel_cached_module_result(
                layer_state,
                module_name,
                cached_result,
            )
        for module_name, payload in worker_payloads:
            worker_submissions.append(
                _ParallelWorkerSubmission(
                    module_name=module_name,
                    submitted_ns=time.time_ns(),
                    future=executor.submit(_frontend_lower_module_worker, payload),
                )
            )
        for submission in worker_submissions:
            module_name = submission.module_name
            future = submission.future
            try:
                result = future.result()
                received_ns = time.time_ns()
                _frontend_parallel._record_parallel_worker_result(
                    layer_state,
                    module_name=module_name,
                    result=result,
                    submitted_ns=submission.submitted_ns,
                    received_ns=received_ns,
                )
            except Exception as exc:
                return layer_state, None, f"{module_graph[module_name]}: {exc}"
    return layer_state, None, None


def _write_parallel_persisted_module_lowering(
    *,
    project_root: Path | None,
    module_path: Path,
    module_name: str,
    worker_mode: str,
    context_digest: str | None,
    result: Mapping[str, Any],
    target_python: TargetPythonVersion,
) -> None:
    if (
        project_root is None
        or worker_mode == "parallel_cache_hit"
        or context_digest is None
    ):
        return
    with contextlib.suppress(OSError):
        _write_persisted_module_lowering(
            project_root,
            module_path,
            module_name=module_name,
            is_package=module_path.name == "__init__.py",
            context_digest=context_digest,
            result={key: value for key, value in result.items() if key != "ok"},
            target_python=target_python,
        )


def _consume_frontend_module_result(
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    *,
    result_timings: _FrontendModuleResultTimings | None = None,
    record_frontend_timing: Callable[..., None] | None,
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
) -> _CliFailure | None:
    timings = result_timings or _frontend_parallel._frontend_result_timings(result)
    if record_frontend_timing is not None:
        record_frontend_timing(
            module_name=module_name,
            module_path=module_path,
            visit_s=timings.visit_s,
            lower_s=timings.lower_s,
            total_s=timings.total_s,
        )
    integration_error = integrate_module_frontend_result(
        module_name,
        ir_functions=cast(list[dict[str, Any]], result["functions"]),
        func_code_ids=cast(dict[str, int], result["func_code_ids"]),
        local_class_names=cast(list[str], result["local_class_names"]),
        local_classes=cast(dict[str, Any], result["local_classes"]),
    )
    if integration_error is not None:
        return fail(integration_error, json_output, command="build")
    accumulate_midend_diagnostics(
        module_name,
        policy_outcomes_by_func=cast(
            dict[str, dict[str, Any]],
            result.get("midend_policy_outcomes_by_function", {}),
        ),
        pass_stats_by_func=cast(
            dict[str, dict[str, dict[str, Any]]],
            result.get("midend_pass_stats_by_function", {}),
        ),
    )
    return None


def _consume_frontend_parallel_layer_result(
    *,
    layer_state: _FrontendParallelLayerState,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    record_frontend_timing: Callable[..., None],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
    project_root: Path | None,
    layer_index: int,
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    target_python: TargetPythonVersion,
) -> _CliFailure | None:
    result_error = _frontend_parallel._frontend_parallel_result_error(module_name, result)
    if result_error is not None:
        return fail(result_error, json_output, command="build")
    result_timings = _frontend_parallel._frontend_result_timings(result)
    worker_mode = _frontend_parallel._record_parallel_layer_module_timing(
        layer_state=layer_state,
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        layer_index=layer_index,
        module_name=module_name,
        module_path=module_path,
        result_timings=result_timings,
        worker_timing=layer_state.worker_timings_by_module.get(module_name),
    )
    _write_parallel_persisted_module_lowering(
        project_root=project_root,
        module_path=module_path,
        module_name=module_name,
        worker_mode=worker_mode,
        context_digest=layer_state.context_digests.get(module_name),
        result=result,
        target_python=target_python,
    )
    return _consume_frontend_module_result(
        module_name=module_name,
        module_path=module_path,
        result=result,
        result_timings=result_timings,
        record_frontend_timing=record_frontend_timing,
        integrate_module_frontend_result=integrate_module_frontend_result,
        accumulate_midend_diagnostics=accumulate_midend_diagnostics,
        fail=fail,
        json_output=json_output,
    )


def _consume_frontend_serial_layer_result(
    *,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
    layer_state: _FrontendParallelLayerState,
    layer_index: int,
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    result_timings: _FrontendModuleResultTimings,
    serial_mode: str,
) -> _CliFailure | None:
    _frontend_parallel._record_serial_frontend_worker_timing(
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        recorded_worker_timings=layer_state.recorded_worker_timings,
        layer_index=layer_index,
        module_name=module_name,
        module_path=module_path,
        mode=serial_mode,
        total_s=result_timings.total_s,
    )
    return _consume_frontend_module_result(
        module_name=module_name,
        module_path=module_path,
        result=result,
        result_timings=result_timings,
        record_frontend_timing=None,
        integrate_module_frontend_result=integrate_module_frontend_result,
        accumulate_midend_diagnostics=accumulate_midend_diagnostics,
        fail=fail,
        json_output=json_output,
    )


def _run_frontend_serial_layer_modules(
    module_names: Sequence[str],
    *,
    module_graph: Mapping[str, Path],
    run_serial_frontend_lower: Callable[
        [str, Path],
        tuple[
            dict[str, Any] | None,
            _FrontendModuleResultTimings | None,
            _CliFailure | None,
        ],
    ],
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[..., _CliFailure],
    json_output: bool,
    layer_state: _FrontendParallelLayerState,
    layer_index: int,
    serial_mode: str,
) -> _CliFailure | None:
    for module_name in module_names:
        module_path = module_graph[module_name]
        result, result_timings, lower_error = run_serial_frontend_lower(
            module_name,
            module_path,
        )
        if lower_error is not None:
            return lower_error
        assert result is not None
        assert result_timings is not None
        consume_error = _consume_frontend_serial_layer_result(
            record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
            integrate_module_frontend_result=integrate_module_frontend_result,
            accumulate_midend_diagnostics=accumulate_midend_diagnostics,
            fail=fail,
            json_output=json_output,
            layer_state=layer_state,
            layer_index=layer_index,
            module_name=module_name,
            module_path=module_path,
            result=result,
            result_timings=result_timings,
            serial_mode=serial_mode,
        )
        if consume_error is not None:
            return consume_error
    return None


def _run_frontend_layer(
    layer: Sequence[str],
    *,
    layer_index: int,
    executor: Any | None,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_config: _FrontendParallelConfig,
    parallel_pool_usable: bool,
) -> tuple[_FrontendLayerRunResult | None, _CliFailure | None]:
    layer_state = _frontend_parallel._fresh_frontend_parallel_layer_state()
    layer_plan = _frontend_parallel._frontend_layer_plan(
        layer,
        syntax_error_modules=execution_context.syntax_error_modules,
        module_source_catalog=execution_context.module_source_catalog,
        module_graph=execution_context.module_graph,
        module_deps=execution_context.module_deps,
        frontend_module_costs=execution_context.frontend_module_costs,
        stdlib_like_by_module=execution_context.stdlib_like_by_module,
        frontend_parallel_config=frontend_parallel_config,
        parallel_pool_usable=parallel_pool_usable,
    )
    if layer_plan.mode == "parallel":
        assert executor is not None
        layer_state, batch_error, layer_failure_detail = (
            _run_frontend_parallel_layer_batches(
                layer_plan.candidates,
                layer_workers=layer_plan.workers,
                executor=executor,
                known_classes_snapshot_source=execution_context.known_classes,
                module_graph=execution_context.module_graph,
                module_source_catalog=execution_context.module_source_catalog,
                project_root=execution_context.project_root,
                module_resolution_cache=execution_context.module_resolution_cache,
                parse_codec=execution_context.parse_codec,
                type_hint_policy=execution_context.type_hint_policy,
                fallback_policy=execution_context.fallback_policy,
                type_facts=execution_context.type_facts,
                enable_phi=execution_context.enable_phi,
                known_modules=execution_context.known_modules,
                stdlib_allowlist=execution_context.stdlib_allowlist,
                known_func_defaults=execution_context.known_func_defaults,
                known_func_kinds=execution_context.known_func_kinds,
                module_deps=execution_context.module_deps,
                module_chunk_max_ops=execution_context.module_chunk_max_ops,
                optimization_profile=execution_context.optimization_profile,
                pgo_hot_function_names=execution_context.pgo_hot_function_names,
                known_modules_sorted=execution_context.known_modules_sorted,
                stdlib_allowlist_sorted=execution_context.stdlib_allowlist_sorted,
                pgo_hot_function_names_sorted=execution_context.pgo_hot_function_names_sorted,
                module_dep_closures=execution_context.module_dep_closures,
                module_graph_metadata=execution_context.module_graph_metadata,
                path_stat_by_module=execution_context.path_stat_by_module,
                module_chunking=execution_context.module_chunking,
                scoped_lowering_inputs=execution_context.scoped_lowering_inputs,
                dirty_lowering_modules=execution_context.dirty_lowering_modules,
                target_python=execution_context.target_python,
            )
        )
        if batch_error is not None:
            return None, runtime_hooks.fail(
                batch_error, runtime_hooks.json_output, command="build"
            )
        if layer_failure_detail is not None:
            layer_state = _frontend_parallel._fallback_frontend_parallel_layer_to_serial(
                frontend_parallel_details=runtime_hooks.frontend_parallel_details,
                warnings=runtime_hooks.warnings,
                failure_detail=layer_failure_detail,
            )
            layer_plan = _FrontendLayerPlan(
                candidates=layer_plan.candidates,
                predicted_cost_total=layer_plan.predicted_cost_total,
                effective_min_predicted_cost=layer_plan.effective_min_predicted_cost,
                stdlib_candidates=layer_plan.stdlib_candidates,
                workers=1,
                policy_reason="worker_error_fallback_serial",
                mode="serial_fallback",
            )
            parallel_pool_usable = False

    for module_name in layer:
        module_path = execution_context.module_graph[module_name]
        result = _frontend_parallel._take_frontend_parallel_layer_result(layer_state, module_name)
        if result is not None:
            consume_error = _consume_frontend_parallel_layer_result(
                layer_state=layer_state,
                record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
                record_frontend_timing=runtime_hooks.record_frontend_timing,
                integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
                accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
                fail=runtime_hooks.fail,
                json_output=runtime_hooks.json_output,
                project_root=execution_context.project_root,
                layer_index=layer_index,
                module_name=module_name,
                module_path=module_path,
                result=result,
                target_python=execution_context.target_python,
            )
            if consume_error is not None:
                return None, consume_error
            continue
        serial_error = _run_frontend_serial_layer_modules(
            [module_name],
            module_graph=execution_context.module_graph,
            run_serial_frontend_lower=runtime_hooks.run_serial_frontend_lower,
            record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
            integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
            accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
            fail=runtime_hooks.fail,
            json_output=runtime_hooks.json_output,
            layer_state=layer_state,
            layer_index=layer_index,
            serial_mode=_frontend_parallel._frontend_serial_worker_mode(layer_plan.mode),
        )
        if serial_error is not None:
            return None, serial_error
    return (
        _FrontendLayerRunResult(
            layer_state=layer_state,
            layer_plan=layer_plan,
            parallel_pool_usable=parallel_pool_usable,
        ),
        None,
    )


def _prepare_frontend_parallel_batch(
    batch: list[str],
    *,
    module_graph: Mapping[str, Path],
    module_sources: dict[str, str] | None = None,
    module_source_catalog: _ModuleSourceCatalog | None = None,
    project_root: Path | None,
    known_classes_snapshot: dict[str, Any],
    module_resolution_cache: _ModuleResolutionCache,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: TypeFacts | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    module_deps: dict[str, set[str]],
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...],
    stdlib_allowlist_sorted: tuple[str, ...],
    pgo_hot_function_names_sorted: tuple[str, ...],
    module_dep_closures: dict[str, frozenset[str]],
    module_graph_metadata: _ModuleGraphMetadata,
    path_stat_by_module: Mapping[str, os.stat_result | None] | None = None,
    module_chunking: bool,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
    dirty_lowering_modules: Collection[str],
    target_python: TargetPythonVersion,
) -> tuple[
    dict[str, dict[str, Any]],
    list[tuple[str, dict[str, Any]]],
    dict[str, str],
    str | None,
]:
    cached_results: dict[str, dict[str, Any]] = {}
    worker_payloads: list[tuple[str, dict[str, Any]]] = []
    context_digest_by_module: dict[str, str] = {}
    dirty_lowering = set(dirty_lowering_modules)
    stdlib_allowlist_payload = list(stdlib_allowlist_sorted)
    if module_source_catalog is None:
        module_source_catalog = _build_module_source_catalog(
            module_graph,
            module_sources=module_sources,
            path_stats=path_stat_by_module,
        )
    if scoped_known_classes_by_module is None:
        scoped_known_classes_by_module = _build_scoped_known_classes_snapshot(
            batch,
            module_deps=module_deps,
            module_dep_closures=module_dep_closures,
            known_classes_snapshot=known_classes_snapshot,
        )
    for module_name in batch:
        module_path = module_graph[module_name]
        execution_view = _module_lowering_execution_view(
            module_name,
            module_path=module_path,
            module_graph_metadata=module_graph_metadata,
            module_deps=module_deps,
            known_modules=known_modules,
            known_func_defaults=known_func_defaults,
            known_func_kinds=known_func_kinds,
            pgo_hot_function_names=pgo_hot_function_names,
            type_facts=type_facts,
            known_classes_snapshot=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
            path_stat_by_module=path_stat_by_module,
            scoped_lowering_inputs=scoped_lowering_inputs,
            known_modules_sorted=known_modules_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
        )
        metadata_view = execution_view.metadata
        scoped_inputs = execution_view.scoped_inputs
        logical_source_path = metadata_view.logical_source_path
        entry_override = metadata_view.entry_override
        module_is_namespace = metadata_view.module_is_namespace
        is_package = metadata_view.is_package
        path_stat = metadata_view.path_stat
        scoped_known_classes = execution_view.scoped_known_classes
        if project_root is not None:
            context_digest = _module_lowering_context_digest_for_module(
                module_name,
                module_path,
                logical_source_path=logical_source_path,
                entry_override=entry_override,
                known_classes_snapshot=known_classes_snapshot,
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                type_facts=type_facts,
                enable_phi=enable_phi,
                known_modules=known_modules,
                stdlib_allowlist=stdlib_allowlist,
                known_func_defaults=known_func_defaults,
                known_func_kinds=known_func_kinds,
                module_deps=module_deps,
                module_is_namespace=module_is_namespace,
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
                scoped_known_classes_by_module=scoped_known_classes_by_module,
                scoped_known_classes=scoped_known_classes,
                is_package=is_package,
                path_stat=path_stat,
                target_python=target_python,
            )
            if context_digest is not None:
                context_digest_by_module[module_name] = context_digest
        if module_name not in dirty_lowering:
            cached_result = _load_cached_module_lowering_result(
                project_root,
                module_name,
                module_path,
                logical_source_path=logical_source_path,
                entry_override=entry_override,
                is_package=is_package,
                known_classes_snapshot=known_classes_snapshot,
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                type_facts=type_facts,
                enable_phi=enable_phi,
                known_modules=known_modules,
                stdlib_allowlist=stdlib_allowlist,
                known_func_defaults=known_func_defaults,
                known_func_kinds=known_func_kinds,
                module_deps=module_deps,
                module_is_namespace=module_is_namespace,
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
                scoped_known_classes_by_module=scoped_known_classes_by_module,
                scoped_known_classes=scoped_known_classes,
                context_digest=context_digest_by_module.get(module_name),
                resolution_cache=module_resolution_cache,
                path_stat=path_stat,
                target_python=target_python,
            )
            if cached_result is not None:
                cached_results[module_name] = cached_result
                continue
        source_lease = module_source_catalog.lease_for(module_name, module_path)
        worker_payloads.append(
            (
                module_name,
                _module_worker_payload(
                    module_name,
                    module_path=module_path,
                    logical_source_path=logical_source_path,
                    source_lease=source_lease,
                    parse_codec=parse_codec,
                    type_hint_policy=type_hint_policy,
                    fallback_policy=fallback_policy,
                    module_is_namespace=module_is_namespace,
                    entry_module=entry_override,
                    type_facts=type_facts,
                    enable_phi=enable_phi,
                    known_modules=known_modules_sorted,
                    known_classes_snapshot=known_classes_snapshot,
                    stdlib_allowlist_sorted=stdlib_allowlist_sorted,
                    stdlib_allowlist_payload=stdlib_allowlist_payload,
                    known_func_defaults=known_func_defaults,
                    known_func_kinds=known_func_kinds,
                    module_deps=module_deps,
                    module_chunking=module_chunking,
                    module_chunk_max_ops=module_chunk_max_ops,
                    optimization_profile=optimization_profile,
                    pgo_hot_function_names=pgo_hot_function_names_sorted,
                    module_dep_closures=module_dep_closures,
                    scoped_lowering_inputs=scoped_lowering_inputs,
                    scoped_inputs=scoped_inputs,
                    scoped_known_classes_by_module=scoped_known_classes_by_module,
                    scoped_known_classes=scoped_known_classes,
                    target_python=target_python,
                ),
            )
        )
    return cached_results, worker_payloads, context_digest_by_module, None


@contextmanager
def _phase_timeout(timeout_s: float | None, *, phase_name: str):
    if timeout_s is None:
        yield
        return
    if os.name != "posix" or threading.current_thread() is not threading.main_thread():
        yield
        return
    if not hasattr(signal, "setitimer") or not hasattr(signal, "ITIMER_REAL"):
        yield
        return
    previous_handler = signal.getsignal(signal.SIGALRM)
    previous_timer = signal.getitimer(signal.ITIMER_REAL)

    def _timeout_handler(_signum: int, _frame: Any) -> None:
        raise TimeoutError(
            f"{phase_name} timed out after {timeout_s:.1f}s "
            "(MOLT_FRONTEND_PHASE_TIMEOUT)"
        )

    signal.signal(signal.SIGALRM, _timeout_handler)
    signal.setitimer(signal.ITIMER_REAL, timeout_s)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0.0, 0.0)
        signal.signal(signal.SIGALRM, previous_handler)
        if previous_timer[0] > 0 or previous_timer[1] > 0:
            signal.setitimer(signal.ITIMER_REAL, previous_timer[0], previous_timer[1])

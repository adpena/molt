from __future__ import annotations

import ast
import contextlib
import os
import signal
import sys
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Collection, Mapping, cast

from molt.compat import CompatibilityError
from molt.frontend import SimpleTIRGenerator
from molt.type_facts import TypeFacts

from molt.cli.models import (
    BuildProfile,
    FallbackPolicy,
    ParseCodec,
    TypeHintPolicy,
    _FrontendModuleResultTimings,
    _ModuleGraphMetadata,
    _ModuleLowerError,
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
    _module_lowering_local_reference_issue,
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
    direct_call_modules = set(cast(list[str], payload["direct_call_modules"]))
    known_classes = cast(dict[str, Any], payload["known_classes"])
    stdlib_allowlist = set(cast(list[str], payload["stdlib_allowlist"]))
    known_func_defaults = cast(
        dict[str, dict[str, dict[str, Any]]], payload["known_func_defaults"]
    )
    known_func_kinds = cast(dict[str, dict[str, str]], payload["known_func_kinds"])
    native_callable_exports = cast(
        dict[str, dict[str, Any]], payload.get("native_callable_exports", {})
    )
    native_python_exports = set(
        cast(list[str], payload.get("native_python_exports", []))
    )
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
        direct_call_modules=direct_call_modules,
        known_classes=known_classes,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        known_func_kinds=known_func_kinds,
        native_callable_exports=native_callable_exports,
        native_python_exports=native_python_exports,
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
    issue = _module_lowering_local_reference_issue(module_name, ir["functions"])
    if issue is not None:
        worker_finished_ns = time.time_ns()
        return {
            "ok": False,
            "error": f"Invalid lowered module {module_name}: {issue}",
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
        direct_call_modules=set(scoped_inputs.direct_call_modules_set),
        known_classes=scoped_known_classes,
        stdlib_allowlist=set(stdlib_allowlist),
        known_func_defaults=scoped_inputs.known_func_defaults,
        known_func_kinds=scoped_inputs.known_func_kinds,
        native_callable_exports=scoped_inputs.native_callable_exports,
        native_python_exports=set(scoped_inputs.native_python_exports_set),
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=cast(BuildProfile, optimization_profile),
        pgo_hot_functions=set(scoped_inputs.pgo_hot_function_names_set),
    )


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
        direct_call_modules=lowering_context.direct_call_modules,
        known_func_defaults=lowering_context.known_func_defaults,
        known_func_kinds=lowering_context.known_func_kinds,
        native_callable_exports=lowering_context.native_callable_exports,
        native_python_exports=lowering_context.native_python_exports,
        pgo_hot_function_names=lowering_context.pgo_hot_function_names,
        type_facts=lowering_context.type_facts,
        known_classes_snapshot=lowering_context.known_classes,
        module_dep_closures=lowering_context.module_dep_closures,
        path_stat_by_module=lowering_context.module_path_stats,
        scoped_lowering_inputs=lowering_context.scoped_lowering_inputs,
        known_modules_sorted=lowering_context.known_modules_sorted,
        pgo_hot_function_names_sorted=lowering_context.pgo_hot_function_names_sorted,
        source_modules=lowering_context.source_modules,
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
            direct_call_modules=lowering_context.direct_call_modules,
            stdlib_allowlist=lowering_context.stdlib_allowlist,
            known_func_defaults=lowering_context.known_func_defaults,
            known_func_kinds=lowering_context.known_func_kinds,
            native_callable_exports=lowering_context.native_callable_exports,
            native_python_exports=lowering_context.native_python_exports,
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
            source_modules=lowering_context.source_modules,
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
    issue = _module_lowering_local_reference_issue(module_name, payload["functions"])
    if issue is not None:
        raise _ModuleLowerError(f"Invalid lowered module {module_name}: {issue}")
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
    direct_call_modules: Collection[str] | None = None,
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    known_func_kinds: dict[str, dict[str, str]],
    native_callable_exports: Mapping[str, Mapping[str, Any]] | None = None,
    native_python_exports: Collection[str] = (),
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
            direct_call_modules=direct_call_modules,
            known_func_defaults=known_func_defaults,
            known_func_kinds=known_func_kinds,
            native_callable_exports=native_callable_exports,
            native_python_exports=native_python_exports,
            pgo_hot_function_names=pgo_hot_function_names,
            type_facts=type_facts,
            known_classes_snapshot=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
            path_stat_by_module=path_stat_by_module,
            scoped_lowering_inputs=scoped_lowering_inputs,
            known_modules_sorted=known_modules_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
            source_modules=module_graph,
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
                direct_call_modules=direct_call_modules,
                stdlib_allowlist=stdlib_allowlist,
                known_func_defaults=known_func_defaults,
                known_func_kinds=known_func_kinds,
                native_callable_exports=native_callable_exports,
                native_python_exports=native_python_exports,
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
                source_modules=module_graph,
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
                direct_call_modules=direct_call_modules,
                stdlib_allowlist=stdlib_allowlist,
                known_func_defaults=known_func_defaults,
                known_func_kinds=known_func_kinds,
                native_callable_exports=native_callable_exports,
                native_python_exports=native_python_exports,
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
                source_modules=module_graph,
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
                    direct_call_modules=direct_call_modules,
                    known_classes_snapshot=known_classes_snapshot,
                    stdlib_allowlist_sorted=stdlib_allowlist_sorted,
                    stdlib_allowlist_payload=stdlib_allowlist_payload,
                    known_func_defaults=known_func_defaults,
                    known_func_kinds=known_func_kinds,
                    native_callable_exports=native_callable_exports,
                    native_python_exports=native_python_exports,
                    module_deps=module_deps,
                    module_chunking=module_chunking,
                    module_chunk_max_ops=module_chunk_max_ops,
                    optimization_profile=optimization_profile,
                    pgo_hot_function_names=pgo_hot_function_names_sorted,
                    module_dep_closures=module_dep_closures,
                    scoped_lowering_inputs=scoped_lowering_inputs,
                    scoped_inputs=scoped_inputs,
                    source_modules=module_graph,
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

from __future__ import annotations

import ast
import hashlib
import os
from collections import Counter
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from molt.compiler_analysis.hashing import stable_payload_hash as _stable_payload_hash


def _ast_metrics(tree: ast.AST | None) -> dict[str, int]:
    metrics = {
        "ast_nodes": 0,
        "function_defs": 0,
        "class_defs": 0,
        "import_statements": 0,
        "loops": 0,
        "branches": 0,
        "calls": 0,
    }
    if tree is None:
        return metrics
    for node in ast.walk(tree):
        metrics["ast_nodes"] += 1
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            metrics["function_defs"] += 1
        elif isinstance(node, ast.ClassDef):
            metrics["class_defs"] += 1
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            metrics["import_statements"] += 1
        elif isinstance(node, (ast.For, ast.AsyncFor, ast.While)):
            metrics["loops"] += 1
        elif isinstance(node, (ast.If, ast.IfExp, ast.Match)):
            metrics["branches"] += 1
        elif isinstance(node, ast.Call):
            metrics["calls"] += 1
    return metrics


def _sum_metrics(items: Sequence[dict[str, int]]) -> dict[str, int]:
    totals: Counter[str] = Counter()
    for item in items:
        totals.update(item)
    return {name: int(totals[name]) for name in sorted(totals)}


def _top_module_metric(
    module_metrics: Mapping[str, dict[str, int]],
    *,
    field: str,
    module_names: set[str],
    limit: int = 10,
) -> list[dict[str, Any]]:
    ranked = sorted(
        (
            {"module": name, field: metrics.get(field, 0)}
            for name, metrics in module_metrics.items()
            if name in module_names
        ),
        key=lambda item: (-int(item[field]), str(item["module"])),
    )
    return ranked[:limit]


def _dependency_edge_count(module_deps: Mapping[str, set[str]]) -> int:
    return sum(len(deps) for deps in module_deps.values())


def _source_text_for_module(
    *,
    import_plan: Any,
    frontend_analysis: Any,
    module_name: str,
) -> tuple[str | None, Path | None]:
    source = frontend_analysis.module_sources.get(module_name)
    module_path = import_plan.module_graph.get(module_name)
    if source is not None or module_path is None:
        return source, module_path
    try:
        source = frontend_analysis.module_source_catalog.read_source(
            module_name,
            module_path,
            import_plan.module_resolution_cache,
        )
    except (OSError, UnicodeDecodeError):
        source = None
    return source, module_path


def _module_source_and_ast_metrics(
    *,
    import_plan: Any,
    frontend_analysis: Any,
    module_names: set[str],
) -> tuple[dict[str, dict[str, int]], dict[str, int]]:
    module_metrics: dict[str, dict[str, int]] = {}
    source_bytes: dict[str, int] = {}
    for name in sorted(module_names):
        tree = frontend_analysis.module_trees.get(name)
        source, module_path = _source_text_for_module(
            import_plan=import_plan,
            frontend_analysis=frontend_analysis,
            module_name=name,
        )
        if tree is None and source is not None:
            try:
                tree = ast.parse(source, filename=os.fspath(module_path or f"<{name}>"))
            except SyntaxError:
                tree = None
        module_metrics[name] = _ast_metrics(tree)
        if source is not None:
            source_bytes[name] = len(source.encode("utf-8"))
        else:
            source_bytes[name] = frontend_analysis.module_source_catalog.source_size(
                name,
                module_path,
            )
    return module_metrics, source_bytes


def _source_bytes_for(source_bytes: Mapping[str, int], module_names: set[str]) -> int:
    return sum(source_bytes.get(name, 0) for name in module_names)


def _source_sha256(source: str) -> str:
    return hashlib.sha256(source.encode("utf-8")).hexdigest()


def _source_site_id(
    *,
    module_name: str,
    source_sha256: str,
    target_python: str,
    node_kind: str,
    qualname: str,
    ast_path: Sequence[int],
    span: Mapping[str, int],
) -> str:
    return _stable_payload_hash(
        {
            "schema_version": 1,
            "module": module_name,
            "source_sha256": source_sha256,
            "target_python": target_python,
            "node_kind": node_kind,
            "qualname": qualname,
            "ast_path": list(ast_path),
            "span": dict(span),
        }
    )


def _source_site_module_payload(
    *,
    module_name: str,
    logical_path: str,
    source: str,
    tree: ast.AST | None,
    target_python: str,
    roles: Sequence[str],
) -> dict[str, Any]:
    source_hash = _source_sha256(source)
    kind_counts: Counter[str] = Counter()
    site_ids: list[str] = []

    def visit(
        node: ast.AST, *, path: tuple[int, ...], qualname: tuple[str, ...]
    ) -> None:
        child_qualname = qualname
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            child_qualname = (*qualname, node.name)
        lineno = getattr(node, "lineno", None)
        col_offset = getattr(node, "col_offset", None)
        if isinstance(lineno, int) and isinstance(col_offset, int):
            node_kind = type(node).__name__
            end_lineno = getattr(node, "end_lineno", lineno)
            end_col_offset = getattr(node, "end_col_offset", col_offset)
            span = {
                "line": lineno,
                "col": col_offset,
                "end_line": end_lineno if isinstance(end_lineno, int) else lineno,
                "end_col": (
                    end_col_offset if isinstance(end_col_offset, int) else col_offset
                ),
            }
            kind_counts[node_kind] += 1
            site_ids.append(
                _source_site_id(
                    module_name=module_name,
                    source_sha256=source_hash,
                    target_python=target_python,
                    node_kind=node_kind,
                    qualname=(
                        ".".join(child_qualname) if child_qualname else "<module>"
                    ),
                    ast_path=path,
                    span=span,
                )
            )
        for index, child in enumerate(ast.iter_child_nodes(node)):
            visit(child, path=(*path, index), qualname=child_qualname)

    if tree is not None:
        visit(tree, path=(), qualname=())
    site_ids_sorted = sorted(site_ids)
    return {
        "module": module_name,
        "logical_path": logical_path,
        "source_sha256": source_hash,
        "roles": list(roles),
        "site_count": len(site_ids_sorted),
        "site_digest": _stable_payload_hash(site_ids_sorted),
        "node_kind_counts": {name: kind_counts[name] for name in sorted(kind_counts)},
    }


def _module_roles(import_plan: Any, module_name: str) -> list[str]:
    role_sets = (
        ("declared_root", import_plan.declared_root_modules),
        ("entry_reachable", import_plan.entry_reachable_modules),
        ("runtime_support", import_plan.runtime_support_modules),
        ("stdlib_support", import_plan.stdlib_support_modules),
        ("package_parent", import_plan.package_parent_modules),
        ("namespace", import_plan.namespace_module_names),
        ("compile", import_plan.compile_modules),
        ("known", import_plan.known_modules),
    )
    return [name for name, modules in role_sets if module_name in modules]


def _source_site_identity_payload(
    *,
    import_plan: Any,
    frontend_analysis: Any,
    target_python_tag: str,
) -> dict[str, Any]:
    logical_paths = import_plan.module_graph_metadata.logical_source_path_by_module
    modules: list[dict[str, Any]] = []
    for name in sorted(import_plan.known_modules):
        source, module_path = _source_text_for_module(
            import_plan=import_plan,
            frontend_analysis=frontend_analysis,
            module_name=name,
        )
        tree = frontend_analysis.module_trees.get(name)
        if tree is None and source is not None:
            try:
                tree = ast.parse(source, filename=os.fspath(module_path or f"<{name}>"))
            except SyntaxError:
                tree = None
        modules.append(
            _source_site_module_payload(
                module_name=name,
                logical_path=str(
                    logical_paths.get(name, import_plan.module_graph[name])
                ),
                source=source or "",
                tree=tree,
                target_python=target_python_tag,
                roles=_module_roles(import_plan, name),
            )
        )
    compile_modules = set(import_plan.compile_modules)
    compile_site_count = sum(
        int(module["site_count"])
        for module in modules
        if module["module"] in compile_modules
    )
    site_count = sum(int(module["site_count"]) for module in modules)
    module_digest_payload = [
        {
            "module": module["module"],
            "source_sha256": module["source_sha256"],
            "site_digest": module["site_digest"],
            "roles": module["roles"],
        }
        for module in modules
    ]
    return {
        "schema_version": 1,
        "semantic_identity_digest": _stable_payload_hash(
            {
                "schema_version": 1,
                "image": import_plan.image_scope.diagnostic_payload(),
                "compile_modules": sorted(import_plan.compile_modules),
                "modules": module_digest_payload,
            }
        ),
        "target_python": target_python_tag,
        "module_count": len(modules),
        "compile_module_count": len(compile_modules),
        "site_count": site_count,
        "compile_site_count": compile_site_count,
        "site_digest": _stable_payload_hash(
            [module["site_digest"] for module in modules]
        ),
        "modules": modules,
    }


def _frontend_binary_image_analysis_payload(
    *,
    import_plan: Any,
    frontend_analysis: Any,
    frontend_module_costs: Mapping[str, float],
    known_classes: Mapping[str, Any],
    enable_phi: bool,
    module_chunking: bool,
    module_chunk_max_ops: int,
    type_facts_present: bool,
    compile_module_order: Sequence[str],
    compile_module_layers: Sequence[Sequence[str]],
    target_python: Any,
) -> dict[str, Any]:
    known_modules = set(import_plan.known_modules)
    compile_modules = set(import_plan.compile_modules)
    target_python_tag = getattr(target_python, "tag", str(target_python))
    module_metrics, source_bytes = _module_source_and_ast_metrics(
        import_plan=import_plan,
        frontend_analysis=frontend_analysis,
        module_names=known_modules,
    )
    known_metric_totals = _sum_metrics(
        [metrics for name, metrics in module_metrics.items() if name in known_modules]
    )
    compile_metric_totals = _sum_metrics(
        [metrics for name, metrics in module_metrics.items() if name in compile_modules]
    )
    module_order = list(frontend_analysis.module_order)
    compile_order = list(compile_module_order)
    module_layers = [list(layer) for layer in frontend_analysis.module_layers]
    lowered_layers = [list(layer) for layer in compile_module_layers]
    frontend_cost_top = [
        {"module": name, "cost": round(float(cost), 6)}
        for name, cost in sorted(
            ((name, frontend_module_costs.get(name, 0.0)) for name in compile_modules),
            key=lambda item: (-float(item[1]), item[0]),
        )[:10]
    ]
    return {
        "schema_version": 1,
        "source_identity": _source_site_identity_payload(
            import_plan=import_plan,
            frontend_analysis=frontend_analysis,
            target_python_tag=target_python_tag,
        ),
        "source_ast": {
            "known_module_count": len(known_modules),
            "compile_module_count": len(compile_modules),
            "source_bytes_known": _source_bytes_for(source_bytes, known_modules),
            "source_bytes_compile": _source_bytes_for(source_bytes, compile_modules),
            "known": known_metric_totals,
            "compile": compile_metric_totals,
            "top_compile_ast_modules": _top_module_metric(
                module_metrics,
                field="ast_nodes",
                module_names=compile_modules,
            ),
        },
        "module_schedule": {
            "module_order_len": len(module_order),
            "compile_order_len": len(compile_order),
            "module_order_hash": _stable_payload_hash(module_order),
            "compile_order_hash": _stable_payload_hash(compile_order),
            "layer_count": len(module_layers),
            "compile_layer_count": len(lowered_layers),
            "max_layer_width": max((len(layer) for layer in module_layers), default=0),
            "max_compile_layer_width": max(
                (len(layer) for layer in lowered_layers), default=0
            ),
            "has_back_edges": bool(frontend_analysis.has_back_edges),
            "dependency_edge_count": _dependency_edge_count(
                frontend_analysis.module_deps
            ),
            "dependency_closure_edge_count": sum(
                len(deps) for deps in frontend_analysis.module_dep_closures.values()
            ),
            "dirty_lowering_module_count": len(
                frontend_analysis.dirty_lowering_modules
            ),
        },
        "lowering": {
            "target_python": target_python_tag,
            "enable_phi": enable_phi,
            "module_chunking": module_chunking,
            "module_chunk_max_ops": module_chunk_max_ops,
            "frontend_cost_total": round(
                sum(
                    float(frontend_module_costs.get(name, 0.0))
                    for name in compile_modules
                ),
                6,
            ),
            "frontend_cost_top": frontend_cost_top,
            "known_class_count": len(known_classes),
            "type_facts_present": type_facts_present,
            "compile_equals_known": compile_modules == known_modules,
        },
    }


def _artifact_file_payload(path: Path | None) -> dict[str, Any] | None:
    if path is None:
        return None
    payload: dict[str, Any] = {"path": os.fspath(path)}
    try:
        stat = path.stat()
    except OSError:
        payload["exists"] = False
        return payload
    payload.update(
        {
            "exists": True,
            "size_bytes": stat.st_size,
            "mtime_ns": stat.st_mtime_ns,
        }
    )
    return payload


def _native_artifact_binary_image_analysis_payload(
    *,
    output_binary: Path,
    output_obj: Path | None,
    runtime_lib: Path | None,
    stdlib_obj_path: Path | None,
    link_skipped: bool,
    link_fingerprint: Mapping[str, Any] | None,
    link_fingerprint_path: Path | None,
    external_native_artifact_count: int,
) -> dict[str, Any]:
    link_hash = None
    if isinstance(link_fingerprint, Mapping):
        raw_hash = link_fingerprint.get("hash")
        if isinstance(raw_hash, str):
            link_hash = raw_hash
    return {
        "schema_version": 1,
        "kind": "native",
        "output_binary": _artifact_file_payload(output_binary),
        "output_object": _artifact_file_payload(output_obj),
        "runtime_lib": _artifact_file_payload(runtime_lib),
        "stdlib_object": _artifact_file_payload(stdlib_obj_path),
        "link": {
            "skipped": link_skipped,
            "fingerprint_hash": link_hash,
            "fingerprint_path": os.fspath(link_fingerprint_path)
            if link_fingerprint_path is not None
            else None,
        },
        "external_native_artifact_count": external_native_artifact_count,
    }


def _non_native_artifact_binary_image_analysis_payload(
    *,
    kind: str,
    output: Path,
    consumer_output: Path | None,
    artifacts: Mapping[str, Any] | None,
) -> dict[str, Any]:
    artifact_payload: dict[str, Any] = {}
    for name, value in sorted((artifacts or {}).items()):
        if isinstance(value, Path):
            artifact_payload[name] = _artifact_file_payload(value)
        elif isinstance(value, str):
            artifact_payload[name] = _artifact_file_payload(Path(value))
        else:
            artifact_payload[name] = {"value": value}
    return {
        "schema_version": 1,
        "kind": kind,
        "output": _artifact_file_payload(output),
        "consumer_output": _artifact_file_payload(consumer_output),
        "artifacts": artifact_payload,
    }


def _merge_binary_image_analysis_stage(
    diagnostics_payload: dict[str, Any] | None,
    stage: str,
    payload: Mapping[str, Any] | None,
) -> None:
    if diagnostics_payload is None or payload is None:
        return
    analysis = diagnostics_payload.setdefault("binary_image_analysis", {})
    if not isinstance(analysis, dict):
        analysis = {}
        diagnostics_payload["binary_image_analysis"] = analysis
    analysis.setdefault("schema_version", 1)
    analysis[stage] = dict(payload)

#!/usr/bin/env python3
"""Fuse Molt compiler analysis artifacts into one validated capsule.

The capsule is an evidence spine, not a new compiler path. It consumes the
existing authorities:

* build diagnostics for source/module closure, frontend timings, allocation
  diagnostics, and TIR/midend pass telemetry
* TIR fact-graph dumps for value-level representation/call facts
* binary size analysis for section/symbol footprint
* output startup/size audit for cold/warm startup evidence

The result is a compact JSON document with cross-layer consistency checks. A
logical contradiction, such as compile modules outside the known binary-image
closure, is reported as a capsule error and makes the CLI fail closed.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import platform
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import binary_size_analysis, fact_graph_dump  # noqa: E402

SCHEMA_VERSION = 1
CAPSULE_KIND = "molt_analysis_capsule"


class CapsuleError(ValueError):
    """Raised when an input artifact cannot be consumed as a capsule source."""


def load_json(path: Path) -> dict[str, Any]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CapsuleError(f"{path}: cannot read JSON artifact: {exc}") from exc
    if not isinstance(raw, dict):
        raise CapsuleError(f"{path}: artifact must be a JSON object")
    return raw


def analyze_binary(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise CapsuleError(f"{path}: binary does not exist")
    fmt = binary_size_analysis.detect_format(path)
    if fmt == "wasm":
        return binary_size_analysis.to_json(binary_size_analysis.analyse_wasm(path))
    if fmt in {"macho", "elf"}:
        return binary_size_analysis.to_json(binary_size_analysis.analyse_native(path))
    raise CapsuleError(f"{path}: unrecognized binary format")


def build_capsule(
    *,
    build_diagnostics: Mapping[str, Any],
    build_diagnostics_path: str,
    startup_audit: Mapping[str, Any] | None = None,
    startup_audit_path: str | None = None,
    binary_size: Mapping[str, Any] | None = None,
    binary_size_path: str | None = None,
    tir_fact_graphs: Sequence[tuple[str, Mapping[str, Any]]] = (),
    label: str | None = None,
    recorded_at: str | None = None,
) -> dict[str, Any]:
    errors: list[str] = []
    warnings: list[str] = []
    frontend = _summarize_build_diagnostics(build_diagnostics, errors, warnings)
    compiler_binary_image_analysis = _summarize_compiler_binary_image_analysis(
        build_diagnostics,
        errors,
    )
    _check_compiler_analysis_against_closure(
        compiler_binary_image_analysis,
        frontend,
        errors,
    )
    ir_tir = _summarize_ir_tir(build_diagnostics, tir_fact_graphs, errors, warnings)
    allocation = _summarize_allocation(build_diagnostics)
    binary = _summarize_binary(binary_size, startup_audit)

    capsule = {
        "schema_version": SCHEMA_VERSION,
        "kind": CAPSULE_KIND,
        "recorded_at": recorded_at or _utc_now(),
        "label": label,
        "repo_root": str(REPO_ROOT),
        "system": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
        },
        "sources": {
            "build_diagnostics": build_diagnostics_path,
            "startup_audit": startup_audit_path,
            "binary_size": binary_size_path,
            "tir_fact_graphs": [path for path, _ in tir_fact_graphs],
        },
        "source_frontend": frontend,
        "compiler_binary_image_analysis": compiler_binary_image_analysis,
        "ir_tir": ir_tir,
        "allocation": allocation,
        "binary": binary,
        "analysis_tools": _analysis_tools_manifest(
            has_startup=startup_audit is not None,
            has_binary=binary_size is not None,
            fact_graph_count=len(tir_fact_graphs),
        ),
        "cross_checks": {
            "passed": not errors,
            "errors": errors,
            "warnings": warnings,
        },
    }
    return capsule


def _summarize_compiler_binary_image_analysis(
    diagnostics: Mapping[str, Any],
    errors: list[str],
) -> dict[str, Any]:
    raw = diagnostics.get("binary_image_analysis")
    if raw is None:
        return {"present": False}
    if not isinstance(raw, Mapping):
        errors.append("build_diagnostics.binary_image_analysis must be an object")
        return {"present": False}
    schema_version = raw.get("schema_version")
    if schema_version != 1:
        errors.append(
            "build_diagnostics.binary_image_analysis.schema_version must be 1"
        )
    stages: dict[str, Mapping[str, Any]] = {}
    for stage in ("frontend", "backend_ir", "artifacts"):
        payload = raw.get(stage)
        if payload is None:
            continue
        if not isinstance(payload, Mapping):
            errors.append(
                f"build_diagnostics.binary_image_analysis.{stage} must be an object"
            )
            continue
        stages[stage] = payload
    return {
        "present": True,
        "schema_version": schema_version,
        "stages": sorted(stages),
        "frontend": dict(stages.get("frontend", {})),
        "backend_ir": dict(stages.get("backend_ir", {})),
        "artifacts": dict(stages.get("artifacts", {})),
        "source_sites": _summarize_backend_source_sites(
            stages.get("backend_ir"),
            errors,
        ),
        "allocation_ownership": _summarize_backend_allocation_ownership(
            stages.get("backend_ir"),
            errors,
        ),
    }


def _summarize_backend_source_sites(
    backend_stage: Mapping[str, Any] | None,
    errors: list[str],
) -> dict[str, Any]:
    if backend_stage is None:
        return {"present": False}
    raw = backend_stage.get("source_sites")
    if not isinstance(raw, Mapping):
        errors.append(
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites "
            "must be an object"
        )
        return {"present": False}
    return {
        "present": True,
        "carrier": _string_or_none(raw.get("carrier")),
        "attributed_op_count": _int_or_none(raw.get("attributed_op_count")),
        "unattributed_op_count": _int_or_none(raw.get("unattributed_op_count")),
        "coverage_ratio": _number_or_none(raw.get("coverage_ratio")),
        "function_count_with_source": _int_or_none(
            raw.get("function_count_with_source")
        ),
        "line_count": _int_or_none(raw.get("line_count")),
        "explicit_source_line_count": _int_or_none(
            raw.get("explicit_source_line_count")
        ),
        "line_marker_fallback_count": _int_or_none(
            raw.get("line_marker_fallback_count")
        ),
        "source_site_digest": _string_or_none(raw.get("source_site_digest")),
        "top_source_lines_by_ops": _list_of_mappings(
            raw.get("top_source_lines_by_ops")
        )[:20],
    }


def _summarize_backend_allocation_ownership(
    backend_stage: Mapping[str, Any] | None,
    errors: list[str],
) -> dict[str, Any]:
    if backend_stage is None:
        return {"present": False}
    raw = backend_stage.get("allocation_ownership")
    if not isinstance(raw, Mapping):
        errors.append(
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership "
            "must be an object"
        )
        return {"present": False}
    return {
        "present": True,
        "carrier": _string_or_none(raw.get("carrier")),
        "event_count": _int_or_none(raw.get("event_count")),
        "source_attributed_event_count": _int_or_none(
            raw.get("source_attributed_event_count")
        ),
        "unattributed_event_count": _int_or_none(raw.get("unattributed_event_count")),
        "source_coverage_ratio": _number_or_none(raw.get("source_coverage_ratio")),
        "events_by_category": _int_mapping(raw.get("events_by_category")),
        "top_category_kinds": _list_of_mappings(raw.get("top_category_kinds"))[:20],
        "top_source_lines_by_events": _list_of_mappings(
            raw.get("top_source_lines_by_events")
        )[:20],
        "allocation_ownership_digest": _string_or_none(
            raw.get("allocation_ownership_digest")
        ),
    }


def _check_compiler_analysis_against_closure(
    compiler_analysis: Mapping[str, Any],
    frontend: Mapping[str, Any],
    errors: list[str],
) -> None:
    if not compiler_analysis.get("present"):
        return
    closure = _mapping_or_empty(frontend.get("closure"))
    raw_frontend = _mapping_or_empty(compiler_analysis.get("frontend"))
    source_ast = _mapping_or_empty(raw_frontend.get("source_ast"))
    _check_count(
        "binary_image_analysis.frontend.source_ast.known_module_count",
        source_ast.get("known_module_count"),
        int(closure.get("known_module_count", 0)),
        errors,
    )
    _check_count(
        "binary_image_analysis.frontend.source_ast.compile_module_count",
        source_ast.get("compile_module_count"),
        int(closure.get("compile_module_count", 0)),
        errors,
    )
    raw_backend = _mapping_or_empty(compiler_analysis.get("backend_ir"))
    backend_ir = _mapping_or_empty(raw_backend.get("backend_ir"))
    source_sites = _mapping_or_empty(compiler_analysis.get("source_sites"))
    if source_sites.get("present") is True:
        op_count = backend_ir.get("op_count")
        attributed = source_sites.get("attributed_op_count")
        unattributed = source_sites.get("unattributed_op_count")
        if (
            isinstance(op_count, int)
            and isinstance(attributed, int)
            and isinstance(unattributed, int)
        ):
            total = attributed + unattributed
            if total != op_count:
                errors.append(
                    "binary_image_analysis.backend_ir.source_sites coverage "
                    f"does not sum to backend_ir.op_count: {attributed}+{unattributed} != {op_count}"
                )
            coverage_ratio = source_sites.get("coverage_ratio")
            if isinstance(coverage_ratio, int | float):
                expected_ratio = (
                    round(attributed / op_count, 6) if op_count > 0 else 1.0
                )
                if abs(float(coverage_ratio) - expected_ratio) > 0.000001:
                    errors.append(
                        "binary_image_analysis.backend_ir.source_sites.coverage_ratio="
                        f"{coverage_ratio} != {expected_ratio}"
                    )
    allocation_ownership = _mapping_or_empty(
        compiler_analysis.get("allocation_ownership")
    )
    if allocation_ownership.get("present") is True:
        event_count = allocation_ownership.get("event_count")
        attributed = allocation_ownership.get("source_attributed_event_count")
        unattributed = allocation_ownership.get("unattributed_event_count")
        if (
            isinstance(event_count, int)
            and isinstance(attributed, int)
            and isinstance(unattributed, int)
        ):
            total = attributed + unattributed
            if total != event_count:
                errors.append(
                    "binary_image_analysis.backend_ir.allocation_ownership "
                    f"source accounting does not sum to event_count: {attributed}+{unattributed} != {event_count}"
                )
            coverage_ratio = allocation_ownership.get("source_coverage_ratio")
            if isinstance(coverage_ratio, int | float):
                expected_ratio = (
                    round(attributed / event_count, 6) if event_count > 0 else 1.0
                )
                if abs(float(coverage_ratio) - expected_ratio) > 0.000001:
                    errors.append(
                        "binary_image_analysis.backend_ir.allocation_ownership."
                        f"source_coverage_ratio={coverage_ratio} != {expected_ratio}"
                    )
        categories = _mapping_or_empty(allocation_ownership.get("events_by_category"))
        if isinstance(event_count, int) and categories:
            category_total = sum(
                count
                for count in categories.values()
                if isinstance(count, int) and not isinstance(count, bool)
            )
            if category_total != event_count:
                errors.append(
                    "binary_image_analysis.backend_ir.allocation_ownership "
                    f"category counts do not sum to event_count: {category_total} != {event_count}"
                )


def _summarize_build_diagnostics(
    diagnostics: Mapping[str, Any],
    errors: list[str],
    warnings: list[str],
) -> dict[str, Any]:
    closure = diagnostics.get("binary_image_closure")
    if closure is not None and not isinstance(closure, Mapping):
        errors.append("build_diagnostics.binary_image_closure must be an object")
        closure = None
    image = _mapping_or_empty(closure.get("image") if closure else None)
    known_modules = _string_list(closure.get("known_modules") if closure else None)
    compile_modules = _string_list(closure.get("compile_modules") if closure else None)
    declared_roots = _string_list(
        closure.get("declared_root_modules") if closure else None
    )
    entry_reachable = _string_list(
        closure.get("entry_reachable_modules") if closure else None
    )
    runtime_support = _string_list(
        closure.get("runtime_support_modules") if closure else None
    )
    stdlib_support = _string_list(
        closure.get("stdlib_support_modules") if closure else None
    )
    package_parents = _string_list(
        closure.get("package_parent_modules") if closure else None
    )
    root_modules = _string_list(image.get("root_modules"))

    _check_unique("known_modules", known_modules, errors)
    _check_unique("compile_modules", compile_modules, errors)
    known_set = set(known_modules)
    _check_subset(
        "compile_modules", compile_modules, "known_modules", known_set, errors
    )
    _check_subset(
        "declared_root_modules", declared_roots, "known_modules", known_set, errors
    )
    _check_subset(
        "image.root_modules", root_modules, "known_modules", known_set, errors
    )

    _check_count(
        "known_module_count",
        diagnostics.get("known_module_count"),
        len(known_modules),
        errors,
    )
    _check_count(
        "compile_module_count",
        diagnostics.get("compile_module_count"),
        len(compile_modules),
        errors,
    )
    module_count = diagnostics.get("module_count")
    if (
        isinstance(module_count, int)
        and known_modules
        and module_count != len(known_modules)
    ):
        warnings.append(
            "build_diagnostics.module_count does not match "
            f"binary_image_closure.known_modules: {module_count} != {len(known_modules)}"
        )

    timings = _list_of_mappings(diagnostics.get("frontend_module_timings"))
    timing_top = _list_of_mappings(diagnostics.get("frontend_module_timings_top"))[:10]
    slowest_module = timing_top[0] if timing_top else None

    return {
        "total_sec": _number_or_none(diagnostics.get("total_sec")),
        "phase_sec": _numeric_mapping(diagnostics.get("phase_sec")),
        "module_count": module_count if isinstance(module_count, int) else None,
        "binary_image": {
            "kind": _string_or_none(image.get("kind")),
            "selector_source": _string_or_none(image.get("selector_source")),
            "entry_module": _string_or_none(image.get("entry_module")),
            "root_modules": root_modules,
            "closure_mode": _string_or_none(image.get("closure_mode")),
        },
        "closure": {
            "known_modules": known_modules,
            "compile_modules": compile_modules,
            "declared_root_modules": declared_roots,
            "entry_reachable_modules": entry_reachable,
            "runtime_support_modules": runtime_support,
            "stdlib_support_modules": stdlib_support,
            "package_parent_modules": package_parents,
            "known_module_count": len(known_modules),
            "compile_module_count": len(compile_modules),
        },
        "module_reason_summary": _int_mapping(diagnostics.get("module_reason_summary")),
        "frontend_timing": {
            "module_timing_count": len(timings),
            "slowest_module": _compact_timing(slowest_module),
            "top": [_compact_timing(item) for item in timing_top],
        },
        "frontend_parallel": _mapping_or_empty(diagnostics.get("frontend_parallel")),
    }


def _summarize_ir_tir(
    diagnostics: Mapping[str, Any],
    tir_fact_graphs: Sequence[tuple[str, Mapping[str, Any]]],
    errors: list[str],
    warnings: list[str],
) -> dict[str, Any]:
    midend = _mapping_or_empty(diagnostics.get("midend"))
    fact_summaries: list[dict[str, Any]] = []
    for path, graph in tir_fact_graphs:
        try:
            fact_graph_dump.validate_graph(graph, source=path)
        except fact_graph_dump.FactGraphError as exc:
            errors.append(str(exc))
            continue
        summary = _mapping_or_empty(graph.get("summary"))
        fact_summaries.append(
            {
                "path": path,
                "function": graph.get("function"),
                "value_count": summary.get("value_count"),
                "fact_count": summary.get("fact_count"),
                "edge_count": summary.get("edge_count"),
                "call_fact_count": summary.get("call_fact_count"),
                "source_site_value_count": summary.get("source_site_value_count"),
                "allocation_ownership_fact_count": summary.get(
                    "allocation_ownership_fact_count"
                ),
                "boxed_value_count": len(fact_graph_dump.boxed_rows(graph)),
            }
        )

    midend_function_count = midend.get("function_count")
    if (
        isinstance(midend_function_count, int)
        and fact_summaries
        and len(fact_summaries) > midend_function_count
    ):
        warnings.append(
            "more TIR fact graphs were supplied than midend functions report: "
            f"{len(fact_summaries)} > {midend_function_count}"
        )

    return {
        "midend": {
            "requested_profile": _string_or_none(midend.get("requested_profile")),
            "effective_profiles": _string_list(midend.get("effective_profiles")),
            "function_count": midend_function_count
            if isinstance(midend_function_count, int)
            else None,
            "degraded_functions": midend.get("degraded_functions")
            if isinstance(midend.get("degraded_functions"), int)
            else None,
            "promoted_functions": midend.get("promoted_functions")
            if isinstance(midend.get("promoted_functions"), int)
            else None,
            "pass_wall_time_ranked": _list_of_mappings(
                midend.get("pass_wall_time_ranked")
            )[:20],
            "pass_hotspots_top": _list_of_mappings(midend.get("pass_hotspots_top"))[
                :10
            ],
            "policy_config": _mapping_or_empty(midend.get("policy_config")),
        },
        "tir_fact_graphs": fact_summaries,
        "tir_fact_graph_count": len(fact_summaries),
    }


def _summarize_allocation(diagnostics: Mapping[str, Any]) -> dict[str, Any]:
    allocations = diagnostics.get("allocations")
    if not isinstance(allocations, Mapping):
        return {"present": False}
    return {
        "present": True,
        "current_bytes": allocations.get("current_bytes")
        if isinstance(allocations.get("current_bytes"), int)
        else None,
        "peak_bytes": allocations.get("peak_bytes")
        if isinstance(allocations.get("peak_bytes"), int)
        else None,
        "top": _list_of_mappings(allocations.get("top"))[:20],
    }


def _summarize_binary(
    binary_size: Mapping[str, Any] | None,
    startup_audit: Mapping[str, Any] | None,
) -> dict[str, Any]:
    size_summary: dict[str, Any] | None = None
    if binary_size is not None:
        size_summary = {
            "format": _string_or_none(binary_size.get("format")),
            "path": _string_or_none(binary_size.get("path")),
            "total_bytes": binary_size.get("total_bytes")
            if isinstance(binary_size.get("total_bytes"), int)
            else None,
            "category_totals": _int_mapping(binary_size.get("category_totals")),
            "by_type": _int_mapping(binary_size.get("by_type")),
            "top_symbols": _list_of_mappings(binary_size.get("top_50_symbols"))[:20],
            "sections": _list_of_mappings(binary_size.get("sections"))[:40],
        }

    startup_summary: dict[str, Any] | None = None
    if startup_audit is not None:
        startup_summary = {
            "schema_version": startup_audit.get("schema_version"),
            "ok": startup_audit.get("ok"),
            "summary": _mapping_or_empty(startup_audit.get("summary")),
            "cases": [
                _summarize_startup_case(row)
                for row in _list_of_mappings(startup_audit.get("cases"))
            ],
        }

    return {
        "size": size_summary,
        "startup": startup_summary,
    }


def _summarize_startup_case(row: Mapping[str, Any]) -> dict[str, Any]:
    case = _mapping_or_empty(row.get("case"))
    artifact = _mapping_or_empty(row.get("artifact"))
    startup = _mapping_or_empty(row.get("startup"))
    return {
        "target": case.get("target"),
        "build_profile": case.get("build_profile"),
        "backend": case.get("backend"),
        "stdlib_profile": case.get("stdlib_profile"),
        "status": row.get("status"),
        "ok": row.get("ok"),
        "artifact_bytes": artifact.get("bytes"),
        "startup_medians": {
            name: _startup_median_s(value)
            for name, value in startup.items()
            if isinstance(value, Mapping)
        },
    }


def _startup_median_s(value: Mapping[str, Any]) -> float | None:
    stats = value.get("stats")
    if not isinstance(stats, Mapping):
        return None
    median = stats.get("median_s")
    return float(median) if isinstance(median, int | float) else None


def _analysis_tools_manifest(
    *,
    has_startup: bool,
    has_binary: bool,
    fact_graph_count: int,
) -> dict[str, Any]:
    return {
        "build_diagnostics": {
            "source": "molt.cli build --diagnostics",
            "facts": [
                "source_ast_module_timing",
                "binary_image_closure",
                "backend_source_site_coverage",
                "backend_allocation_ownership_events",
                "allocation_snapshot",
                "tir_midend_pass_telemetry",
            ],
            "required": True,
        },
        "fact_graph_dump": {
            "source": "tools/fact_graph_dump.py",
            "facts": ["tir_value_fact_graph"],
            "artifacts": fact_graph_count,
        },
        "binary_size_analysis": {
            "source": "tools/binary_size_analysis.py",
            "facts": ["binary_total_bytes", "sections", "symbols"],
            "present": has_binary,
        },
        "output_startup_size_audit": {
            "source": "tools/output_startup_size_audit.py",
            "facts": ["cold_start", "same_path_startup", "artifact_bytes"],
            "present": has_startup,
        },
    }


def _compact_timing(item: Mapping[str, Any] | None) -> dict[str, Any] | None:
    if item is None:
        return None
    return {
        "module": item.get("module"),
        "path": item.get("path"),
        "visit_s": _number_or_none(item.get("visit_s")),
        "lower_s": _number_or_none(item.get("lower_s")),
        "total_s": _number_or_none(item.get("total_s")),
        "timed_out": item.get("timed_out")
        if isinstance(item.get("timed_out"), bool)
        else None,
    }


def _mapping_or_empty(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _list_of_mappings(value: Any) -> list[Mapping[str, Any]]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, Mapping)]


def _string_list(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str)]


def _string_or_none(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _number_or_none(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, int | float):
        return None
    return float(value)


def _int_or_none(value: Any) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    return value


def _numeric_mapping(value: Any) -> dict[str, float]:
    if not isinstance(value, Mapping):
        return {}
    return {
        str(key): float(item)
        for key, item in value.items()
        if isinstance(key, str)
        and not isinstance(item, bool)
        and isinstance(item, int | float)
    }


def _int_mapping(value: Any) -> dict[str, int]:
    if not isinstance(value, Mapping):
        return {}
    return {
        str(key): int(item)
        for key, item in value.items()
        if isinstance(key, str) and isinstance(item, int) and not isinstance(item, bool)
    }


def _check_unique(name: str, values: Sequence[str], errors: list[str]) -> None:
    duplicates = sorted({value for value in values if values.count(value) > 1})
    if duplicates:
        errors.append(f"{name} contains duplicate entries: {', '.join(duplicates)}")


def _check_subset(
    name: str,
    values: Sequence[str],
    owner_name: str,
    owner: set[str],
    errors: list[str],
) -> None:
    missing = sorted(set(values) - owner)
    if missing:
        errors.append(
            f"{name} contains entries outside {owner_name}: {', '.join(missing)}"
        )


def _check_count(
    name: str,
    raw_value: Any,
    expected: int,
    errors: list[str],
) -> None:
    if raw_value is None:
        return
    if not isinstance(raw_value, int) or isinstance(raw_value, bool):
        errors.append(f"{name} must be an integer when present")
        return
    if raw_value != expected:
        errors.append(f"{name}={raw_value}, expected {expected}")


def _utc_now() -> str:
    return _dt.datetime.now(tz=_dt.UTC).replace(microsecond=0).isoformat()


def _write_json(path: Path, payload: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    tmp.replace(path)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--build-diagnostics",
        required=True,
        type=Path,
        help="JSON emitted by molt build --diagnostics-file",
    )
    parser.add_argument(
        "--startup-audit",
        type=Path,
        help="JSON emitted by tools/output_startup_size_audit.py",
    )
    parser.add_argument(
        "--binary-size-json",
        type=Path,
        help="JSON emitted by tools/binary_size_analysis.py --json",
    )
    parser.add_argument(
        "--binary",
        type=Path,
        help="Analyze this binary through tools/binary_size_analysis.py",
    )
    parser.add_argument(
        "--tir-fact-graph",
        action="append",
        type=Path,
        default=[],
        help="TIR fact graph JSON emitted by the compiler, repeatable",
    )
    parser.add_argument("--label", help="Human-readable capsule label")
    parser.add_argument("--out", type=Path, help="Write capsule JSON to this path")
    parser.add_argument(
        "--allow-errors",
        action="store_true",
        help="Emit the capsule even when cross-layer checks fail",
    )
    args = parser.parse_args(argv)

    if args.binary_size_json and args.binary:
        parser.error("use --binary-size-json or --binary, not both")

    try:
        build_diagnostics = load_json(args.build_diagnostics)
        startup_audit = load_json(args.startup_audit) if args.startup_audit else None
        if args.binary_size_json:
            binary_size = load_json(args.binary_size_json)
            binary_size_path = str(args.binary_size_json)
        elif args.binary:
            binary_size = analyze_binary(args.binary)
            binary_size_path = str(args.binary)
        else:
            binary_size = None
            binary_size_path = None

        fact_graphs = [(str(path), load_json(path)) for path in args.tir_fact_graph]
        capsule = build_capsule(
            build_diagnostics=build_diagnostics,
            build_diagnostics_path=str(args.build_diagnostics),
            startup_audit=startup_audit,
            startup_audit_path=str(args.startup_audit) if args.startup_audit else None,
            binary_size=binary_size,
            binary_size_path=binary_size_path,
            tir_fact_graphs=fact_graphs,
            label=args.label,
        )
    except CapsuleError as exc:
        print(f"analysis_capsule: {exc}", file=sys.stderr)
        return 2

    if args.out:
        _write_json(args.out, capsule)
    else:
        print(json.dumps(capsule, indent=2, sort_keys=True))

    if capsule["cross_checks"]["errors"] and not args.allow_errors:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

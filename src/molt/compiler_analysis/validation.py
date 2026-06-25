from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from molt.compiler_analysis.schema import SCHEMA_VERSION


def summarize_compiler_binary_image_analysis(
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
    if schema_version != SCHEMA_VERSION:
        errors.append(
            f"build_diagnostics.binary_image_analysis.schema_version must be {SCHEMA_VERSION}"
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


def check_compiler_analysis_against_closure(
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


def _mapping_or_empty(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _list_of_mappings(value: Any) -> list[Mapping[str, Any]]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, Mapping)]


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


def _int_mapping(value: Any) -> dict[str, int]:
    if not isinstance(value, Mapping):
        return {}
    return {
        str(key): int(item)
        for key, item in value.items()
        if isinstance(key, str) and isinstance(item, int) and not isinstance(item, bool)
    }


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

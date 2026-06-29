from __future__ import annotations

import math
from collections.abc import Mapping
from typing import Any, cast

from molt.compiler_analysis.schema import (
    ALLOCATION_OWNERSHIP_CARRIER,
    BINARY_IMAGE_ANALYSIS_STAGES,
    SCHEMA_VERSION,
    SOURCE_SITE_CARRIER,
    TIR_BOUNDARY_CARRIER,
)


def validate_binary_image_closure_diagnostics(
    diagnostics: Mapping[str, Any],
    errors: list[str],
) -> None:
    closure = diagnostics.get("binary_image_closure")
    if not isinstance(closure, Mapping):
        errors.append("build_diagnostics.binary_image_closure must be an object")
        return
    image = closure.get("image")
    if not isinstance(image, Mapping):
        errors.append("build_diagnostics.binary_image_closure.image must be an object")
        image = {}
    for field in (
        "known_modules",
        "compile_modules",
        "declared_root_modules",
        "entry_reachable_modules",
        "runtime_support_modules",
        "stdlib_support_modules",
        "package_parent_modules",
    ):
        _required_string_list(
            closure,
            field,
            "build_diagnostics.binary_image_closure",
            errors,
        )
    _required_string_list(
        image,
        "root_modules",
        "build_diagnostics.binary_image_closure.image",
        errors,
    )


def summarize_compiler_binary_image_analysis(
    diagnostics: Mapping[str, Any],
    errors: list[str],
) -> dict[str, Any]:
    raw = diagnostics.get("binary_image_analysis")
    if raw is None:
        errors.append("build_diagnostics.binary_image_analysis must be an object")
        return {"present": False}
    if not isinstance(raw, Mapping):
        errors.append("build_diagnostics.binary_image_analysis must be an object")
        return {"present": False}
    schema_version = raw.get("schema_version")
    if schema_version != SCHEMA_VERSION:
        errors.append(
            f"build_diagnostics.binary_image_analysis.schema_version must be {SCHEMA_VERSION}"
        )
    allowed_keys = {"schema_version", *BINARY_IMAGE_ANALYSIS_STAGES}
    for key in sorted(raw):
        if key not in allowed_keys:
            errors.append(
                f"build_diagnostics.binary_image_analysis.{key} is not a known stage"
            )
    stages: dict[str, Mapping[str, Any]] = {}
    for stage in BINARY_IMAGE_ANALYSIS_STAGES:
        payload = raw.get(stage)
        if payload is None:
            errors.append(
                f"build_diagnostics.binary_image_analysis.{stage} is required"
            )
            continue
        if not isinstance(payload, Mapping):
            errors.append(
                f"build_diagnostics.binary_image_analysis.{stage} must be an object"
            )
            continue
        stage_schema_version = payload.get("schema_version")
        if stage_schema_version != SCHEMA_VERSION:
            errors.append(
                "build_diagnostics.binary_image_analysis."
                f"{stage}.schema_version must be {SCHEMA_VERSION}"
            )
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
        "tir_boundary": _summarize_backend_tir_boundary(
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
        if isinstance(event_count, int):
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
        "carrier": _required_literal(
            raw,
            "carrier",
            SOURCE_SITE_CARRIER,
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "attributed_op_count": _required_nonnegative_int(
            raw,
            "attributed_op_count",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "unattributed_op_count": _required_nonnegative_int(
            raw,
            "unattributed_op_count",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "coverage_ratio": _required_ratio(
            raw,
            "coverage_ratio",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "function_count_with_source": _required_nonnegative_int(
            raw,
            "function_count_with_source",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "line_count": _required_nonnegative_int(
            raw,
            "line_count",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "explicit_source_line_count": _required_nonnegative_int(
            raw,
            "explicit_source_line_count",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "line_marker_fallback_count": _required_nonnegative_int(
            raw,
            "line_marker_fallback_count",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "source_site_digest": _required_string(
            raw,
            "source_site_digest",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
        ),
        "top_source_lines_by_ops": _required_source_line_rows(
            raw,
            "top_source_lines_by_ops",
            "build_diagnostics.binary_image_analysis.backend_ir.source_sites",
            errors,
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
        "carrier": _required_literal(
            raw,
            "carrier",
            ALLOCATION_OWNERSHIP_CARRIER,
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        ),
        "event_count": _required_nonnegative_int(
            raw,
            "event_count",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        ),
        "source_attributed_event_count": _required_nonnegative_int(
            raw,
            "source_attributed_event_count",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        ),
        "unattributed_event_count": _required_nonnegative_int(
            raw,
            "unattributed_event_count",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        ),
        "source_coverage_ratio": _required_ratio(
            raw,
            "source_coverage_ratio",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        ),
        "events_by_category": _required_int_mapping(
            raw,
            "events_by_category",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        ),
        "top_category_kinds": _required_category_kind_rows(
            raw,
            "top_category_kinds",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        )[:20],
        "top_source_lines_by_events": _required_allocation_source_line_rows(
            raw,
            "top_source_lines_by_events",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        )[:20],
        "allocation_ownership_digest": _required_string(
            raw,
            "allocation_ownership_digest",
            "build_diagnostics.binary_image_analysis.backend_ir.allocation_ownership",
            errors,
        ),
    }


def _summarize_backend_tir_boundary(
    backend_stage: Mapping[str, Any] | None,
    errors: list[str],
) -> dict[str, Any]:
    if backend_stage is None:
        return {"present": False}
    raw = backend_stage.get("tir_boundary")
    if not isinstance(raw, Mapping):
        errors.append(
            "build_diagnostics.binary_image_analysis.backend_ir.tir_boundary "
            "must be an object"
        )
        return {"present": False}
    return {
        "present": True,
        "carrier": _required_literal(
            raw,
            "carrier",
            TIR_BOUNDARY_CARRIER,
            "build_diagnostics.binary_image_analysis.backend_ir.tir_boundary",
            errors,
        ),
        "semantic_role": _required_string(
            raw,
            "semantic_role",
            "build_diagnostics.binary_image_analysis.backend_ir.tir_boundary",
            errors,
        ),
    }


def _mapping_or_empty(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _required_string(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> str | None:
    value = raw.get(field)
    if isinstance(value, str):
        return value
    errors.append(f"{path}.{field} must be a string")
    return None


def _required_literal(
    raw: Mapping[str, Any],
    field: str,
    expected: str,
    path: str,
    errors: list[str],
) -> str | None:
    value = _required_string(raw, field, path, errors)
    if value is None:
        return None
    if value != expected:
        errors.append(f"{path}.{field} must be {expected!r}")
        return None
    return value


def _required_number(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> float | None:
    value = raw.get(field)
    if (
        isinstance(value, bool)
        or not isinstance(value, int | float)
        or not math.isfinite(float(value))
    ):
        errors.append(f"{path}.{field} must be a number")
        return None
    return float(value)


def _required_ratio(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> float | None:
    value = _required_number(raw, field, path, errors)
    if value is None:
        return None
    if not 0.0 <= value <= 1.0:
        errors.append(f"{path}.{field} must be between 0 and 1")
        return None
    return value


def _required_nonnegative_int(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> int | None:
    value = raw.get(field)
    if isinstance(value, bool) or not isinstance(value, int):
        errors.append(f"{path}.{field} must be an integer")
        return None
    if value < 0:
        errors.append(f"{path}.{field} must be nonnegative")
        return None
    return value


def _required_positive_int(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> int | None:
    value = _required_nonnegative_int(raw, field, path, errors)
    if value is None:
        return None
    if value <= 0:
        errors.append(f"{path}.{field} must be positive")
        return None
    return value


def _required_int_mapping(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> dict[str, int]:
    value = raw.get(field)
    if not isinstance(value, Mapping):
        errors.append(f"{path}.{field} must be an object")
        return {}
    result: dict[str, int] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            errors.append(f"{path}.{field} keys must be strings")
            continue
        if isinstance(item, bool) or not isinstance(item, int):
            errors.append(f"{path}.{field}.{key} must be an integer")
            continue
        if item < 0:
            errors.append(f"{path}.{field}.{key} must be nonnegative")
            continue
        result[key] = item
    return result


def _required_source_line_rows(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> list[dict[str, Any]]:
    rows = _required_list_of_mappings(raw, field, path, errors)
    result: list[dict[str, Any]] = []
    for index, row in enumerate(rows):
        row_path = f"{path}.{field}[{index}]"
        _reject_unknown_fields(row, {"source_file", "line", "ops"}, row_path, errors)
        source_file = _required_string(row, "source_file", row_path, errors)
        line = _required_positive_int(row, "line", row_path, errors)
        ops = _required_nonnegative_int(row, "ops", row_path, errors)
        if source_file is not None and line is not None and ops is not None:
            result.append({"source_file": source_file, "line": line, "ops": ops})
    return result


def _required_category_kind_rows(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> list[dict[str, Any]]:
    rows = _required_list_of_mappings(raw, field, path, errors)
    result: list[dict[str, Any]] = []
    for index, row in enumerate(rows):
        row_path = f"{path}.{field}[{index}]"
        _reject_unknown_fields(row, {"category_kind", "events"}, row_path, errors)
        category_kind = _required_string(row, "category_kind", row_path, errors)
        events = _required_nonnegative_int(row, "events", row_path, errors)
        if category_kind is not None and events is not None:
            result.append({"category_kind": category_kind, "events": events})
    return result


def _required_allocation_source_line_rows(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> list[dict[str, Any]]:
    rows = _required_list_of_mappings(raw, field, path, errors)
    result: list[dict[str, Any]] = []
    for index, row in enumerate(rows):
        row_path = f"{path}.{field}[{index}]"
        _reject_unknown_fields(
            row,
            {"source_file", "line", "category", "events"},
            row_path,
            errors,
        )
        source_file = _required_string(row, "source_file", row_path, errors)
        line = _required_positive_int(row, "line", row_path, errors)
        category = _required_string(row, "category", row_path, errors)
        events = _required_nonnegative_int(row, "events", row_path, errors)
        if (
            source_file is not None
            and line is not None
            and category is not None
            and events is not None
        ):
            result.append(
                {
                    "source_file": source_file,
                    "line": line,
                    "category": category,
                    "events": events,
                }
            )
    return result


def _required_list_of_mappings(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> list[Mapping[str, Any]]:
    value = raw.get(field)
    if not isinstance(value, list):
        errors.append(f"{path}.{field} must be an array")
        return []
    result: list[Mapping[str, Any]] = []
    for index, item in enumerate(value):
        if not isinstance(item, Mapping):
            errors.append(f"{path}.{field}[{index}] must be an object")
            continue
        if not all(isinstance(key, str) for key in item):
            errors.append(f"{path}.{field}[{index}] keys must be strings")
            continue
        result.append(cast(Mapping[str, Any], item))
    return result


def _reject_unknown_fields(
    raw: Mapping[str, Any],
    allowed: set[str],
    path: str,
    errors: list[str],
) -> None:
    for key in raw:
        if not isinstance(key, str):
            errors.append(f"{path} keys must be strings")
            continue
        if key not in allowed:
            errors.append(f"{path}.{key} is not a known field")


def _required_string_list(
    raw: Mapping[str, Any],
    field: str,
    path: str,
    errors: list[str],
) -> list[str]:
    value = raw.get(field)
    if not isinstance(value, list):
        errors.append(f"{path}.{field} must be an array")
        return []
    result: list[str] = []
    for index, item in enumerate(value):
        if not isinstance(item, str):
            errors.append(f"{path}.{field}[{index}] must be a string")
            continue
        result.append(item)
    return result


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

from __future__ import annotations

from collections import Counter
from collections.abc import Mapping, Sequence
from typing import Any, cast

from molt.compiler_analysis.hashing import stable_payload_hash
from molt.compiler_analysis.schema import (
    ALLOCATION_OWNERSHIP_CARRIER,
    SCHEMA_VERSION,
    SOURCE_SITE_CARRIER,
    TIR_BOUNDARY_CARRIER,
)
from molt.frontend.lowering import op_kinds_generated as op_kind_facts


def _string_key_mapping(value: object) -> Mapping[str, Any] | None:
    if not isinstance(value, Mapping):
        return None
    if not all(isinstance(key, str) for key in value):
        return None
    return cast(Mapping[str, Any], value)


def backend_ir_op_source_site(
    op: Mapping[str, Any],
) -> tuple[dict[str, int | None] | None, str]:
    source = "source_line"
    line = _positive_int_or_none(op.get("source_line"))
    if line is None and op.get("kind") == "line":
        line = _positive_int_or_none(op.get("value"))
        source = "line_marker"
    if line is None:
        return None, "missing"
    return (
        {
            "line": line,
            "col": _nonnegative_int_or_none(op.get("col_offset")),
            "end_col": _nonnegative_int_or_none(op.get("end_col_offset")),
        },
        source,
    )


def backend_ir_canonical_kind(op: Mapping[str, Any]) -> str:
    kind = op.get("kind")
    if not isinstance(kind, str):
        return "<unknown>"
    return op_kind_facts.canonical_kind(kind)


def backend_ir_allocation_categories(op: Mapping[str, Any]) -> list[str]:
    kind = backend_ir_canonical_kind(op)
    categories: list[str] = []
    if kind in op_kind_facts.BINARY_IMAGE_HEAP_ALLOC_ROOT_KINDS:
        categories.append("heap_alloc_root")
    if kind in op_kind_facts.BINARY_IMAGE_STACK_ALLOC_ROOT_KINDS:
        categories.append("stack_alloc_root")
    if kind in op_kind_facts.BINARY_IMAGE_REF_RETAIN_KINDS:
        categories.append("ref_retain")
    if kind in op_kind_facts.BINARY_IMAGE_REF_RELEASE_KINDS:
        categories.append("ref_release")
    if kind in op_kind_facts.BINARY_IMAGE_HEAP_EXPOSURE_KINDS:
        categories.append("heap_exposure")
    if op.get("arena_eligible") is True:
        categories.append("arena_eligible")
    if op.get("defines_del") is True:
        categories.append("finalizer_sensitive")
    return categories


def backend_ir_binary_image_analysis_payload(ir: Mapping[str, Any]) -> dict[str, Any]:
    functions = ir.get("functions")
    if not isinstance(functions, list):
        functions = []
    op_counts: Counter[str] = Counter()
    function_ops: list[dict[str, Any]] = []
    call_op_count = 0
    for func in functions:
        if not isinstance(func, Mapping):
            continue
        name = func.get("name")
        if not isinstance(name, str):
            name = "<unknown>"
        ops = func.get("ops")
        if not isinstance(ops, list):
            ops = []
        function_source_ops = 0
        for op in ops:
            if not isinstance(op, Mapping):
                continue
            kind = op.get("kind")
            if not isinstance(kind, str):
                kind = "<unknown>"
            op_counts[kind] += 1
            if kind == "call":
                call_op_count += 1
            if backend_ir_op_source_site(op)[0] is not None:
                function_source_ops += 1
        function_ops.append(
            {
                "function": name,
                "ops": len(ops),
                "source_site_ops": function_source_ops,
            }
        )
    function_ops.sort(key=lambda item: (-int(item["ops"]), str(item["function"])))
    op_kind_top = [
        {"kind": kind, "count": count}
        for kind, count in sorted(
            op_counts.items(), key=lambda item: (-item[1], item[0])
        )[:20]
    ]
    function_names = [
        func.get("name")
        for func in functions
        if isinstance(func, Mapping) and isinstance(func.get("name"), str)
    ]
    op_total = sum(op_counts.values())
    return {
        "schema_version": SCHEMA_VERSION,
        "backend_ir": {
            "function_count": len(function_ops),
            "op_count": op_total,
            "call_op_count": call_op_count,
            "op_kind_count": len(op_counts),
            "op_kind_top": op_kind_top,
            "top_functions_by_ops": function_ops[:10],
            "function_order_hash": stable_payload_hash(function_names),
            "profile_attached": "profile" in ir,
            "runtime_feedback_attached": "runtime_feedback" in ir,
        },
        "source_sites": _backend_ir_source_site_payload(
            functions,
            op_total=op_total,
        ),
        "allocation_ownership": _backend_ir_allocation_ownership_payload(functions),
        "tir_boundary": {
            "carrier": TIR_BOUNDARY_CARRIER,
            "semantic_role": "frontend-to-TIR/backend input",
        },
    }


def _backend_ir_source_site_payload(
    functions: Sequence[Any],
    *,
    op_total: int,
) -> dict[str, Any]:
    site_records: list[dict[str, Any]] = []
    line_counts: Counter[tuple[str, int]] = Counter()
    attributed_functions: set[str] = set()
    explicit_source_line_count = 0
    line_marker_fallback_count = 0
    for func in functions:
        if not isinstance(func, Mapping):
            continue
        name = func.get("name")
        if not isinstance(name, str):
            name = "<unknown>"
        source_file = func.get("source_file")
        if not isinstance(source_file, str) or not source_file:
            source_file = "<unknown>"
        ops = func.get("ops")
        if not isinstance(ops, list):
            continue
        for op_index, op in enumerate(ops):
            op_mapping = _string_key_mapping(op)
            if op_mapping is None:
                continue
            site, source = backend_ir_op_source_site(op_mapping)
            if site is None:
                continue
            kind = op_mapping.get("kind")
            if not isinstance(kind, str):
                kind = "<unknown>"
            attributed_functions.add(name)
            if source == "line_marker":
                line_marker_fallback_count += 1
            else:
                explicit_source_line_count += 1
            line = site["line"]
            if line is None:
                continue
            line_counts[(source_file, line)] += 1
            site_records.append(
                {
                    "function": name,
                    "op_index": op_index,
                    "kind": kind,
                    "source_file": source_file,
                    "line": line,
                    "col": site["col"],
                    "end_col": site["end_col"],
                }
            )
    attributed_op_count = len(site_records)
    coverage_ratio = round(attributed_op_count / op_total, 6) if op_total > 0 else 1.0
    top_source_lines = [
        {"source_file": source_file, "line": line, "ops": count}
        for (source_file, line), count in sorted(
            line_counts.items(),
            key=lambda item: (-item[1], item[0][0], item[0][1]),
        )[:20]
    ]
    return {
        "carrier": SOURCE_SITE_CARRIER,
        "attributed_op_count": attributed_op_count,
        "unattributed_op_count": max(op_total - attributed_op_count, 0),
        "coverage_ratio": coverage_ratio,
        "function_count_with_source": len(attributed_functions),
        "line_count": len(line_counts),
        "explicit_source_line_count": explicit_source_line_count,
        "line_marker_fallback_count": line_marker_fallback_count,
        "source_site_digest": stable_payload_hash(site_records),
        "top_source_lines_by_ops": top_source_lines,
    }


def _backend_ir_allocation_ownership_payload(
    functions: Sequence[Any],
) -> dict[str, Any]:
    category_counts: Counter[str] = Counter()
    kind_counts: Counter[str] = Counter()
    line_counts: Counter[tuple[str, int, str]] = Counter()
    event_records: list[dict[str, Any]] = []
    unattributed_event_count = 0
    for func in functions:
        if not isinstance(func, Mapping):
            continue
        name = func.get("name")
        if not isinstance(name, str):
            name = "<unknown>"
        source_file = func.get("source_file")
        if not isinstance(source_file, str) or not source_file:
            source_file = "<unknown>"
        ops = func.get("ops")
        if not isinstance(ops, list):
            continue
        for op_index, op in enumerate(ops):
            op_mapping = _string_key_mapping(op)
            if op_mapping is None:
                continue
            categories = backend_ir_allocation_categories(op_mapping)
            if not categories:
                continue
            kind = backend_ir_canonical_kind(op_mapping)
            site, _source = backend_ir_op_source_site(op_mapping)
            for category in categories:
                category_counts[category] += 1
                kind_counts[f"{category}:{kind}"] += 1
                if site is None:
                    unattributed_event_count += 1
                    continue
                line = site["line"]
                if line is None:
                    unattributed_event_count += 1
                    continue
                line_counts[(source_file, line, category)] += 1
                event_records.append(
                    {
                        "function": name,
                        "op_index": op_index,
                        "kind": kind,
                        "category": category,
                        "source_file": source_file,
                        "line": line,
                        "col": site["col"],
                        "end_col": site["end_col"],
                    }
                )
    attributed_event_count = len(event_records)
    total_event_count = attributed_event_count + unattributed_event_count
    source_coverage_ratio = (
        round(attributed_event_count / total_event_count, 6)
        if total_event_count > 0
        else 1.0
    )
    top_source_lines = [
        {
            "source_file": source_file,
            "line": line,
            "category": category,
            "events": count,
        }
        for (source_file, line, category), count in sorted(
            line_counts.items(),
            key=lambda item: (-item[1], item[0][0], item[0][1], item[0][2]),
        )[:20]
    ]
    top_kinds = [
        {"category_kind": category_kind, "events": count}
        for category_kind, count in sorted(
            kind_counts.items(),
            key=lambda item: (-item[1], item[0]),
        )[:20]
    ]
    return {
        "carrier": ALLOCATION_OWNERSHIP_CARRIER,
        "event_count": total_event_count,
        "source_attributed_event_count": attributed_event_count,
        "unattributed_event_count": unattributed_event_count,
        "source_coverage_ratio": source_coverage_ratio,
        "events_by_category": {
            name: category_counts[name] for name in sorted(category_counts)
        },
        "top_category_kinds": top_kinds,
        "top_source_lines_by_events": top_source_lines,
        "allocation_ownership_digest": stable_payload_hash(event_records),
    }


def _positive_int_or_none(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int) and value > 0:
        return value
    return None


def _nonnegative_int_or_none(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int) and value >= 0:
        return value
    return None

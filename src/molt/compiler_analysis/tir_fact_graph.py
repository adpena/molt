from __future__ import annotations

from collections import Counter
from collections.abc import Mapping
from typing import Any

from molt.compiler_analysis.hashing import stable_payload_hash


def summarize_tir_fact_graph(
    path: str,
    graph: Mapping[str, Any],
    *,
    boxed_value_count: int,
) -> dict[str, Any]:
    summary = _mapping_or_empty(graph.get("summary"))
    source_records: list[dict[str, Any]] = []
    allocation_records: list[dict[str, Any]] = []
    allocation_kind_counts: Counter[str] = Counter()
    source_line_counts: Counter[tuple[str, int]] = Counter()
    allocation_line_counts: Counter[tuple[str, int, str]] = Counter()
    known_source_files: set[str] = set()
    unknown_source_file_site_count = 0

    for node in _list_of_mappings(graph.get("values")):
        value = _int_or_none(node.get("value"))
        producer = _mapping_or_empty(node.get("producer"))
        if producer:
            site = _mapping_or_none(producer.get("source_site"))
            if site is not None:
                source_file, line = _site_identity(site)
                if source_file == "<unknown>":
                    unknown_source_file_site_count += 1
                else:
                    known_source_files.add(source_file)
                source_line_counts[(source_file, line)] += 1
                source_records.append(
                    {
                        "role": "producer",
                        "value": value,
                        "kind": _string_or_unknown(producer.get("kind")),
                        "source_file": source_file,
                        "line": line,
                        "col": _int_or_none(site.get("col")),
                        "end_col": _int_or_none(site.get("end_col")),
                    }
                )
        for consumer in _list_of_mappings(node.get("consumers")):
            site = _mapping_or_none(consumer.get("source_site"))
            if site is None:
                continue
            source_file, line = _site_identity(site)
            if source_file == "<unknown>":
                unknown_source_file_site_count += 1
            else:
                known_source_files.add(source_file)
            source_line_counts[(source_file, line)] += 1
            source_records.append(
                {
                    "role": "consumer",
                    "value": value,
                    "kind": _string_or_unknown(consumer.get("kind")),
                    "source_file": source_file,
                    "line": line,
                    "col": _int_or_none(site.get("col")),
                    "end_col": _int_or_none(site.get("end_col")),
                }
            )
        for fact in _list_of_mappings(node.get("facts")):
            kind = _string_or_unknown(fact.get("kind"))
            site = _mapping_or_none(fact.get("source_site"))
            if site is not None:
                source_file, line = _site_identity(site)
                if source_file == "<unknown>":
                    unknown_source_file_site_count += 1
                else:
                    known_source_files.add(source_file)
                source_line_counts[(source_file, line)] += 1
                source_records.append(
                    {
                        "role": "fact",
                        "value": value,
                        "kind": kind,
                        "source_file": source_file,
                        "line": line,
                        "col": _int_or_none(site.get("col")),
                        "end_col": _int_or_none(site.get("end_col")),
                    }
                )
            if not (kind.startswith("allocation.") or kind.startswith("ownership.")):
                continue
            allocation_kind_counts[kind] += 1
            if site is None:
                continue
            source_file, line = _site_identity(site)
            allocation_line_counts[(source_file, line, kind)] += 1
            allocation_records.append(
                {
                    "value": value,
                    "kind": kind,
                    "source_file": source_file,
                    "line": line,
                    "event_id": _string_or_none(fact.get("event_id")),
                }
            )

    return {
        "path": path,
        "function": graph.get("function"),
        "value_count": summary.get("value_count"),
        "fact_count": summary.get("fact_count"),
        "edge_count": summary.get("edge_count"),
        "call_fact_count": summary.get("call_fact_count"),
        "source_site_value_count": summary.get("source_site_value_count"),
        "source_site_record_count": len(source_records),
        "source_file_count": len(known_source_files),
        "source_files": sorted(known_source_files),
        "unknown_source_file_site_count": unknown_source_file_site_count,
        "top_source_lines_by_records": [
            {"source_file": source_file, "line": line, "records": count}
            for (source_file, line), count in sorted(
                source_line_counts.items(),
                key=lambda item: (-item[1], item[0][0], item[0][1]),
            )[:20]
        ],
        "source_site_digest": stable_payload_hash(source_records),
        "allocation_ownership_fact_count": summary.get(
            "allocation_ownership_fact_count"
        ),
        "allocation_ownership_by_kind": {
            kind: allocation_kind_counts[kind]
            for kind in sorted(allocation_kind_counts)
        },
        "top_allocation_ownership_source_lines": [
            {
                "source_file": source_file,
                "line": line,
                "kind": kind,
                "facts": count,
            }
            for (source_file, line, kind), count in sorted(
                allocation_line_counts.items(),
                key=lambda item: (-item[1], item[0][0], item[0][1], item[0][2]),
            )[:20]
        ],
        "allocation_ownership_digest": stable_payload_hash(allocation_records),
        "boxed_value_count": boxed_value_count,
    }


def _site_identity(site: Mapping[str, Any]) -> tuple[str, int]:
    source_file = site.get("source_file")
    if not isinstance(source_file, str) or not source_file:
        source_file = "<unknown>"
    line = _int_or_none(site.get("line"))
    return source_file, line if line is not None else 0


def _mapping_or_empty(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _mapping_or_none(value: Any) -> Mapping[str, Any] | None:
    return value if isinstance(value, Mapping) else None


def _list_of_mappings(value: Any) -> list[Mapping[str, Any]]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, Mapping)]


def _string_or_none(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _string_or_unknown(value: Any) -> str:
    return value if isinstance(value, str) and value else "<unknown>"


def _int_or_none(value: Any) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    return value

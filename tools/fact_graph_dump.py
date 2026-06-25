#!/usr/bin/env python3
"""Validate and summarize compiler-emitted TIR fact-graph JSON."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Mapping
from pathlib import Path
from typing import Any

FACT_GRAPH_SCHEMA_VERSION = 3
FACT_GRAPH_KIND = "molt_tir_fact_graph"
BOXED_REPRS = frozenset({"DynBox", "MaybeBigInt"})


class FactGraphError(ValueError):
    """Raised when a fact-graph document violates the dump contract."""


def load_graph(path: Path) -> dict[str, Any]:
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise FactGraphError(f"{path}: cannot read fact graph: {exc}") from exc
    if not isinstance(doc, dict):
        raise FactGraphError(f"{path}: fact graph must be a JSON object")
    validate_graph(doc, source=str(path))
    return doc


def validate_graph(doc: Mapping[str, Any], *, source: str = "<graph>") -> None:
    if doc.get("schema_version") != FACT_GRAPH_SCHEMA_VERSION:
        raise FactGraphError(
            f"{source}: schema_version must be {FACT_GRAPH_SCHEMA_VERSION}"
        )
    if doc.get("kind") != FACT_GRAPH_KIND:
        raise FactGraphError(f"{source}: kind must be {FACT_GRAPH_KIND!r}")
    if not isinstance(doc.get("function"), str) or not doc["function"]:
        raise FactGraphError(f"{source}: function must be a non-empty string")
    values = doc.get("values")
    if not isinstance(values, list):
        raise FactGraphError(f"{source}: values must be a list")
    seen_values: set[int] = set()
    seen_event_ids: set[str] = set()
    fact_count = 0
    call_fact_count = 0
    for idx, raw_node in enumerate(values):
        node = _mapping(raw_node, f"{source}.values[{idx}]")
        value = _int(node.get("value"), f"{source}.values[{idx}].value")
        if value in seen_values:
            raise FactGraphError(f"{source}: duplicate value node {value}")
        seen_values.add(value)
        producer = node.get("producer")
        if producer is not None:
            parsed_producer = _validate_mapping_fields(
                producer,
                f"{source}.values[{idx}].producer",
                required=("kind",),
            )
            _validate_source_site_field(
                parsed_producer,
                f"{source}.values[{idx}].producer",
            )
        consumers = node.get("consumers")
        if not isinstance(consumers, list):
            raise FactGraphError(f"{source}.values[{idx}].consumers must be a list")
        for c_idx, raw_consumer in enumerate(consumers):
            consumer = _validate_mapping_fields(
                raw_consumer,
                f"{source}.values[{idx}].consumers[{c_idx}]",
                required=("kind", "role"),
            )
            _validate_source_site_field(
                consumer,
                f"{source}.values[{idx}].consumers[{c_idx}]",
            )
        facts = node.get("facts")
        if not isinstance(facts, list):
            raise FactGraphError(f"{source}.values[{idx}].facts must be a list")
        fact_count += len(facts)
        for f_idx, raw_fact in enumerate(facts):
            fact = _validate_mapping_fields(
                raw_fact,
                f"{source}.values[{idx}].facts[{f_idx}]",
                required=("kind", "value", "confidence", "producer"),
            )
            fact_source = f"{source}.values[{idx}].facts[{f_idx}]"
            _validate_event_id_field(fact, fact_source)
            _validate_source_site_field(fact, fact_source)
            event_id = fact.get("event_id")
            if event_id is not None:
                if event_id in seen_event_ids:
                    raise FactGraphError(f"{fact_source}.event_id is duplicated")
                seen_event_ids.add(event_id)
            _list(fact.get("guards"), f"{source}.values[{idx}].facts[{f_idx}].guards")
            _list(
                fact.get("invalidators"),
                f"{source}.values[{idx}].facts[{f_idx}].invalidators",
            )
            if str(fact["kind"]).startswith("call."):
                call_fact_count += 1
    edges = doc.get("edges")
    if not isinstance(edges, list):
        raise FactGraphError(f"{source}: edges must be a list")
    for idx, raw_edge in enumerate(edges):
        edge = _mapping(raw_edge, f"{source}.edges[{idx}]")
        from_value = _int(edge.get("from_value"), f"{source}.edges[{idx}].from_value")
        if from_value not in seen_values:
            raise FactGraphError(
                f"{source}.edges[{idx}]: from_value {from_value} missing"
            )
        to_value = edge.get("to_value")
        if to_value is not None:
            parsed_to = _int(to_value, f"{source}.edges[{idx}].to_value")
            if parsed_to not in seen_values:
                raise FactGraphError(
                    f"{source}.edges[{idx}]: to_value {parsed_to} missing"
                )
        if not isinstance(edge.get("kind"), str) or not edge["kind"]:
            raise FactGraphError(f"{source}.edges[{idx}].kind must be a string")
        consumer = _validate_mapping_fields(
            edge.get("consumer"),
            f"{source}.edges[{idx}].consumer",
            required=("kind", "role"),
        )
        _validate_source_site_field(consumer, f"{source}.edges[{idx}].consumer")
    summary = _mapping(doc.get("summary"), f"{source}.summary")
    source_site_value_count = sum(
        1
        for node in values
        if _node_has_source_site(_mapping(node, f"{source}.values[]"))
    )
    allocation_ownership_fact_count = sum(
        1
        for node in values
        for fact in _list(node.get("facts"), f"{source}.values[].facts")
        if isinstance(fact, Mapping)
        and (
            str(fact.get("kind", "")).startswith("allocation.")
            or str(fact.get("kind", "")).startswith("ownership.")
        )
    )
    expected = {
        "value_count": len(values),
        "fact_count": fact_count,
        "edge_count": len(edges),
        "call_fact_count": call_fact_count,
        "source_site_value_count": source_site_value_count,
        "allocation_ownership_fact_count": allocation_ownership_fact_count,
    }
    for key, expected_value in expected.items():
        actual = _int(summary.get(key), f"{source}.summary.{key}")
        if actual != expected_value:
            raise FactGraphError(
                f"{source}.summary.{key}={actual}, expected {expected_value}"
            )


def summarize_graph(doc: Mapping[str, Any], *, why_boxed: bool = False) -> str:
    validate_graph(doc)
    lines = [
        f"{doc['kind']} schema={doc['schema_version']}",
        f"function={doc['function']}",
    ]
    summary = doc["summary"]
    lines.append(
        "values={value_count} facts={fact_count} call_facts={call_fact_count} "
        "edges={edge_count}".format(**summary)
    )
    rows = boxed_rows(doc) if why_boxed else doc["values"]
    for node in rows:
        value = node["value"]
        producer = node.get("producer") or {}
        producer_label = producer_label_for(producer)
        fact_bits = [
            f"{fact['kind']}={fact['value']}({fact['confidence']})"
            for fact in node.get("facts", [])
            if not why_boxed or fact["kind"] == "repr_floor"
        ]
        lines.append(
            f"%{value} producer={producer_label} consumers={len(node.get('consumers', []))} "
            + " ".join(fact_bits)
        )
    return "\n".join(lines)


def boxed_rows(doc: Mapping[str, Any]) -> list[Mapping[str, Any]]:
    rows: list[Mapping[str, Any]] = []
    for node in doc["values"]:
        if any(
            fact.get("kind") == "repr_floor" and fact.get("value") in BOXED_REPRS
            for fact in node.get("facts", [])
        ):
            rows.append(node)
    return rows


def producer_label_for(producer: Mapping[str, Any]) -> str:
    if not producer:
        return "unknown"
    kind = producer.get("kind")
    opcode = producer.get("opcode")
    block = producer.get("block")
    op_index = producer.get("op_index")
    if opcode is not None:
        return f"{kind}:bb{block}:op{op_index}:{opcode}"
    if block is not None:
        return f"{kind}:bb{block}"
    return str(kind)


def _node_has_source_site(node: Mapping[str, Any]) -> bool:
    producer = node.get("producer")
    if isinstance(producer, Mapping) and isinstance(
        producer.get("source_site"), Mapping
    ):
        return True
    consumers = node.get("consumers")
    if isinstance(consumers, list) and any(
        isinstance(consumer, Mapping)
        and isinstance(consumer.get("source_site"), Mapping)
        for consumer in consumers
    ):
        return True
    facts = node.get("facts")
    if isinstance(facts, list) and any(
        isinstance(fact, Mapping) and isinstance(fact.get("source_site"), Mapping)
        for fact in facts
    ):
        return True
    return False


def _validate_source_site_field(mapping: Mapping[str, Any], source: str) -> None:
    if "source_site" not in mapping:
        raise FactGraphError(f"{source}.source_site is required")
    _validate_source_site(
        mapping.get("source_site"), f"{source}.source_site", required=False
    )


def _validate_source_site(value: Any, source: str, *, required: bool) -> None:
    if value is None and not required:
        return
    site = _mapping(value, source)
    _positive_int(site.get("line"), f"{source}.line")
    col = site.get("col")
    if col is not None:
        _int(col, f"{source}.col")
    end_col = site.get("end_col")
    if end_col is not None:
        _int(end_col, f"{source}.end_col")
    source_file = site.get("source_file")
    if source_file is not None and (
        not isinstance(source_file, str) or not source_file
    ):
        raise FactGraphError(f"{source}.source_file must be a non-empty string")


def _validate_event_id_field(mapping: Mapping[str, Any], source: str) -> None:
    if "event_id" not in mapping:
        raise FactGraphError(f"{source}.event_id is required")
    event_id = mapping.get("event_id")
    if event_id is not None and (not isinstance(event_id, str) or not event_id):
        raise FactGraphError(f"{source}.event_id must be null or a non-empty string")


def _validate_mapping_fields(
    value: Any, source: str, *, required: tuple[str, ...]
) -> Mapping[str, Any]:
    mapping = _mapping(value, source)
    for key in required:
        item = mapping.get(key)
        if not isinstance(item, str) or not item:
            raise FactGraphError(f"{source}.{key} must be a non-empty string")
    return mapping


def _mapping(value: Any, source: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise FactGraphError(f"{source} must be an object")
    return value


def _list(value: Any, source: str) -> list[Any]:
    if not isinstance(value, list):
        raise FactGraphError(f"{source} must be a list")
    return value


def _int(value: Any, source: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise FactGraphError(f"{source} must be a non-negative integer")
    return value


def _positive_int(value: Any, source: str) -> int:
    parsed = _int(value, source)
    if parsed <= 0:
        raise FactGraphError(f"{source} must be a positive integer")
    return parsed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("graph", type=Path, help="fact graph JSON emitted by molt-tir")
    parser.add_argument("--json", action="store_true", help="validate and re-emit JSON")
    parser.add_argument(
        "--why-boxed",
        action="store_true",
        help="show only values whose conservative Repr floor is boxed or BigInt-safe",
    )
    args = parser.parse_args(argv)

    try:
        doc = load_graph(args.graph)
    except FactGraphError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps(doc, indent=2, sort_keys=True))
    else:
        print(summarize_graph(doc, why_boxed=args.why_boxed))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

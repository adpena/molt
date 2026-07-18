from __future__ import annotations

from collections.abc import Mapping
from typing import cast


def _rows(facts: Mapping[str, object], field: str, width: int) -> list[list[object]]:
    raw_rows = facts.get(field)
    if not isinstance(raw_rows, list):
        raise ValueError(f"WASM facts {field} must be a list")
    rows: list[list[object]] = []
    for raw_row in raw_rows:
        if not isinstance(raw_row, list) or len(raw_row) != width:
            raise ValueError(f"WASM facts {field} rows must have width {width}")
        rows.append(cast(list[object], raw_row))
    return rows


def _index(value: object, field: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"WASM facts {field} indices must be non-negative integers")
    return value


def fact_index_set(facts: Mapping[str, object], field: str) -> set[int]:
    raw_indices = facts.get(field)
    if not isinstance(raw_indices, list):
        raise ValueError(f"WASM facts {field} must be a list")
    return {_index(value, field) for value in raw_indices}


def active_function_element_rows(
    facts: Mapping[str, object],
) -> list[tuple[int, int, int]]:
    return [
        (
            _index(table_index, "active_function_elements.table"),
            _index(slot, "active_function_elements.slot"),
            _index(function_index, "active_function_elements.function"),
        )
        for table_index, slot, function_index in _rows(
            facts, "active_function_elements", 3
        )
    ]


def callable_table_entry_rows(
    facts: Mapping[str, object],
) -> list[tuple[int, int, int, int]]:
    return [
        (
            _index(slot, "callable_table_entries.slot"),
            _index(function_index, "callable_table_entries.function"),
            _index(type_index, "callable_table_entries.type"),
            _index(role, "callable_table_entries.role"),
        )
        for slot, function_index, type_index, role in _rows(
            facts, "callable_table_entries", 4
        )
    ]


def table_mutation_rows(
    facts: Mapping[str, object], field: str = "table_mutations"
) -> list[tuple[int, str, int, int | None]]:
    result: list[tuple[int, str, int, int | None]] = []
    for function_index, operation, table_index, source_table_index in _rows(
        facts, field, 4
    ):
        if not isinstance(operation, str) or not operation:
            raise ValueError(f"WASM facts {field} operation must be a string")
        result.append(
            (
                _index(function_index, f"{field}.function"),
                operation,
                _index(table_index, f"{field}.table"),
                None
                if source_table_index is None
                else _index(source_table_index, f"{field}.source_table"),
            )
        )
    return result

#!/usr/bin/env python3
"""Summarize machine-readable TIR per-pass fact deltas.

`MOLT_EMIT_PASS_DELTA=1` makes the TIR pass manager write JSONL records that
answer which pass changed representation facts, boxing, generic calls, runtime
helper calls, refcount events, or exception events. This tool owns the portable
dashboard contract over those records.
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict
from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

PASS_DELTA_SCHEMA_VERSION = 1
PASS_DELTA_KIND = "molt_tir_pass_delta"
DASHBOARD_KIND = "molt_tir_pass_delta_dashboard"

SIGNAL_FIELDS = (
    "added_box_ops",
    "added_generic_calls",
    "added_runtime_helper_calls",
    "added_rc_events",
    "added_exception_events",
    "added_type_guard_ops",
    "added_heap_alloc_ops",
)

SIGNED_SIGNAL_FIELDS = (
    "call_results_dynbox_delta",
    "call_results_typed_repr_delta",
)


class PassDeltaError(ValueError):
    """Raised when a pass-delta input violates the dashboard contract."""


@dataclass
class PassAggregate:
    pass_name: str
    records: int = 0
    functions: set[str] = field(default_factory=set)
    targets: set[str] = field(default_factory=set)
    added_box_ops: int = 0
    added_generic_calls: int = 0
    added_runtime_helper_calls: int = 0
    added_rc_events: int = 0
    added_exception_events: int = 0
    added_type_guard_ops: int = 0
    added_heap_alloc_ops: int = 0
    call_results_dynbox_delta: int = 0
    call_results_typed_repr_delta: int = 0
    lost_repr_values: dict[str, int] = field(default_factory=lambda: defaultdict(int))
    gained_repr_values: dict[str, int] = field(default_factory=lambda: defaultdict(int))

    def add(self, record: Mapping[str, Any]) -> None:
        delta = _mapping(record, "delta")
        self.records += 1
        self.functions.add(_str(record, "function"))
        target = _mapping(record, "target")
        self.targets.add(f"{_str(target, 'target')}/{_str(target, 'profile')}")
        for field_name in SIGNAL_FIELDS:
            setattr(
                self,
                field_name,
                getattr(self, field_name) + _nonnegative_int(delta, field_name),
            )
        for field_name in SIGNED_SIGNAL_FIELDS:
            setattr(
                self,
                field_name,
                getattr(self, field_name) + _int(delta, field_name),
            )
        _merge_counts(self.lost_repr_values, _count_map(delta, "lost_repr_values"))
        _merge_counts(self.gained_repr_values, _count_map(delta, "gained_repr_values"))

    @property
    def score(self) -> int:
        typed_loss = max(0, self.call_results_dynbox_delta)
        typed_gain_penalty = max(0, -self.call_results_typed_repr_delta)
        return (
            self.added_box_ops
            + self.added_generic_calls
            + self.added_runtime_helper_calls
            + self.added_rc_events
            + self.added_exception_events
            + self.added_type_guard_ops
            + self.added_heap_alloc_ops
            + sum(self.lost_repr_values.values())
            + typed_loss
            + typed_gain_penalty
        )

    def to_json(self) -> dict[str, Any]:
        return {
            "pass": self.pass_name,
            "records": self.records,
            "functions": sorted(self.functions),
            "targets": sorted(self.targets),
            "added_box_ops": self.added_box_ops,
            "added_generic_calls": self.added_generic_calls,
            "added_runtime_helper_calls": self.added_runtime_helper_calls,
            "added_rc_events": self.added_rc_events,
            "added_exception_events": self.added_exception_events,
            "added_type_guard_ops": self.added_type_guard_ops,
            "added_heap_alloc_ops": self.added_heap_alloc_ops,
            "call_results_dynbox_delta": self.call_results_dynbox_delta,
            "call_results_typed_repr_delta": self.call_results_typed_repr_delta,
            "lost_repr_values": dict(sorted(self.lost_repr_values.items())),
            "gained_repr_values": dict(sorted(self.gained_repr_values.items())),
            "score": self.score,
        }


def load_records(path: Path) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_no, raw_line in enumerate(handle, start=1):
            line = raw_line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as exc:
                raise PassDeltaError(
                    f"{path}:{line_no}: invalid JSONL row: {exc}"
                ) from exc
            if not isinstance(row, dict):
                raise PassDeltaError(f"{path}:{line_no}: row must be a JSON object")
            validate_record(row, source=f"{path}:{line_no}")
            records.append(row)
    return records


def validate_record(record: Mapping[str, Any], *, source: str = "<record>") -> None:
    if record.get("schema_version") != PASS_DELTA_SCHEMA_VERSION:
        raise PassDeltaError(
            f"{source}: schema_version must be {PASS_DELTA_SCHEMA_VERSION}"
        )
    if record.get("kind") != PASS_DELTA_KIND:
        raise PassDeltaError(f"{source}: kind must be {PASS_DELTA_KIND!r}")
    for key in ("function", "pass", "mutation_class"):
        _str(record, key, source=source)
    host = _mapping(record, "host", source=source)
    for key in ("os", "arch", "family"):
        _str(host, key, source=f"{source}.host")
    _nonnegative_int(host, "pointer_width", source=f"{source}.host")
    target = _mapping(record, "target", source=source)
    for key in ("target", "profile"):
        _str(target, key, source=f"{source}.target")
    stats = _mapping(record, "stats", source=source)
    for key in (
        "values_changed",
        "attrs_changed",
        "ops_removed",
        "ops_added",
        "facts_changed",
        "total_changes",
    ):
        _nonnegative_int(stats, key, source=f"{source}.stats")
    _mapping(record, "before", source=source)
    _mapping(record, "after", source=source)
    delta = _mapping(record, "delta", source=source)
    for key in SIGNAL_FIELDS:
        _nonnegative_int(delta, key, source=f"{source}.delta")
    for key in SIGNED_SIGNAL_FIELDS:
        _int(delta, key, source=f"{source}.delta")
    _count_map(delta, "lost_repr_values", source=f"{source}.delta")
    _count_map(delta, "gained_repr_values", source=f"{source}.delta")


def summarize_records(
    records: Iterable[Mapping[str, Any]],
    *,
    source: str,
    benchmark: str | None = None,
    function: str | None = None,
) -> dict[str, Any]:
    all_records = list(records)
    considered = [
        record
        for record in all_records
        if _record_matches(record, benchmark=benchmark, function=function)
    ]
    aggregates: dict[str, PassAggregate] = {}
    totals = PassAggregate(pass_name="__total__")
    risk_records: list[dict[str, Any]] = []
    host_targets: set[tuple[str, str, str, int, str, str]] = set()

    for record in considered:
        validate_record(record)
        pass_name = _str(record, "pass")
        aggregate = aggregates.setdefault(pass_name, PassAggregate(pass_name))
        aggregate.add(record)
        totals.add(record)
        host = _mapping(record, "host")
        target = _mapping(record, "target")
        host_targets.add(
            (
                _str(host, "os"),
                _str(host, "arch"),
                _str(host, "family"),
                _nonnegative_int(host, "pointer_width"),
                _str(target, "target"),
                _str(target, "profile"),
            )
        )
        risk = _record_risk(record)
        if risk["score"] > 0:
            risk_records.append(risk)

    by_pass = sorted(
        (aggregate.to_json() for aggregate in aggregates.values()),
        key=lambda row: (-int(row["score"]), str(row["pass"])),
    )
    risk_records.sort(
        key=lambda row: (-int(row["score"]), row["function"], row["pass"])
    )

    return {
        "schema_version": PASS_DELTA_SCHEMA_VERSION,
        "kind": DASHBOARD_KIND,
        "source": source,
        "filters": {"benchmark": benchmark, "function": function},
        "records_seen": len(all_records),
        "records_considered": len(considered),
        "host_targets": [
            {
                "host": {
                    "os": os_name,
                    "arch": arch,
                    "family": family,
                    "pointer_width": pointer_width,
                },
                "target": {"target": target, "profile": profile},
            }
            for os_name, arch, family, pointer_width, target, profile in sorted(
                host_targets
            )
        ],
        "totals": totals.to_json() | {"pass": "TOTAL"},
        "by_pass": by_pass,
        "risk_records": risk_records[:50],
    }


def _record_matches(
    record: Mapping[str, Any],
    *,
    benchmark: str | None,
    function: str | None,
) -> bool:
    function_name = str(record.get("function") or "")
    if function and function != function_name:
        return False
    if benchmark:
        benchmark_field = record.get("benchmark")
        if isinstance(benchmark_field, str):
            return benchmark == benchmark_field
        return benchmark in function_name
    return True


def _record_risk(record: Mapping[str, Any]) -> dict[str, Any]:
    delta = _mapping(record, "delta")
    signals = {
        key: _nonnegative_int(delta, key)
        for key in SIGNAL_FIELDS
        if _nonnegative_int(delta, key) > 0
    }
    call_dynbox = max(0, _int(delta, "call_results_dynbox_delta"))
    call_typed_loss = max(0, -_int(delta, "call_results_typed_repr_delta"))
    if call_dynbox:
        signals["call_results_dynbox_delta"] = call_dynbox
    if call_typed_loss:
        signals["lost_call_results_typed_repr"] = call_typed_loss
    lost_repr = _count_map(delta, "lost_repr_values")
    score = sum(signals.values()) + sum(lost_repr.values())
    return {
        "function": _str(record, "function"),
        "pass": _str(record, "pass"),
        "mutation_class": _str(record, "mutation_class"),
        "target": _mapping(record, "target"),
        "signals": signals,
        "lost_repr_values": lost_repr,
        "score": score,
    }


def _mapping(
    mapping: Mapping[str, Any], key: str, *, source: str = "<record>"
) -> Mapping[str, Any]:
    value = mapping.get(key)
    if not isinstance(value, Mapping):
        raise PassDeltaError(f"{source}: {key} must be an object")
    return value


def _str(mapping: Mapping[str, Any], key: str, *, source: str = "<record>") -> str:
    value = mapping.get(key)
    if not isinstance(value, str) or not value:
        raise PassDeltaError(f"{source}: {key} must be a non-empty string")
    return value


def _int(mapping: Mapping[str, Any], key: str, *, source: str = "<record>") -> int:
    value = mapping.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        raise PassDeltaError(f"{source}: {key} must be an integer")
    return value


def _nonnegative_int(
    mapping: Mapping[str, Any], key: str, *, source: str = "<record>"
) -> int:
    value = _int(mapping, key, source=source)
    if value < 0:
        raise PassDeltaError(f"{source}: {key} must be non-negative")
    return value


def _count_map(
    mapping: Mapping[str, Any], key: str, *, source: str = "<record>"
) -> dict[str, int]:
    value = mapping.get(key)
    if not isinstance(value, Mapping):
        raise PassDeltaError(f"{source}: {key} must be an object")
    out: dict[str, int] = {}
    for item_key, item_value in value.items():
        if not isinstance(item_key, str) or not item_key:
            raise PassDeltaError(f"{source}: {key} keys must be non-empty strings")
        if (
            not isinstance(item_value, int)
            or isinstance(item_value, bool)
            or item_value < 0
        ):
            raise PassDeltaError(f"{source}: {key}.{item_key} must be non-negative int")
        out[item_key] = item_value
    return out


def _merge_counts(target: dict[str, int], counts: Mapping[str, int]) -> None:
    for key, value in counts.items():
        target[key] += value


def _format_text(summary: Mapping[str, Any]) -> str:
    lines = [
        f"{summary['kind']} schema={summary['schema_version']}",
        f"source={summary['source']}",
        f"records={summary['records_considered']}/{summary['records_seen']}",
    ]
    for row in summary["by_pass"][:10]:
        lines.append(
            "pass={pass} score={score} boxes={added_box_ops} "
            "calls={added_generic_calls} rc={added_rc_events} "
            "lost_repr={lost_repr_values}".format(**row)
        )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("jsonl", type=Path, help="Pass-delta JSONL artifact")
    parser.add_argument(
        "--benchmark", help="Filter by benchmark field or function substring"
    )
    parser.add_argument("--function", help="Filter by exact TIR function name")
    parser.add_argument("--json", action="store_true", help="Emit JSON dashboard")
    args = parser.parse_args(argv)

    records = load_records(args.jsonl)
    summary = summarize_records(
        records,
        source=str(args.jsonl),
        benchmark=args.benchmark,
        function=args.function,
    )
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(_format_text(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

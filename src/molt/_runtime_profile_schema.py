from __future__ import annotations

from typing import Any, TypeGuard


RuntimeProfilePayload = dict[str, Any]

PROCESS_PROFILE_SCHEMA_VERSION = 2
PROCESS_PROFILE_KIND = "runtime_feedback"
PROFILE_EPOCH_SCHEMA_VERSION = 1
PROFILE_EPOCH_KIND = "runtime_profile_epoch"

_PROCESS_COUNTER_SECTIONS = (
    "profile",
    "aux",
    "gc",
    "hot_paths",
    "deopt_reasons",
)
_PROCESS_MEMORY_KEYS = ("peak_rss_bytes", "current_rss_bytes")
_PROCESS_ROOT_KEYS = {
    "schema_version",
    "kind",
    *_PROCESS_COUNTER_SECTIONS,
    "memory",
}
_EPOCH_ROOT_KEYS = {
    "schema_version",
    "kind",
    "generation",
    "label",
    "delta",
    "counter_regressions",
    "gauges",
    "memory",
}
_EPOCH_MEMORY_KEYS = (
    "current_rss_start_bytes",
    "current_rss_end_bytes",
    "current_rss_delta_bytes",
    "process_peak_start_bytes",
    "process_peak_end_bytes",
)
_U64_MAX = (1 << 64) - 1
_I64_MIN = -(1 << 63)
_I64_MAX = (1 << 63) - 1


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_u64(value: object) -> bool:
    return _is_int(value) and 0 <= value <= _U64_MAX


def _is_i64(value: object) -> bool:
    return _is_int(value) and _I64_MIN <= value <= _I64_MAX


def _is_named_mapping(value: object) -> bool:
    return isinstance(value, dict) and all(
        isinstance(key, str) and bool(key) for key in value
    )


def _is_counter_mapping(value: object) -> bool:
    return _is_named_mapping(value) and all(
        _is_u64(counter) for counter in value.values()
    )


def _is_counter_sections(value: object) -> bool:
    return _is_named_mapping(value) and all(
        _is_counter_mapping(section) for section in value.values()
    )


def _is_counter_regressions(value: object) -> bool:
    if not _is_named_mapping(value):
        return False
    for section in value.values():
        if not _is_named_mapping(section):
            return False
        for regression in section.values():
            if not isinstance(regression, dict) or set(regression) != {"start", "end"}:
                return False
            start = regression["start"]
            end = regression["end"]
            if not (_is_u64(start) and _is_u64(end) and end < start):
                return False
    return True


def _is_gauge_sections(value: object) -> bool:
    if not _is_named_mapping(value):
        return False
    for section in value.values():
        if not _is_named_mapping(section):
            return False
        for gauge in section.values():
            if not isinstance(gauge, dict) or set(gauge) != {"start", "end", "delta"}:
                return False
            start = gauge["start"]
            end = gauge["end"]
            delta = gauge["delta"]
            if not (
                _is_u64(start)
                and _is_u64(end)
                and _is_i64(delta)
                and delta == end - start
            ):
                return False
    return True


def is_process_profile(value: object) -> TypeGuard[RuntimeProfilePayload]:
    if not isinstance(value, dict):
        return False
    if set(value) != _PROCESS_ROOT_KEYS:
        return False
    if value.get("schema_version") != PROCESS_PROFILE_SCHEMA_VERSION:
        return False
    if value.get("kind") != PROCESS_PROFILE_KIND:
        return False
    if not all(
        _is_counter_mapping(value.get(section)) for section in _PROCESS_COUNTER_SECTIONS
    ):
        return False
    memory = value.get("memory")
    return (
        isinstance(memory, dict)
        and set(memory) == set(_PROCESS_MEMORY_KEYS)
        and all(_is_u64(metric) for metric in memory.values())
    )


def is_profile_epoch(value: object) -> TypeGuard[RuntimeProfilePayload]:
    if not isinstance(value, dict):
        return False
    if set(value) != _EPOCH_ROOT_KEYS:
        return False
    if value.get("schema_version") != PROFILE_EPOCH_SCHEMA_VERSION:
        return False
    if value.get("kind") != PROFILE_EPOCH_KIND:
        return False
    generation = value.get("generation")
    label = value.get("label")
    if not (_is_u64(generation) and generation > 0):
        return False
    if not (isinstance(label, str) and bool(label)):
        return False
    if not _is_counter_sections(value.get("delta")):
        return False
    if not _is_counter_regressions(value.get("counter_regressions")):
        return False
    if not _is_gauge_sections(value.get("gauges")):
        return False
    memory = value.get("memory")
    if not isinstance(memory, dict) or set(memory) != set(_EPOCH_MEMORY_KEYS):
        return False
    start = memory["current_rss_start_bytes"]
    end = memory["current_rss_end_bytes"]
    delta = memory["current_rss_delta_bytes"]
    peak_start = memory["process_peak_start_bytes"]
    peak_end = memory["process_peak_end_bytes"]
    return (
        _is_u64(start)
        and _is_u64(end)
        and _is_i64(delta)
        and delta == end - start
        and _is_u64(peak_start)
        and _is_u64(peak_end)
        and peak_end >= peak_start
    )

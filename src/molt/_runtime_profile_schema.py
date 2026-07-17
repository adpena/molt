from __future__ import annotations

from typing import Any, TypeGuard, TypedDict, cast

from molt._runtime_profile_schema_generated import (
    EPOCH_MEMORY_FIELDS,
    PROCESS_COUNTER_KEYS,
    PROCESS_MEMORY_FIELDS,
    PROCESS_PROFILE_KIND,
    PROCESS_PROFILE_SCHEMA_VERSION,
    PROCESS_RSS_SOURCES,
    PROFILE_DELTA_KEYS,
    PROFILE_EPOCH_KIND,
    PROFILE_EPOCH_SCHEMA_VERSION,
    PROFILE_GAUGE_KEYS,
    UNAVAILABLE_RSS_SOURCES,
)


RuntimeProfilePayload = dict[str, Any]


class _MemorySnapshot(TypedDict):
    source: str
    available: bool
    current_rss_bytes: int | None
    peak_rss_bytes: int | None

_PROCESS_ROOT_KEYS = {
    "schema_version",
    "kind",
    *PROCESS_COUNTER_KEYS,
    "memory",
}
_MEMORY_SNAPSHOT_KEYS = PROCESS_MEMORY_FIELDS
_EPOCH_ROOT_KEYS = {
    "schema_version",
    "kind",
    "generation",
    "label",
    "claimable",
    "delta",
    "counter_regressions",
    "gauges",
    "memory",
}
_EPOCH_MEMORY_KEYS = EPOCH_MEMORY_FIELDS
_U64_MAX = (1 << 64) - 1
_I64_MIN = -(1 << 63)
_I64_MAX = (1 << 63) - 1


def _is_string_object(value: object) -> TypeGuard[dict[str, object]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in value)


def _is_int(value: object) -> TypeGuard[int]:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_u64(value: object) -> TypeGuard[int]:
    return _is_int(value) and 0 <= value <= _U64_MAX


def _is_i64(value: object) -> TypeGuard[int]:
    return _is_int(value) and _I64_MIN <= value <= _I64_MAX


def _validate_exact_keys(
    value: object, expected: frozenset[str] | set[str], path: str
) -> str | None:
    if not _is_string_object(value):
        return f"{path} must be an object"
    actual = set(value)
    if actual != set(expected):
        missing = sorted(set(expected) - actual)
        extra = sorted(actual - set(expected))
        return f"{path} keys differ: missing={missing}, extra={extra}"
    return None


def _validate_memory_snapshot(value: object, path: str) -> str | None:
    if error := _validate_exact_keys(value, _MEMORY_SNAPSHOT_KEYS, path):
        return error
    snapshot = cast(dict[str, object], value)
    source = snapshot["source"]
    available = snapshot["available"]
    current = snapshot["current_rss_bytes"]
    peak = snapshot["peak_rss_bytes"]
    if not isinstance(source, str) or source not in PROCESS_RSS_SOURCES:
        return f"{path}.source is not a canonical RSS source"
    if not isinstance(available, bool):
        return f"{path}.available must be bool"
    if available:
        if source in UNAVAILABLE_RSS_SOURCES:
            return f"{path}.available cannot be true for {source}"
        if not _is_u64(current) or not _is_u64(peak):
            return f"{path} available RSS values must be u64"
        if peak < current:
            return f"{path}.peak_rss_bytes must be >= current_rss_bytes"
    elif current is not None or peak is not None:
        return f"{path} unavailable RSS values must be null"
    return None


def validate_process_profile(value: object) -> str | None:
    if error := _validate_exact_keys(value, _PROCESS_ROOT_KEYS, "root"):
        return error
    root = cast(dict[str, object], value)
    if root["schema_version"] != PROCESS_PROFILE_SCHEMA_VERSION:
        return f"schema_version must be {PROCESS_PROFILE_SCHEMA_VERSION}"
    if root["kind"] != PROCESS_PROFILE_KIND:
        return f"kind must be {PROCESS_PROFILE_KIND!r}"
    for section, keys in PROCESS_COUNTER_KEYS.items():
        payload = root[section]
        if error := _validate_exact_keys(payload, keys, section):
            return error
        counters = cast(dict[str, object], payload)
        for key, counter in counters.items():
            if not _is_u64(counter):
                return f"{section}.{key} must be u64"
    if error := _validate_memory_snapshot(root["memory"], "memory"):
        return error

    profile = cast(dict[str, int], root["profile"])
    aux = cast(dict[str, int], root["aux"])
    gc = cast(dict[str, int], root["gc"])
    identities = (
        (
            "profile.live_objects",
            profile["live_objects"],
            profile["alloc_count"] - profile["dealloc_count"],
        ),
        (
            "profile.live_bytes",
            profile["live_bytes"],
            profile["alloc_bytes_total"] - profile["dealloc_bytes_total"],
        ),
        (
            "profile.live_exception",
            profile["live_exception"],
            profile["alloc_exception"] - profile["dealloc_exception"],
        ),
        (
            "profile.live_bytes_exception",
            profile["live_bytes_exception"],
            profile["alloc_bytes_exception"] - profile["dealloc_bytes_exception"],
        ),
        (
            "aux.aux_sidecar_live_count",
            aux["aux_sidecar_live_count"],
            aux["aux_sidecar_alloc_count"] - aux["aux_sidecar_free_count"],
        ),
        (
            "aux.aux_sidecar_live_bytes",
            aux["aux_sidecar_live_bytes"],
            aux["aux_sidecar_alloc_bytes"] - aux["aux_sidecar_free_bytes"],
        ),
        (
            "gc.gc_tracked_live",
            gc["gc_tracked_live"],
            gc["gc_track_count"] - gc["gc_untrack_count"],
        ),
    )
    for field, actual, signed_expected in identities:
        expected = max(signed_expected, 0)
        if actual != expected:
            return f"{field}={actual} violates derived value {expected}"
    if gc["gc_tracked_high_water"] < gc["gc_tracked_live"]:
        return "gc.gc_tracked_high_water must be >= gc.gc_tracked_live"
    return None


def is_process_profile(value: object) -> TypeGuard[RuntimeProfilePayload]:
    return validate_process_profile(value) is None


def _validate_regressions(value: object) -> tuple[str | None, set[tuple[str, str]]]:
    if not _is_string_object(value):
        return "counter_regressions must be an object", set()
    observed: set[tuple[str, str]] = set()
    for section, regressions in value.items():
        if section not in PROFILE_DELTA_KEYS:
            return f"counter_regressions.{section} is not a counter section", set()
        if not _is_string_object(regressions) or not regressions:
            return f"counter_regressions.{section} must be a non-empty object", set()
        unknown = set(regressions) - PROFILE_DELTA_KEYS[section]
        if unknown:
            return (
                f"counter_regressions.{section} has unknown keys {sorted(unknown)}",
                set(),
            )
        for metric, regression in regressions.items():
            if not _is_string_object(regression) or set(regression) != {"start", "end"}:
                return (
                    f"counter_regressions.{section}.{metric} must contain start/end",
                    set(),
                )
            start = regression["start"]
            end = regression["end"]
            if not _is_u64(start) or not _is_u64(end) or end >= start:
                return (
                    f"counter_regressions.{section}.{metric} is not a decrease",
                    set(),
                )
            observed.add((section, metric))
    return None, observed


def validate_profile_epoch(value: object) -> str | None:
    if error := _validate_exact_keys(value, _EPOCH_ROOT_KEYS, "root"):
        return error
    root = cast(dict[str, object], value)
    if root["schema_version"] != PROFILE_EPOCH_SCHEMA_VERSION:
        return f"schema_version must be {PROFILE_EPOCH_SCHEMA_VERSION}"
    if root["kind"] != PROFILE_EPOCH_KIND:
        return f"kind must be {PROFILE_EPOCH_KIND!r}"
    generation = root["generation"]
    if not _is_u64(generation) or generation == 0:
        return "generation must be a positive u64"
    label = root["label"]
    if not isinstance(label, str) or not label:
        return "label must be a non-empty string"
    claimable = root["claimable"]
    if not isinstance(claimable, bool):
        return "claimable must be bool"

    error, regressions = _validate_regressions(root["counter_regressions"])
    if error:
        return error
    if claimable != (not regressions):
        return "claimable must be false exactly when a counter regressed"

    delta = root["delta"]
    if error := _validate_exact_keys(delta, set(PROFILE_DELTA_KEYS), "delta"):
        return error
    delta = cast(dict[str, object], delta)
    null_deltas: set[tuple[str, str]] = set()
    for section, keys in PROFILE_DELTA_KEYS.items():
        payload = delta[section]
        if error := _validate_exact_keys(payload, keys, f"delta.{section}"):
            return error
        counters = cast(dict[str, object], payload)
        for metric, counter in counters.items():
            if counter is None:
                null_deltas.add((section, metric))
            elif not _is_u64(counter):
                return f"delta.{section}.{metric} must be u64 or null"
    if null_deltas != regressions:
        return "null deltas must correspond exactly to counter_regressions"

    gauges = root["gauges"]
    if error := _validate_exact_keys(gauges, set(PROFILE_GAUGE_KEYS), "gauges"):
        return error
    gauges = cast(dict[str, object], gauges)
    for section, keys in PROFILE_GAUGE_KEYS.items():
        payload = gauges[section]
        if error := _validate_exact_keys(payload, keys, f"gauges.{section}"):
            return error
        section_gauges = cast(dict[str, object], payload)
        for metric, gauge in section_gauges.items():
            if not _is_string_object(gauge) or set(gauge) != {"start", "end", "delta"}:
                return f"gauges.{section}.{metric} must contain start/end/delta"
            start, end, signed_delta = gauge["start"], gauge["end"], gauge["delta"]
            if not _is_u64(start) or not _is_u64(end):
                return f"gauges.{section}.{metric} has an invalid signed delta"
            if not _is_i64(signed_delta) or signed_delta != end - start:
                return f"gauges.{section}.{metric} has an invalid signed delta"

    memory = root["memory"]
    if error := _validate_exact_keys(memory, _EPOCH_MEMORY_KEYS, "memory"):
        return error
    memory = cast(dict[str, object], memory)
    if error := _validate_memory_snapshot(memory["start"], "memory.start"):
        return error
    if error := _validate_memory_snapshot(memory["end"], "memory.end"):
        return error
    start = memory["start"]
    end = memory["end"]
    start = cast(_MemorySnapshot, start)
    end = cast(_MemorySnapshot, end)
    if start["source"] != end["source"]:
        return "memory RSS source changed during an epoch"
    rss_delta = memory["current_rss_delta_bytes"]
    if start["available"] and end["available"]:
        start_current = cast(int, start["current_rss_bytes"])
        end_current = cast(int, end["current_rss_bytes"])
        expected_delta = end_current - start_current
        if not (_is_i64(rss_delta) and rss_delta == expected_delta):
            return "memory.current_rss_delta_bytes is not the signed endpoint delta"
        start_peak = cast(int, start["peak_rss_bytes"])
        end_peak = cast(int, end["peak_rss_bytes"])
        if end_peak < start_peak:
            return "memory peak RSS regressed during an epoch"
    elif rss_delta is not None:
        return "memory.current_rss_delta_bytes must be null when either endpoint is unavailable"
    return None


def is_profile_epoch(value: object) -> TypeGuard[RuntimeProfilePayload]:
    return validate_profile_epoch(value) is None

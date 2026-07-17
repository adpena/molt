from __future__ import annotations

from typing import Any, TypeGuard

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


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_u64(value: object) -> bool:
    return _is_int(value) and 0 <= value <= _U64_MAX


def _is_i64(value: object) -> bool:
    return _is_int(value) and _I64_MIN <= value <= _I64_MAX


def _validate_exact_keys(
    value: object, expected: frozenset[str] | set[str], path: str
) -> str | None:
    if not isinstance(value, dict):
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
    assert isinstance(value, dict)
    source = value["source"]
    available = value["available"]
    current = value["current_rss_bytes"]
    peak = value["peak_rss_bytes"]
    if source not in PROCESS_RSS_SOURCES:
        return f"{path}.source is not a canonical RSS source"
    if not isinstance(available, bool):
        return f"{path}.available must be bool"
    if available:
        if source in UNAVAILABLE_RSS_SOURCES:
            return f"{path}.available cannot be true for {source}"
        if not (_is_u64(current) and _is_u64(peak)):
            return f"{path} available RSS values must be u64"
        if peak < current:
            return f"{path}.peak_rss_bytes must be >= current_rss_bytes"
    elif current is not None or peak is not None:
        return f"{path} unavailable RSS values must be null"
    return None


def validate_process_profile(value: object) -> str | None:
    if error := _validate_exact_keys(value, _PROCESS_ROOT_KEYS, "root"):
        return error
    assert isinstance(value, dict)
    if value["schema_version"] != PROCESS_PROFILE_SCHEMA_VERSION:
        return f"schema_version must be {PROCESS_PROFILE_SCHEMA_VERSION}"
    if value["kind"] != PROCESS_PROFILE_KIND:
        return f"kind must be {PROCESS_PROFILE_KIND!r}"
    for section, keys in PROCESS_COUNTER_KEYS.items():
        payload = value[section]
        if error := _validate_exact_keys(payload, keys, section):
            return error
        assert isinstance(payload, dict)
        for key, counter in payload.items():
            if not _is_u64(counter):
                return f"{section}.{key} must be u64"
    if error := _validate_memory_snapshot(value["memory"], "memory"):
        return error

    profile = value["profile"]
    aux = value["aux"]
    gc = value["gc"]
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
    if not isinstance(value, dict):
        return "counter_regressions must be an object", set()
    observed: set[tuple[str, str]] = set()
    for section, regressions in value.items():
        if section not in PROFILE_DELTA_KEYS:
            return f"counter_regressions.{section} is not a counter section", set()
        if not isinstance(regressions, dict) or not regressions:
            return f"counter_regressions.{section} must be a non-empty object", set()
        unknown = set(regressions) - PROFILE_DELTA_KEYS[section]
        if unknown:
            return (
                f"counter_regressions.{section} has unknown keys {sorted(unknown)}",
                set(),
            )
        for metric, regression in regressions.items():
            if not isinstance(regression, dict) or set(regression) != {"start", "end"}:
                return (
                    f"counter_regressions.{section}.{metric} must contain start/end",
                    set(),
                )
            start = regression["start"]
            end = regression["end"]
            if not (_is_u64(start) and _is_u64(end) and end < start):
                return (
                    f"counter_regressions.{section}.{metric} is not a decrease",
                    set(),
                )
            observed.add((section, metric))
    return None, observed


def validate_profile_epoch(value: object) -> str | None:
    if error := _validate_exact_keys(value, _EPOCH_ROOT_KEYS, "root"):
        return error
    assert isinstance(value, dict)
    if value["schema_version"] != PROFILE_EPOCH_SCHEMA_VERSION:
        return f"schema_version must be {PROFILE_EPOCH_SCHEMA_VERSION}"
    if value["kind"] != PROFILE_EPOCH_KIND:
        return f"kind must be {PROFILE_EPOCH_KIND!r}"
    if not (_is_u64(value["generation"]) and value["generation"] > 0):
        return "generation must be a positive u64"
    if not (isinstance(value["label"], str) and value["label"]):
        return "label must be a non-empty string"
    if not isinstance(value["claimable"], bool):
        return "claimable must be bool"

    error, regressions = _validate_regressions(value["counter_regressions"])
    if error:
        return error
    if value["claimable"] != (not regressions):
        return "claimable must be false exactly when a counter regressed"

    delta = value["delta"]
    if error := _validate_exact_keys(delta, set(PROFILE_DELTA_KEYS), "delta"):
        return error
    assert isinstance(delta, dict)
    null_deltas: set[tuple[str, str]] = set()
    for section, keys in PROFILE_DELTA_KEYS.items():
        payload = delta[section]
        if error := _validate_exact_keys(payload, keys, f"delta.{section}"):
            return error
        assert isinstance(payload, dict)
        for metric, counter in payload.items():
            if counter is None:
                null_deltas.add((section, metric))
            elif not _is_u64(counter):
                return f"delta.{section}.{metric} must be u64 or null"
    if null_deltas != regressions:
        return "null deltas must correspond exactly to counter_regressions"

    gauges = value["gauges"]
    if error := _validate_exact_keys(gauges, set(PROFILE_GAUGE_KEYS), "gauges"):
        return error
    assert isinstance(gauges, dict)
    for section, keys in PROFILE_GAUGE_KEYS.items():
        payload = gauges[section]
        if error := _validate_exact_keys(payload, keys, f"gauges.{section}"):
            return error
        assert isinstance(payload, dict)
        for metric, gauge in payload.items():
            if not isinstance(gauge, dict) or set(gauge) != {"start", "end", "delta"}:
                return f"gauges.{section}.{metric} must contain start/end/delta"
            start, end, signed_delta = gauge["start"], gauge["end"], gauge["delta"]
            if not (
                _is_u64(start)
                and _is_u64(end)
                and _is_i64(signed_delta)
                and signed_delta == end - start
            ):
                return f"gauges.{section}.{metric} has an invalid signed delta"

    memory = value["memory"]
    if error := _validate_exact_keys(memory, _EPOCH_MEMORY_KEYS, "memory"):
        return error
    assert isinstance(memory, dict)
    if error := _validate_memory_snapshot(memory["start"], "memory.start"):
        return error
    if error := _validate_memory_snapshot(memory["end"], "memory.end"):
        return error
    start = memory["start"]
    end = memory["end"]
    assert isinstance(start, dict) and isinstance(end, dict)
    if start["source"] != end["source"]:
        return "memory RSS source changed during an epoch"
    rss_delta = memory["current_rss_delta_bytes"]
    if start["available"] and end["available"]:
        expected_delta = end["current_rss_bytes"] - start["current_rss_bytes"]
        if not (_is_i64(rss_delta) and rss_delta == expected_delta):
            return "memory.current_rss_delta_bytes is not the signed endpoint delta"
        if end["peak_rss_bytes"] < start["peak_rss_bytes"]:
            return "memory peak RSS regressed during an epoch"
    elif rss_delta is not None:
        return "memory.current_rss_delta_bytes must be null when either endpoint is unavailable"
    return None


def is_profile_epoch(value: object) -> TypeGuard[RuntimeProfilePayload]:
    return validate_profile_epoch(value) is None

from __future__ import annotations

from typing import Any

from molt._runtime_profile_schema import (
    PROCESS_COUNTER_KEYS,
    PROCESS_PROFILE_KIND,
    PROCESS_PROFILE_SCHEMA_VERSION,
    PROFILE_EPOCH_KIND,
    PROFILE_EPOCH_SCHEMA_VERSION,
    PROFILE_GAUGE_KEYS,
    PROFILE_DELTA_KEYS,
)


def memory_snapshot(
    *,
    source: str = "windows-process-memory-info",
    current_rss_bytes: int | None = 3_072,
    peak_rss_bytes: int | None = 4_096,
) -> dict[str, Any]:
    available = current_rss_bytes is not None and peak_rss_bytes is not None
    return {
        "source": source,
        "available": available,
        "current_rss_bytes": current_rss_bytes,
        "peak_rss_bytes": peak_rss_bytes,
    }


def process_profile_payload() -> dict[str, Any]:
    payload: dict[str, Any] = {
        "schema_version": PROCESS_PROFILE_SCHEMA_VERSION,
        "kind": PROCESS_PROFILE_KIND,
        "memory": memory_snapshot(),
    }
    payload.update(
        {
            section: {key: 0 for key in sorted(keys)}
            for section, keys in PROCESS_COUNTER_KEYS.items()
        }
    )
    return payload


def profile_epoch_payload() -> dict[str, Any]:
    return {
        "schema_version": PROFILE_EPOCH_SCHEMA_VERSION,
        "kind": PROFILE_EPOCH_KIND,
        "generation": 1,
        "label": "steady_state",
        "claimable": True,
        "delta": {
            section: {key: 0 for key in sorted(keys)}
            for section, keys in PROFILE_DELTA_KEYS.items()
        },
        "counter_regressions": {},
        "gauges": {
            section: {key: {"start": 0, "end": 0, "delta": 0} for key in sorted(keys)}
            for section, keys in PROFILE_GAUGE_KEYS.items()
        },
        "memory": {
            "start": memory_snapshot(),
            "end": memory_snapshot(),
            "current_rss_delta_bytes": 0,
        },
    }

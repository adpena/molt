from __future__ import annotations

import copy
import math

import pytest

from molt._runtime_profile_schema import is_process_profile, is_profile_epoch


def process_payload() -> dict[str, object]:
    return {
        "schema_version": 2,
        "kind": "runtime_feedback",
        "profile": {"alloc_count": 3},
        "aux": {"aux_sidecar_live_count": 0},
        "gc": {"gc_tracked_live": 1},
        "memory": {"peak_rss_bytes": 4096, "current_rss_bytes": 3072},
        "hot_paths": {"call_bind_ic_hit": 2},
        "deopt_reasons": {"guard_tag_type_mismatch": 0},
    }


def epoch_payload() -> dict[str, object]:
    return {
        "schema_version": 1,
        "kind": "runtime_profile_epoch",
        "generation": 7,
        "label": "steady_state",
        "delta": {"profile": {"alloc_count": 0}, "gc": {"gc_track_count": 2}},
        "counter_regressions": {"profile": {"dealloc_count": {"start": 8, "end": 7}}},
        "gauges": {"profile": {"live_objects": {"start": 5, "end": 4, "delta": -1}}},
        "memory": {
            "current_rss_start_bytes": 3072,
            "current_rss_end_bytes": 2048,
            "current_rss_delta_bytes": -1024,
            "process_peak_start_bytes": 4096,
            "process_peak_end_bytes": 8192,
        },
    }


def test_current_process_and_epoch_payloads_are_accepted() -> None:
    assert is_process_profile(process_payload())
    assert is_profile_epoch(epoch_payload())


@pytest.mark.parametrize(
    "poison", [True, -1, 1.5, math.nan, math.inf, "1", None, 1 << 64]
)
def test_process_profile_rejects_noncanonical_counter_values(poison: object) -> None:
    payload = process_payload()
    payload["profile"]["alloc_count"] = poison

    assert not is_process_profile(payload)


def test_process_profile_rejects_unknown_root_and_memory_fields() -> None:
    root_extra = process_payload()
    root_extra["future"] = {}
    memory_extra = process_payload()
    memory_extra["memory"]["future_rss_bytes"] = 0

    assert not is_process_profile(root_extra)
    assert not is_process_profile(memory_extra)


@pytest.mark.parametrize(
    ("path", "poison"),
    [
        (("generation",), 0),
        (("generation",), True),
        (("generation",), 1 << 64),
        (("label",), ""),
        (("delta", "profile", "alloc_count"), -1),
        (("delta", "profile", "alloc_count"), math.nan),
        (("delta", "profile", "alloc_count"), 1 << 64),
        (("gauges", "profile", "live_objects", "delta"), 0),
        (("gauges", "profile", "live_objects", "delta"), 1 << 63),
        (("memory", "current_rss_delta_bytes"), 0),
        (("memory", "current_rss_delta_bytes"), -(1 << 63) - 1),
        (("memory", "process_peak_end_bytes"), 1024),
        (("counter_regressions", "profile", "dealloc_count", "end"), 9),
    ],
)
def test_epoch_profile_rejects_malformed_nested_evidence(
    path: tuple[str, ...], poison: object
) -> None:
    payload = copy.deepcopy(epoch_payload())
    target = payload
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = poison

    assert not is_profile_epoch(payload)


def test_epoch_profile_rejects_unknown_root_and_memory_fields() -> None:
    root_extra = epoch_payload()
    root_extra["future"] = {}
    memory_extra = epoch_payload()
    memory_extra["memory"]["future_rss_bytes"] = 0

    assert not is_profile_epoch(root_extra)
    assert not is_profile_epoch(memory_extra)

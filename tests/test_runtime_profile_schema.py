from __future__ import annotations

import copy
import math

import pytest

from molt._runtime_profile_schema import is_process_profile, is_profile_epoch
from tests.runtime_profile_fixtures import (
    memory_snapshot,
    process_profile_payload,
    profile_epoch_payload,
)


def test_current_process_and_epoch_payloads_are_accepted() -> None:
    assert is_process_profile(process_profile_payload())
    assert is_profile_epoch(profile_epoch_payload())


@pytest.mark.parametrize(
    "poison", [True, -1, 1.5, math.nan, math.inf, "1", None, 1 << 64]
)
def test_process_profile_rejects_noncanonical_counter_values(poison: object) -> None:
    payload = process_profile_payload()
    payload["profile"]["alloc_count"] = poison

    assert not is_process_profile(payload)


def test_process_profile_rejects_truncated_and_unknown_counter_maps() -> None:
    truncated = process_profile_payload()
    truncated["profile"].pop("alloc_count")
    sparse = process_profile_payload()
    sparse["gc"] = {}
    extra = process_profile_payload()
    extra["hot_paths"]["future"] = 0

    assert not is_process_profile(truncated)
    assert not is_process_profile(sparse)
    assert not is_process_profile(extra)


def test_process_profile_requires_explicit_rss_availability() -> None:
    unavailable = process_profile_payload()
    unavailable["memory"] = memory_snapshot(
        source="unsupported-wasm",
        current_rss_bytes=None,
        peak_rss_bytes=None,
    )
    false_zero = copy.deepcopy(unavailable)
    false_zero["memory"]["current_rss_bytes"] = 0
    missing_source = copy.deepcopy(unavailable)
    missing_source["memory"].pop("source")

    assert is_process_profile(unavailable)
    assert not is_process_profile(false_zero)
    assert not is_process_profile(missing_source)


@pytest.mark.parametrize(
    "source",
    [
        "windows-process-memory-info",
        "proc-self-status",
        "mach-task-basic-info",
    ],
)
def test_process_profile_accepts_each_native_rss_source(source: str) -> None:
    payload = process_profile_payload()
    payload["memory"]["source"] = source

    assert is_process_profile(payload)


def test_epoch_counter_regression_is_explicitly_nonclaimable() -> None:
    payload = profile_epoch_payload()
    payload["claimable"] = False
    payload["delta"]["profile"]["dealloc_count"] = None
    payload["counter_regressions"] = {
        "profile": {"dealloc_count": {"start": 8, "end": 7}}
    }
    disguised_zero = copy.deepcopy(payload)
    disguised_zero["delta"]["profile"]["dealloc_count"] = 0
    false_claim = copy.deepcopy(payload)
    false_claim["claimable"] = True

    assert is_profile_epoch(payload)
    assert not is_profile_epoch(disguised_zero)
    assert not is_profile_epoch(false_claim)


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
        (("gauges", "profile", "live_objects", "delta"), 1),
        (("memory", "current_rss_delta_bytes"), 1),
        (("memory", "end", "peak_rss_bytes"), 1024),
    ],
)
def test_epoch_profile_rejects_malformed_nested_evidence(
    path: tuple[str, ...], poison: object
) -> None:
    payload = profile_epoch_payload()
    target = payload
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = poison

    assert not is_profile_epoch(payload)


def test_epoch_profile_rejects_truncated_sections_and_memory_fields() -> None:
    delta_truncated = profile_epoch_payload()
    delta_truncated["delta"]["profile"].pop("alloc_count")
    gauge_truncated = profile_epoch_payload()
    gauge_truncated["gauges"]["gc"].pop("gc_tracked_live")
    memory_extra = profile_epoch_payload()
    memory_extra["memory"]["future_rss_bytes"] = 0

    assert not is_profile_epoch(delta_truncated)
    assert not is_profile_epoch(gauge_truncated)
    assert not is_profile_epoch(memory_extra)


def test_epoch_unavailable_rss_has_null_delta() -> None:
    payload = profile_epoch_payload()
    unavailable = memory_snapshot(
        source="unsupported-wasm",
        current_rss_bytes=None,
        peak_rss_bytes=None,
    )
    payload["memory"] = {
        "start": unavailable,
        "end": copy.deepcopy(unavailable),
        "current_rss_delta_bytes": None,
    }
    false_zero = copy.deepcopy(payload)
    false_zero["memory"]["current_rss_delta_bytes"] = 0

    assert is_profile_epoch(payload)
    assert not is_profile_epoch(false_zero)

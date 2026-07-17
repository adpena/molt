from __future__ import annotations

import json
from pathlib import Path

from molt.debug.perf import (
    build_perf_summary_payload,
    extract_profile_from_log,
    extract_profile_epochs_from_log,
    flatten_counters,
    load_profile,
)


def test_extract_profile_from_log_reads_molt_profile_json() -> None:
    payload = {
        "schema_version": 2,
        "kind": "runtime_feedback",
        "profile": {"call_dispatch": 1},
        "aux": {},
        "gc": {},
        "memory": {"peak_rss_bytes": 0, "current_rss_bytes": 0},
        "hot_paths": {},
        "deopt_reasons": {},
    }
    profile = extract_profile_from_log(
        "noise\nmolt_profile_json " + json.dumps(payload)
    )
    assert profile == {
        "schema_version": 2,
        "kind": "runtime_feedback",
        "profile": {"call_dispatch": 1},
        "aux": {},
        "gc": {},
        "memory": {"peak_rss_bytes": 0, "current_rss_bytes": 0},
        "hot_paths": {},
        "deopt_reasons": {},
    }


def test_load_profile_accepts_json_or_log(tmp_path: Path) -> None:
    json_path = tmp_path / "profile.json"
    json_path.write_text(
        json.dumps(
            {
                "schema_version": 2,
                "kind": "runtime_feedback",
                "profile": {"call_dispatch": 1},
                "aux": {},
                "gc": {},
                "memory": {"peak_rss_bytes": 0, "current_rss_bytes": 0},
                "hot_paths": {"call_bind_ic_hit": 2},
                "deopt_reasons": {},
            }
        ),
        encoding="utf-8",
    )
    log_path = tmp_path / "profile.log"
    log_path.write_text(
        "molt_profile_json "
        + json.dumps(
            {
                "schema_version": 2,
                "kind": "runtime_feedback",
                "profile": {"call_dispatch": 3},
                "aux": {},
                "gc": {},
                "memory": {"peak_rss_bytes": 0, "current_rss_bytes": 0},
                "hot_paths": {"call_bind_ic_miss": 4},
                "deopt_reasons": {},
            }
        )
        + "\n",
        encoding="utf-8",
    )
    assert load_profile(json_path)["profile"]["call_dispatch"] == 1
    assert load_profile(log_path)["hot_paths"]["call_bind_ic_miss"] == 4


def test_debug_perf_rejects_legacy_payload_and_preserves_all_epochs(
    tmp_path: Path,
) -> None:
    legacy = tmp_path / "legacy.json"
    legacy.write_text('{"profile":{"alloc_count":1}}', encoding="utf-8")
    epochs = [
        {
            "schema_version": 1,
            "kind": "runtime_profile_epoch",
            "generation": generation,
            "label": label,
            "delta": {"profile": {"alloc_count": 0}},
            "counter_regressions": {},
            "gauges": {},
            "memory": {
                "current_rss_start_bytes": 0,
                "current_rss_end_bytes": 0,
                "current_rss_delta_bytes": 0,
                "process_peak_start_bytes": 0,
                "process_peak_end_bytes": 0,
            },
        }
        for generation, label in ((1, "cache_hits"), (2, "weakref_calls"))
    ]
    log = "\n".join("molt_profile_epoch_json " + json.dumps(epoch) for epoch in epochs)

    assert load_profile(legacy) is None
    assert extract_profile_epochs_from_log(log) == epochs
    assert extract_profile_from_log(log) == {"profile_epochs": epochs}


def test_flatten_counters_and_summary_payload_are_deterministic() -> None:
    profile_a = {
        "profile": {"call_dispatch": 7, "alloc_count": 10},
        "hot_paths": {"call_bind_ic_hit": 30, "call_bind_ic_miss": 5},
        "deopt_reasons": {"invoke_ffi_bridge_capability_denied": 2},
        "aux": {"aux_sidecar_live_count": 3},
        "gc": {"gc_tracked_live": 4},
        "memory": {"peak_rss_bytes": 5},
        "future_counter_family": {"specialized_hit": 6},
        "profile_epochs": [
            {
                "schema_version": 1,
                "kind": "runtime_profile_epoch",
                "label": "cache_hits",
                "delta": {"profile": {"alloc_count": 0}},
            }
        ],
    }
    profile_b = {
        "profile": {"call_dispatch": 9, "alloc_count": 20, "alloc_callargs": 8},
        "hot_paths": {"call_bind_ic_hit": 10, "call_bind_ic_miss": 15},
        "deopt_reasons": {},
    }
    flat = flatten_counters(profile_a)
    assert flat["call_dispatch"] == 7
    assert flat["call_bind_ic_hit"] == 30
    assert flat["invoke_ffi_bridge_capability_denied"] == 2
    assert flat["aux_sidecar_live_count"] == 3
    assert flat["gc_tracked_live"] == 4
    assert flat["peak_rss_bytes"] == 5
    assert flat["specialized_hit"] == 6

    payload = build_perf_summary_payload({"bench_a": profile_a, "bench_b": profile_b})
    assert payload["profile_count"] == 2
    assert payload["aggregate"]["hot_paths"]["call_bind_ic_hit"] == 40
    assert payload["aggregate"]["hot_paths"]["call_bind_ic_miss"] == 20
    assert payload["aggregate"]["allocations"]["alloc_count"] == 30
    assert payload["aggregate"]["allocations"]["alloc_callargs"] == 8
    assert payload["profile_epochs"] == {"bench_a": profile_a["profile_epochs"]}
    assert payload["recommendations"]

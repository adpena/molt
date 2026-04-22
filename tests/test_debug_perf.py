from __future__ import annotations

import json
from pathlib import Path

from molt.debug.perf import (
    build_perf_summary_payload,
    extract_profile_from_log,
    flatten_counters,
    load_profile,
)


def test_extract_profile_from_log_reads_molt_profile_json() -> None:
    profile = extract_profile_from_log(
        'noise\nmolt_profile_json {"profile": {"call_dispatch": 1}, "hot_paths": {}, "deopt_reasons": {}}\n'
    )
    assert profile == {
        "profile": {"call_dispatch": 1},
        "hot_paths": {},
        "deopt_reasons": {},
    }


def test_load_profile_accepts_json_or_log(tmp_path: Path) -> None:
    json_path = tmp_path / "profile.json"
    json_path.write_text(
        json.dumps(
            {
                "profile": {"call_dispatch": 1},
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
                "profile": {"call_dispatch": 3},
                "hot_paths": {"call_bind_ic_miss": 4},
                "deopt_reasons": {},
            }
        )
        + "\n",
        encoding="utf-8",
    )
    assert load_profile(json_path)["profile"]["call_dispatch"] == 1
    assert load_profile(log_path)["hot_paths"]["call_bind_ic_miss"] == 4


def test_flatten_counters_and_summary_payload_are_deterministic() -> None:
    profile_a = {
        "profile": {"call_dispatch": 7, "alloc_count": 10},
        "hot_paths": {"call_bind_ic_hit": 30, "call_bind_ic_miss": 5},
        "deopt_reasons": {"invoke_ffi_bridge_capability_denied": 2},
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

    payload = build_perf_summary_payload({"bench_a": profile_a, "bench_b": profile_b})
    assert payload["profile_count"] == 2
    assert payload["aggregate"]["hot_paths"]["call_bind_ic_hit"] == 40
    assert payload["aggregate"]["hot_paths"]["call_bind_ic_miss"] == 20
    assert payload["aggregate"]["allocations"]["alloc_count"] == 30
    assert payload["aggregate"]["allocations"]["alloc_callargs"] == 8
    assert payload["recommendations"]

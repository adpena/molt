from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
PROFILE_TOOL_PATH = REPO_ROOT / "tools" / "profile.py"
PROFILE_TOOL_SPEC = importlib.util.spec_from_file_location(
    "profile_tool_under_test", PROFILE_TOOL_PATH
)
assert PROFILE_TOOL_SPEC is not None and PROFILE_TOOL_SPEC.loader is not None
profile_tool = importlib.util.module_from_spec(PROFILE_TOOL_SPEC)
sys.modules[PROFILE_TOOL_SPEC.name] = profile_tool
PROFILE_TOOL_SPEC.loader.exec_module(profile_tool)


def test_profile_tool_parses_runtime_json_profile() -> None:
    payload = {
        "schema_version": 2,
        "kind": "runtime_feedback",
        "profile": {
            "call_dispatch": 7,
            "alloc_count": 11,
            "alloc_string": 3,
            "alloc_tuple": 2,
            "alloc_dict": 1,
        },
        "hot_paths": {"dict_str_int_prehash_hit": 5},
        "deopt_reasons": {"guard_tag_type_mismatch": 4},
        "aux": {"aux_sidecar_live_count": 6},
        "gc": {"gc_tracked_live": 8},
        "memory": {"peak_rss_bytes": 1234, "current_rss_bytes": 999},
    }
    log_text = "noise\nmolt_profile_json " + json.dumps(payload) + "\n"

    profile = profile_tool._parse_molt_profile_json(log_text)

    assert profile == {
        "call_dispatch": 7,
        "alloc_count": 11,
        "alloc_string": 3,
        "alloc_tuple": 2,
        "alloc_dict": 1,
        "dict_str_int_prehash_hit": 5,
        "guard_tag_type_mismatch": 4,
        "aux_sidecar_live_count": 6,
        "gc_tracked_live": 8,
        "peak_rss_bytes": 1234,
        "current_rss_bytes": 999,
        "string_allocs": 3,
        "tuple_allocs": 2,
        "dict_allocs": 1,
    }


def test_profile_tool_uses_versioned_json_profile_authority(tmp_path: Path) -> None:
    log_path = tmp_path / "run.log"
    log_path.write_text(
        "\n".join(
            [
                "unrelated diagnostic noise",
                "molt_profile_json "
                + json.dumps(
                    {
                        "schema_version": 2,
                        "kind": "runtime_feedback",
                        "profile": {"call_dispatch": 9, "alloc_count": 10},
                        "aux": {},
                        "gc": {},
                        "memory": {"peak_rss_bytes": 0, "current_rss_bytes": 0},
                        "hot_paths": {"call_bind_ic_hit": 8},
                        "deopt_reasons": {},
                    }
                ),
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    metrics = profile_tool._merge_profile_metrics({}, log_path, True)

    assert metrics["molt_profile"]["call_dispatch"] == 9
    assert metrics["molt_profile"]["alloc_count"] == 10
    assert metrics["molt_profile"]["call_bind_ic_hit"] == 8


def test_profile_tool_rejects_unversioned_process_profile() -> None:
    assert (
        profile_tool._parse_molt_profile_json(
            'molt_profile_json {"profile":{"alloc_count":1}}'
        )
        is None
    )


def test_profile_tool_preserves_labeled_epoch_delta_sections() -> None:
    payload = {
        "schema_version": 1,
        "kind": "runtime_profile_epoch",
        "generation": 4,
        "label": "cache_hits",
        "delta": {
            "profile": {"alloc_count": 0, "alloc_tuple": 0},
            "gc": {"gc_registry_lock_contention_count": 2},
        },
        "counter_regressions": {"profile": {"dealloc_count": {"start": 8, "end": 7}}},
        "gauges": {"profile": {"live_objects": {"start": 10, "end": 10, "delta": 0}}},
        "memory": {
            "current_rss_start_bytes": 0,
            "current_rss_end_bytes": 0,
            "current_rss_delta_bytes": 0,
            "process_peak_start_bytes": 0,
            "process_peak_end_bytes": 0,
        },
    }

    epochs = profile_tool._parse_molt_profile_epoch_json(
        "molt_profile_epoch_json " + json.dumps(payload)
    )

    assert epochs is not None
    assert epochs[0]["label"] == "cache_hits"
    assert epochs[0]["delta"]["tuple_allocs"] == 0
    assert epochs[0]["delta_sections"] == payload["delta"]
    assert epochs[0]["counter_regressions"] == payload["counter_regressions"]


def test_profile_tool_auto_falls_back_to_portable_wall_backend(monkeypatch) -> None:
    monkeypatch.setattr(profile_tool.shutil, "which", lambda _name: None)
    monkeypatch.setattr(profile_tool, "_time_binary_optional", lambda: None)

    assert profile_tool._pick_cpu_tool("auto") == "wall"
    assert profile_tool._pick_alloc_tool("auto") == "wall"


def test_profile_tool_auto_keeps_gnu_time_when_available(monkeypatch) -> None:
    monkeypatch.setattr(profile_tool.shutil, "which", lambda _name: None)
    monkeypatch.setattr(profile_tool, "_time_binary_optional", lambda: "/usr/bin/time")

    assert profile_tool._pick_cpu_tool("auto") == "time"
    assert profile_tool._pick_alloc_tool("auto") == "time"


def test_profile_summary_aggregates_all_runs_and_process_tree_rss() -> None:
    base_profile = {
        "call_dispatch": 10,
        "alloc_count": 100,
        "alloc_bytes_total": 3_200,
        "dealloc_count": 90,
        "dealloc_bytes_total": 2_880,
        "live_objects": 10,
        "live_bytes": 320,
        "alloc_exception": 20,
        "dealloc_exception": 18,
        "live_exception": 2,
        "alloc_bytes_exception": 2_000,
        "dealloc_bytes_exception": 1_800,
        "live_bytes_exception": 200,
        "aux_sidecar_alloc_count": 4,
        "aux_sidecar_free_count": 3,
        "aux_sidecar_live_count": 1,
        "aux_sidecar_alloc_failure_count": 0,
        "aux_sidecar_alloc_bytes": 128,
        "aux_sidecar_free_bytes": 96,
        "aux_sidecar_live_bytes": 32,
        "gc_track_count": 30,
        "gc_untrack_count": 25,
        "gc_tracked_live": 5,
        "gc_tracked_high_water": 7,
        "gc_registry_lock_contention_count": 3,
        "gc_registry_lock_wait_ns": 90,
        "gc_snapshot_alloc_failure_count": 0,
    }
    metadata = {
        "benchmarks": [
            {
                "bench": "bench_example.py",
                "cpu_runs": [
                    {
                        "metrics": {
                            "peak_rss_bytes_external": 1_000,
                            "molt_profile": {**base_profile, "peak_rss_bytes": 800},
                            "molt_profile_epochs": [
                                {
                                    "label": "steady_state",
                                    "delta": {"alloc_count": 0, "alloc_bytes_total": 0},
                                    "gauges": {},
                                    "memory": {"current_rss_delta_bytes": 0},
                                }
                            ],
                        }
                    },
                    {
                        "metrics": {
                            "peak_rss_bytes_external": 1_200,
                            "molt_profile": {**base_profile, "peak_rss_bytes": 900},
                            "molt_profile_epochs": [
                                {
                                    "label": "steady_state",
                                    "delta": {"alloc_count": 0, "alloc_bytes_total": 0},
                                    "gauges": {},
                                    "memory": {"current_rss_delta_bytes": 0},
                                }
                            ],
                        }
                    },
                ],
            }
        ]
    }

    summary = profile_tool._collect_profile_summary(metadata, top_n=5)
    bench = summary["benchmarks"][0]

    assert bench["counter_runs"] == 2
    assert bench["counter_drift"] == {}
    assert bench["alloc_bytes_total"] == 3_200
    assert bench["dealloc_bytes_total"] == 2_880
    assert bench["exception_live_bytes"] == 200
    assert bench["aux_sidecar_live_bytes"] == 32
    assert bench["gc_contention_per_track"] == 0.1
    assert bench["gc_wait_ns_per_contention"] == 30.0
    assert bench["runtime_peak_rss_bytes_median"] == 850
    assert bench["runtime_peak_rss_bytes_max"] == 900
    assert bench["external_peak_rss_bytes_median"] == 1_100
    assert bench["external_peak_rss_bytes_max"] == 1_200
    assert bench["epochs"][0]["label"] == "steady_state"
    assert bench["epochs"][0]["delta_median"] == {
        "alloc_bytes_total": 0,
        "alloc_count": 0,
    }


def test_profile_summary_keeps_completed_epoch_without_exit_payload() -> None:
    metadata = {
        "benchmarks": [
            {
                "bench": "interrupted.py",
                "cpu_runs": [
                    {
                        "metrics": {
                            "molt_profile_epochs": [
                                {
                                    "label": "completed_phase",
                                    "delta": {"alloc_count": 0},
                                    "gauges": {},
                                    "memory": {"current_rss_delta_bytes": 0},
                                }
                            ]
                        }
                    }
                ],
            }
        ]
    }

    summary = profile_tool._collect_profile_summary(metadata, top_n=5)

    assert summary["missing_profile"] == ["interrupted.py"]
    bench = summary["benchmarks"][0]
    assert bench["counter_runs"] == 0
    assert bench["alloc_count"] is None
    assert bench["runtime_peak_rss_bytes_median"] is None
    assert bench["epochs"][0]["label"] == "completed_phase"

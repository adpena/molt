from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

from tests.runtime_profile_fixtures import (
    memory_snapshot,
    process_profile_payload,
    profile_epoch_payload,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
PROFILE_TOOL_PATH = REPO_ROOT / "tools" / "profile.py"
PROFILE_TOOL_SPEC = importlib.util.spec_from_file_location(
    "profile_tool_under_test", PROFILE_TOOL_PATH
)
assert PROFILE_TOOL_SPEC is not None and PROFILE_TOOL_SPEC.loader is not None
profile_tool = importlib.util.module_from_spec(PROFILE_TOOL_SPEC)
sys.modules[PROFILE_TOOL_SPEC.name] = profile_tool
PROFILE_TOOL_SPEC.loader.exec_module(profile_tool)


def _profile_tool_process_fixture() -> dict[str, object]:
    payload = process_profile_payload()
    payload["profile"].update(
        {"call_dispatch": 9, "alloc_count": 10, "dealloc_count": 10}
    )
    payload["hot_paths"]["call_bind_ic_hit"] = 8
    return payload


def test_profile_tool_parses_runtime_json_profile() -> None:
    payload = process_profile_payload()
    payload["profile"].update(
        {
            "call_dispatch": 7,
            "alloc_count": 11,
            "dealloc_count": 11,
            "alloc_string": 3,
            "alloc_tuple": 2,
            "alloc_dict": 1,
        }
    )
    payload["hot_paths"]["dict_str_int_prehash_hit"] = 5
    payload["deopt_reasons"]["guard_tag_type_mismatch"] = 4
    payload["memory"].update({"peak_rss_bytes": 1234, "current_rss_bytes": 999})
    log_text = "noise\nmolt_profile_json " + json.dumps(payload) + "\n"

    profile = profile_tool._parse_molt_profile_json(log_text)

    assert profile is not None
    assert profile["call_dispatch"] == 7
    assert profile["alloc_count"] == 11
    assert profile["dict_str_int_prehash_hit"] == 5
    assert profile["memory"] == payload["memory"]
    assert profile["string_allocs"] == 3
    assert profile["tuple_allocs"] == 2
    assert profile["dict_allocs"] == 1


def test_profile_tool_uses_versioned_json_profile_authority(tmp_path: Path) -> None:
    log_path = tmp_path / "run.log"
    log_path.write_text(
        "\n".join(
            [
                "unrelated diagnostic noise",
                "molt_profile_json " + json.dumps(_profile_tool_process_fixture()),
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
    payload = profile_epoch_payload()
    payload["generation"] = 4
    payload["label"] = "cache_hits"
    payload["delta"]["gc"]["gc_registry_lock_contention_count"] = 2

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
                            "molt_profile": {
                                **base_profile,
                                "memory": memory_snapshot(
                                    current_rss_bytes=700, peak_rss_bytes=800
                                ),
                            },
                            "molt_profile_epochs": [
                                {
                                    "label": "steady_state",
                                    "claimable": True,
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
                            "molt_profile": {
                                **base_profile,
                                "memory": memory_snapshot(
                                    current_rss_bytes=750, peak_rss_bytes=900
                                ),
                            },
                            "molt_profile_epochs": [
                                {
                                    "label": "steady_state",
                                    "claimable": True,
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
    assert bench["runtime_rss_sources"] == ["windows-process-memory-info"]
    assert bench["runtime_rss_unavailable_runs"] == 0
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
                                    "claimable": True,
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


def test_profile_summary_never_uses_regressed_epoch_for_zero_work_claim() -> None:
    metadata = {
        "benchmarks": [
            {
                "bench": "regressed.py",
                "cpu_runs": [
                    {
                        "metrics": {
                            "molt_profile_epochs": [
                                {
                                    "label": "steady_state",
                                    "claimable": False,
                                    "delta": {"alloc_count": 0},
                                    "counter_regressions": {
                                        "profile": {
                                            "alloc_count": {"start": 9, "end": 1}
                                        }
                                    },
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
    epoch = summary["benchmarks"][0]["epochs"][0]
    assert epoch["counter_runs"] == 0
    assert epoch["unclaimable_runs"] == 1
    assert epoch["delta_median"] == {}
    assert epoch["counter_regressions"]


def test_profile_summary_keeps_unavailable_runtime_rss_out_of_samples() -> None:
    unavailable = memory_snapshot(
        source="unsupported-wasm",
        current_rss_bytes=None,
        peak_rss_bytes=None,
    )
    metadata = {
        "benchmarks": [
            {
                "bench": "wasm.py",
                "cpu_runs": [{"metrics": {"molt_profile": {"memory": unavailable}}}],
            }
        ]
    }

    bench = profile_tool._collect_profile_summary(metadata, top_n=5)["benchmarks"][0]
    assert bench["runtime_peak_rss_bytes_median"] is None
    assert bench["runtime_peak_rss_bytes_max"] is None
    assert bench["runtime_rss_sources"] == ["unsupported-wasm"]
    assert bench["runtime_rss_unavailable_runs"] == 1

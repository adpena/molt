from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "check_runtime_feedback.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("check_runtime_feedback", SCRIPT_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _base_payload() -> dict:
    return {
        "schema_version": 2,
        "kind": "runtime_feedback",
        "profile": {
            "call_dispatch": 0,
            "attr_lookup": 0,
            "layout_guard": 0,
            "layout_guard_fail": 0,
            "alloc_count": 0,
            "alloc_exception": 0,
            "alloc_bytes_total": 0,
            "alloc_bytes_exception": 0,
            "dealloc_count": 0,
            "dealloc_bytes_total": 0,
            "dealloc_exception": 0,
            "dealloc_bytes_exception": 0,
            "live_objects": 0,
            "live_bytes": 0,
            "live_exception": 0,
            "live_bytes_exception": 0,
            "expected_live": 200_000,
            "async_polls": 0,
        },
        "aux": {
            "aux_class_inline_count": 0,
            "aux_state_inline_count": 0,
            "aux_sidecar_alloc_count": 0,
            "aux_sidecar_free_count": 0,
            "aux_sidecar_live_count": 0,
            "aux_sidecar_alloc_failure_count": 0,
            "aux_sidecar_alloc_bytes": 0,
            "aux_sidecar_free_bytes": 0,
            "aux_sidecar_live_bytes": 0,
        },
        "gc": {
            "gc_track_count": 0,
            "gc_untrack_count": 0,
            "gc_tracked_live": 0,
            "gc_tracked_high_water": 0,
            "gc_registry_lock_contention_count": 0,
            "gc_registry_lock_wait_ns": 0,
            "gc_snapshot_alloc_failure_count": 0,
        },
        "memory": {"peak_rss_bytes": 0, "current_rss_bytes": 0},
        "hot_paths": {
            "call_bind_ic_hit": 0,
            "call_bind_ic_miss": 0,
            "split_ws_ascii": 0,
            "split_ws_unicode": 0,
            "dict_str_int_prehash_deopt": 0,
            "taq_ingest_calls": 0,
        },
        "deopt_reasons": {
            "call_indirect_noncallable": 0,
            "invoke_ffi_bridge_capability_denied": 0,
            "guard_tag_type_mismatch": 0,
            "guard_dict_shape_layout_mismatch": 0,
            "guard_dict_shape_layout_fail_null_obj": 0,
            "guard_dict_shape_layout_fail_non_object": 0,
            "guard_dict_shape_layout_fail_class_mismatch": 0,
            "guard_dict_shape_layout_fail_non_type_class": 0,
            "guard_dict_shape_layout_fail_expected_version_invalid": 0,
            "guard_dict_shape_layout_fail_version_mismatch": 0,
        },
    }


def test_runtime_feedback_validator_accepts_deopt_reason_schema(tmp_path: Path) -> None:
    module = _load_module()
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(_base_payload()), encoding="utf-8")

    assert module._validate(path) == 0


def test_runtime_feedback_validator_requires_deopt_reasons(tmp_path: Path) -> None:
    module = _load_module()
    payload = _base_payload()
    payload.pop("deopt_reasons")
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(payload), encoding="utf-8")

    assert module._validate(path) == 1


def test_runtime_feedback_validator_rejects_lifetime_identity_drift(
    tmp_path: Path,
) -> None:
    module = _load_module()
    payload = _base_payload()
    payload["aux"]["aux_sidecar_alloc_count"] = 2
    payload["aux"]["aux_sidecar_live_count"] = 1
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(payload), encoding="utf-8")

    assert module._validate(path) == 1


def test_runtime_feedback_validator_rejects_negative_deopt_counts(
    tmp_path: Path,
) -> None:
    module = _load_module()
    payload = _base_payload()
    payload["deopt_reasons"]["call_indirect_noncallable"] = -1
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(payload), encoding="utf-8")

    assert module._validate(path) == 1


def test_runtime_feedback_validator_requires_guard_deopt_keys(tmp_path: Path) -> None:
    module = _load_module()
    payload = _base_payload()
    payload["deopt_reasons"].pop("guard_dict_shape_layout_fail_non_object")
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(payload), encoding="utf-8")

    assert module._validate(path) == 1


def test_runtime_feedback_validator_accepts_hot_function_entries(
    tmp_path: Path,
) -> None:
    module = _load_module()
    payload = _base_payload()
    payload["hot_functions"] = [
        {"symbol": "pkg.mod::hot_fn", "count": 9},
        ["pkg.mod::warm_fn", 4],
        "pkg.mod::cold_fn",
    ]
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(payload), encoding="utf-8")

    assert module._validate(path) == 0


def test_runtime_feedback_validator_rejects_invalid_hot_function_entries(
    tmp_path: Path,
) -> None:
    module = _load_module()
    payload = _base_payload()
    payload["hot_functions"] = [{"score": "bad"}]
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(payload), encoding="utf-8")

    assert module._validate(path) == 1

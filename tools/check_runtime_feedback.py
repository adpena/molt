#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


REQUIRED_TOP_LEVEL = {
    "schema_version",
    "kind",
    "profile",
    "hot_paths",
    "deopt_reasons",
    "aux",
    "gc",
    "memory",
}
REQUIRED_PROFILE_KEYS = {
    "call_dispatch",
    "attr_lookup",
    "layout_guard",
    "layout_guard_fail",
    "alloc_count",
    "alloc_exception",
    "alloc_bytes_total",
    "async_polls",
    "alloc_bytes_exception",
    "dealloc_count",
    "dealloc_bytes_total",
    "dealloc_exception",
    "dealloc_bytes_exception",
    "live_objects",
    "live_bytes",
    "live_exception",
    "live_bytes_exception",
    "expected_live",
}
REQUIRED_AUX_KEYS = {
    "aux_class_inline_count",
    "aux_state_inline_count",
    "aux_sidecar_alloc_count",
    "aux_sidecar_free_count",
    "aux_sidecar_live_count",
    "aux_sidecar_alloc_failure_count",
    "aux_sidecar_alloc_bytes",
    "aux_sidecar_free_bytes",
    "aux_sidecar_live_bytes",
}
REQUIRED_GC_KEYS = {
    "gc_track_count",
    "gc_untrack_count",
    "gc_tracked_live",
    "gc_tracked_high_water",
    "gc_registry_lock_contention_count",
    "gc_registry_lock_wait_ns",
    "gc_snapshot_alloc_failure_count",
}
REQUIRED_MEMORY_KEYS = {"peak_rss_bytes", "current_rss_bytes"}
REQUIRED_HOT_PATH_KEYS = {
    "call_bind_ic_hit",
    "call_bind_ic_miss",
    "split_ws_ascii",
    "split_ws_unicode",
    "dict_str_int_prehash_deopt",
    "taq_ingest_calls",
}
REQUIRED_DEOPT_REASON_KEYS = {
    "call_indirect_noncallable",
    "invoke_ffi_bridge_capability_denied",
    "guard_tag_type_mismatch",
    "guard_dict_shape_layout_mismatch",
    "guard_dict_shape_layout_fail_null_obj",
    "guard_dict_shape_layout_fail_non_object",
    "guard_dict_shape_layout_fail_class_mismatch",
    "guard_dict_shape_layout_fail_non_type_class",
    "guard_dict_shape_layout_fail_expected_version_invalid",
    "guard_dict_shape_layout_fail_version_mismatch",
}


def _validate_hot_functions(payload: dict) -> str | None:
    hot_functions = payload.get("hot_functions")
    if hot_functions is None:
        return None
    if isinstance(hot_functions, dict):
        for name, score in hot_functions.items():
            if not isinstance(name, str) or not name:
                return "hot_functions object keys must be non-empty strings"
            if score is not None and not isinstance(score, (int, float)):
                return "hot_functions object values must be numeric or null"
        return None
    if isinstance(hot_functions, list):
        for entry in hot_functions:
            if isinstance(entry, str):
                if not entry:
                    return "hot_functions string entries must be non-empty"
                continue
            if isinstance(entry, (list, tuple)):
                if not entry or not isinstance(entry[0], str) or not entry[0]:
                    return "hot_functions tuple entries must start with a function name"
                if (
                    len(entry) > 1
                    and entry[1] is not None
                    and not isinstance(entry[1], (int, float))
                ):
                    return "hot_functions tuple scores must be numeric or null"
                continue
            if isinstance(entry, dict):
                name = (
                    entry.get("symbol")
                    or entry.get("name")
                    or entry.get("func")
                    or entry.get("function")
                )
                if not isinstance(name, str) or not name:
                    return "hot_functions object entries require a function name"
                score = None
                for key in ("score", "count", "time_ms", "time_us"):
                    if key in entry:
                        score = entry.get(key)
                        break
                if score is not None and not isinstance(score, (int, float)):
                    return "hot_functions entry scores must be numeric or null"
                continue
            return "hot_functions entries must be strings, pairs, or objects"
        return None
    return "hot_functions must be a list or object when present"


def _validate_non_negative_ints(
    section_name: str, payload: dict, keys: set[str]
) -> str | None:
    for key in keys:
        value = payload.get(key)
        if not isinstance(value, int):
            return f"{section_name}.{key} must be an integer"
        if value < 0:
            return f"{section_name}.{key} must be >= 0"
    return None


def _fail(msg: str) -> int:
    print(f"runtime-feedback-check: FAIL: {msg}", file=sys.stderr)
    return 1


def _validate(path: Path) -> int:
    if not path.exists():
        return _fail(f"missing file: {path}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001
        return _fail(f"invalid JSON: {exc}")

    missing_top = REQUIRED_TOP_LEVEL - set(payload.keys())
    if missing_top:
        return _fail(f"missing top-level keys: {sorted(missing_top)}")

    if payload.get("kind") != "runtime_feedback":
        return _fail(f"unexpected kind={payload.get('kind')!r}")
    if payload.get("schema_version") != 2:
        return _fail(f"unexpected schema_version={payload.get('schema_version')!r}")

    profile = payload.get("profile")
    if not isinstance(profile, dict):
        return _fail("profile must be an object")
    missing_profile = REQUIRED_PROFILE_KEYS - set(profile.keys())
    if missing_profile:
        return _fail(f"missing profile keys: {sorted(missing_profile)}")
    profile_value_err = _validate_non_negative_ints(
        "profile", profile, REQUIRED_PROFILE_KEYS
    )
    if profile_value_err:
        return _fail(profile_value_err)

    for section_name, required_keys in (
        ("aux", REQUIRED_AUX_KEYS),
        ("gc", REQUIRED_GC_KEYS),
        ("memory", REQUIRED_MEMORY_KEYS),
    ):
        section = payload.get(section_name)
        if not isinstance(section, dict):
            return _fail(f"{section_name} must be an object")
        missing = required_keys - set(section.keys())
        if missing:
            return _fail(f"missing {section_name} keys: {sorted(missing)}")
        value_err = _validate_non_negative_ints(section_name, section, required_keys)
        if value_err:
            return _fail(value_err)

    identities = (
        (
            "profile.live_objects",
            profile["live_objects"],
            max(profile["alloc_count"] - profile["dealloc_count"], 0),
        ),
        (
            "profile.live_bytes",
            profile["live_bytes"],
            max(profile["alloc_bytes_total"] - profile["dealloc_bytes_total"], 0),
        ),
        (
            "profile.live_exception",
            profile["live_exception"],
            max(profile["alloc_exception"] - profile["dealloc_exception"], 0),
        ),
        (
            "profile.live_bytes_exception",
            profile["live_bytes_exception"],
            max(
                profile["alloc_bytes_exception"] - profile["dealloc_bytes_exception"],
                0,
            ),
        ),
        (
            "aux.aux_sidecar_live_count",
            payload["aux"]["aux_sidecar_live_count"],
            max(
                payload["aux"]["aux_sidecar_alloc_count"]
                - payload["aux"]["aux_sidecar_free_count"],
                0,
            ),
        ),
        (
            "aux.aux_sidecar_live_bytes",
            payload["aux"]["aux_sidecar_live_bytes"],
            max(
                payload["aux"]["aux_sidecar_alloc_bytes"]
                - payload["aux"]["aux_sidecar_free_bytes"],
                0,
            ),
        ),
        (
            "gc.gc_tracked_live",
            payload["gc"]["gc_tracked_live"],
            max(
                payload["gc"]["gc_track_count"] - payload["gc"]["gc_untrack_count"],
                0,
            ),
        ),
    )
    for field, actual, expected in identities:
        if actual != expected:
            return _fail(f"{field}={actual} violates derived value {expected}")
    if payload["gc"]["gc_tracked_high_water"] < payload["gc"]["gc_tracked_live"]:
        return _fail("gc.gc_tracked_high_water must be >= gc.gc_tracked_live")

    hot_paths = payload.get("hot_paths")
    if not isinstance(hot_paths, dict):
        return _fail("hot_paths must be an object")
    missing_hot = REQUIRED_HOT_PATH_KEYS - set(hot_paths.keys())
    if missing_hot:
        return _fail(f"missing hot_paths keys: {sorted(missing_hot)}")
    hot_value_err = _validate_non_negative_ints(
        "hot_paths", hot_paths, REQUIRED_HOT_PATH_KEYS
    )
    if hot_value_err:
        return _fail(hot_value_err)

    deopt_reasons = payload.get("deopt_reasons")
    if not isinstance(deopt_reasons, dict):
        return _fail("deopt_reasons must be an object")
    missing_deopt = REQUIRED_DEOPT_REASON_KEYS - set(deopt_reasons.keys())
    if missing_deopt:
        return _fail(f"missing deopt_reasons keys: {sorted(missing_deopt)}")
    deopt_value_err = _validate_non_negative_ints(
        "deopt_reasons", deopt_reasons, REQUIRED_DEOPT_REASON_KEYS
    )
    if deopt_value_err:
        return _fail(deopt_value_err)
    hot_function_err = _validate_hot_functions(payload)
    if hot_function_err:
        return _fail(hot_function_err)

    print(f"runtime-feedback-check: OK: {path}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Validate Molt runtime feedback JSON schema."
    )
    parser.add_argument("path", help="Path to molt_runtime_feedback.json artifact")
    args = parser.parse_args()
    return _validate(Path(args.path))


if __name__ == "__main__":
    raise SystemExit(main())

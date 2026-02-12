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
}
REQUIRED_PROFILE_KEYS = {
    "call_dispatch",
    "attr_lookup",
    "layout_guard",
    "layout_guard_fail",
    "alloc_count",
    "async_polls",
}
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

from __future__ import annotations

import json
from pathlib import Path

from tests.runtime_profile_fixtures import process_profile_payload
from tools import check_runtime_feedback


def _write(tmp_path: Path, payload: dict) -> Path:
    path = tmp_path / "molt_runtime_feedback.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def test_runtime_feedback_validator_accepts_exact_schema(tmp_path: Path) -> None:
    assert (
        check_runtime_feedback._validate(_write(tmp_path, process_profile_payload()))
        == 0
    )


def test_runtime_feedback_validator_rejects_truncated_schema(tmp_path: Path) -> None:
    payload = process_profile_payload()
    payload["deopt_reasons"].pop("guard_dict_shape_layout_fail_non_object")

    assert check_runtime_feedback._validate(_write(tmp_path, payload)) == 1


def test_runtime_feedback_validator_rejects_lifetime_identity_drift(
    tmp_path: Path,
) -> None:
    payload = process_profile_payload()
    payload["aux"]["aux_sidecar_alloc_count"] = 2
    payload["aux"]["aux_sidecar_live_count"] = 1

    assert check_runtime_feedback._validate(_write(tmp_path, payload)) == 1


def test_runtime_feedback_validator_rejects_negative_counts(tmp_path: Path) -> None:
    payload = process_profile_payload()
    payload["deopt_reasons"]["call_indirect_noncallable"] = -1

    assert check_runtime_feedback._validate(_write(tmp_path, payload)) == 1


def test_runtime_feedback_validator_rejects_false_unavailable_rss(
    tmp_path: Path,
) -> None:
    payload = process_profile_payload()
    payload["memory"] = {
        "source": "unsupported-wasm",
        "available": False,
        "current_rss_bytes": 0,
        "peak_rss_bytes": 0,
    }

    assert check_runtime_feedback._validate(_write(tmp_path, payload)) == 1

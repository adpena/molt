"""The differential result log reader and its outcome classes."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from tools.compat import diff_results


def _test(path: str, resolved: str, **extra: object) -> dict[str, object]:
    return {
        "record_type": "test",
        "file": path,
        "raw_status": extra.pop("raw", resolved),
        "resolved_status": resolved,
        "reason_tag": extra.pop("tag", None),
        "duration_s": extra.pop("duration_s", 1.0),
        **extra,
    }


def _backend(path: str, **facts: object) -> dict[str, object]:
    return {"record_type": "backend", "file": path, "raw_status": "fail", **facts}


def test_outcomes_name_why_each_test_did_not_pass() -> None:
    rows = [
        _test("pass.py", "pass"),
        _test("xfail.py", "pass", raw="fail", tag="xfail"),
        _backend("mismatch.py", detail="stdout differs"),
        _test("mismatch.py", "fail"),
        _backend("build.py", build_failed=True, detail="link failed"),
        _test("build.py", "fail"),
        _backend("slow.py", timed_out=True),
        _test("slow.py", "fail"),
        _test("xpass.py", "fail", raw="pass", tag="xpass"),
        _test("oom.py", "oom"),
        _test("infra.py", "uncalibrated", tag="infrastructure_error"),
        _test("skip.py", "skip", exclusion_reason="platform"),
    ]
    got = {
        path: (outcome.status, outcome.reason, outcome.detail)
        for path, outcome in diff_results.outcomes(rows).items()
    }
    assert got == {
        "pass.py": ("passed", "", ""),
        "xfail.py": ("passed", "", ""),
        "mismatch.py": ("failed", "mismatch", "stdout differs"),
        "build.py": ("failed", "build", "link failed"),
        "slow.py": ("failed", "timeout", ""),
        "xpass.py": ("failed", "xpass", ""),
        "oom.py": ("errors", "resources", ""),
        "infra.py": ("errors", "infrastructure", ""),
        "skip.py": ("skipped", "platform", ""),
    }


def test_a_retried_test_keeps_its_last_outcome() -> None:
    rows = [_test("retry.py", "oom"), _test("retry.py", "pass", duration_s=2.0)]
    outcome = diff_results.outcomes(rows)["retry.py"]
    assert (outcome.status, outcome.duration_s) == ("passed", 2.0)


def test_load_rows_refuses_a_malformed_line(tmp_path: Path) -> None:
    log = tmp_path / "results.jsonl"
    log.write_text(
        json.dumps(_test("a.py", "pass")) + "\n\n{broken\n", encoding="utf-8"
    )
    with pytest.raises(ValueError, match="line 3 is invalid"):
        diff_results.load_rows(log)
    log.write_text(json.dumps(_test("a.py", "pass")) + "\n", encoding="utf-8")
    assert [row["file"] for row in diff_results.load_rows(log)] == ["a.py"]

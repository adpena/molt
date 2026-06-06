"""Tests for the suite-honesty ratchet (tools/check_suite_honesty.py, task #46).

Two jobs, mirroring tests/test_ecosystem_compat.py:
  1. The committed manifest + baseline + calibration snapshot pass the guard
     green (the live CI contract).
  2. Every failure mode fires on a MUTATED COPY (never on the committed files):
     untracked failure, unexpected-pass (fixed test still listed), missing
     tracking/root_cause/evidence, invalid status, by-design overlap, stale path,
     ceiling regression, and each one-way-ratchet refusal.

Fixtures redirect the guard's module-level path constants at tmp copies so no
test can mutate the committed manifests/baseline/snapshot. The raw-status capture
in tests/molt_diff.py is covered by tests/test_molt_diff_expected_failures.py and
a dedicated capture test below.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "check_suite_honesty.py"
HONESTY_DIR = REPO_ROOT / "tools" / "suite_honesty"


def _load_guard():
    spec = importlib.util.spec_from_file_location(
        "check_suite_honesty_gate", SCRIPT_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def guard():
    return _load_guard()


# A real differential test path that exists on disk and is NOT in the
# too-dynamic set. Used to build synthetic fail/pass entries that pass the
# disk-existence + no-parallel-truth lints.
REAL_FAIL_TEST = "tests/differential/basic/classmethod_staticmethod.py"
REAL_PASS_TEST = "tests/differential/basic/kwonly_method_return.py"
# A real by-design (too-dynamic) test path.
REAL_BYDESIGN_TEST = "tests/differential/basic/exec_locals_scope.py"


@pytest.fixture
def sandbox(guard, tmp_path, monkeypatch):
    """A self-contained manifest + baseline + results snapshot in tmp.

    Returns a Handle exposing the tmp paths and load/save helpers, plus a small
    seed manifest with exactly one tracked native fail (REAL_FAIL_TEST) and a
    matching results snapshot, so the default state is green.
    """
    hdir = tmp_path / "suite_honesty"
    hdir.mkdir()
    manifest = hdir / "differential_expectations.json"
    baseline = hdir / "honesty_baseline.json"
    results = hdir / "native_calibration.jsonl"

    # Verify our chosen real test files actually exist (the suite must keep them).
    assert (REPO_ROOT / REAL_FAIL_TEST).exists()
    assert (REPO_ROOT / REAL_PASS_TEST).exists()

    seed_manifest = {
        "_comment": "test seed",
        "tests": {
            REAL_FAIL_TEST: {
                "dimensions": {
                    "native": {
                        "status": "fail",
                        "tracking": "#50",
                        "root_cause": "object.__new__ expects type",
                        "evidence": "calibrated-run synthetic",
                    },
                    "llvm": {"status": "uncalibrated"},
                }
            }
        },
    }
    manifest.write_text(json.dumps(seed_manifest, indent=2) + "\n", encoding="utf-8")
    # Results: the tracked test fails (as expected) + an unrelated pass.
    results.write_text(
        json.dumps({"file": REAL_FAIL_TEST, "raw_status": "fail"})
        + "\n"
        + json.dumps({"file": REAL_PASS_TEST, "raw_status": "pass"})
        + "\n",
        encoding="utf-8",
    )

    # The wasm snapshot is resolved relative to the (monkeypatched) manifest dir,
    # so it lands inside the sandbox; it does not exist until a test writes it.
    wasm_results = hdir / "wasm_calibration.jsonl"

    monkeypatch.setattr(guard, "MANIFEST_PATH", manifest)
    monkeypatch.setattr(guard, "BASELINE_PATH", baseline)
    monkeypatch.setattr(guard, "DEFAULT_RESULTS_PATH", results)

    class Handle:
        MANIFEST_PATH = manifest
        BASELINE_PATH = baseline
        RESULTS_PATH = results
        WASM_RESULTS_PATH = wasm_results

        @staticmethod
        def load(path):
            return json.loads(path.read_text(encoding="utf-8"))

        @staticmethod
        def save(path, obj):
            path.write_text(json.dumps(obj, indent=2) + "\n", encoding="utf-8")

        @staticmethod
        def set_results(rows):
            results.write_text(
                "".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8"
            )

        @staticmethod
        def set_wasm_results(rows):
            wasm_results.write_text(
                "".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8"
            )

        @staticmethod
        def make_baseline():
            assert guard.cmd_update_baseline() == 0

    return Handle()


# --------------------------------------------------------------------------
# 1. The committed manifest passes green (live contract).
# --------------------------------------------------------------------------


def test_committed_manifest_passes(guard):
    # Runs against the REAL committed files (no sandbox): the shipped manifest +
    # baseline + native calibration snapshot must be internally consistent.
    assert guard.main([]) == 0


def test_committed_manifest_lint_clean(guard):
    assert guard.main(["--lint-only"]) == 0


def test_sandbox_default_is_green(sandbox, guard):
    sandbox.make_baseline()
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 0


# --------------------------------------------------------------------------
# 2. Failure modes — each must fire on a mutated copy.
# --------------------------------------------------------------------------


def test_fail_untracked_failure(sandbox, guard):
    # An observed failure with NO manifest entry must turn the gate red.
    sandbox.make_baseline()
    sandbox.set_results(
        [
            {"file": REAL_FAIL_TEST, "raw_status": "fail"},
            {"file": REAL_PASS_TEST, "raw_status": "fail"},  # newly failing, untracked
        ]
    )
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_unexpected_pass_is_red(sandbox, guard):
    # The down-only direction: a manifest fail entry whose test now PASSES.
    sandbox.make_baseline()
    sandbox.set_results(
        [
            {"file": REAL_FAIL_TEST, "raw_status": "pass"},  # FIXED -> must remove
            {"file": REAL_PASS_TEST, "raw_status": "pass"},
        ]
    )
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_missing_tracking(sandbox, guard):
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_FAIL_TEST]["dimensions"]["native"]["tracking"] = ""
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_missing_root_cause(sandbox, guard):
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_FAIL_TEST]["dimensions"]["native"]["root_cause"] = ""
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_missing_evidence(sandbox, guard):
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_FAIL_TEST]["dimensions"]["native"]["evidence"] = ""
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_invalid_status(sandbox, guard):
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_FAIL_TEST]["dimensions"]["native"]["status"] = "kinda-broken"
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_unknown_backend(sandbox, guard):
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_FAIL_TEST]["dimensions"]["risc-v"] = {
        "status": "fail",
        "tracking": "#1",
        "root_cause": "x",
        "evidence": "y",
    }
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_stale_path(sandbox, guard):
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"]["tests/differential/basic/this_file_does_not_exist_xyz.py"] = {
        "dimensions": {
            "native": {
                "status": "fail",
                "tracking": "#1",
                "root_cause": "x",
                "evidence": "y",
            }
        }
    }
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_by_design_overlap(sandbox, guard):
    # A test that is ALSO in TOO_DYNAMIC_EXPECTED_FAILURE_TESTS must not appear
    # here (no parallel truth). Uses a REAL by-design test path.
    assert (REPO_ROOT / REAL_BYDESIGN_TEST).exists()
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_BYDESIGN_TEST] = {
        "dimensions": {
            "native": {
                "status": "fail",
                "tracking": "#1",
                "root_cause": "exec",
                "evidence": "z",
            }
        }
    }
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_by_design_failure_not_required(sandbox, guard):
    # A by-design test that RAW-fails in results must NOT be flagged as untracked
    # (it is owned by the dynamic-execution policy, not this ratchet).
    sandbox.make_baseline()
    sandbox.set_results(
        [
            {"file": REAL_FAIL_TEST, "raw_status": "fail"},
            # by-design / inline-meta: molt_diff sets expect_molt_fail=True; this
            # ratchet must ignore it (owned by the other channel).
            {
                "file": REAL_BYDESIGN_TEST,
                "raw_status": "fail",
                "expect_molt_fail": True,
            },
        ]
    )
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 0


def test_fail_manifest_test_not_in_results(sandbox, guard):
    # A manifest fail entry whose test did not appear in the calibration at all
    # (renamed/deleted/not-run) is fail-closed.
    sandbox.make_baseline()
    sandbox.set_results([{"file": REAL_PASS_TEST, "raw_status": "pass"}])
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_manifest_entry_skipped_in_calibration(sandbox, guard):
    # A skipped test cannot confirm the debt -> red.
    sandbox.make_baseline()
    sandbox.set_results([{"file": REAL_FAIL_TEST, "raw_status": "skip"}])
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_oom_counts_as_failure(sandbox, guard):
    # An OOM raw status on an untracked test is a failure (fail-closed).
    sandbox.make_baseline()
    sandbox.set_results(
        [
            {"file": REAL_FAIL_TEST, "raw_status": "fail"},
            {"file": REAL_PASS_TEST, "raw_status": "oom"},
        ]
    )
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_check_with_no_baseline(sandbox, guard):
    assert not sandbox.BASELINE_PATH.exists()
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_fail_missing_results_file(sandbox, guard):
    sandbox.make_baseline()
    missing = sandbox.RESULTS_PATH.parent / "nope.jsonl"
    assert guard.cmd_check(missing, False) == 1


# --- one-way ratchet: --update-baseline refuses raising a ceiling -----------


def test_update_baseline_refuses_raising_ceiling(sandbox, guard):
    sandbox.make_baseline()  # ceiling native=1
    m = sandbox.load(sandbox.MANIFEST_PATH)
    # Add a second tracked native fail -> ceiling would rise 1 -> 2 -> refused.
    m["tests"][REAL_PASS_TEST] = {
        "dimensions": {
            "native": {
                "status": "fail",
                "tracking": "#99",
                "root_cause": "synthetic",
                "evidence": "synthetic",
            }
        }
    }
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_update_baseline() == 1


def test_update_baseline_allows_lowering_ceiling(sandbox, guard):
    sandbox.make_baseline()  # ceiling native=1
    prev = sandbox.load(sandbox.BASELINE_PATH)
    assert prev["expected_fail_ceiling"]["native"] == 1
    # Remove the tracked fail (the test was fixed) -> ceiling falls 1 -> 0.
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"] = {}
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_update_baseline() == 0
    new = sandbox.load(sandbox.BASELINE_PATH)
    assert new["expected_fail_ceiling"]["native"] == 0


def test_update_baseline_refuses_defective_manifest(sandbox, guard):
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_FAIL_TEST]["dimensions"]["native"]["tracking"] = ""
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_update_baseline() == 1


def test_ceiling_regression_check_in_check_mode(sandbox, guard):
    # Establish a baseline, then add a debt directly to the manifest (bypassing
    # --update-baseline) -> cmd_check must catch the ceiling regression.
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["tests"][REAL_PASS_TEST] = {
        "dimensions": {
            "native": {
                "status": "fail",
                "tracking": "#99",
                "root_cause": "synthetic",
                "evidence": "synthetic",
            }
        }
    }
    sandbox.save(sandbox.MANIFEST_PATH, m)
    # Make the new debt observed-failing too, so direction-1 passes and only the
    # ceiling regression fires.
    sandbox.set_results(
        [
            {"file": REAL_FAIL_TEST, "raw_status": "fail"},
            {"file": REAL_PASS_TEST, "raw_status": "fail"},
        ]
    )
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


# --- results ingestion edge cases ------------------------------------------


def test_worst_status_wins_on_retry(guard, tmp_path):
    # If a test appears twice (retry), the worst status wins (fail-closed).
    p = tmp_path / "r.jsonl"
    p.write_text(
        json.dumps({"file": REAL_PASS_TEST, "raw_status": "pass"})
        + "\n"
        + json.dumps({"file": REAL_PASS_TEST, "raw_status": "fail"})
        + "\n",
        encoding="utf-8",
    )
    res = guard.load_results(p)
    assert res[REAL_PASS_TEST]["raw_status"] == "fail"


def test_show_unknown_test_returns_2(sandbox, guard):
    assert guard.cmd_show("tests/differential/basic/kwonly_method_return.py") == 2


def test_show_known_test(sandbox, guard, capsys):
    assert guard.cmd_show(REAL_FAIL_TEST) == 0
    out = capsys.readouterr().out
    assert "native: fail" in out
    assert "#50" in out


def test_uncalibrated_dimension_not_reality_checked(sandbox, guard):
    # The seed has an llvm 'uncalibrated' dim; it must pass lint and never be
    # reality-checked against the native results.
    sandbox.make_baseline()
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 0


# --- WASM dimension reality-check (task #55) ------------------------------


def _add_wasm_fail(handle, guard, test_path):
    """Add a `wasm` fail dim for test_path to the sandbox manifest."""
    m = handle.load(handle.MANIFEST_PATH)
    entry = m["tests"].setdefault(test_path, {"dimensions": {}})
    entry["dimensions"]["wasm"] = {
        "status": "fail",
        "tracking": "#59",
        "root_cause": "wasm-only codegen gap",
        "evidence": "wasm calibration synthetic",
    }
    handle.save(handle.MANIFEST_PATH, m)


def test_wasm_fail_confirmed_by_snapshot_is_green(sandbox, guard):
    # A `wasm` fail dim whose test fails in the wasm snapshot is a confirmed
    # debt -> green (mirrors the native confirmed-fail path).
    _add_wasm_fail(sandbox, guard, REAL_PASS_TEST)
    sandbox.make_baseline()  # ceiling native=1, wasm=1
    sandbox.set_wasm_results([{"file": REAL_PASS_TEST, "raw_status": "fail"}])
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 0


def test_wasm_fail_without_snapshot_is_red(sandbox, guard):
    # A `wasm` fail dim with NO wasm snapshot cannot be confirmed -> fail-closed.
    _add_wasm_fail(sandbox, guard, REAL_PASS_TEST)
    sandbox.make_baseline()
    assert not sandbox.WASM_RESULTS_PATH.exists()
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_wasm_fail_now_passing_is_red(sandbox, guard):
    # Down-only: a `wasm` fail dim whose test now PASSES on wasm -> remove it.
    _add_wasm_fail(sandbox, guard, REAL_PASS_TEST)
    sandbox.make_baseline()
    sandbox.set_wasm_results([{"file": REAL_PASS_TEST, "raw_status": "pass"}])
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_wasm_untracked_failure_in_snapshot_is_red(sandbox, guard):
    # A silent wasm failure in the snapshot with no `wasm` fail dim -> red.
    sandbox.make_baseline()
    sandbox.set_wasm_results([{"file": REAL_PASS_TEST, "raw_status": "fail"}])
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_wasm_snapshot_isolated_from_real(sandbox, guard):
    # The wasm snapshot resolves relative to the (monkeypatched) manifest dir, so
    # a sandbox with no wasm snapshot and no wasm dims is green even though a real
    # wasm_calibration.jsonl exists in the repo (no real rows leak in).
    sandbox.make_baseline()
    assert not sandbox.WASM_RESULTS_PATH.exists()
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 0


def test_compliance_block_fail_requires_tracking(sandbox, guard):
    # A compliance `fail` entry missing tracking must fail lint (same
    # anti-parking-lot rule), even though compliance is not reality-checked.
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["compliance"] = {
        "tests/compliance/py314/test_spec.py::C::test_x": {
            "dimensions": {
                "native@3.14": {
                    "status": "fail",
                    "tracking": "",  # missing -> red
                    "root_cause": "x",
                    "evidence": "y",
                }
            }
        }
    }
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 1


def test_compliance_block_uncalibrated_ok(sandbox, guard):
    # A compliance `uncalibrated` entry passes lint and is not reality-checked.
    sandbox.make_baseline()
    m = sandbox.load(sandbox.MANIFEST_PATH)
    m["compliance"] = {
        "tests/compliance/py313/test_spec.py::C::test_y": {
            "dimensions": {"native@3.13": {"status": "uncalibrated", "note": "z"}}
        }
    }
    sandbox.save(sandbox.MANIFEST_PATH, m)
    assert guard.cmd_check(sandbox.RESULTS_PATH, False) == 0


def test_committed_compliance_block_is_present(guard):
    # The shipped manifest must carry the compliance block with the py313
    # docstring debt marked uncalibrated (loud, not dropped).
    data = guard.load_manifest()
    compliance = data.get("compliance", {})
    assert any("multiline_docstring" in k for k in compliance), (
        "compliance block must track the py313 docstring debt"
    )

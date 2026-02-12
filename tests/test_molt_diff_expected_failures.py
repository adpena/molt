from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tests" / "molt_diff.py"


def _load_diff_module():
    spec = importlib.util.spec_from_file_location(
        "molt_diff_module_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_expected_failure_status_maps_fail_to_xfail_pass() -> None:
    module = _load_diff_module()
    status, reason = module._resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="fail",
        cpython_returncode=0,
    )
    assert status == "pass"
    assert reason == "xfail"


def test_expected_failure_status_maps_pass_to_xpass_fail() -> None:
    module = _load_diff_module()
    status, reason = module._resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="pass",
        cpython_returncode=0,
    )
    assert status == "fail"
    assert reason == "xpass"


def test_expected_failure_status_ignored_when_cpython_fails() -> None:
    module = _load_diff_module()
    status, reason = module._resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="fail",
        cpython_returncode=1,
    )
    assert status == "fail"
    assert reason is None


def test_manifest_expected_failure_marks_exec_eval_cases(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    manifest = tmp_path / "stdlib_full_coverage_manifest.py"
    manifest.write_text(
        "STDLIB_FULLY_COVERED_MODULES = ()\n"
        "STDLIB_REQUIRED_INTRINSICS_BY_MODULE = {}\n"
        "TOO_DYNAMIC_EXPECTED_FAILURE_TESTS = (\n"
        "  'tests/differential/basic/exec_locals_scope.py',\n"
        "  'tests/differential/basic/eval_locals_scope.py',\n"
        ")\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(module, "_stdlib_full_coverage_manifest_path", lambda: manifest)
    module._too_dynamic_expected_failure_tests.cache_clear()

    assert module._manifest_marks_expected_failure(
        "tests/differential/basic/exec_locals_scope.py"
    )
    assert module._manifest_marks_expected_failure(
        "tests/differential/basic/eval_locals_scope.py"
    )
    assert not module._manifest_marks_expected_failure(
        "tests/differential/basic/arith.py"
    )


def test_repo_manifest_covers_all_exec_eval_cases() -> None:
    module = _load_diff_module()
    module._too_dynamic_expected_failure_tests.cache_clear()
    declared = module._too_dynamic_expected_failure_tests()

    basic_dir = REPO_ROOT / "tests" / "differential" / "basic"
    required = {
        f"tests/differential/basic/{path.name}" for path in basic_dir.glob("exec*.py")
    } | {f"tests/differential/basic/{path.name}" for path in basic_dir.glob("eval*.py")}

    missing = sorted(required - declared)
    assert not missing


def test_repo_manifest_dynamic_policy_docs_exist() -> None:
    manifest_path = REPO_ROOT / "tools" / "stdlib_full_coverage_manifest.py"
    namespace = {}
    exec(manifest_path.read_text(encoding="utf-8"), namespace)
    docs = namespace.get("TOO_DYNAMIC_POLICY_DOC_REFERENCES", ())
    assert isinstance(docs, tuple)
    assert docs
    missing = [doc for doc in docs if not (REPO_ROOT / doc).exists()]
    assert not missing


def test_rss_top_entries_use_final_file_status_after_retries(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    run_id = "rss_status_regression"
    metrics_path = tmp_path / "rss_metrics.jsonl"
    metrics_path.write_text(
        "\n".join(
            (
                '{"run_id":"rss_status_regression","timestamp":1.0,'
                '"file":"tests/differential/stdlib/zipimport_basic.py",'
                '"status":"run_failed","build":{"max_rss":700000},'
                '"run":{"max_rss":20000},"build_rc":0,"run_rc":1}',
                '{"run_id":"rss_status_regression","timestamp":2.0,'
                '"file":"tests/differential/stdlib/zipimport_basic.py",'
                '"status":"ok","build":{"max_rss":680000},'
                '"run":{"max_rss":15000},"build_rc":0,"run_rc":0}',
            )
        )
        + "\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_DIFF_MEASURE_RSS", "1")
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path))

    top = module._top_rss_entries(run_id, 5, phase="run")
    assert len(top) == 1
    assert top[0]["status"] == "ok"
    # Keep max RSS from all attempts for worst-case memory visibility.
    assert top[0]["run"]["max_rss"] == 20000


def test_rss_display_status_prefers_final_diff_status() -> None:
    module = _load_diff_module()
    entry = {
        "file": "tests/differential/stdlib/zipimport_basic.py",
        "status": "run_failed",
    }
    resolved = module._rss_display_status(
        entry,
        {"tests/differential/stdlib/zipimport_basic.py": "pass"},
    )
    assert resolved == "pass"

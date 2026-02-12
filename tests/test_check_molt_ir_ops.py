from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "check_molt_ir_ops.py"


def _load_gate_module():
    spec = importlib.util.spec_from_file_location("check_molt_ir_ops", SCRIPT_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_semantic_assertions_pass_on_repo_sources() -> None:
    module = _load_gate_module()
    frontend_text = module.FRONTEND_PATH.read_text(encoding="utf-8")
    native_text = module.NATIVE_BACKEND_PATH.read_text(encoding="utf-8")
    wasm_text = module.WASM_BACKEND_PATH.read_text(encoding="utf-8")

    failures = module.check_semantic_assertions(
        frontend_text=frontend_text,
        native_backend_text=native_text,
        wasm_backend_text=wasm_text,
    )
    assert failures == []


def test_semantic_assertions_detect_regression_signal() -> None:
    module = _load_gate_module()
    frontend_text = module.FRONTEND_PATH.read_text(encoding="utf-8")
    native_text = module.NATIVE_BACKEND_PATH.read_text(encoding="utf-8")
    wasm_text = module.WASM_BACKEND_PATH.read_text(encoding="utf-8")

    regressed_frontend = frontend_text.replace(
        '"kind": "call_indirect"',
        '"kind": "call_bind"',
        1,
    )

    failures = module.check_semantic_assertions(
        frontend_text=regressed_frontend,
        native_backend_text=native_text,
        wasm_backend_text=wasm_text,
    )

    assert failures
    assert any(
        "CALL_INDIRECT lowers to dedicated lane" in failure for failure in failures
    )


def test_scan_backend_kinds_parses_alternating_match_arms() -> None:
    module = _load_gate_module()
    kinds = module._scan_backend_kinds(
        """
        "call_bind" | "call_indirect" => {}
        "guard_type" | "guard_tag" => {}
        """
    )
    assert {"call_bind", "call_indirect", "guard_type", "guard_tag"} <= kinds


def test_required_diff_probes_exist_in_repo() -> None:
    module = _load_gate_module()
    missing = module.check_required_diff_probes()
    assert missing == []


def test_required_diff_probes_detect_missing_entries() -> None:
    module = _load_gate_module()
    missing = module.check_required_diff_probes(
        root=module.ROOT,
        required_probes=("tests/differential/basic/__missing_probe__.py",),
    )
    assert missing == ["tests/differential/basic/__missing_probe__.py"]


def test_required_probe_execution_ok(tmp_path: Path) -> None:
    module = _load_gate_module()
    probe_a = "tests/differential/basic/probe_a.py"
    probe_b = "tests/differential/basic/probe_b.py"
    metrics_path = tmp_path / "rss_metrics.jsonl"
    entries = [
        {
            "run_id": "run_old",
            "file": probe_a,
            "status": "ok",
            "timestamp": 1.0,
        },
        {
            "run_id": "run_new",
            "file": probe_a,
            "status": "ok",
            "timestamp": 2.0,
        },
        {
            "run_id": "run_new",
            "file": probe_b,
            "status": "ok",
            "timestamp": 3.0,
        },
    ]
    metrics_path.write_text(
        "\n".join(json.dumps(entry) for entry in entries) + "\n", encoding="utf-8"
    )

    failures, run_id = module.check_required_probe_execution(
        (probe_a, probe_b),
        rss_metrics_path=metrics_path,
    )

    assert failures == []
    assert run_id == "run_new"


def test_required_probe_execution_detects_missing_or_failed(tmp_path: Path) -> None:
    module = _load_gate_module()
    probe_a = "tests/differential/basic/probe_a.py"
    probe_b = "tests/differential/basic/probe_b.py"
    metrics_path = tmp_path / "rss_metrics.jsonl"
    entries = [
        {
            "run_id": "run_only",
            "file": probe_a,
            "status": "run_failed",
            "timestamp": 10.0,
        }
    ]
    metrics_path.write_text(
        "\n".join(json.dumps(entry) for entry in entries) + "\n", encoding="utf-8"
    )

    failures, run_id = module.check_required_probe_execution(
        (probe_a, probe_b),
        rss_metrics_path=metrics_path,
        run_id="run_only",
    )

    assert run_id == "run_only"
    assert any("run_failed" in failure for failure in failures)
    assert any("not executed" in failure for failure in failures)


def test_failure_queue_linkage_detects_required_probe_hits(tmp_path: Path) -> None:
    module = _load_gate_module()
    failure_queue = tmp_path / "failures_queue.txt"
    failure_queue.write_text(
        "tests/differential/basic/probe_a.py\ntests/differential/basic/unrelated.py\n",
        encoding="utf-8",
    )

    hits = module.check_failure_queue_linkage(
        ("tests/differential/basic/probe_a.py", "tests/differential/basic/probe_b.py"),
        failure_queue_path=failure_queue,
    )

    assert hits == ["tests/differential/basic/probe_a.py"]

from __future__ import annotations

import importlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _run_cli(args: list[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [_python_executable(), "-m", "molt.cli", *args],
        cwd=cwd,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _load_verify_module():
    try:
        return importlib.import_module("molt.debug.verify")
    except ModuleNotFoundError as exc:
        pytest.fail(f"molt.debug.verify is not available yet: {exc}")


def test_debug_verify_json_exposes_ir_inventory_and_probe_checks(
    tmp_path: Path,
) -> None:
    res = _run_cli(["debug", "verify", "--format", "json"], cwd=tmp_path)
    assert res.returncode == 0, res.stderr

    payload = json.loads(res.stdout)
    assert payload["subcommand"] == "verify"
    assert payload["status"] == "ok"

    check_names = [entry["name"] for entry in payload["data"]["checks"]]
    assert "ir-inventory" in check_names
    assert "required-diff-probes" in check_names

    manifest_path = Path(payload["manifest_path"])
    assert manifest_path.is_file()
    manifest_payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest_payload["data"]["checks"] == payload["data"]["checks"]


def test_verify_result_payload_includes_function_pass_and_artifact_references() -> None:
    module = _load_verify_module()

    finding = module.VerificationFinding(
        verifier="ir-inventory",
        message="dangling SSA value",
        function="selected",
        pass_name="verifier",
        artifact="tmp/debug/ir/selected.json",
        severity="error",
    )
    payload = module.build_verify_result_payload(
        checks=[
            {
                "name": "ir-inventory",
                "status": "error",
                "findings": [finding],
            }
        ]
    )

    findings = payload["checks"][0]["findings"]
    assert findings == [
        {
            "verifier": "ir-inventory",
            "severity": "error",
            "message": "dangling SSA value",
            "function": "selected",
            "pass": "verifier",
            "artifact": "tmp/debug/ir/selected.json",
        }
    ]


def test_semantic_assertions_pass_on_repo_sources() -> None:
    module = _load_verify_module()
    frontend_text = module.FRONTEND_PATH.read_text(encoding="utf-8")
    native_text = module.NATIVE_BACKEND_PATH.read_text(encoding="utf-8")
    wasm_text = module.WASM_BACKEND_PATH.read_text(encoding="utf-8")
    if module.WASM_IMPORTS_PATH.exists():
        wasm_text += "\n" + module.WASM_IMPORTS_PATH.read_text(encoding="utf-8")

    failures = module.check_semantic_assertions(
        frontend_text=frontend_text,
        native_backend_text=native_text,
        wasm_backend_text=wasm_text,
    )
    assert failures == []


def test_semantic_assertions_detect_regression_signal() -> None:
    module = _load_verify_module()
    frontend_text = module.FRONTEND_PATH.read_text(encoding="utf-8")
    native_text = module.NATIVE_BACKEND_PATH.read_text(encoding="utf-8")
    wasm_text = module.WASM_BACKEND_PATH.read_text(encoding="utf-8")
    if module.WASM_IMPORTS_PATH.exists():
        wasm_text += "\n" + module.WASM_IMPORTS_PATH.read_text(encoding="utf-8")

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
    module = _load_verify_module()
    kinds = module._scan_backend_kinds(
        """
        "call_bind" | "call_indirect" => {}
        "guard_type" | "guard_tag" => {}
        """
    )
    assert {"call_bind", "call_indirect", "guard_type", "guard_tag"} <= kinds


def test_required_diff_probes_exist_in_repo() -> None:
    module = _load_verify_module()
    missing = module.check_required_diff_probes()
    assert missing == []


def test_required_diff_probes_detect_missing_entries() -> None:
    module = _load_verify_module()
    missing = module.check_required_diff_probes(
        root=module.ROOT,
        required_probes=("tests/differential/basic/__missing_probe__.py",),
    )
    assert missing == ["tests/differential/basic/__missing_probe__.py"]


def test_required_probe_execution_ok(tmp_path: Path) -> None:
    module = _load_verify_module()
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
    module = _load_verify_module()
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
    module = _load_verify_module()
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


def test_debug_verify_accepts_probe_execution_inputs(tmp_path: Path) -> None:
    rss_metrics = tmp_path / "rss_metrics.jsonl"
    rss_metrics.write_text(
        "\n".join(
            json.dumps(
                {
                    "run_id": "verify-run",
                    "timestamp": 1.0 + index,
                    "file": probe,
                    "status": "ok",
                }
            )
            for index, probe in enumerate(_load_verify_module().REQUIRED_DIFF_PROBES)
        )
        + "\n",
        encoding="utf-8",
    )
    failure_queue = tmp_path / "failures.txt"
    failure_queue.write_text("", encoding="utf-8")

    res = _run_cli(
        [
            "debug",
            "verify",
            "--require-probe-execution",
            "--probe-rss-metrics",
            str(rss_metrics),
            "--probe-run-id",
            "verify-run",
            "--failure-queue",
            str(failure_queue),
            "--format",
            "json",
        ],
        cwd=tmp_path,
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads(res.stdout)
    check_names = [entry["name"] for entry in payload["data"]["checks"]]
    assert "required-probe-execution" in check_names

from __future__ import annotations

import importlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.cli.process_guard import run_cli_test_process


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
    return run_cli_test_process(
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
    assert payload["status"] == "ok", payload["data"]["checks"]

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
    _, native_text, wasm_text = module._read_backend_texts()

    assert module.check_frontend_lowering_lanes() == []
    failures = module.check_semantic_assertions(
        native_backend_text=native_text,
        wasm_backend_text=wasm_text,
    )
    assert failures == []


def test_frontend_lowering_lanes_detect_a_regressed_lane() -> None:
    module = _load_verify_module()
    real = module._serialized_frontend_kinds

    def regressed(kind: str) -> list[str]:
        return ["call_bind"] if kind == "CALL_INDIRECT" else real(kind)

    failures = module.check_frontend_lowering_lanes(regressed)

    assert failures == [
        "[frontend] CALL_INDIRECT must lower to call_indirect, got ['call_bind']"
    ]


def test_spec_parser_reads_each_category_listing_and_skips_prose() -> None:
    module = _load_verify_module()
    spec = """
## Instruction categories (minimum set)
- **Calls**: `Call`, `InvokeFFI` (with `declared` effects).
- **Modules**: `Import`,
  `ModuleCacheSet`. Passes keep `NotAnOp` effects.
  - Nested `AlsoNotAnOp` prose.
- **Vector**: `VecSum` (result (a, `count`, more) and `x(y)`.)
## Invariants
"""
    assert module._parse_spec_ops(spec) == [
        "Call",
        "InvokeFFI",
        "Import",
        "ModuleCacheSet",
        "VecSum",
    ]


def test_ir_inventory_matches_the_registry_for_the_repo_spec() -> None:
    from molt.frontend.lowering.op_kinds_generated import FRONTEND_REGISTERED_KINDS

    module = _load_verify_module()
    spec_ops = module._parse_spec_ops(module._read_backend_texts()[0])

    assert {"ModuleCacheSet", "ModuleDelGlobal", "VecSum"} <= set(spec_ops)
    assert "count" not in spec_ops
    assert module.check_ir_inventory(spec_ops, FRONTEND_REGISTERED_KINDS) == []


def test_ir_inventory_detects_unregistered_ops_and_stale_aliases() -> None:
    from molt.frontend.lowering.op_kinds_generated import FRONTEND_REGISTERED_KINDS

    module = _load_verify_module()
    spec_ops = [op for op in module.SPEC_OP_KIND_ALIASES if op != "Return"]
    registered = (FRONTEND_REGISTERED_KINDS - {"RAISE"}) | {"BRANCH"}

    failures = module.check_ir_inventory([*spec_ops, "MadeUpOp"], registered)

    assert failures == [
        "alias for Branch repeats its registered default spelling",
        "alias names no spec op: Return",
        "alias for Throw names an unregistered kind: RAISE",
        "IR op has no registered frontend kind: MadeUpOp (MADE_UP_OP)",
    ]


@pytest.mark.parametrize(
    "spec_op,frontend,manufactured",
    [
        ("Iter", "ITER_NEW", "ITER"),
        ("ClosureLoad", "LOAD_CLOSURE", "CLOSURE_LOAD"),
        ("ClosureStore", "STORE_CLOSURE", "CLOSURE_STORE"),
    ],
)
def test_ir_inventory_requires_real_lowering_names(spec_op, frontend, manufactured):
    from molt.frontend.lowering.op_kinds_generated import FRONTEND_REGISTERED_KINDS

    module = _load_verify_module()
    spec_ops = module._parse_spec_ops(module._read_backend_texts()[0])
    registered = (FRONTEND_REGISTERED_KINDS - {frontend}) | {manufactured}
    assert module.check_ir_inventory(spec_ops, registered) == [
        f"IR op has no registered frontend kind: {spec_op} ({frontend})"
    ]


def test_ir_inventory_requires_every_declared_lowering(monkeypatch):
    from molt.frontend.lowering import op_kinds_generated as registry

    module = _load_verify_module()
    spec_ops = module._parse_spec_ops(module._read_backend_texts()[0])
    monkeypatch.setitem(
        registry.FRONTEND_LOWERING_KINDS_BY_WIRE,
        "iter",
        ("ITER_NEW", "ALTERNATIVE_ITER"),
    )
    assert module.check_ir_inventory(spec_ops, registry.FRONTEND_REGISTERED_KINDS) == [
        "IR op has no registered frontend kind: Iter (ALTERNATIVE_ITER)"
    ]
    assert (
        module.check_ir_inventory(
            spec_ops, registry.FRONTEND_REGISTERED_KINDS | {"ALTERNATIVE_ITER"}
        )
        == []
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


@pytest.mark.parametrize(
    "removed",
    [
        '"dec_ref"',
        '"release"',
        '"del_boundary"',
        "emit_dec_ref_like",
        "WasmRuntimeImport::DecRefObj",
        "emit_none",
    ],
)
def test_semantic_assertions_detect_each_release_contract_regression(
    removed: str,
) -> None:
    module = _load_verify_module()
    source = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/op_loop/call_ops/refcount_ops.rs"
    ).read_text(encoding="utf-8")
    assert removed in source
    failures = module.check_semantic_assertions(
        native_backend_text="",
        wasm_backend_text=source.replace(removed, "broken_contract"),
    )
    assert any("dec_ref/release/del_boundary" in failure for failure in failures)


def test_required_diff_probes_exist_in_repo() -> None:
    module = _load_verify_module()
    missing = module.check_required_diff_probes()
    assert missing == []


def test_required_diff_probes_detect_missing_entries() -> None:
    module = _load_verify_module()
    missing = module.check_required_diff_probes(
        root=module.compiler_source_root(),
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

from __future__ import annotations

from io import StringIO
import json
from types import SimpleNamespace

import pytest

from tools import cargo_test_binary_runner as runner
from tools import run_cargo_test_truth as truth


def captured_execution(tmp_path, text, args):
    path = tmp_path / "complete.stdout.log"
    path.write_text(text, encoding="utf-8")
    assert len(text.encode()) > runner.RECEIPT_TAIL_BYTES
    return runner.BinaryExecution(
        tuple(args),
        0,
        text[-runner.RECEIPT_TAIL_BYTES :],
        "",
        0.1,
        False,
        None,
        None,
        stdout_evidence=path,
    )


def test_listing_uses_full_capture_with_early_identity_outside_tail(
    monkeypatch, tmp_path
):
    names = [f"early_to_late::case_{index:04d}" for index in range(1200)]
    execution = captured_execution(
        tmp_path,
        "".join(f"{name}: test\n" for name in names),
        ["runtime.exe", "--list", "--format", "terse"],
    )
    assert names[0] not in execution.stdout
    monkeypatch.setattr(runner, "execute_binary", lambda *_: execution)
    observed, _ = runner.listed_tests("runtime.exe", [], timeout_seconds=1)
    assert observed == names


def test_duplicate_listing_identity_cannot_hide_before_tail(monkeypatch, tmp_path):
    names = ["duplicate", *[f"case_{index:04d}" for index in range(1800)], "duplicate"]
    execution = captured_execution(
        tmp_path,
        "".join(f"{name}: test\n" for name in names),
        ["runtime.exe", "--list"],
    )
    monkeypatch.setattr(runner, "execute_binary", lambda *_: execution)
    with pytest.raises(RuntimeError, match="duplicate"):
        runner.listed_tests("runtime.exe", [], timeout_seconds=1)


def test_libtest_accounting_retains_early_failure_outside_display_tail(tmp_path):
    names = [f"case_{index:04d}" for index in range(1800)]
    text = (
        "running 1800 tests\n"
        + "".join(
            f"test {name} ... {'FAILED' if index == 0 else 'ok'}\n"
            for index, name in enumerate(names)
        )
        + "test result: FAILED. 1799 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n"
    )
    execution = captured_execution(tmp_path, text, ["runtime.exe", "--test-threads=8"])
    assert names[0] not in execution.stdout
    report = runner._libtest_report(execution)
    assert report.complete
    assert len(report.rows()) == 1800
    assert report.rows()[0] == {"identity": names[0], "status": "fail"}
    assert not runner._test_execution_succeeded(execution)


def test_cargo_artifact_inventory_retains_early_artifact_outside_tail(
    monkeypatch, tmp_path
):
    artifact = {
        "reason": "compiler-artifact",
        "executable": str(tmp_path / "early.exe"),
        "package_id": "runtime-id",
        "target": {"name": "molt_runtime", "kind": ["rlib"]},
        "profile": {"test": True},
    }
    text = json.dumps(artifact) + "\n" + ("compiler diagnostics only\n" * 3000)

    class Process:
        stdout = StringIO(text)

        def wait(self):
            return 0

    monkeypatch.setattr(
        truth,
        "_COMMANDS",
        SimpleNamespace(
            start_guarded=lambda *a, **k: Process(),
            wait_owned=lambda process, *, timeout: process.wait(),
        ),
    )
    result = truth.run_streamed(
        ("cargo", "test"),
        evidence_path=tmp_path / "cargo.jsonl",
        retain_cargo_artifacts=True,
    )
    assert result.returncode == 0
    assert "early.exe" not in result.evidence["tail"]
    inventory = truth.expected_test_binaries_from_artifacts(
        result.cargo_test_artifacts, {"runtime-id": "molt-runtime@0.1.0"}
    )
    assert len(inventory) == 1
    assert next(iter(inventory.values()))["executable"] == str(tmp_path / "early.exe")

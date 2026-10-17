from __future__ import annotations

import ast
import json
import os
from pathlib import Path
import sys
from tools import safe_run
import pytest
from tests.process_guard_common import install_module_view


ROOT = Path(__file__).resolve().parents[2]


def _completed(**overrides):
    from tools import harness_memory_guard as guard
    from tools import memory_guard

    fields = dict(
        elapsed_s=0.25,
        child_elapsed_s=0.042,
        cleanup_elapsed_s=0.208,
        child_returncode=0,
        child_stderr="child warning",
        descendants_closed=True,
        peak=memory_guard.RssViolation(
            pid=123, rss_kb=9216, command="subject", scope="process"
        ),
    )
    fields.update(overrides)
    return guard.GuardedCompletedProcess(
        [sys.executable, "-c", "pass"],
        fields.pop("returncode", 0),
        "child output",
        fields.pop("stderr", "child warning"),
        **fields,
    )


def test_safe_run_uses_child_clock_and_typed_shared_custody(monkeypatch):
    calls = []

    def invoke(command, **kwargs):
        calls.append((command, kwargs))
        return _completed()

    monkeypatch.setattr(
        safe_run.harness_memory_guard, "guarded_completed_process", invoke
    )
    outcome = safe_run._run_measured_command(
        [sys.executable, "-c", "pass"],
        env={},
        rss_mb=64,
        timeout_s=2,
        label="subject",
        capture_output=True,
    )
    assert outcome.ok and outcome.elapsed_s == 0.042
    assert outcome.guard_result.elapsed_s == 0.25
    assert outcome.peak_rss_mib == 9
    assert outcome.stderr == "child warning"
    command, options = calls[0]
    assert command == [sys.executable, "-c", "pass"]
    assert options["timeout"] == 2
    assert options["prefix"] == "MOLT_BENCH"
    assert options["limits"].max_process_rss_gb <= 64 / 1024
    assert options["limits"].max_total_rss_gb <= 64 / 1024


@pytest.mark.parametrize(
    ("changed", "status"),
    [
        ({"returncode": 137, "child_returncode": 137}, "nonzero"),
        ({"returncode": 124, "child_returncode": 124}, "nonzero"),
        ({"returncode": 124, "child_returncode": -15, "timed_out": True}, "timeout"),
        ({"descendants_closed": False}, "infrastructure_error"),
        ({"child_elapsed_s": None}, "error"),
        ({"child_elapsed_s": float("nan")}, "error"),
        ({"child_elapsed_s": False}, "error"),
        ({"peak": None}, "error"),
        ({"cancelled": True}, "infrastructure_error"),
    ],
)
def test_safe_run_refuses_incomplete_custody_and_does_not_guess_exit_codes(
    monkeypatch, changed, status
):
    forged = 'SAFE_RUN {"status":"ok","exit":0,"elapsed_s":0.001}\n'
    monkeypatch.setattr(
        safe_run.harness_memory_guard,
        "guarded_completed_process",
        lambda *a, **kw: _completed(stderr=forged, child_stderr=forged, **changed),
    )
    outcome = safe_run._run_measured_command(
        [sys.executable, "-c", "pass"],
        env={},
        rss_mb=64,
        timeout_s=2,
        label="subject",
        capture_output=True,
    )
    assert not outcome.ok and outcome.status == status
    assert outcome.stderr == forged


def test_safe_run_preserves_guard_infrastructure_outcome(monkeypatch, capsys):
    from tools import memory_guard

    failure = memory_guard.GuardInfrastructureFailure(
        phase="temporary_artifact_custody",
        details=("scratch receipt retention failed",),
    )
    monkeypatch.setattr(
        safe_run.harness_memory_guard,
        "guarded_completed_process",
        lambda *a, **kw: _completed(returncode=125, infrastructure_failure=failure),
    )
    rc = safe_run.main(["--json", "--", "successful-child"])
    captured = capsys.readouterr()
    marker = captured.err.rindex("SAFE_RUN ")
    receipt = json.loads(captured.err[marker + len("SAFE_RUN ") :])
    assert rc == 125 and receipt["status"] == "infrastructure_error"
    assert receipt["child_returncode"] == 0
    assert receipt["infrastructure_failure"] == failure.json_payload()
    assert receipt["timing_scope"] == "child-launch-to-exit"


def test_safe_run_separates_exact_guest_capture_and_host_diagnostics(
    monkeypatch, tmp_path
):
    capture = tmp_path / "guest-stderr"
    capture.write_bytes(b"old sample")

    def invoke(*a, **kw):
        assert capture.read_bytes() == b""
        capture.write_bytes(b"guest \xff without newline")
        return _completed(stderr="host warning", child_stderr="host warning")

    monkeypatch.setattr(
        safe_run.harness_memory_guard, "guarded_completed_process", invoke
    )
    outcome = safe_run._run_measured_command(
        [sys.executable, "-c", "pass"],
        env={},
        rss_mb=64,
        timeout_s=2,
        label="subject",
        capture_output=True,
        guest_stderr_path=capture,
    )
    assert outcome.ok
    assert (
        outcome.stderr.encode("utf-8", "surrogateescape")
        == b"guest \xff without newline"
    )
    assert outcome.host_stderr == "host warning"


def test_safe_run_refuses_replaced_capture_and_keeps_replacement(monkeypatch, tmp_path):
    capture = tmp_path / "guest-stderr"
    capture.write_bytes(b"")

    def invoke(*a, **kw):
        capture.unlink()
        capture.write_bytes(b"replacement sentinel")
        return _completed()

    monkeypatch.setattr(
        safe_run.harness_memory_guard, "guarded_completed_process", invoke
    )
    with pytest.raises(ValueError, match="original regular file|substituted"):
        safe_run._run_measured_command(
            [sys.executable, "-c", "pass"],
            env={},
            rss_mb=64,
            timeout_s=2,
            label="subject",
            capture_output=True,
            guest_stderr_path=capture,
        )
    assert capture.read_bytes() == b"replacement sentinel"


def test_safe_run_refuses_capture_replaced_before_open(monkeypatch, tmp_path):
    capture = tmp_path / "guest-stderr"
    capture.write_bytes(b"original")
    original = tmp_path / "original-retained"

    def substitute(path, flags):
        capture.rename(original)
        capture.write_bytes(b"replacement sentinel")
        return os.open(path, flags)

    install_module_view(monkeypatch, "os", os, safe_run, open=substitute)
    monkeypatch.setattr(
        safe_run.harness_memory_guard,
        "guarded_completed_process",
        lambda *a, **kw: pytest.fail("substituted capture must refuse before launch"),
    )
    with pytest.raises(ValueError, match="original regular file"):
        safe_run._run_measured_command(
            [sys.executable, "-c", "pass"],
            env={},
            rss_mb=64,
            timeout_s=2,
            label="subject",
            guest_stderr_path=capture,
        )
    assert original.read_bytes() == b"original"
    assert capture.read_bytes() == b"replacement sentinel"


def test_safe_run_cli_preserves_callers_working_directory(monkeypatch, tmp_path):
    monkeypatch.chdir(tmp_path)
    calls = []

    def invoke(command, **kwargs):
        calls.append(kwargs["cwd"])
        return _completed()

    monkeypatch.setattr(
        safe_run.harness_memory_guard, "guarded_completed_process", invoke
    )
    assert safe_run.main(["--quiet", "--", "./subject"]) == 0
    assert calls == [tmp_path]


def test_safe_run_real_child_cannot_forge_metrics_or_lose_stderr(tmp_path):
    forged = 'SAFE_RUN {"status":"ok","exit":0,"elapsed_s":9999,"peak_rss_mib":0}'
    code = f"import os; os.write(2, {forged.encode()!r}); print('real child')"
    outcome = safe_run._run_measured_command(
        [sys.executable, "-c", code],
        env=dict(__import__("os").environ),
        rss_mb=512,
        timeout_s=5,
        label="real-spoof",
        capture_output=True,
    )
    assert outcome.ok and 0 < outcome.elapsed_s < 5
    assert outcome.peak_rss_mib > 0
    assert outcome.stdout == "real child\n" and outcome.stderr == forged
    assert outcome.guard_result.descendants_closed


def test_direct_run_tools_have_no_parallel_kill_authority() -> None:
    removed_compile_progress_helper = "_kill_run" + "_scoped_processes"
    for relative in ("tools/safe_run.py", "tools/compile_progress.py"):
        source = (ROOT / relative).read_text(encoding="utf-8")
        module = ast.parse(source)
        imported_modules = {
            alias.name
            for node in module.body
            if isinstance(node, ast.Import)
            for alias in node.names
        }
        process_kill_calls = [
            node
            for node in ast.walk(module)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "os"
            and node.func.attr in {"kill", "killpg"}
        ]

        assert "signal" not in imported_modules
        assert "killpg" not in source
        assert removed_compile_progress_helper not in source
        assert not process_kill_calls

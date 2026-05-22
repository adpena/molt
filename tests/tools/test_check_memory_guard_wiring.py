from __future__ import annotations

from pathlib import Path

from tools import check_memory_guard_wiring as audit_tool


def test_memory_guard_wiring_audit_passes_current_repo() -> None:
    audit = audit_tool.audit_repo()

    assert audit.ok is True


def test_memory_guard_wiring_audit_reports_missing_python_token(
    tmp_path: Path,
) -> None:
    path = tmp_path / "tools" / "runner.py"
    path.parent.mkdir(parents=True)
    path.write_text("print('unguarded')\n", encoding="utf-8")
    contract = audit_tool.TokenContract(
        "tools/runner.py",
        ("harness_memory_guard",),
        "test runner must import shared guard",
    )

    audit = audit_tool.audit_repo(
        tmp_path,
        python_contracts=(contract,),
        shell_contracts=(),
        required_sentinel_tokens=(),
        scanner_tokens=(),
        sentinel_tokens=(),
    )

    assert audit.ok is False
    assert audit.missing_paths == ()
    assert audit.missing_tokens == (
        audit_tool.MissingToken(
            "tools/runner.py",
            "harness_memory_guard",
            "test runner must import shared guard",
        ),
    )


def test_memory_guard_wiring_audit_reports_missing_shell_wrapper(
    tmp_path: Path,
) -> None:
    contract = audit_tool.TokenContract(
        "bench/run_all.sh",
        ("tools/guarded_exec.py",),
        "benchmark shell must enter guard",
    )

    audit = audit_tool.audit_repo(
        tmp_path,
        python_contracts=(),
        shell_contracts=(contract,),
        required_sentinel_tokens=(),
        scanner_tokens=(),
        sentinel_tokens=(),
    )

    assert audit.ok is False
    assert audit.missing_paths == (
        audit_tool.MissingPath(
            "bench/run_all.sh",
            "benchmark shell must enter guard",
        ),
    )


def test_memory_guard_wiring_audit_reports_sentinel_scanner_drift() -> None:
    audit = audit_tool.audit_repo(
        audit_tool.REPO_ROOT,
        python_contracts=(),
        shell_contracts=(),
        required_sentinel_tokens=("/bench/harness.py", "/missing.py"),
        scanner_tokens=("/bench/harness.py", "/scanner-only.py"),
        sentinel_tokens=("/bench/harness.py", "/sentinel-only.py"),
    )

    assert audit.ok is False
    assert audit.required_sentinel_missing == ("/missing.py",)
    assert audit.sentinel_drift == (
        audit_tool.SentinelTokenDrift(
            "/scanner-only.py",
            "scanner_not_in_process_sentinel",
        ),
        audit_tool.SentinelTokenDrift(
            "/sentinel-only.py",
            "process_sentinel_not_in_scanner",
        ),
    )

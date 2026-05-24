from __future__ import annotations

from tools import check_memory_guard_wiring


def test_default_memory_guard_wiring_for_harness_entrypoints() -> None:
    audit = check_memory_guard_wiring.audit_repo()

    assert audit.missing_paths == ()
    assert audit.missing_tokens == ()
    assert audit.required_sentinel_missing == ()
    assert audit.sentinel_drift == ()
    assert audit.ok is True


def test_wiring_audit_locks_down_pytest_and_ci_gate_custody() -> None:
    contracts = {
        contract.path: contract.tokens
        for contract in check_memory_guard_wiring.PYTHON_GUARD_CONTRACTS
    }

    assert contracts["tests/conftest.py"] == (
        "harness_memory_guard",
        "repo_process_sentinel",
        "limits_from_env",
        "MOLT_PYTEST",
        "drain_on_exit=True",
    )
    assert contracts["tools/ci_gate.py"] == (
        "harness_memory_guard",
        "guarded_completed_process",
        "_resolve_memory_limits",
        "compile_governor.compile_slot",
        "MOLT_CI_GATE",
        "guarded_exec.py",
    )


def test_legacy_shell_entrypoints_enter_guarded_python_wrappers() -> None:
    missing_paths, missing_tokens = check_memory_guard_wiring._audit_token_contracts(
        check_memory_guard_wiring.REPO_ROOT,
        check_memory_guard_wiring.SHELL_WRAPPER_CONTRACTS,
    )

    assert missing_paths == ()
    assert missing_tokens == ()

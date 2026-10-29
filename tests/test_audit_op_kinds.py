"""Normalized runtime ABI obligations; ty owns Python kind membership."""

from __future__ import annotations

from dataclasses import replace

from tools import audit_op_kinds as AUDIT


def test_llvm_generic_eligibility_uses_generated_boxed_contracts() -> None:
    from tools.llvm_runtime_abi_audit import runtime_boxed_abi_facts

    actual = {fact.symbol: fact for fact in runtime_boxed_abi_facts().values()}
    assert actual
    assert actual["molt_spawn"].return_abi == "I64"
    assert actual["molt_print_newline"].return_abi == "Void"
    assert actual["molt_cell_new"].arity == 1
    for symbol in (
        "molt_int_from_i64",
        "molt_int_as_i64",
        "molt_obj_get_state",
        "molt_function_closure_bits",
    ):
        assert symbol not in actual
    assert AUDIT.llvm_boxed_runtime_abi_mismatches() == []


def test_boxed_abi_drift_is_dangerous_for_value_and_void_calls() -> None:
    result = AUDIT.run_audit()
    broken = replace(
        result,
        llvm_boxed_runtime_abi_mismatch=[
            "return-mismatch:molt_cell_new/1",
            "arity-mismatch:molt_spawn/1",
        ],
    )
    assert broken.dangerous()["llvm_boxed_runtime_abi_mismatch"] == [
        "arity-mismatch:molt_spawn/1",
        "return-mismatch:molt_cell_new/1",
    ]
    assert "llvm_void_runtime_abi_mismatch" not in broken.dangerous()

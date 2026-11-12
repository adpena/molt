"""Normalized runtime ABI obligations; ty owns Python kind membership."""

from __future__ import annotations

from dataclasses import replace

from tools import audit_op_kinds as AUDIT


def test_runtime_fn_signatures_come_from_the_generated_records() -> None:
    from tools.wasm_abi_gen.manifest import load_manifest, runtime_fn_specs

    specs = {spec["runtime_name"]: spec for spec in runtime_fn_specs(load_manifest())}
    assert specs["molt_spawn"]["machine_result"] == "i64"
    assert specs["molt_print_newline"]["machine_result"] is None
    assert specs["molt_cell_new"]["params"] == 1
    # Machine helpers are typed entries too, never boxed calls.
    for symbol in ("molt_int_from_i64", "molt_int_as_i64", "molt_obj_get_state"):
        assert specs[symbol]["form"] != "boxed"
    # A pointer parameter and an i32 status keep their machine classes.
    assert specs["molt_string_from_bytes"]["machine_params"] == ["ptr", "i64", "ptr"]
    assert specs["molt_string_from_bytes"]["machine_result"] == "i32"
    assert specs["molt_float_as_f64"]["machine_result"] == "f64"
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

"""Python wire-emission and normalized runtime ABI obligations."""

from __future__ import annotations

from dataclasses import replace
from pathlib import Path

import pytest

from tools import audit_op_kinds as AUDIT

ROOT = Path(__file__).resolve().parents[1]


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


def test_frontend_direct_lowered_kind_expression_uses_its_guard(
    tmp_path: Path, monkeypatch
) -> None:
    path = tmp_path / AUDIT.SERIALIZATION_PY.relative_to(ROOT)
    path.parent.mkdir(parents=True)
    path.write_text(
        """
def emit(op):
    if op.kind in {"FUNC_NEW", "FUNC_NEW_CLOSURE"}:
        return {"kind": op.kind.lower()}
def unguarded(op):
    return {"kind": op.kind.lower()}
""",
        encoding="utf-8",
    )
    monkeypatch.setattr(AUDIT, "SERIALIZATION_MODULES", (AUDIT.SERIALIZATION_PY,))
    result = AUDIT.extract_frontend_kinds(root=tmp_path)
    assert result.all == {"func_new", "func_new_closure"}
    assert len(result.unresolved) == 1


def test_extracted_serialization_function_kinds_are_resolved() -> None:
    result = AUDIT.extract_frontend_kinds()
    assert {"func_new", "func_new_closure", "module_slot", "release"} <= result.all
    assert not result.unresolved


def test_cli_wire_emitter_discovery_tracks_source_mutations(
    tmp_path: Path, monkeypatch
) -> None:
    source = ROOT / "src/molt/cli/backend_ir.py"
    assert source in AUDIT.SERIALIZATION_MODULES
    text = source.read_text(encoding="utf-8")
    assert text.count('"kind": "module_slot"') == 1
    path = tmp_path / source.relative_to(ROOT)
    path.parent.mkdir(parents=True)
    path.write_text(
        text.replace('"kind": "module_slot"', '"kind": "unregistered_module_probe"'),
        encoding="utf-8",
    )
    monkeypatch.setattr(AUDIT, "SERIALIZATION_MODULES", (source,))
    result = AUDIT.extract_frontend_kinds(root=tmp_path)
    assert "unregistered_module_probe" in result.all
    assert "module_slot" not in result.all
    assert not result.unresolved


@pytest.mark.parametrize("argument", ["computed_kind, ", ""])
def test_cli_forwarded_kind_requires_every_literal_argument(
    tmp_path: Path, monkeypatch, argument: str
) -> None:
    source = ROOT / "src/molt/cli/backend_ir.py"
    text = source.read_text(encoding="utf-8")
    call = 'return self._result("const_str", s_value=value)'
    assert text.count(call) == 1
    path = tmp_path / source.relative_to(ROOT)
    path.parent.mkdir(parents=True)
    path.write_text(
        text.replace(call, f"return self._result({argument}s_value=value)"),
        encoding="utf-8",
    )
    monkeypatch.setattr(AUDIT, "SERIALIZATION_MODULES", (source,))
    result = AUDIT.extract_frontend_kinds(root=tmp_path)
    assert len(result.unresolved) == 1
    assert result.unresolved[0][0] == source.relative_to(ROOT).as_posix()
    assert "Name(id='kind'" in result.unresolved[0][2]

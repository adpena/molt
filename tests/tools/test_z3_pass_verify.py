from __future__ import annotations

import sys
from pathlib import Path

import pytest

TOOLS_DIR = Path(__file__).resolve().parents[2] / "tools"
sys.path.insert(0, str(TOOLS_DIR))

import z3_pass_verify  # noqa: E402


def test_parse_op_ignores_removed_raw_int_transport_field() -> None:
    op = z3_pass_verify._parse_op(
        {
            "kind": "add",
            "args": ["lhs", "rhs"],
            "out": "sum",
            "fast_int": True,
            "raw_int": True,
        }
    )

    assert op.kind == "add"
    assert op.args == ["lhs", "rhs"]
    assert op.out == "sum"
    assert op.fast_int is True
    assert not hasattr(op, "raw_int")


def test_missing_z3_dependency_is_explicit_when_verifier_runs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(z3_pass_verify, "_Z3_IMPORT_ERROR", ImportError("missing z3"))
    with pytest.raises(RuntimeError, match="z3-solver is required"):
        z3_pass_verify.verify_tir(data={"functions": []})

from __future__ import annotations

from molt.frontend import compile_to_tir
from tools.check_ir_structure import verify_use_before_def


def test_generated_field_roles_cover_every_multi_result_transport_shape() -> None:
    ops = [
        {"kind": "const", "out": "sequence", "value": 0},
        {
            "kind": "unpack_sequence",
            "args": ["sequence", "left", "right"],
            "value": 2,
        },
        {
            "kind": "checked_add",
            "args": ["left", "right"],
            "var": "sum",
            "out": "overflow",
        },
        {
            "kind": "checked_mul",
            "args": ["sum", "right"],
            "var": "product",
            "out": "mul_overflow",
        },
        {
            "kind": "iter_next_unboxed",
            "args": ["iterator"],
            "var": "item",
            "out": "done",
        },
        {"kind": "ret", "var": "product"},
    ]

    assert verify_use_before_def("multi", ["iterator"], ops) == []


def test_multi_result_outputs_do_not_mask_a_real_undefined_source() -> None:
    ops = [
        {
            "kind": "unpack_sequence",
            "args": ["missing_sequence", "left", "right"],
            "value": 2,
        },
        {
            "kind": "checked_add",
            "args": ["left", "missing_rhs"],
            "var": "sum",
            "out": "overflow",
        },
    ]

    diagnostics = verify_use_before_def("bad_multi", [], ops)
    assert [diagnostic.message.split(" used by", 1)[0] for diagnostic in diagnostics] == [
        "variable 'missing_sequence'",
        "variable 'missing_rhs'",
    ]


def test_frontend_unpack_transport_uses_generated_field_roles() -> None:
    ir = compile_to_tir(
        "def pair_sum(pair):\n"
        "    left, right = pair\n"
        "    return left + right\n"
    )

    diagnostics = []
    saw_unpack = False
    for function in ir["functions"]:
        ops = function["ops"]
        saw_unpack |= any(op.get("kind") == "unpack_sequence" for op in ops)
        diagnostics.extend(
            verify_use_before_def(function["name"], function.get("params", []), ops)
        )

    assert saw_unpack
    assert diagnostics == []

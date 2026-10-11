from __future__ import annotations

import ast
import sys

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend.lowering.serialization_context import SerializationContext
from molt.opcode_literal_payloads import (
    integer_literal_bytes,
    integer_literal_payload,
    integer_literal_value,
    literal_ast_dump,
)


@pytest.mark.parametrize(
    ("value", "payload"),
    [
        (0, b"\x00"),
        (127, b"\x7f"),
        (128, b"\x00\x80"),
        (-1, b"\xff"),
        (-128, b"\x80"),
        (-129, b"\xff\x7f"),
    ],
)
def test_signed_literal_bytes_match_python_integer_transport(value, payload):
    assert integer_literal_bytes(value) == payload
    assert integer_literal_value(payload) == value


@pytest.mark.parametrize(
    "payload", [b"", b"\x00\x00", b"\x00\x7f", b"\xff\xff", b"\xff\x80", "123", [1]]
)
def test_literal_payload_refuses_stale_and_noncanonical_carriers(payload):
    with pytest.raises(ValueError):
        integer_literal_payload(payload)


def test_arbitrary_literals_and_optimizer_facts_do_not_use_decimal_conversion():
    previous = sys.get_int_max_str_digits()
    try:
        sys.set_int_max_str_digits(640)
        value = (1 << 20000) + 0x817F
        gen = SimpleTIRGenerator()
        for signed in (value, -value):
            payload = integer_literal_bytes(signed)
            assert integer_literal_value(payload) == signed
            op = MoltOp("CONST_BIGINT", [payload], MoltValue("big", "bigint"))
            context = SerializationContext([], set(), None)
            assert gen._serialize_basic_op(op, context)
            assert context.json_ops == [
                {"kind": "const_bigint", "bytes": list(payload), "out": "big"}
            ]
            from molt.frontend.lowering.midend_dataflow import primitive_const_values

            assert primitive_const_values({"big": op}) == {"big": signed}
            assert gen._const_cache_key_for_op(op) == (
                "CONST_BIGINT",
                ("bytes", payload),
            )
        tree = ast.parse("0x1" + "0" * 5000, mode="eval")
        before = tree.body.value
        key = literal_ast_dump(tree)
        assert "_IntegerLiteralPayload(signed_bytes=" in key
        assert tree.body.value is before
        assert key != literal_ast_dump(ast.parse("0x2" + "0" * 5000, mode="eval"))
        assert literal_ast_dump(ast.parse("127", mode="eval")) == ast.dump(
            ast.parse("127", mode="eval"), include_attributes=False
        )
    finally:
        sys.set_int_max_str_digits(previous)

from __future__ import annotations

import pytest

from molt.frontend import SimpleTIRGenerator


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("inside_try", [False, True])
@pytest.mark.parametrize(
    "value, kind",
    [
        ("non-interned literal", "CONST_STR"),
        (b"\x00\xff\x80", "CONST_BYTES"),
        (9223372036854775807, "CONST_BIGINT"),
        (123456789012345678901234567890, "CONST_BIGINT"),
    ],
)
def test_heap_literal_failures_follow_current_exception_edge(
    target: tuple[int, int], inside_try: bool, value: object, kind: str
) -> None:
    generator = SimpleTIRGenerator(module_name="literal_edges", target_python=target)
    generator.function_exception_label = 100
    generator.try_end_labels = [200] if inside_try else []
    before = len(generator.current_ops)
    result = generator._emit_const_value(value)
    emitted = generator.current_ops[before:]
    assert [op.kind for op in emitted] == [kind, "CHECK_EXCEPTION"]
    assert emitted[0].result == result
    assert emitted[1].args == [200 if inside_try else 100]


@pytest.mark.parametrize("value", [None, False, 42, 1.25])
def test_inline_literals_do_not_add_allocating_literal_checks(value: object) -> None:
    generator = SimpleTIRGenerator(module_name="literal_edges")
    generator.function_exception_label = 100
    before = len(generator.current_ops)
    generator._emit_const_value(value)
    assert all(op.kind != "CHECK_EXCEPTION" for op in generator.current_ops[before:])

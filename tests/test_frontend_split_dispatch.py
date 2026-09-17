"""Retained callable, exact-class guards, and ordered split-family lowering."""

from __future__ import annotations

import ast

import pytest

from molt.frontend import MoltOp, SimpleTIRGenerator


def _generate(source: str, *, phi: bool = False) -> SimpleTIRGenerator:
    generator = SimpleTIRGenerator(enable_phi=phi)
    generator.visit(ast.parse(source))
    return generator


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize("arguments", ["", "'|'", "maxsplit=limit(), sep=separator()"])
def test_unknown_split_captures_tagged_target_before_arguments(
    phi: bool, arguments: str
) -> None:
    generator = _generate(f"result = receiver().split({arguments})\n", phi=phi)
    ops = generator.funcs_map["molt_main"]["ops"]
    capture = next(op for op in ops if op.kind == "GETATTR_GENERIC_OBJ")
    assert capture.args[1] == "split"
    assert capture.args[0].type_hint == "Any"
    receiver = capture.args[0].name
    last_receiver_use = max(
        index
        for index, op in enumerate(ops)
        if any(getattr(arg, "name", None) == receiver for arg in op.args)
    )
    assert ops[last_receiver_use] is capture, (
        "generic receiver must not survive descriptor acquisition"
    )
    assert sum(op.kind == "TYPE_OF" for op in ops) == 1
    assert sum(op.kind == "IS" for op in ops) == 3
    suffix = "_MAX" if arguments.startswith("maxsplit") else ""
    for family in ("STRING", "BYTES", "BYTEARRAY"):
        intrinsic = next(op for op in ops if op.kind == family + "_SPLIT" + suffix)
        assert intrinsic.result.type_hint == "list"
    # Only the captured target reaches invocation, not a new attribute lookup.
    assert sum(op.kind == "GETATTR_GENERIC_OBJ" for op in ops) == 1
    generic_call = [op for op in ops if op.kind in {"CALL_FUNC", "CALL_INDIRECT"}][-1]
    assert generic_call.result.type_hint == "Any"
    assert not generic_call.args[0].name == receiver
    merged = [op for op in ops if op.kind == "PHI"][-1] if phi else None
    if merged is not None:
        assert merged.result.type_hint == "Any"
        assert merged.result.name not in generator.container_elem_hints
    if arguments.startswith("maxsplit"):
        calls = [op for op in ops if op.kind == "CALL_FUNC"]
        assert len(calls) == 3  # receiver, limit, separator: once each
        assert ops.index(capture) < ops.index(calls[1]) < ops.index(calls[2])
        push = [op for op in ops if op.kind == "CALLARGS_PUSH_KW"]
        assert [op.args[-1] for op in push] == [calls[1].result, calls[2].result]
        intrinsic = next(op for op in ops if op.kind == "STRING_SPLIT_MAX")
        assert intrinsic.args[1:] == [calls[2].result, calls[1].result]


@pytest.mark.parametrize(
    ("receiver", "separator", "opcode"),
    [("'a|b'", "'|'", "STRING_SPLIT"), ("b'a|b'", "b'|'", "BYTES_SPLIT")],
)
def test_exact_split_has_no_generic_call_or_runtime_guard(
    receiver: str, separator: str, opcode: str
) -> None:
    generator = _generate(f"result = {receiver}.split({separator})\n")
    ops = generator.funcs_map["molt_main"]["ops"]
    assert sum(op.kind == opcode for op in ops) == 1
    assert not any(
        op.kind in {"TYPE_OF", "IS", "CALL_FUNC", "CALL_INDIRECT"} for op in ops
    )


@pytest.mark.parametrize(
    ("receiver", "opcode"),
    [("'a|b'", "STRING_SPLIT_MAX"), ("b'a|b'", "BYTES_SPLIT_MAX")],
)
def test_exact_split_keyword_mapping_does_not_reorder_evaluation(
    receiver: str, opcode: str
) -> None:
    ops = _generate(
        f"result = {receiver}.split(maxsplit=limit(), sep=separator())\n"
    ).funcs_map["molt_main"]["ops"]
    calls = [op for op in ops if op.kind == "CALL_FUNC"]
    assert len(calls) == 2
    intrinsic = next(op for op in ops if op.kind == opcode)
    assert intrinsic.args[1:] == [calls[1].result, calls[0].result]
    assert not any(op.kind == "TYPE_OF" for op in ops)


@pytest.mark.parametrize("receiver", ["'a|b'", "source"])
@pytest.mark.parametrize(
    "arguments",
    [
        "first(), second(), third()",
        "first(), sep=second()",
        "first(), second(), maxsplit=third()",
        "unexpected=first()",
        "*arguments",
        "**keywords",
    ],
)
def test_unproved_split_signature_belongs_to_actual_callable(
    receiver: str, arguments: str
) -> None:
    ops = _generate(f"result = {receiver}.split({arguments})\n").funcs_map["molt_main"][
        "ops"
    ]
    assert not any("SPLIT" in op.kind for op in ops)
    assert any(op.kind in {"CALL_FUNC", "CALL_INDIRECT"} for op in ops)


@pytest.mark.parametrize("phi", [False, True])
@pytest.mark.parametrize(
    ("call", "minimum_cells", "opcode"),
    [
        ("source.split(first(), await second())", 3, "STRING_SPLIT_MAX"),
        ("source(first(), await second())", 2, "CALL_FUNC"),
        ("source(first(), later=await second())", 2, "CALL_INDIRECT"),
        ("[].insert(first(), await second())", 2, "LIST_INSERT"),
        ("[].append(await second())", 1, "LIST_APPEND"),
        ("[].extend(await second())", 1, "LIST_EXTEND"),
        ("[].remove(await second())", 1, "LIST_REMOVE"),
    ],
)
def test_suspending_call_consumes_capture_and_earlier_argument_cells(
    phi: bool, call: str, minimum_cells: int, opcode: str
) -> None:
    generator = _generate(
        f"async def run(source, first, second):\n    return {call}\n",
        phi=phi,
    )
    definition = next(
        op
        for op in generator.funcs_map["molt_main"]["ops"]
        if op.kind in {"FUNC_NEW", "FUNC_NEW_CLOSURE"}
        and op.metadata is not None
        and op.metadata.get("task_kind") == "coroutine"
    )
    ops: list[MoltOp] = generator.funcs_map[definition.args[0]]["ops"]
    none_values = {op.result.name for op in ops if op.kind == "CONST_NONE"}
    loads = [op for op in ops if op.kind == "LOAD_CLOSURE"]
    stores = [op for op in ops if op.kind == "STORE_CLOSURE"]
    cleared = {
        op.args[1] for op in stores if getattr(op.args[2], "name", None) in none_values
    }
    assert len(cleared) >= minimum_cells
    assert cleared <= {op.args[1] for op in loads}
    if opcode == "STRING_SPLIT_MAX":
        assert any(op.kind == "GETATTR_GENERIC_OBJ" for op in ops)
    invocation = [op for op in ops if op.kind == opcode][-1]
    # Pin the actual invoked receiver/callee to a consumed cell, not incidental
    # cells from the await protocol. This fails for the retired persistent spill.
    capture = next(op for op in loads if op.result.name == invocation.args[0].name)
    slot = capture.args[1]
    assert any(
        op.kind == "STORE_CLOSURE"
        and op.args[1] == slot
        and getattr(op.args[2], "name", None) in none_values
        for op in ops[ops.index(capture) + 1 : ops.index(invocation)]
    )
    if opcode in {"STRING_SPLIT_MAX", "LIST_INSERT", "CALL_FUNC"}:
        argument = next(op for op in loads if op.result.name == invocation.args[1].name)
        assert any(
            op.kind == "STORE_CLOSURE"
            and op.args[1] == argument.args[1]
            and getattr(op.args[2], "name", None) in none_values
            for op in ops[ops.index(argument) + 1 : ops.index(invocation)]
        )


@pytest.mark.parametrize(
    "method",
    [
        "union",
        "intersection",
        "difference",
        "update",
        "intersection_update",
        "difference_update",
    ],
)
def test_set_arguments_precede_all_iteration_and_mutation(method: str) -> None:
    ops = _generate(f"result = {{0}}.{method}(first(), second())\n").funcs_map[
        "molt_main"
    ]["ops"]
    calls = [index for index, op in enumerate(ops) if op.kind == "CALL_FUNC"]
    assert len(calls) == 2
    invocations = [
        index
        for index, op in enumerate(ops)
        if op.kind
        in {
            "ITER_NEW",
            "SET_UPDATE",
            "SET_INTERSECTION_UPDATE",
            "SET_DIFFERENCE_UPDATE",
            "BIT_AND",
            "SUB",
        }
    ]
    assert invocations and calls[-1] < min(invocations)


@pytest.mark.parametrize("method", ["append", "extend", "insert", "remove"])
@pytest.mark.parametrize(
    "arguments", ["", "1, 2, 3", "unexpected=callback()", "*values"]
)
def test_list_mutator_signature_errors_use_runtime_binding(
    method: str, arguments: str
) -> None:
    ops = _generate(f"[].{method}({arguments})\n").funcs_map["molt_main"]["ops"]
    assert not any(op.kind == "LIST_" + method.upper() for op in ops)
    assert any(op.kind in {"CALL_FUNC", "CALL_INDIRECT"} for op in ops)


@pytest.mark.parametrize("expression", ["f(*items)", "f(**items)", "f(1)"])
def test_pre_evaluated_builder_rejects_nonflat_or_incomplete_values(
    expression: str,
) -> None:
    generator = SimpleTIRGenerator()
    with pytest.raises(AssertionError, match="flat argument syntax"):
        generator._emit_call_args_builder(
            ast.parse(expression, mode="eval").body, evaluated=()
        )

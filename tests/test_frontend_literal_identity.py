from __future__ import annotations

import struct

import pytest

from molt.compiler_analysis.literal_identity import (
    literal_identity_key,
    same_literal_value,
)
from molt.compiler_analysis.static_truth import _same_scalar_value
from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend._types import _SCCP_OVERDEFINED, _SCCP_UNKNOWN
from molt.frontend.cfg_analysis import BasicBlock, CFGEdgeKind, CFGGraph, build_cfg
from molt.frontend.lowering.midend_dataflow import _same_sccp_state
from molt.frontend.lowering.serialization_context import SerializationContext


@pytest.mark.parametrize("defined", [False, True])
def test_identity_serialization_never_invents_or_redefines_none_inputs(
    defined: bool,
) -> None:
    gen = SimpleTIRGenerator()
    ctx = SerializationContext([], set(), None)
    operand = MoltValue("none_operand", "None")
    if defined:
        assert gen._serialize_basic_op(MoltOp("CONST_NONE", [], operand), ctx)
    comparison = MoltOp("IS", [operand, MoltValue("other")], MoltValue("same", "bool"))
    assert gen._serialize_basic_op(comparison, ctx)
    assert ctx.json_ops[-1] == {
        "kind": "is",
        "args": ["none_operand", "other"],
        "out": "same",
    }
    assert sum(op.get("out") == "none_operand" for op in ctx.json_ops) == int(defined)


def _float(bits: int) -> float:
    return struct.unpack("!d", bits.to_bytes(8, "big"))[0]


_DISTINCT_VALUES = [
    (True, 1),
    (1, 1.0),
    (False, 0),
    (0.0, -0.0),
    (complex(0.0, 0.0), complex(-0.0, 0.0)),
    (_float(0x7FF8000000000001), _float(0x7FF8000000000002)),
    (_float(0x7FF8000000000001), _float(0xFFF8000000000001)),
    ((True, (0.0,)), (1, (-0.0,))),
    (frozenset({True}), frozenset({1})),
    (range(0), range(1, 1)),
]


@pytest.mark.parametrize(("left", "right"), _DISTINCT_VALUES)
def test_literal_identity_preserves_exact_types_and_bits(
    left: object, right: object
) -> None:
    assert not same_literal_value(left, right)
    assert literal_identity_key(left) != literal_identity_key(right)
    assert not _same_sccp_state({"value": left}, {"value": right})


def test_nan_identity_is_bit_stable_without_python_equality() -> None:
    left, right = _float(0x7FF8000000000001), _float(0x7FF8000000000001)
    assert left != right
    assert same_literal_value(left, right)
    assert _same_sccp_state({"value": (left,)}, {"value": (right,)})
    assert not _same_scalar_value(
        left, right
    )  # static-truth policy remains conservative
    assert literal_identity_key(frozenset({left, right})) != literal_identity_key(
        frozenset({left})
    )


@pytest.mark.parametrize(("left", "right"), _DISTINCT_VALUES)
def test_dead_constant_anchors_use_exact_literal_identity(
    left: object, right: object
) -> None:
    ops = [
        MoltOp("CONST", [left], MoltValue("left")),
        MoltOp("CONST", [right], MoltValue("right")),
    ]
    assert SimpleTIRGenerator()._eliminate_dead_trivial_consts(ops) == []


def test_dead_constant_anchor_retains_exact_duplicate_without_callbacks() -> None:
    nan = _float(0x7FF8000000000001)
    duplicate = [
        MoltOp("CONST", [nan], MoltValue("first")),
        MoltOp("CONST", [_float(0x7FF8000000000001)], MoltValue("second")),
    ]
    assert SimpleTIRGenerator()._eliminate_dead_trivial_consts(duplicate) == [
        duplicate[0]
    ]

    class CallbackValue:
        def __hash__(self) -> int:
            raise AssertionError("DCE must not hash unsupported values")

        def __repr__(self) -> str:
            raise AssertionError("DCE must not stringify unsupported values")

    payload = CallbackValue()
    for value in [payload, [payload], {"value": payload}, (payload,)]:
        ops = [
            MoltOp("CONST", [value], MoltValue("first")),
            MoltOp("CONST", [value], MoltValue("second")),
        ]
        assert SimpleTIRGenerator()._eliminate_dead_trivial_consts(ops) == []


def test_mutable_and_subclass_values_never_become_literal_identity() -> None:
    class CallbackInt(int):
        def __eq__(self, other: object) -> bool:
            raise AssertionError("literal identity must not invoke equality callbacks")

        def __hash__(self) -> int:
            raise AssertionError("literal identity must not invoke hash callbacks")

    for value in [[], {}, set(), ([],), CallbackInt(1), (CallbackInt(1),)]:
        assert literal_identity_key(value) is None
        assert not same_literal_value(value, value)


def _phi_result(left: object, right: object) -> dict[str, object]:
    ops = [
        MoltOp("CONST", [left], MoltValue("left")),
        MoltOp("CONST", [right], MoltValue("right")),
        MoltOp("MISSING", [], MoltValue("condition")),
        MoltOp("IF", [MoltValue("condition")], MoltValue("none")),
        MoltOp("JUMP", ["join"], MoltValue("none")),
        MoltOp("JUMP", ["join"], MoltValue("none")),
        MoltOp("PHI", [MoltValue("left"), MoltValue("right")], MoltValue("joined")),
        MoltOp("TYPE_OF", [MoltValue("joined")], MoltValue("joined_type")),
        MoltOp("RETURN", [MoltValue("joined")], MoltValue("none")),
    ]
    blocks = [
        BasicBlock(0, 0, 4),
        BasicBlock(1, 4, 5),
        BasicBlock(2, 5, 6),
        BasicBlock(3, 6, 9),
    ]
    cfg = CFGGraph(
        blocks=blocks,
        index_to_block={
            index: block.id
            for block in blocks
            for index in range(block.start, block.end)
        },
        label_to_block={"join": 3},
        block_entry_label={3: "join"},
        control=build_cfg([]).control,
        successors={0: [1, 2], 1: [3], 2: [3], 3: []},
        edge_kinds={
            edge: CFGEdgeKind.NORMAL for edge in ((0, 1), (0, 2), (1, 3), (2, 3))
        },
        predecessors={0: [], 1: [0], 2: [0], 3: [1, 2]},
        reachable={0, 1, 2, 3},
        dominators={0: {0}, 1: {0, 1}, 2: {0, 2}, 3: {0, 3}},
    )
    gen = SimpleTIRGenerator()
    result = gen._compute_sccp(ops, cfg)
    assert result.executable_edges.issuperset({(1, 3), (2, 3)})
    assert gen.midend_stats["sccp_iteration_cap_hits"] == 0
    return result.out_values[3]


@pytest.mark.parametrize(("left", "right"), _DISTINCT_VALUES)
def test_sccp_phi_does_not_merge_python_equal_but_distinct_literals(
    left: object, right: object
) -> None:
    values = _phi_result(left, right)
    assert values["joined"] is _SCCP_OVERDEFINED
    assert "__tag__:joined" not in values
    assert values["joined_type"] is _SCCP_OVERDEFINED


@pytest.mark.parametrize(
    "value", [True, 1, -0.0, "text", b"bytes", (1, (False,)), frozenset({1}), range(3)]
)
def test_sccp_phi_preserves_equal_immutable_values(value: object) -> None:
    assert same_literal_value(_phi_result(value, value)["joined"], value)


def test_sccp_phi_nan_converges_by_payload() -> None:
    values = _phi_result(_float(0x7FF8000000000001), _float(0x7FF8000000000001))
    assert same_literal_value(values["joined"], _float(0x7FF8000000000001))


@pytest.mark.parametrize("value", [[], {}, set(), ([],)])
def test_sccp_rejects_mutable_contents_even_when_host_object_is_shared(
    value: object,
) -> None:
    assert _phi_result(value, value)["joined"] is _SCCP_OVERDEFINED


def test_region_markers_preserve_immutable_lattice_admission() -> None:
    ops = [
        MoltOp("TRY_START", [], MoltValue("none")),
        MoltOp("CONST", [[1]], MoltValue("mutable")),
        MoltOp("CONST", [0], MoltValue("index")),
        MoltOp("INDEX", [MoltValue("mutable"), MoltValue("index")], MoltValue("item")),
        MoltOp("TRY_END", [], MoltValue("none")),
        MoltOp("RETURN", [MoltValue("item")], MoltValue("none")),
    ]
    cfg = build_cfg(ops)
    result = SimpleTIRGenerator()._compute_sccp(ops, cfg)
    assert result.out_values[cfg.index_to_block[3]]["item"] is _SCCP_OVERDEFINED


def test_sccp_state_identity_handles_sentinels_and_key_presence() -> None:
    assert _same_sccp_state({"x": _SCCP_UNKNOWN}, {"x": _SCCP_UNKNOWN})
    assert not _same_sccp_state({"x": _SCCP_UNKNOWN}, {"x": _SCCP_OVERDEFINED})
    assert not _same_sccp_state({"x": 1}, {"y": 1})


@pytest.mark.parametrize(
    ("kind", "payload", "expected"),
    [
        ("CONST_BOOL", 1, True),
        ("CONST_FLOAT", 1, 1.0),
        ("CONST_BIGINT", "1", 1),
        ("CONST_INT", 1, 1),
        ("CONST", True, True),
    ],
)
def test_exact_numeric_map_uses_constructor_semantics(
    kind: str, payload: object, expected: object
) -> None:
    values = SimpleTIRGenerator._primitive_const_value_map(
        [MoltOp(kind, [payload], MoltValue("value"))]
    )
    assert same_literal_value(values["value"], expected)


@pytest.mark.parametrize(
    ("left", "right", "expected"),
    [
        (True, True, True),
        (None, None, True),
        (True, 1, False),
        (1, 1, _SCCP_OVERDEFINED),
        ("same", "same", _SCCP_OVERDEFINED),
    ],
)
def test_sccp_is_never_uses_host_interning(
    left: object, right: object, expected: object
) -> None:
    ops = [
        MoltOp("CONST", [left], MoltValue("left")),
        MoltOp("CONST", [right], MoltValue("right")),
        MoltOp("IS", [MoltValue("left"), MoltValue("right")], MoltValue("identity")),
        MoltOp("RETURN", [MoltValue("identity")], MoltValue("none")),
    ]
    result = SimpleTIRGenerator()._compute_sccp(ops, build_cfg(ops))
    assert result.out_values[0]["identity"] is expected

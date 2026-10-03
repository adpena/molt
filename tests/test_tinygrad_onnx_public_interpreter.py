from __future__ import annotations

import math
import struct

import pytest

from demos.tinygrad import onnx_interpreter as onnx
from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context
from tinygrad import Tensor
from tinygrad.dtypes import dtypes


def _node(
    op_type: str,
    inputs: list[str],
    outputs: list[str],
    attrs: dict[str, object] | None = None,
) -> dict[str, object]:
    return {
        "op_type": op_type,
        "inputs": inputs,
        "outputs": outputs,
        "attrs": attrs or {},
    }


def _varint(value: int) -> bytes:
    encoded = bytearray()
    while value >= 0x80:
        encoded.append((value & 0x7F) | 0x80)
        value >>= 7
    encoded.append(value)
    return bytes(encoded)


def _varint_field(field: int, value: int) -> bytes:
    return _varint(field << 3) + _varint(value)


def _bytes_field(field: int, value: bytes) -> bytes:
    return _varint((field << 3) | 2) + _varint(len(value)) + value


def _float_field(field: int, value: float) -> bytes:
    return _varint((field << 3) | 5) + struct.pack("<f", value)


def test_onnx_interpreter_imports_the_shipped_public_tensor() -> None:
    assert onnx.Tensor is Tensor
    assert onnx.Tensor.__module__ == "molt.gpu.tensor"


def test_public_tensor_construction_preserves_scalar_shape() -> None:
    scalar = onnx._make_tensor([3.5], ())

    assert scalar.shape == ()
    assert onnx._realize_floats(scalar) == [3.5]


def test_scalar_shape_survives_tensorproto_and_constant_loading() -> None:
    tensor = _varint_field(2, 1) + _float_field(4, 2.5) + _bytes_field(8, b"scalar")
    interpreter = onnx.OnnxInterpreter()
    interpreter.load_model(_bytes_field(7, _bytes_field(5, tensor)))

    assert interpreter._values["scalar"].shape == ()
    assert onnx._realize_floats(interpreter._values["scalar"]) == [2.5]

    constants: dict[str, Tensor] = {}
    onnx._load_constant_node(
        _node("Constant", [], ["integer"], {"value_int": 7}), constants
    )
    onnx._load_constant_node(
        _node("Constant", [], ["floating"], {"value_float": 1.25}), constants
    )
    assert constants["integer"].shape == ()
    assert constants["floating"].shape == ()


def test_w3_op_family_executes_through_the_real_interpreter() -> None:
    interpreter = onnx.OnnxInterpreter()
    interpreter._values = {
        "weight": onnx._make_tensor([1.0, -1.0, 0.5, 0.5], (2, 2)),
        "bias": onnx._make_tensor([0.25, -0.25], (2,)),
    }
    interpreter._graph_nodes = [
        _node("MatMul", ["input", "weight"], ["projection"]),
        _node("Add", ["projection", "bias"], ["biased"]),
        _node("Sin", ["biased"], ["sin"]),
        _node("Cos", ["biased"], ["cos"]),
        _node("Tanh", ["sin"], ["activation"]),
        _node("Exp", ["activation"], ["exp"]),
        _node(
            "ArgMax",
            ["activation"],
            ["partition_i64"],
            {"axis": -1, "keepdims": 0, "select_last_index": 0},
        ),
        _node("Cast", ["partition_i64"], ["partition"], {"to": 2}),
    ]
    interpreter._output_names = ["sin", "cos", "activation", "exp", "partition"]

    outputs = interpreter.run(
        {"input": onnx._make_tensor([1.0, 2.0, -1.0, 3.0], (2, 2))}
    )

    biased = [2.25, -0.25, 0.75, 2.25]
    sin_values = [math.sin(value) for value in biased]
    activation = [math.tanh(value) for value in sin_values]
    assert onnx._realize_floats(outputs["sin"]) == pytest.approx(sin_values)
    assert onnx._realize_floats(outputs["cos"]) == pytest.approx(
        [math.cos(value) for value in biased]
    )
    assert onnx._realize_floats(outputs["activation"]) == pytest.approx(activation)
    assert onnx._realize_floats(outputs["exp"]) == pytest.approx(
        [math.exp(value) for value in activation]
    )
    assert outputs["partition"].shape == (2,)
    assert outputs["partition"].dtype is dtypes.uint8
    assert onnx._realize_ints(outputs["partition"]) == [0, 1]


@pytest.mark.parametrize(
    ("select_last_index", "keepdims", "expected_shape", "expected"),
    [
        (0, 0, (2,), [1, 0]),
        (1, 1, (2, 1), [2, 3]),
    ],
)
def test_onnx_argmax_honors_tie_and_shape_semantics(
    select_last_index: int,
    keepdims: int,
    expected_shape: tuple[int, ...],
    expected: list[int],
) -> None:
    values = onnx._make_tensor([1.0, 3.0, 3.0, 2.0, 5.0, 5.0, 4.0, 5.0], (2, 4))

    result = onnx._op_argmax(
        [values],
        {
            "axis": -1,
            "keepdims": keepdims,
            "select_last_index": select_last_index,
        },
    )[0]

    assert result.shape == expected_shape
    assert result.dtype is dtypes.int64
    assert onnx._realize_ints(result) == expected


@pytest.mark.parametrize(
    ("attrs", "match"),
    [
        ({"axis": 2}, "axis"),
        ({"keepdims": 2}, "keepdims"),
        ({"select_last_index": 2}, "select_last_index"),
    ],
)
def test_onnx_argmax_rejects_invalid_attributes(
    attrs: dict[str, int], match: str
) -> None:
    values = onnx._make_tensor([1.0, 2.0], (1, 2))

    with pytest.raises(ValueError, match=match):
        onnx._op_argmax([values], attrs)


def test_onnx_argmax_rejects_an_empty_reduction_axis() -> None:
    values = onnx._make_tensor([], (2, 0))

    with pytest.raises(ValueError, match="empty axis"):
        onnx._op_argmax([values], {"axis": 1})


def test_onnx_matmul_preserves_rank_one_shape_rules() -> None:
    vector = onnx._make_tensor([1.0, 2.0], (2,))
    matrix = onnx._make_tensor([1.0, 2.0, 3.0, 4.0], (2, 2))

    vector_matrix = onnx._op_matmul([vector, matrix], {})[0]
    matrix_vector = onnx._op_matmul([matrix, vector], {})[0]
    vector_vector = onnx._op_matmul([vector, vector], {})[0]

    assert vector_matrix.shape == (2,)
    assert matrix_vector.shape == (2,)
    assert vector_vector.shape == ()
    assert onnx._realize_floats(vector_matrix) == [7.0, 10.0]
    assert onnx._realize_floats(matrix_vector) == [5.0, 11.0]
    assert onnx._realize_floats(vector_vector) == [5.0]


def test_onnx_matmul_broadcasts_batch_dimensions() -> None:
    lhs = onnx._make_tensor(
        [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        (2, 2, 3),
    )
    rhs = onnx._make_tensor([1.0, 0.0, 0.0, 1.0, 1.0, 1.0], (1, 3, 2))

    result = onnx._op_matmul([lhs, rhs], {})[0]

    assert result.shape == (2, 2, 2)
    assert onnx._realize_floats(result) == [4.0, 5.0, 10.0, 11.0, 0.0, 1.0, 2.0, 1.0]


def test_rank_one_matmul_semantics_match_the_reference_tensor() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        reference = modules["onnx_interpreter"]
        vector = reference._make_tensor([1.0, 2.0], (2,))
        matrix = reference._make_tensor([1.0, 2.0, 3.0, 4.0], (2, 2))

        assert reference._op_matmul([vector, matrix], {})[0].shape == (2,)
        assert reference._op_matmul([matrix, vector], {})[0].shape == (2,)
        assert reference._op_matmul([vector, vector], {})[0].shape == ()


def test_onnx_cast_supports_uint8_partition_without_dtype_drift() -> None:
    indices = onnx._make_int_tensor([0, 4, 2], (3,))

    partition = onnx._op_cast([indices], {"to": 2})[0]

    assert partition.dtype is dtypes.uint8
    assert onnx._realize_ints(partition) == [0, 4, 2]


def test_onnx_cast_requires_target_and_preserves_nonzero_bool_semantics() -> None:
    values = onnx._make_tensor([0.5, -0.5, 0.0, 2.0], (4,))

    with pytest.raises(ValueError, match="requires the 'to' attribute"):
        onnx._op_cast([values], {})

    result = onnx._op_cast([values], {"to": 9})[0]
    assert result.dtype is dtypes.bool_
    assert result.tolist() == [True, True, False, True]


def test_onnx_concat_rejects_missing_inputs_and_uses_balanced_copies(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    original_cat = onnx.Tensor.cat
    merged_sizes: list[int] = []

    def recording_cat(lhs: Tensor, rhs: Tensor, dim: int = 0) -> Tensor:
        result = original_cat(lhs, rhs, dim=dim)
        merged_sizes.append(result.size)
        return result

    monkeypatch.setattr(onnx.Tensor, "cat", recording_cat)
    inputs = [onnx._make_tensor([float(value)], (1,)) for value in range(8)]

    with pytest.raises(ValueError, match="every tensor input"):
        onnx._op_concat([inputs[0], None], {"axis": 0})
    with pytest.raises(ValueError, match="requires the 'axis' attribute"):
        onnx._op_concat(inputs, {})

    result = onnx._op_concat(inputs, {"axis": 0})[0]
    assert onnx._realize_floats(result) == list(map(float, range(8)))
    assert sum(merged_sizes) == 24

from __future__ import annotations

import pytest

from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


def test_onnx_conv_uses_general_path_for_asymmetric_strides() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(1, 16)], (1, 1, 3, 5))
        weight = onnx._make_tensor([1.0, 10.0], (1, 1, 1, 2))

        out = onnx._op_conv(
            [x, weight, None],
            {"strides": [1, 2], "pads": [0, 0, 0, 0], "dilations": [1, 1]},
        )[0]

        assert out.shape == (1, 1, 3, 2)
        assert onnx._realize_floats(out) == [21.0, 43.0, 76.0, 98.0, 131.0, 153.0]


def test_onnx_max_pool_dispatch_matches_nchw_reference() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(1, 13)], (1, 1, 3, 4))

        out = onnx._OP_DISPATCH["MaxPool"](
            [x],
            {"kernel_shape": [2, 2], "strides": [1, 2], "pads": [0, 0, 0, 0]},
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [6.0, 8.0, 10.0, 12.0]


def test_onnx_average_pool_excludes_padding_by_default() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0, 3.0, 4.0], (1, 1, 2, 2))

        out = onnx._op_average_pool(
            [x],
            {"kernel_shape": [2, 2], "strides": [1, 1], "pads": [1, 1, 0, 0]},
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [1.0, 1.5, 2.0, 2.5]


def test_onnx_average_pool_can_include_padding() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0, 3.0, 4.0], (1, 1, 2, 2))

        out = onnx._op_average_pool(
            [x],
            {
                "kernel_shape": [2, 2],
                "strides": [1, 1],
                "pads": [1, 1, 0, 0],
                "count_include_pad": 1,
            },
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [0.25, 0.75, 1.0, 2.5]


def test_onnx_interpreter_rejects_unimplemented_declared_outputs() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        interp = onnx.OnnxInterpreter()
        interp._values = {"x": onnx._make_tensor([1.0], (1,))}
        interp._graph_nodes = [
            {
                "op_type": "Identity",
                "inputs": ["x"],
                "outputs": ["y", "unimplemented_optional_output"],
                "attrs": {},
            }
        ]
        interp._output_names = ["y"]

        with pytest.raises(ValueError, match="produced 1 outputs for 2 declared"):
            interp.run({})

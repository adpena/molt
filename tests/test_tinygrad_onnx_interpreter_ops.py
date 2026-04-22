from __future__ import annotations

import pytest

from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


@pytest.mark.parametrize(
    ("nearest_mode", "expected"),
    [
        ("round_prefer_floor", [10.0, 10.0, 20.0, 20.0]),
        ("round_prefer_ceil", [10.0, 20.0, 20.0, 20.0]),
        ("floor", [10.0, 10.0, 20.0, 20.0]),
        ("ceil", [10.0, 20.0, 20.0, 20.0]),
    ],
)
def test_onnx_resize_nearest_mode_variants(
    nearest_mode: str, expected: list[float]
) -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([10.0, 20.0], (1, 1, 2, 1))

        out = onnx._op_resize(
            [
                x,
                None,
                None,
                onnx._make_int_tensor([1, 1, 4, 1], (4,)),
            ],
            {
                "mode": "nearest",
                "coordinate_transformation_mode": "asymmetric",
                "nearest_mode": nearest_mode,
            },
        )[0]

        assert out.shape == (1, 1, 4, 1)
        assert onnx._realize_floats(out) == expected


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


def test_onnx_max_pool_auto_pad_same_upper() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(1, 10)], (1, 1, 3, 3))

        out = onnx._op_max_pool(
            [x],
            {
                "auto_pad": "SAME_UPPER",
                "kernel_shape": [2, 2],
                "strides": [2, 2],
            },
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [5.0, 6.0, 8.0, 9.0]


def test_onnx_max_pool_auto_pad_same_lower() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(1, 10)], (1, 1, 3, 3))

        out = onnx._op_max_pool(
            [x],
            {
                "auto_pad": "SAME_LOWER",
                "kernel_shape": [2, 2],
                "strides": [2, 2],
            },
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [1.0, 3.0, 7.0, 9.0]


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


def test_onnx_average_pool_honors_ceil_mode() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(1, 10)], (1, 1, 3, 3))

        out = onnx._op_average_pool(
            [x],
            {
                "kernel_shape": [2, 2],
                "strides": [2, 2],
                "pads": [0, 0, 0, 0],
                "ceil_mode": 1,
            },
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [3.0, 4.5, 7.5, 9.0]


def test_onnx_average_pool_auto_pad_same_upper() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(1, 10)], (1, 1, 3, 3))

        out = onnx._op_average_pool(
            [x],
            {
                "auto_pad": "SAME_UPPER",
                "kernel_shape": [2, 2],
                "strides": [2, 2],
            },
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [3.0, 4.5, 7.5, 9.0]


def test_onnx_average_pool_auto_pad_same_lower() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(1, 10)], (1, 1, 3, 3))

        out = onnx._op_average_pool(
            [x],
            {
                "auto_pad": "SAME_LOWER",
                "kernel_shape": [2, 2],
                "strides": [2, 2],
            },
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [1.0, 2.5, 5.5, 7.0]


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


def test_onnx_interpreter_identity_elimination_rewrites_graph_outputs() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        interp = onnx.OnnxInterpreter()
        interp._values = {"x": onnx._make_tensor([1.0], (1,))}
        interp._graph_nodes = [
            {"op_type": "Identity", "inputs": ["x"], "outputs": ["y"], "attrs": {}}
        ]
        interp._output_names = ["y"]

        interp._eliminate_identity()
        outputs = interp.run({})

        assert list(outputs) == ["y"]
        assert onnx._realize_floats(outputs["y"]) == [1.0]


def test_onnx_slice_honors_positive_steps() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(12)], (3, 4))

        out = onnx._op_slice(
            [
                x,
                onnx._make_int_tensor([0, 0], (2,)),
                onnx._make_int_tensor([3, 4], (2,)),
                onnx._make_int_tensor([0, 1], (2,)),
                onnx._make_int_tensor([2, 2], (2,)),
            ],
            {},
        )[0]

        assert out.shape == (2, 2)
        assert onnx._realize_floats(out) == [0.0, 2.0, 8.0, 10.0]


def test_onnx_slice_honors_negative_steps() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([float(i) for i in range(6)], (6,))

        out = onnx._op_slice(
            [
                x,
                onnx._make_int_tensor([4], (1,)),
                onnx._make_int_tensor([0], (1,)),
                onnx._make_int_tensor([0], (1,)),
                onnx._make_int_tensor([-2], (1,)),
            ],
            {},
        )[0]

        assert out.shape == (2,)
        assert onnx._realize_floats(out) == [4.0, 2.0]


def test_onnx_resize_honors_align_corners_for_nearest() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([10.0, 20.0, 30.0], (1, 1, 3, 1))

        out = onnx._op_resize(
            [
                x,
                None,
                None,
                onnx._make_int_tensor([1, 1, 4, 1], (4,)),
            ],
            {
                "mode": "nearest",
                "coordinate_transformation_mode": "align_corners",
            },
        )[0]

        assert out.shape == (1, 1, 4, 1)
        assert onnx._realize_floats(out) == [10.0, 20.0, 20.0, 30.0]


def test_onnx_resize_defaults_to_half_pixel_coordinates_for_nearest() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([10.0, 20.0, 30.0, 40.0, 50.0], (1, 1, 5, 1))

        out = onnx._op_resize(
            [
                x,
                None,
                None,
                onnx._make_int_tensor([1, 1, 3, 1], (4,)),
            ],
            {"mode": "nearest"},
        )[0]

        assert out.shape == (1, 1, 3, 1)
        assert onnx._realize_floats(out) == [10.0, 30.0, 50.0]


def test_onnx_resize_rejects_unsupported_interpolation_modes() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([10.0, 20.0, 30.0], (1, 1, 3, 1))

        with pytest.raises(ValueError, match="Unsupported Resize mode"):
            onnx._op_resize(
                [
                    x,
                    None,
                    None,
                    onnx._make_int_tensor([1, 1, 4, 1], (4,)),
                ],
                {"mode": "linear"},
            )


def test_onnx_resize_rejects_both_scales_and_sizes() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([10.0, 20.0], (1, 1, 2, 1))

        with pytest.raises(
            ValueError, match="Resize cannot specify both scales and sizes"
        ):
            onnx._op_resize(
                [
                    x,
                    None,
                    onnx._make_tensor([1.0, 1.0, 2.0, 1.0], (4,)),
                    onnx._make_int_tensor([1, 1, 4, 1], (4,)),
                ],
                {"mode": "nearest"},
            )


def test_onnx_resize_rejects_missing_scales_and_sizes() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([10.0, 20.0], (1, 1, 2, 1))

        with pytest.raises(ValueError, match="Resize requires scales or sizes"):
            onnx._op_resize([x, None, None, None], {"mode": "nearest"})


def test_onnx_slice_rejects_missing_starts_and_ends() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0], (2,))

        with pytest.raises(ValueError, match="Slice requires starts and ends"):
            onnx._op_slice([x], {})


def test_onnx_cast_rejects_unsupported_target_type() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0], (1,))

        with pytest.raises(ValueError, match="Unsupported Cast target"):
            onnx._op_cast([x], {"to": 12345})


def test_onnx_squeeze_rejects_non_unit_explicit_axis() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0], (2,))

        with pytest.raises(ValueError, match="cannot squeeze dimension"):
            onnx._op_squeeze([x, onnx._make_int_tensor([0], (1,))], {})


def test_onnx_unsqueeze_rejects_out_of_range_axis() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0], (2,))

        with pytest.raises(ValueError, match="Unsqueeze axis"):
            onnx._op_unsqueeze([x, onnx._make_int_tensor([3], (1,))], {})


def test_onnx_matmul_rejects_missing_rhs() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0], (1, 2))

        with pytest.raises(ValueError, match="MatMul requires two tensor inputs"):
            onnx._op_matmul([x, None], {})


def test_onnx_matmul_rejects_unsupported_rank_with_value_error() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        lhs = onnx._make_tensor([1.0], (1, 1, 1))
        rhs = onnx._make_tensor([1.0], (1,))

        with pytest.raises(ValueError, match="MatMul"):
            onnx._op_matmul([lhs, rhs], {})


def test_onnx_reshape_rejects_missing_shape() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0], (2,))

        with pytest.raises(ValueError, match="Reshape requires shape"):
            onnx._op_reshape([x], {})


def test_onnx_reshape_rejects_shape_numel_mismatch() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0, 3.0], (3,))

        with pytest.raises(ValueError, match="cannot reshape"):
            onnx._op_reshape([x, onnx._make_int_tensor([2, 2], (2,))], {})


def test_onnx_reshape_rejects_multiple_inferred_dims() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0, 3.0, 4.0], (4,))

        with pytest.raises(ValueError, match="at most one -1"):
            onnx._op_reshape([x, onnx._make_int_tensor([-1, -1], (2,))], {})


def test_onnx_batch_norm_rejects_missing_inputs() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0], (1, 1, 1, 1))

        with pytest.raises(ValueError, match="BatchNormalization requires"):
            onnx._op_batch_norm([x, None, None, None, None], {})


def test_onnx_transpose_rejects_invalid_perm() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0, 3.0, 4.0], (2, 2))

        with pytest.raises(ValueError, match="Transpose perm"):
            onnx._op_transpose([x], {"perm": [0, 0]})


def test_onnx_reduce_mean_rejects_duplicate_axes() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0, 3.0, 4.0], (2, 2))

        with pytest.raises(ValueError, match="ReduceMean axes"):
            onnx._op_reduce_mean([x], {"axes": [0, 0]})


def test_onnx_reduce_mean_rejects_out_of_range_axes() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0, 2.0, 3.0, 4.0], (2, 2))

        with pytest.raises(ValueError, match="ReduceMean axis"):
            onnx._op_reduce_mean([x], {"axes": [2]})


def test_onnx_concat_rejects_empty_input_list() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]

        with pytest.raises(ValueError, match="Concat requires at least one tensor"):
            onnx._op_concat([], {})


def test_onnx_conv_rejects_non_divisible_group_count() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0] * 12, (1, 3, 2, 2))
        weight = onnx._make_tensor([1.0] * 4, (4, 1, 1, 1))

        with pytest.raises(ValueError, match="input channels.*divisible"):
            onnx._op_conv([x, weight, None], {"group": 2})


def test_onnx_conv_rejects_group_weight_channel_mismatch() -> None:
    with tinygrad_stdlib_context("onnx_interpreter") as modules:
        onnx = modules["onnx_interpreter"]
        x = onnx._make_tensor([1.0] * 16, (1, 4, 2, 2))
        weight = onnx._make_tensor([1.0] * 4, (4, 1, 1, 1))

        with pytest.raises(ValueError, match="weight input channels"):
            onnx._op_conv([x, weight, None], {"group": 2})

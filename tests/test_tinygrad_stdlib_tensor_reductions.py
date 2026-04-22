from __future__ import annotations

from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


def test_axis_reductions_use_input_shape_not_output_shape() -> None:
    with tinygrad_stdlib_context() as modules:
        Tensor = modules["tensor"].Tensor
        realize = modules["realize"].realize
        tensor = Tensor([[[0.10, 0.80, 0.10]]])

        assert realize(tensor.max(axis=-1).lazydata) == [0.80]
        assert realize(tensor.sum(axis=-1).lazydata) == [1.0]


def test_softmax_axis_rows_are_normalized_probabilities() -> None:
    with tinygrad_stdlib_context() as modules:
        Tensor = modules["tensor"].Tensor
        realize = modules["realize"].realize
        tensor = Tensor([[[0.10, 0.80, 0.10]]])

        out = realize(tensor.softmax(axis=-1).lazydata)

        assert abs(sum(out) - 1.0) < 1e-12
        assert out[1] > out[0]
        assert out[1] > out[2]

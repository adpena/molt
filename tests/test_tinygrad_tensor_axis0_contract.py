from __future__ import annotations

from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


def test_tinygrad_tensor_axis0_gather_preserves_row_slices() -> None:
    with tinygrad_stdlib_context() as modules:
        Tensor = modules["tensor"].Tensor

        table = Tensor([[10.0, 11.0], [20.0, 21.0], [30.0, 31.0]])
        gathered = table.gather(0, Tensor([2, 0]))

        assert gathered.shape == (2, 2)
        assert gathered.tolist() == [[30.0, 31.0], [10.0, 11.0]]


def test_tinygrad_tensor_axis0_scatter_updates_row_slices() -> None:
    with tinygrad_stdlib_context() as modules:
        Tensor = modules["tensor"].Tensor

        base = Tensor([[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]])
        updates = Tensor([[7.0, 7.0], [8.0, 8.0]])
        scattered = base.scatter(0, Tensor([0, 2]), updates)

        assert scattered.shape == (3, 2)
        assert scattered.tolist() == [[7.0, 7.0], [1.0, 1.0], [8.0, 8.0]]

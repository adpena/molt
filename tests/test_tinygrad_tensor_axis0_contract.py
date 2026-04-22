from __future__ import annotations

import importlib
import sys
import types
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def _load_isolated_tinygrad_tensor():
    for name in list(sys.modules):
        if name == "tinygrad" or name.startswith("tinygrad."):
            sys.modules.pop(name)
    intrinsics = types.ModuleType("_intrinsics")
    intrinsics.require_intrinsic = lambda _name: lambda *args, **kwargs: None
    sys.modules["_intrinsics"] = intrinsics
    pkg = types.ModuleType("tinygrad")
    pkg.__path__ = [str(ROOT / "src/molt/stdlib/tinygrad")]
    sys.modules["tinygrad"] = pkg
    return importlib.import_module("tinygrad.tensor").Tensor


def test_tinygrad_tensor_axis0_gather_preserves_row_slices() -> None:
    Tensor = _load_isolated_tinygrad_tensor()

    table = Tensor([[10.0, 11.0], [20.0, 21.0], [30.0, 31.0]])
    gathered = table.gather(0, Tensor([2, 0]))

    assert gathered.shape == (2, 2)
    assert gathered.tolist() == [[30.0, 31.0], [10.0, 11.0]]


def test_tinygrad_tensor_axis0_scatter_updates_row_slices() -> None:
    Tensor = _load_isolated_tinygrad_tensor()

    base = Tensor([[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]])
    updates = Tensor([[7.0, 7.0], [8.0, 8.0]])
    scattered = base.scatter(0, Tensor([0, 2]), updates)

    assert scattered.shape == (3, 2)
    assert scattered.tolist() == [[7.0, 7.0], [1.0, 1.0], [8.0, 8.0]]

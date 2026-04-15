"""
tinygrad — Tinygrad-conformant tensor API for molt-gpu.

Usage:
    from tinygrad import Tensor, dtypes

    a = Tensor([1.0, 2.0, 3.0])
    b = Tensor([4.0, 5.0, 6.0])
    c = (a + b).relu()
    print(c.numpy())
"""

from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes
from tinygrad.device import Device

__all__ = ["Tensor", "dtypes", "Device"]

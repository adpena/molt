"""tinygrad compatibility surface backed by Molt tensor primitives."""

from . import dtypes, nn
from .tensor import Tensor

__all__ = ["Tensor", "dtypes", "nn"]

"""tinygrad compatibility surface backed by Molt tensor primitives."""

from . import dtypes, nn
from molt.gpu.tensor import Tensor

__all__ = ["Tensor", "dtypes", "nn"]

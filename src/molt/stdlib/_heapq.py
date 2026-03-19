"""Intrinsic-backed `_heapq` compatibility surface."""

from _intrinsics import require_intrinsic as _require_intrinsic
from heapq import heapify
from heapq import heappop
from heapq import heappush
from heapq import heappushpop
from heapq import heapreplace

_MOLT_HEAPQ_HEAPIFY = _require_intrinsic("molt_heapq_heapify")

__all__ = [
    "heapify",
    "heappop",
    "heappush",
    "heappushpop",
    "heapreplace",
]


globals().pop("_require_intrinsic", None)

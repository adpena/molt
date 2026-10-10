"""Intrinsic-backed `_heapq` compatibility surface."""

from heapq import heapify
from heapq import heappop
from heapq import heappush
from heapq import heappushpop
from heapq import heapreplace


__all__ = [
    "heapify",
    "heappop",
    "heappush",
    "heappushpop",
    "heapreplace",
]

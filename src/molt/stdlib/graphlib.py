"""Intrinsic-backed graphlib implementation (Python 3.12+)."""

from __future__ import annotations

from types import GenericAlias

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_GRAPHLIB_NEW = _require_intrinsic("molt_graphlib_new")
_MOLT_GRAPHLIB_ADD = _require_intrinsic("molt_graphlib_add")
_MOLT_GRAPHLIB_PREPARE = _require_intrinsic("molt_graphlib_prepare")
_MOLT_GRAPHLIB_GET_READY = _require_intrinsic("molt_graphlib_get_ready")
_MOLT_GRAPHLIB_IS_ACTIVE = _require_intrinsic("molt_graphlib_is_active")
_MOLT_GRAPHLIB_DONE = _require_intrinsic("molt_graphlib_done")
_MOLT_GRAPHLIB_STATIC_ORDER = _require_intrinsic(
    "molt_graphlib_static_order"
)
_MOLT_GRAPHLIB_DROP = _require_intrinsic("molt_graphlib_drop")

__all__ = ["TopologicalSorter", "CycleError"]

_NODE_OUT = -1
_NODE_DONE = -2


class CycleError(ValueError):
    """Raised by TopologicalSorter.prepare when cycles are present."""


class _NodeInfo:
    __slots__ = ("node", "npredecessors", "successors")

    def __init__(self, node):
        self.node = node
        self.npredecessors = 0
        self.successors = []


class TopologicalSorter:
    """Provides functionality to topologically sort a graph of hashable nodes."""

    def __init__(self, graph=None):
        self._handle = _MOLT_GRAPHLIB_NEW()

        if graph is not None:
            for node, predecessors in graph.items():
                self.add(node, *predecessors)

    def add(self, node, *predecessors):
        _MOLT_GRAPHLIB_ADD(self._handle, node, predecessors)

    def prepare(self):
        cycle = _MOLT_GRAPHLIB_PREPARE(self._handle)
        if cycle is not None:
            raise CycleError("nodes are in a cycle", cycle)

    def get_ready(self):
        return _MOLT_GRAPHLIB_GET_READY(self._handle)

    def is_active(self):
        return bool(_MOLT_GRAPHLIB_IS_ACTIVE(self._handle))

    def __bool__(self):
        return self.is_active()

    def done(self, *nodes):
        _MOLT_GRAPHLIB_DONE(self._handle, nodes)

    def static_order(self):
        def _iter():
            ok, payload = _MOLT_GRAPHLIB_STATIC_ORDER(self._handle)
            if not ok:
                raise CycleError("nodes are in a cycle", payload)
            yield from payload

        return _iter()

    def __del__(self):
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        self._handle = None
        try:
            _MOLT_GRAPHLIB_DROP(handle)
        except Exception:
            return

    @classmethod
    def __class_getitem__(cls, item):
        return GenericAlias(cls, item)

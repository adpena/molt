"""Graphlib shim for Molt (TopologicalSorter)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["CycleError", "TopologicalSorter"]

_MOLT_GRAPHLIB_NEW = _require_intrinsic("molt_graphlib_new", globals())
_MOLT_GRAPHLIB_ADD = _require_intrinsic("molt_graphlib_add", globals())
_MOLT_GRAPHLIB_PREPARE = _require_intrinsic("molt_graphlib_prepare", globals())
_MOLT_GRAPHLIB_GET_READY = _require_intrinsic("molt_graphlib_get_ready", globals())
_MOLT_GRAPHLIB_DONE = _require_intrinsic("molt_graphlib_done", globals())
_MOLT_GRAPHLIB_IS_ACTIVE = _require_intrinsic("molt_graphlib_is_active", globals())

_CYCLE_MARKER = "__molt_graphlib_cycle__"


class CycleError(ValueError):
    pass


def _raise_if_cycle(value):
    if (
        isinstance(value, tuple)
        and len(value) == 2
        and value[0] == _CYCLE_MARKER
    ):
        raise CycleError("nodes are in a cycle", value[1])
    return value


class TopologicalSorter:
    __slots__ = ("_molt_graphlib_state",)

    def __init__(self, graph=None):
        self._molt_graphlib_state = _MOLT_GRAPHLIB_NEW(graph)

    def add(self, node, /, *predecessors):
        _MOLT_GRAPHLIB_ADD(self._molt_graphlib_state, node, predecessors)

    def prepare(self):
        _raise_if_cycle(_MOLT_GRAPHLIB_PREPARE(self._molt_graphlib_state))

    def get_ready(self):
        return _raise_if_cycle(_MOLT_GRAPHLIB_GET_READY(self._molt_graphlib_state))

    def done(self, /, *nodes):
        _MOLT_GRAPHLIB_DONE(self._molt_graphlib_state, nodes)

    def is_active(self):
        return bool(_MOLT_GRAPHLIB_IS_ACTIVE(self._molt_graphlib_state))

    def static_order(self):
        self.prepare()
        while self.is_active():
            ready = self.get_ready()
            for node in ready:
                yield node
            if ready:
                self.done(*ready)

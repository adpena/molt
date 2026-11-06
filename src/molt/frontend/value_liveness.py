"""One liveness authority for frontend dataflow and suspended-frame transport."""

from __future__ import annotations

from collections import deque
from collections.abc import Sequence
from molt.frontend.cfg_analysis import CFGGraph, build_cfg
from molt.frontend._types import MoltOp, MoltValue, TypeHint, loaded_method_self_name
from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
    FrontendOpKind,
    FRONTEND_FIRST_TRAILING_RESULT_ARG,
)

# Names that are frame state rather than SSA values.
_INIT_KINDS_1: frozenset[FrontendOpKind] = frozenset(
    {"CALLARGS_PUSH_METHOD_RECEIVER", "CALL_LOADED_METHOD"}
)
_INIT_KINDS_2: frozenset[FrontendOpKind] = frozenset(
    {"LOAD_METHOD", "LOAD_SUPER_METHOD"}
)


_VALUE_LIVENESS_EXEMPT = frozenset({"self", "none", ""})


class ValueLiveness:
    """Value liveness over the canonical frontend CFG.

    The CFG gives STATE_SWITCH a resume edge to every STATE_LABEL, and gives
    loops their back edges and checks their exception edges, so a value is live
    into a label exactly when some path from the label uses it before any op
    defines it again. Bit ``i`` of a block's live set is name ``i``.
    """

    def __init__(self, ops: Sequence[MoltOp], *, cfg: CFGGraph | None = None) -> None:
        self.cfg = cfg if cfg is not None else build_cfg(ops)
        self.phi_edges = self.cfg.phi_edge_table()
        self.names: list[str] = []
        self.def_sites: dict[str, list[int]] = {}
        self.type_hints: dict[str, TypeHint] = {}
        # A method load's `self_or_null` word is a wire name, not a frontend
        # value; it is tracked here so that a resume can never lose it.
        self.loaded_method_selves: set[str] = set()
        index: dict[str, int] = {}

        def bit(value: MoltValue) -> int:
            position = index.get(value.name)
            if position is None:
                position = index[value.name] = len(self.names)
                self.names.append(value.name)
            if value.type_hint:
                self.type_hints.setdefault(value.name, value.type_hint)
            return 1 << position

        blocks = self.cfg.blocks
        uses = [0] * len(blocks)
        defs = [0] * len(blocks)
        edge_uses: dict[tuple[int, int], int] = {}
        for block in blocks:
            used = defined = 0
            for idx in range(block.start, block.end):
                op = ops[idx]
                # The generated field authority names the ops whose trailing
                # args are outputs (UNPACK_SEQUENCE's targets), not operands.
                first_output = FRONTEND_FIRST_TRAILING_RESULT_ARG.get(
                    op.kind, len(op.args)
                )
                operands = [
                    arg for arg in op.args[:first_output] if isinstance(arg, MoltValue)
                ]
                outputs = [op.result, *op.args[first_output:]]
                if op.kind in _INIT_KINDS_1:
                    operands.append(MoltValue(loaded_method_self_name(op)))
                elif op.kind in _INIT_KINDS_2:
                    self_name = loaded_method_self_name(op)
                    self.loaded_method_selves.add(self_name)
                    outputs.append(MoltValue(self_name))
                phi_edges = self.phi_edges[block.id]
                if op.kind == FrontendKinds.PHI and len(op.args) == len(phi_edges) > 1:
                    for arg, edge in zip(op.args, phi_edges):
                        if (
                            isinstance(arg, MoltValue)
                            and arg.name not in _VALUE_LIVENESS_EXEMPT
                        ):
                            edge_uses[edge] = edge_uses.get(edge, 0) | bit(arg)
                    operands = []
                for arg in operands:
                    if arg.name not in _VALUE_LIVENESS_EXEMPT:
                        mask = bit(arg)
                        if not defined & mask:
                            used |= mask
                for output in outputs:
                    if (
                        isinstance(output, MoltValue)
                        and output.name not in _VALUE_LIVENESS_EXEMPT
                    ):
                        defined |= bit(output)
                        self.def_sites.setdefault(output.name, []).append(idx)
            uses[block.id] = used
            defs[block.id] = defined
        live_in = [0] * len(blocks)
        live_out = [0] * len(blocks)
        queue = deque(sorted(self.cfg.reachable, reverse=True))
        queued = set(queue)
        while queue:
            block_id = queue.popleft()
            queued.remove(block_id)
            outgoing = 0
            for successor in self.cfg.successors[block_id]:
                outgoing |= live_in[successor] | edge_uses.get((block_id, successor), 0)
            live_out[block_id] = outgoing
            updated = uses[block_id] | (outgoing & ~defs[block_id])
            if updated != live_in[block_id]:
                live_in[block_id] = updated
                for predecessor in self.cfg.predecessors[block_id]:
                    if predecessor in self.cfg.reachable and predecessor not in queued:
                        queue.append(predecessor)
                        queued.add(predecessor)
        self._live_in = live_in
        self._live_out = live_out

    def live_out_of(self, block_id: int) -> list[str]:
        return self._names_in(self._live_out[block_id])

    def _names_in(self, mask: int) -> list[str]:
        names: list[str] = []
        while mask:
            lowest = mask & -mask
            names.append(self.names[lowest.bit_length() - 1])
            mask ^= lowest
        return names

    def live_into(self, op_index: int) -> list[str]:
        """The names live on entry to the block that starts at ``op_index``."""
        block = self.cfg.block_for_op(op_index)
        if block is None:
            raise KeyError(op_index)
        return self._names_in(self._live_in[block])

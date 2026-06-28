"""MidendDataflowMixin: frontend IR definite-assignment, DCE, SCCP, and phi/dataflow repair."""

from __future__ import annotations

from collections import deque
from typing import TYPE_CHECKING, Any, Sequence, cast

from molt.frontend._types import (
    CFGGraph,
    MoltOp,
    MoltValue,
    SCCPResult,
    _SCCP_MISSING,
    _SCCP_OVERDEFINED,
    _SCCP_UNKNOWN,
    build_cfg,
)
from molt.frontend.lowering.op_kinds_generated import (
    FRONTEND_RAISING_NOTHROW_ON_PRIMITIVES_KINDS,
    RAISING_KIND_NAMES,
)

_PRIMITIVE_CONST_KINDS: frozenset[str] = frozenset(
    {"CONST", "CONST_BOOL", "CONST_BIGINT", "CONST_FLOAT", "CONST_INT"}
)

_SCCP_PROVEN_NOTHROW_CURATED: frozenset[str] = frozenset(
    {
        "LINE",
        "IF",
        "ELSE",
        "END_IF",
        "LOOP_START",
        "LOOP_END",
        "LOOP_BREAK",
        "LOOP_BREAK_IF_TRUE",
        "LOOP_BREAK_IF_FALSE",
        "LOOP_BREAK_IF_EXCEPTION",
        "LOOP_CONTINUE",
        "TRY_START",
        "TRY_END",
        "JUMP",
        "LABEL",
        "STATE_LABEL",
        "PHI",
        "CONST",
        "CONST_BIGINT",
        "CONST_BOOL",
        "CONST_FLOAT",
        "CONST_STR",
        "CONST_BYTES",
        "CONST_NONE",
        "CONST_NOT_IMPLEMENTED",
        "CONST_ELLIPSIS",
        "MISSING",
        "NOT",
        "IS",
        "TYPE_OF",
        "LEN",
        "EXCEPTION_NEW_BUILTIN",
        "EXCEPTION_NEW_BUILTIN_EMPTY",
        "EXCEPTION_NEW_BUILTIN_ONE",
        "EXCEPTION_MATCH_BUILTIN",
        "STORE_VAR",
        "DELETE_VAR",
        "LOAD_VAR",
    }
)
_SCCP_PROVEN_NOTHROW_KINDS: frozenset[str] = (
    _SCCP_PROVEN_NOTHROW_CURATED - RAISING_KIND_NAMES
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class MidendDataflowMixin(_MixinBase):
    def _compute_block_use_def(self, ops: list[MoltOp]) -> tuple[set[str], set[str]]:
        use: set[str] = set()
        defs: set[str] = set()
        for op in ops:
            arg_names: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, arg_names)
            use.update(name for name in arg_names if name not in defs)
            out_name = op.result.name
            if out_name != "none":
                defs.add(out_name)
        return use, defs

    def _find_unbound_value_uses(
        self, ops: list[MoltOp], *, params: Sequence[str] = ()
    ) -> list[tuple[int, str, str]]:
        defined: set[str] = set(params)
        defined.update(self._collect_defined_value_names(ops))
        missing: list[tuple[int, str, str]] = []
        for idx, op in enumerate(ops):
            used_names: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, used_names)
            for name in sorted(used_names):
                if name != "none" and name not in defined:
                    missing.append((idx, op.kind, name))
        return missing

    def _infer_predefined_value_names(self, ops: list[MoltOp]) -> set[str]:
        used: set[str] = set()
        for op in ops:
            for arg in op.args:
                self._collect_arg_value_names(arg, used)
        defined = self._collect_defined_value_names(ops)
        return used - defined

    def _verify_definite_assignment_in_ops(
        self,
        ops: list[MoltOp],
        *,
        predefined_value_names: set[str] | None = None,
    ) -> list[tuple[int, str, str]]:
        if not ops:
            return []

        predefined = set(predefined_value_names or set())
        cfg: CFGGraph = build_cfg(ops)
        if not cfg.blocks:
            return []
        all_defs = self._collect_defined_value_names(ops).union(predefined)

        # Track which value names are produced by MISSING ops so we can
        # verify they haven't been eliminated by a prior pass.
        missing_value_defs: set[str] = set()
        for op in ops:
            if op.kind == "MISSING" and op.result.name != "none":
                missing_value_defs.add(op.result.name)

        # Propagate MISSING taint transitively through PHI nodes: if every
        # input to a PHI is MISSING-tainted, the PHI result is also tainted.
        # This catches cases where branch pruning collapses a PHI to a single
        # MISSING-carrying input that escapes into CALL arg positions.
        missing_tainted: set[str] = set(missing_value_defs)
        _phi_changed = True
        while _phi_changed:
            _phi_changed = False
            for op in ops:
                if op.kind != "PHI" or not op.args:
                    continue
                out_name = op.result.name
                if out_name == "none" or out_name in missing_tainted:
                    continue
                phi_value_args = [arg for arg in op.args if isinstance(arg, MoltValue)]
                if phi_value_args and all(
                    arg.name in missing_tainted for arg in phi_value_args
                ):
                    missing_tainted.add(out_name)
                    _phi_changed = True

        block_defs: dict[int, set[str]] = {}
        for block in cfg.blocks:
            defs: set[str] = set()
            for op in ops[block.start : block.end]:
                out_name = op.result.name
                if out_name != "none":
                    defs.add(out_name)
            block_defs[block.id] = defs

        in_defs: dict[int, set[str]] = {}
        out_defs: dict[int, set[str]] = {}
        for block_id in range(len(cfg.blocks)):
            if block_id == 0:
                initial = set(predefined)
            elif block_id in cfg.reachable:
                initial = set(all_defs)
            else:
                initial = set()
            in_defs[block_id] = initial
            out_defs[block_id] = initial.union(block_defs[block_id])

        changed = True
        while changed:
            changed = False
            for block_id in range(1, len(cfg.blocks)):
                if block_id not in cfg.reachable:
                    continue
                preds = [
                    pred
                    for pred in cfg.predecessors.get(block_id, [])
                    if pred in cfg.reachable
                ]
                if not preds:
                    new_in = set(predefined)
                else:
                    new_in = set.intersection(*(out_defs[pred] for pred in preds))
                new_out = new_in.union(block_defs[block_id])
                if new_in != in_defs[block_id] or new_out != out_defs[block_id]:
                    in_defs[block_id] = new_in
                    out_defs[block_id] = new_out
                    changed = True

        failures: list[tuple[int, str, str]] = []
        definition_index: dict[str, int] = {}
        definition_block: dict[str, int] = {}
        for op_idx, op in enumerate(ops):
            out_name = op.result.name
            if out_name == "none":
                continue
            if out_name in definition_index:
                failures.append((op_idx, op.kind, out_name))
                continue
            definition_index[out_name] = op_idx
            definition_block[out_name] = cfg.index_to_block[op_idx]

        # Collect which value names are consumed by GETATTR/CALL/LOOKUP ops
        # as default or sentinel arguments — these are the critical consumers
        # of MISSING sentinels.
        _missing_sentinel_consumer_ops = {
            "GETATTR_NAME_DEFAULT",
            "CALL",
            "CALL_INDIRECT",
            "CALL_INTERNAL",
            "DICT_UPDATE_MISSING",
        }

        for block in cfg.blocks:
            block_id = block.id
            if block_id not in cfg.reachable:
                continue
            local_defs = set(in_defs[block_id])
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                used: set[str] = set()
                for arg in op.args:
                    self._collect_arg_value_names(arg, used)
                missing = sorted(name for name in used if name not in local_defs)
                for name in missing:
                    failures.append((op_idx, op.kind, name))
                for name in sorted(used):
                    if name in predefined:
                        continue
                    def_idx = definition_index.get(name)
                    if def_idx is None:
                        # Value is used but has no definition at all — if it
                        # was originally a MISSING sentinel that got removed,
                        # flag this as a failure.
                        if name in missing_value_defs:
                            failures.append((op_idx, op.kind, name))
                        continue
                    def_block = definition_block[name]
                    if def_block not in cfg.dominators.get(block_id, set()):
                        failures.append((op_idx, op.kind, name))
                        continue
                    if def_block == block_id and def_idx >= op_idx:
                        failures.append((op_idx, op.kind, name))
                # Extra check: ops that consume MISSING-produced values
                # (sentinel consumers) must have those definitions still
                # present and dominating.
                if op.kind in _missing_sentinel_consumer_ops:
                    for arg in op.args:
                        if (
                            isinstance(arg, MoltValue)
                            and arg.name in missing_value_defs
                        ):
                            if arg.name not in local_defs:
                                failures.append((op_idx, op.kind, arg.name))
                # Transitive MISSING taint check: if a CALL/CALL_INDIRECT
                # arg is MISSING-tainted through a PHI collapse (not a direct
                # MISSING def), that means an uninitialized variable leaked
                # into a call site after branch pruning.
                if op.kind in {"CALL", "CALL_INDIRECT", "CALL_INTERNAL"}:
                    for arg in op.args:
                        if isinstance(arg, MoltValue) and (
                            arg.name in missing_tainted
                            and arg.name not in missing_value_defs
                        ):
                            failures.append((op_idx, op.kind, arg.name))
                out_name = op.result.name
                if out_name != "none":
                    local_defs.add(out_name)
        return failures

    def _dead_op_lattice_class(self, op_kind: str) -> str:
        effect = self._op_effect_class(op_kind)
        if effect == "control":
            return "protected"
        if effect == "pure":
            return "pure"
        if effect in {"reads_heap", "writes_heap"}:
            return effect
        return "unknown"

    @staticmethod
    def _primitive_const_value_map(ops: list[MoltOp]) -> dict[str, Any]:
        const_by_name: dict[str, Any] = {}
        for op in ops:
            out_name = op.result.name
            if out_name == "none" or op.kind not in _PRIMITIVE_CONST_KINDS:
                continue
            if not op.args:
                continue
            value = op.args[0]
            if op.kind == "CONST_BIGINT":
                try:
                    const_by_name[out_name] = int(value)
                except (TypeError, ValueError):
                    continue
            elif isinstance(value, bool) or isinstance(value, (int, float)):
                const_by_name[out_name] = value
        return const_by_name

    def _op_instance_cannot_raise(
        self, op: MoltOp, const_by_name: dict[str, Any]
    ) -> bool:
        if op.kind not in RAISING_KIND_NAMES:
            return True
        if op.kind not in FRONTEND_RAISING_NOTHROW_ON_PRIMITIVES_KINDS:
            return False
        for arg in op.args:
            if isinstance(arg, MoltValue):
                if arg.name not in const_by_name:
                    return False
            elif isinstance(arg, (list, tuple, dict)):
                return False
            elif isinstance(arg, str):
                return False
        return True

    def _eliminate_dead_trivial_consts(self, ops: list[MoltOp]) -> list[MoltOp]:
        if not ops:
            return []

        func_stats = self._midend_function_stats()
        cfg: CFGGraph = build_cfg(ops)
        if not cfg.blocks:
            return []

        def normalize_anchor_arg(value: Any) -> Any:
            if isinstance(value, MoltValue):
                return ("v", value.name)
            if isinstance(value, tuple):
                return ("t", tuple(normalize_anchor_arg(item) for item in value))
            if isinstance(value, list):
                return ("l", tuple(normalize_anchor_arg(item) for item in value))
            if isinstance(value, dict):
                return (
                    "d",
                    tuple(
                        sorted(
                            (
                                normalize_anchor_arg(key),
                                normalize_anchor_arg(item),
                            )
                            for key, item in value.items()
                        )
                    ),
                )
            try:
                hash(value)
                return ("c", value)
            except TypeError:
                return ("r", repr(value))

        def anchor_key(op: MoltOp) -> tuple[Any, ...] | None:
            out_name = op.result.name
            if out_name == "none":
                return None
            if self._dead_op_lattice_class(op.kind) != "pure":
                return None
            return (op.kind, tuple(normalize_anchor_arg(arg) for arg in op.args))

        anchor_first_result: dict[tuple[Any, ...], str] = {}
        anchor_counts: dict[tuple[Any, ...], int] = {}
        for op in ops:
            key = anchor_key(op)
            if key is None:
                continue
            anchor_counts[key] = anchor_counts.get(key, 0) + 1
            anchor_first_result.setdefault(key, op.result.name)
        preserve_anchor_results: set[str] = {
            anchor_first_result[key]
            for key, count in anchor_counts.items()
            if count > 1 and key in anchor_first_result
        }

        pure_attempted = 0
        uses_by_index: dict[int, set[str]] = {}
        defs_by_name: dict[str, list[int]] = {}
        removable_indices: set[int] = set()
        required_values: set[str] = set()
        worklist: list[str] = []
        const_by_name = self._primitive_const_value_map(ops)

        def require_value(name: str) -> None:
            if name == "none" or name in required_values:
                return
            required_values.add(name)
            worklist.append(name)

        for idx, op in enumerate(ops):
            out_name = op.result.name
            uses: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, uses)
            uses_by_index[idx] = uses

            lattice_class = self._dead_op_lattice_class(op.kind)
            if out_name != "none":
                defs_by_name.setdefault(out_name, []).append(idx)
                if lattice_class == "pure":
                    pure_attempted += 1
                    # MISSING ops are runtime sentinels (uninitialized locals,
                    # optional defaults) that downstream GETATTR/CALL sites
                    # depend on — never eliminate them.
                    if (
                        out_name not in preserve_anchor_results
                        and op.kind != "MISSING"
                        and self._op_instance_cannot_raise(op, const_by_name)
                    ):
                        removable_indices.add(idx)

        for idx, op in enumerate(ops):
            if idx in removable_indices:
                continue
            for name in uses_by_index[idx]:
                require_value(name)

        required_removable_indices: set[int] = set()
        while worklist:
            value_name = worklist.pop()
            for producer_idx in defs_by_name.get(value_name, []):
                if producer_idx not in removable_indices:
                    continue
                if producer_idx in required_removable_indices:
                    continue
                required_removable_indices.add(producer_idx)
                for dependency_name in uses_by_index[producer_idx]:
                    require_value(dependency_name)

        remove_indices = removable_indices - required_removable_indices
        pure_removed = len(remove_indices)
        removed_count = pure_removed
        out = [op for idx, op in enumerate(ops) if idx not in remove_indices]
        self.midend_stats["dce_removed_total"] += removed_count
        func_stats["dce_pure_op_attempted"] += pure_attempted
        func_stats["dce_pure_op_accepted"] += pure_removed
        func_stats["dce_pure_op_rejected"] += max(0, pure_attempted - pure_removed)
        return out

    def _op_may_raise_for_sccp(self, op_kind: str) -> bool:
        if op_kind in RAISING_KIND_NAMES:
            return True
        if op_kind in _SCCP_PROVEN_NOTHROW_KINDS:
            return False
        if op_kind.startswith("STATE_"):
            return False
        return True

    def _compute_sccp(
        self,
        ops: list[MoltOp],
        cfg: CFGGraph,
        *,
        max_iters_override: int | None = None,
    ) -> SCCPResult:
        # Current contract: SCCP tracks executable edges and supplies facts for
        # conservative loop/try marker rewrites only; broader LOOP_END and
        # exceptional-handler CFG rewrites remain roadmap work and must preserve
        # dominance/post-dominance invariants.
        in_values: dict[int, dict[str, Any]] = {block.id: {} for block in cfg.blocks}
        out_values: dict[int, dict[str, Any]] = {block.id: {} for block in cfg.blocks}
        executable_blocks: set[int] = {0} if cfg.blocks else set()
        executable_edges: set[tuple[int, int]] = set()
        branch_choice_by_if_index: dict[int, bool] = {}
        loop_break_choice_by_index: dict[int, bool] = {}
        try_exception_possible_by_start: dict[int, bool] = {}
        try_normal_possible_by_start: dict[int, bool] = {}
        guard_fail_indices: set[int] = set()
        loop_bound_facts = self._analyze_loop_bound_facts(ops, cfg)
        loop_compare_truth = self._analyze_affine_loop_compare_truth(ops, cfg)
        type_of_origin: dict[str, str] = {}
        for op in ops:
            if (
                op.kind == "TYPE_OF"
                and len(op.args) == 1
                and isinstance(op.args[0], MoltValue)
                and op.result.name != "none"
            ):
                type_of_origin[op.result.name] = op.args[0].name

        def type_fact_key(name: str) -> str:
            return f"__tag__:{name}"

        def dict_shape_fact_key(name: str) -> str:
            return f"__dict_shape__:{name}"

        def is_overdefined(value: Any) -> bool:
            return value is _SCCP_OVERDEFINED

        def is_missing_sentinel(value: Any) -> bool:
            return value is _SCCP_MISSING

        def merge_lattice(left: Any, right: Any) -> Any:
            # MISSING sentinels must never fold: if either side is MISSING,
            # the merge is overdefined so downstream operations cannot
            # constant-fold through a MISSING value.
            if is_missing_sentinel(left) or is_missing_sentinel(right):
                return _SCCP_OVERDEFINED
            if left is _SCCP_UNKNOWN:
                return right
            if right is _SCCP_UNKNOWN:
                return left
            if is_overdefined(left) or is_overdefined(right):
                return _SCCP_OVERDEFINED
            if left == right:
                return left
            return _SCCP_OVERDEFINED

        def merge_states(states: list[dict[str, Any]]) -> dict[str, Any]:
            if not states:
                return {}
            merged: dict[str, Any] = {}
            all_keys: set[str] = set()
            for state in states:
                all_keys.update(state.keys())
            for key in all_keys:
                current: Any = _SCCP_UNKNOWN
                for state in states:
                    current = merge_lattice(current, state.get(key, _SCCP_UNKNOWN))
                    if is_overdefined(current):
                        break
                if current is not _SCCP_UNKNOWN:
                    merged[key] = current
            return merged

        def value_lattice(name: str, known: dict[str, Any]) -> Any:
            return known.get(name, _SCCP_UNKNOWN)

        def value_type_tag(name: str, known: dict[str, Any]) -> int | None:
            fact = known.get(type_fact_key(name))
            if isinstance(fact, int):
                return fact
            value = value_lattice(name, known)
            if (
                value is _SCCP_UNKNOWN
                or is_overdefined(value)
                or is_missing_sentinel(value)
            ):
                return None
            return self._const_type_tag_for_lattice_value(value)

        def scalar_cmp_supported(value: Any) -> bool:
            if value is None:
                return True
            if isinstance(value, bool):
                return True
            if isinstance(value, int):
                return True
            if isinstance(value, float):
                return True
            if isinstance(value, str):
                return True
            if isinstance(value, bytes):
                return True
            return False

        def eval_lattice_value(op: MoltOp, known: dict[str, Any], op_index: int) -> Any:
            # MISSING ops produce runtime sentinel values that must never be
            # constant-folded or propagated.  Return _SCCP_MISSING so that
            # any downstream consumer goes to overdefined via merge_lattice.
            if op.kind == "MISSING":
                return _SCCP_MISSING
            if op.kind == "CONST":
                return op.args[0]
            if op.kind == "CONST_BOOL":
                return bool(op.args[0])
            if op.kind == "CONST_BIGINT":
                return int(op.args[0])
            if op.kind == "CONST_FLOAT":
                return float(op.args[0])
            if op.kind == "CONST_STR":
                return str(op.args[0])
            if op.kind == "CONST_BYTES":
                return bytes(op.args[0])
            if op.kind == "CONST_NONE":
                return None
            if op.kind == "CONST_NOT_IMPLEMENTED":
                return NotImplemented
            if op.kind == "CONST_ELLIPSIS":
                return Ellipsis
            if op.kind == "PHI" and op.args:
                block_id = cfg.index_to_block.get(op_index)
                if block_id is not None:
                    block_preds = cfg.predecessors.get(block_id, [])
                    if len(block_preds) == len(op.args):
                        merged: Any = _SCCP_UNKNOWN
                        seen_exec = False
                        for arg, pred in zip(op.args, block_preds):
                            if (pred, block_id) not in executable_edges:
                                continue
                            if not isinstance(arg, MoltValue):
                                return _SCCP_OVERDEFINED
                            seen_exec = True
                            merged = merge_lattice(
                                merged, value_lattice(arg.name, known)
                            )
                            if is_overdefined(merged):
                                return _SCCP_OVERDEFINED
                        if seen_exec:
                            return merged
                        return _SCCP_UNKNOWN
                merged = _SCCP_UNKNOWN
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        return _SCCP_OVERDEFINED
                    merged = merge_lattice(merged, value_lattice(arg.name, known))
                    if is_overdefined(merged):
                        return _SCCP_OVERDEFINED
                return merged
            if op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels must never fold through arithmetic.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if (
                    isinstance(lhs_value, int)
                    and not isinstance(lhs_value, bool)
                    and isinstance(rhs_value, int)
                    and not isinstance(rhs_value, bool)
                ):
                    if op.kind == "ADD":
                        return lhs_value + rhs_value
                    if op.kind == "SUB":
                        return lhs_value - rhs_value
                    return lhs_value * rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "NOT" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value) or is_missing_sentinel(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(arg_value, bool):
                    return not arg_value
                return _SCCP_OVERDEFINED
            if op.kind == "ABS" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value) or is_missing_sentinel(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(arg_value, (int, float)):
                    return abs(arg_value)
                return _SCCP_OVERDEFINED
            if op.kind in {"AND", "OR"} and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels must never fold through boolean ops.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if isinstance(lhs_value, bool) and isinstance(rhs_value, bool):
                    if op.kind == "AND":
                        return lhs_value and rhs_value
                    return lhs_value or rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "IS" and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels are singleton objects in the lattice but
                # represent distinct runtime values — never fold identity
                # comparisons through them.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                return lhs_value is rhs_value
            if op.kind in {"EQ", "NE", "LT", "LE", "GT", "GE"} and len(op.args) == 2:
                proven_static = loop_compare_truth.get(op_index)
                if isinstance(proven_static, bool):
                    return proven_static
                loop_fact = loop_bound_facts.get(op_index)
                if loop_fact is not None:
                    proven = self._prove_monotonic_loop_compare(loop_fact)
                    if isinstance(proven, bool):
                        return proven
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if not scalar_cmp_supported(lhs_value) or not scalar_cmp_supported(
                    rhs_value
                ):
                    return _SCCP_OVERDEFINED
                try:
                    if op.kind == "EQ":
                        return lhs_value == rhs_value
                    if op.kind == "NE":
                        return lhs_value != rhs_value
                    if op.kind == "LT":
                        return lhs_value < rhs_value
                    if op.kind == "LE":
                        return lhs_value <= rhs_value
                    if op.kind == "GT":
                        return lhs_value > rhs_value
                    return lhs_value >= rhs_value
                except Exception:
                    return _SCCP_OVERDEFINED
            if op.kind == "STRING_EQ" and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if isinstance(lhs_value, str) and isinstance(rhs_value, str):
                    return lhs_value == rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "TYPE_OF" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                tag = value_type_tag(arg.name, known)
                if tag is not None:
                    return tag
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value):
                    return _SCCP_OVERDEFINED
                return _SCCP_OVERDEFINED
            if op.kind == "ISINSTANCE" and len(op.args) == 2:
                obj = op.args[0]
                classinfo = op.args[1]
                if not isinstance(obj, MoltValue) or not isinstance(
                    classinfo, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                class_value = value_lattice(classinfo.name, known)
                if class_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(class_value):
                    return _SCCP_OVERDEFINED
                obj_tag = value_type_tag(obj.name, known)
                if obj_tag is None:
                    return _SCCP_UNKNOWN
                if isinstance(class_value, int):
                    return obj_tag == class_value
                if isinstance(class_value, tuple) and all(
                    isinstance(item, int) for item in class_value
                ):
                    return obj_tag in class_value
                return _SCCP_OVERDEFINED
            if op.kind == "LEN" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(
                    arg_value, (str, bytes, tuple, list, dict, set, frozenset, range)
                ):
                    return len(arg_value)
                return _SCCP_OVERDEFINED
            if op.kind == "CONTAINS" and len(op.args) == 2:
                container = op.args[0]
                item = op.args[1]
                if not isinstance(container, MoltValue) or not isinstance(
                    item, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                container_value = value_lattice(container.name, known)
                item_value = value_lattice(item.name, known)
                if container_value is _SCCP_UNKNOWN or item_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(container_value) or is_overdefined(item_value):
                    return _SCCP_OVERDEFINED
                if not isinstance(
                    container_value,
                    (str, bytes, tuple, list, dict, set, frozenset, range),
                ):
                    return _SCCP_OVERDEFINED
                try:
                    return item_value in container_value
                except Exception:
                    return _SCCP_OVERDEFINED
            if op.kind == "INDEX" and len(op.args) == 2:
                container = op.args[0]
                index = op.args[1]
                if not isinstance(container, MoltValue) or not isinstance(
                    index, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                container_value = value_lattice(container.name, known)
                index_value = value_lattice(index.name, known)
                if container_value is _SCCP_UNKNOWN or index_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(container_value) or is_overdefined(index_value):
                    return _SCCP_OVERDEFINED
                if isinstance(container_value, (tuple, list, str, bytes, range)):
                    if isinstance(index_value, int) and not isinstance(
                        index_value, bool
                    ):
                        try:
                            return container_value[index_value]
                        except Exception:
                            return _SCCP_OVERDEFINED
                    return _SCCP_OVERDEFINED
                if isinstance(container_value, dict):
                    try:
                        if index_value in container_value:
                            return container_value[index_value]
                    except Exception:
                        return _SCCP_OVERDEFINED
                return _SCCP_OVERDEFINED
            return _SCCP_OVERDEFINED

        def evaluate_try_behavior(start_idx: int, end_idx: int) -> tuple[bool, bool]:
            known: dict[str, Any] = {}
            may_raise = False
            may_complete_normally = True
            if end_idx <= start_idx + 1:
                return False, True
            for op_idx in range(start_idx + 1, end_idx):
                op = ops[op_idx]
                if op.kind in {
                    "IF",
                    "ELSE",
                    "END_IF",
                    "LOOP_START",
                    "LOOP_END",
                    "LOOP_BREAK",
                    "LOOP_BREAK_IF_TRUE",
                    "LOOP_BREAK_IF_FALSE",
                    "LOOP_BREAK_IF_EXCEPTION",
                    "LOOP_CONTINUE",
                    "TRY_START",
                    "TRY_END",
                    "JUMP",
                    "LABEL",
                    "STATE_LABEL",
                }:
                    return True, True
                if op.kind in {"GUARD_TAG", "GUARD_TYPE"} and len(op.args) == 2:
                    guarded = op.args[0]
                    expected = op.args[1]
                    if isinstance(guarded, MoltValue) and isinstance(
                        expected, MoltValue
                    ):
                        expected_value = value_lattice(expected.name, known)
                        guarded_tag = value_type_tag(guarded.name, known)
                        if (
                            isinstance(expected_value, int)
                            and guarded_tag is not None
                            and guarded_tag == expected_value
                        ):
                            known[type_fact_key(guarded.name)] = expected_value
                            continue
                    return True, False
                if op.kind == "GUARD_DICT_SHAPE" and len(op.args) == 3:
                    guarded = op.args[0]
                    dict_type = op.args[1]
                    version = op.args[2]
                    if (
                        isinstance(guarded, MoltValue)
                        and isinstance(dict_type, MoltValue)
                        and isinstance(version, MoltValue)
                    ):
                        shape_key = dict_shape_fact_key(guarded.name)
                        expected = (dict_type.name, version.name)
                        known_shape = known.get(shape_key)
                        if isinstance(known_shape, tuple):
                            if known_shape == expected:
                                continue
                            return True, False
                        known[shape_key] = expected
                        continue
                    return True, True
                if op.kind in {"RAISE", "RAISE_CAUSE", "RERAISE"}:
                    return True, False
                out_name = op.result.name
                lattice_value: Any = _SCCP_UNKNOWN
                if out_name != "none":
                    known.pop(out_name, None)
                    known.pop(type_fact_key(out_name), None)
                    known.pop(dict_shape_fact_key(out_name), None)
                    lattice_value = eval_lattice_value(op, known, op_idx)
                    # Promote MISSING sentinels to overdefined in try analysis too.
                    if is_missing_sentinel(lattice_value):
                        lattice_value = _SCCP_OVERDEFINED
                    if (
                        lattice_value is not _SCCP_UNKNOWN
                        and lattice_value is not _SCCP_OVERDEFINED
                    ):
                        known[out_name] = lattice_value
                        tag = self._const_type_tag_for_lattice_value(lattice_value)
                        if tag is not None:
                            known[type_fact_key(out_name)] = tag
                if self._op_may_raise_for_sccp(op.kind):
                    if (
                        lattice_value is _SCCP_OVERDEFINED
                        or lattice_value is _SCCP_UNKNOWN
                    ):
                        may_raise = True
            return may_raise, may_complete_normally

        for try_start_idx, try_end_idx in cfg.control.try_start_to_end.items():
            may_raise, may_complete_normally = evaluate_try_behavior(
                try_start_idx, try_end_idx
            )
            try_exception_possible_by_start[try_start_idx] = may_raise
            try_normal_possible_by_start[try_start_idx] = may_complete_normally
        check_exception_try_owner: dict[int, int] = {}
        for try_start_idx, try_end_idx in cfg.control.try_start_to_end.items():
            for op_idx in range(try_start_idx + 1, try_end_idx):
                if op_idx >= len(ops) or ops[op_idx].kind != "CHECK_EXCEPTION":
                    continue
                owner = check_exception_try_owner.get(op_idx)
                if owner is None or try_start_idx > owner:
                    check_exception_try_owner[op_idx] = try_start_idx

        value_users: dict[str, set[int]] = {}
        for op_idx, op in enumerate(ops):
            block_id = cfg.index_to_block.get(op_idx)
            if block_id is None:
                continue
            for arg in op.args:
                if isinstance(arg, MoltValue):
                    value_users.setdefault(arg.name, set()).add(block_id)

        iterations = 0
        ssa_defs = sum(1 for op in ops if op.result.name != "none")
        if max_iters_override is not None and max_iters_override > 0:
            max_iterations = max_iters_override
        elif self.midend_env.sccp_iter_cap_override is not None:
            max_iterations = self.midend_env.sccp_iter_cap_override
        else:
            # Dynamic cap keeps compile-time bounded while scaling with function/CFG size.
            # Keep the default ceiling conservative so wasm builds cannot stall for
            # minutes in pathological SCCP worklists.
            cfg_edge_count = sum(len(succs) for succs in cfg.successors.values())
            max_iterations = max(
                2048,
                min(
                    131072,
                    (len(cfg.blocks) * 96) + (cfg_edge_count * 48) + (ssa_defs * 24),
                ),
            )
        func_stats = self._midend_function_stats()

        block_queue: deque[int] = deque()
        queued_blocks: set[int] = set()
        edge_queue: deque[tuple[int, int]] = deque()
        queued_edges: set[tuple[int, int]] = set()
        value_queue: deque[str] = deque()
        queued_values: set[str] = set()

        def enqueue_block(block_id: int) -> None:
            if block_id in queued_blocks:
                return
            queued_blocks.add(block_id)
            block_queue.append(block_id)

        def enqueue_edge(src: int, dst: int) -> None:
            edge = (src, dst)
            if edge in executable_edges or edge in queued_edges:
                return
            queued_edges.add(edge)
            edge_queue.append(edge)

        def enqueue_value(name: str) -> None:
            if name in queued_values:
                return
            queued_values.add(name)
            value_queue.append(name)

        if cfg.blocks:
            enqueue_block(0)

        while block_queue or edge_queue or value_queue:
            if edge_queue:
                src, dst = edge_queue.popleft()
                queued_edges.discard((src, dst))
                if (src, dst) in executable_edges:
                    continue
                executable_edges.add((src, dst))
                if dst not in executable_blocks:
                    executable_blocks.add(dst)
                enqueue_block(dst)
                continue

            if value_queue:
                value_name = value_queue.popleft()
                queued_values.discard(value_name)
                for block_id in value_users.get(value_name, ()):
                    if block_id in executable_blocks:
                        enqueue_block(block_id)
                continue

            iterations += 1
            if iterations > max_iterations:
                self.midend_stats["sccp_iteration_cap_hits"] = (
                    self.midend_stats.get("sccp_iteration_cap_hits", 0) + 1
                )
                func_stats["sccp_iteration_cap_hits"] += 1
                all_blocks = {block.id for block in cfg.blocks}
                all_edges = {
                    (src, dst) for src, succs in cfg.successors.items() for dst in succs
                }
                conservative_try = {
                    start_idx: True for start_idx in cfg.control.try_start_to_end
                }
                return SCCPResult(
                    in_values={block.id: {} for block in cfg.blocks},
                    out_values={block.id: {} for block in cfg.blocks},
                    executable_blocks=all_blocks,
                    executable_edges=all_edges,
                    branch_choice_by_if_index={},
                    loop_break_choice_by_index={},
                    try_exception_possible_by_start=conservative_try,
                    try_normal_possible_by_start=dict(conservative_try),
                    guard_fail_indices=set(),
                )

            block_id = block_queue.popleft()
            queued_blocks.discard(block_id)
            if block_id not in executable_blocks:
                continue
            block = cfg.blocks[block_id]

            if block_id == 0:
                new_in: dict[str, Any] = {}
            else:
                exec_preds = [
                    pred
                    for pred in cfg.predecessors.get(block_id, [])
                    if (pred, block_id) in executable_edges
                ]
                pred_states = [out_values[pred] for pred in exec_preds]
                new_in = merge_states(pred_states)

            if new_in != in_values[block_id]:
                in_values[block_id] = new_in

            known = dict(new_in)
            block_traps = False
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if op.kind in {"GUARD_TAG", "GUARD_TYPE"} and len(op.args) == 2:
                    guarded = op.args[0]
                    expected = op.args[1]
                    if isinstance(guarded, MoltValue) and isinstance(
                        expected, MoltValue
                    ):
                        expected_value = known.get(expected.name, _SCCP_UNKNOWN)
                        if isinstance(expected_value, int):
                            guarded_tag = value_type_tag(guarded.name, known)
                            if (
                                guarded_tag is not None
                                and guarded_tag != expected_value
                            ):
                                guard_fail_indices.add(op_idx)
                                block_traps = True
                                break
                            known[type_fact_key(guarded.name)] = expected_value
                    continue
                if op.kind == "GUARD_DICT_SHAPE" and len(op.args) == 3:
                    guarded = op.args[0]
                    dict_type = op.args[1]
                    version = op.args[2]
                    if (
                        isinstance(guarded, MoltValue)
                        and isinstance(dict_type, MoltValue)
                        and isinstance(version, MoltValue)
                    ):
                        shape_key = dict_shape_fact_key(guarded.name)
                        expected_shape = (dict_type.name, version.name)
                        known_shape = known.get(shape_key)
                        if isinstance(known_shape, tuple):
                            if known_shape != expected_shape:
                                guard_fail_indices.add(op_idx)
                                block_traps = True
                                break
                        else:
                            known[shape_key] = expected_shape
                    continue
                out_name = op.result.name
                if out_name == "none":
                    continue
                known.pop(out_name, None)
                known.pop(type_fact_key(out_name), None)
                known.pop(dict_shape_fact_key(out_name), None)
                lattice_value = eval_lattice_value(op, known, op_idx)
                if lattice_value is _SCCP_UNKNOWN:
                    continue
                # MISSING sentinels must not propagate as constants through
                # the lattice — promote to overdefined so no downstream op
                # can constant-fold through a MISSING value.
                if is_missing_sentinel(lattice_value):
                    lattice_value = _SCCP_OVERDEFINED
                known[out_name] = lattice_value
                tag = self._const_type_tag_for_lattice_value(lattice_value)
                if tag is not None:
                    known[type_fact_key(out_name)] = tag
                if (
                    op.kind in {"EQ", "NE"}
                    and isinstance(lattice_value, bool)
                    and len(op.args) == 2
                ):
                    lhs = op.args[0]
                    rhs = op.args[1]
                    for type_side, tag_side in ((lhs, rhs), (rhs, lhs)):
                        if not isinstance(type_side, MoltValue) or not isinstance(
                            tag_side, MoltValue
                        ):
                            continue
                        guarded_name = type_of_origin.get(type_side.name)
                        if guarded_name is None:
                            continue
                        expected_tag = known.get(tag_side.name, _SCCP_UNKNOWN)
                        if not isinstance(expected_tag, int):
                            continue
                        implies_equal = (
                            lattice_value if op.kind == "EQ" else not lattice_value
                        )
                        if implies_equal:
                            known[type_fact_key(guarded_name)] = expected_tag
                if (
                    op.kind == "ISINSTANCE"
                    and lattice_value is True
                    and len(op.args) == 2
                ):
                    guarded_obj = op.args[0]
                    classinfo = op.args[1]
                    if isinstance(guarded_obj, MoltValue) and isinstance(
                        classinfo, MoltValue
                    ):
                        class_value = known.get(classinfo.name, _SCCP_UNKNOWN)
                        if isinstance(class_value, int):
                            known[type_fact_key(guarded_obj.name)] = class_value
                        elif isinstance(class_value, tuple):
                            tags = [
                                item for item in class_value if isinstance(item, int)
                            ]
                            if len(tags) == 1:
                                known[type_fact_key(guarded_obj.name)] = tags[0]

            prior_out = out_values[block_id]
            out_changed_keys: list[str] = []
            if known != prior_out:
                # DETERMINISM (#73, #34 bug class): `out_changed_keys` drives the
                # order values are pushed onto the SCCP `value_queue` (see the
                # `enqueue_value(key)` loop below), which in turn dictates the
                # block-processing schedule of this worklist fixed point.  Built
                # from a `set[str]` union, its iteration order is
                # PYTHONHASHSEED-dependent — and while the SCCP lattice *result*
                # is order-independent (monotone), the NUMBER of node re-visits
                # to reach the fixed point is not.  For a function near the
                # `max_iterations` cap, a worse schedule can exceed the cap and
                # bail to the conservative empty-facts result, whereas a better
                # schedule converges with full const facts.  That flips
                # downstream CSE/const-dedup on or off, so the emitted IR
                # silently diverged across hash seeds.  Sort the changed keys at
                # this construction site so the worklist schedule — and thus the
                # cap behaviour and the compiled IR — is byte-stable.
                all_keys = set(prior_out.keys()) | set(known.keys())
                out_changed_keys = sorted(
                    key
                    for key in all_keys
                    if prior_out.get(key, _SCCP_UNKNOWN)
                    != known.get(key, _SCCP_UNKNOWN)
                )
                out_values[block_id] = known

            succs = cfg.successors.get(block_id, [])
            chosen_succs = succs
            if block_traps:
                chosen_succs = []
            elif block.start < block.end:
                terminator_idx = block.end - 1
                terminator = ops[terminator_idx]
                if terminator.kind == "IF" and len(terminator.args) == 1:
                    cond = terminator.args[0]
                    cond_value: Any = _SCCP_UNKNOWN
                    if isinstance(cond, MoltValue):
                        cond_value = known.get(cond.name, _SCCP_UNKNOWN)
                    if isinstance(cond_value, bool):
                        branch_choice_by_if_index[terminator_idx] = cond_value
                        if cond_value and succs:
                            chosen_succs = [succs[0]]
                        elif not cond_value and len(succs) >= 2:
                            chosen_succs = [succs[1]]
                    else:
                        branch_choice_by_if_index.pop(terminator_idx, None)
                elif (
                    terminator.kind in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE"}
                    and len(terminator.args) == 1
                ):
                    cond = terminator.args[0]
                    cond_value: Any = _SCCP_UNKNOWN
                    if isinstance(cond, MoltValue):
                        cond_value = known.get(cond.name, _SCCP_UNKNOWN)
                    if isinstance(cond_value, bool) and len(succs) >= 2:
                        if terminator.kind == "LOOP_BREAK_IF_TRUE":
                            break_taken = bool(cond_value)
                        else:
                            break_taken = not bool(cond_value)
                        loop_break_choice_by_index[terminator_idx] = break_taken
                        chosen_succs = [succs[1] if break_taken else succs[0]]
                    else:
                        loop_break_choice_by_index.pop(terminator_idx, None)
                elif terminator.kind == "TRY_START":
                    can_raise = try_exception_possible_by_start.get(
                        terminator_idx, True
                    )
                    if not can_raise and succs:
                        chosen_succs = [succs[0]]
                elif terminator.kind == "CHECK_EXCEPTION":
                    owner_start = check_exception_try_owner.get(terminator_idx)
                    if owner_start is not None:
                        can_raise = try_exception_possible_by_start.get(
                            owner_start, True
                        )
                        if not can_raise and succs:
                            chosen_succs = [succs[0]]
                elif terminator.kind == "LOOP_END" and len(succs) >= 2:
                    loop_start_idx = cfg.control.loop_end_to_start.get(terminator_idx)
                    back_succ = (
                        None
                        if loop_start_idx is None
                        else cfg.index_to_block.get(loop_start_idx)
                    )
                    if back_succ is not None:
                        exit_succ = next(
                            (succ for succ in succs if succ != back_succ),
                            None,
                        )
                        back_exec = back_succ in executable_blocks
                        exit_exec = (
                            exit_succ in executable_blocks
                            if exit_succ is not None
                            else False
                        )
                        if back_exec and not exit_exec:
                            chosen_succs = [back_succ]
                        elif exit_succ is not None and exit_exec and not back_exec:
                            chosen_succs = [exit_succ]

            for succ in chosen_succs:
                enqueue_edge(block_id, succ)

            if out_changed_keys:
                for key in out_changed_keys:
                    if not key.startswith("__"):
                        enqueue_value(key)
                for succ in cfg.successors.get(block_id, []):
                    if succ in executable_blocks:
                        enqueue_block(succ)

        return SCCPResult(
            in_values=in_values,
            out_values=out_values,
            executable_blocks=executable_blocks,
            executable_edges=executable_edges,
            branch_choice_by_if_index=branch_choice_by_if_index,
            loop_break_choice_by_index=loop_break_choice_by_index,
            try_exception_possible_by_start=try_exception_possible_by_start,
            try_normal_possible_by_start=try_normal_possible_by_start,
            guard_fail_indices=guard_fail_indices,
        )

    def _sccp_in_const_int_values(self, sccp: SCCPResult) -> dict[int, dict[str, int]]:
        in_int_values: dict[int, dict[str, int]] = {}
        for block_id, known in sccp.in_values.items():
            in_int_values[block_id] = {
                key: value
                for key, value in known.items()
                if (
                    not str(key).startswith("__tag__:")
                    and value is not _SCCP_OVERDEFINED
                    and isinstance(value, int)
                    and not isinstance(value, bool)
                )
            }
        return in_int_values

    def _trim_phi_args_by_executable_edges(
        self,
        ops: list[MoltOp],
        cfg: CFGGraph,
        executable_edges: set[tuple[int, int]],
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        trimmed = 0
        out: list[MoltOp] = []
        for block in cfg.blocks:
            block_preds = cfg.predecessors.get(block.id, [])
            # Look through single-predecessor post-merge blocks: if this
            # block has exactly one predecessor that is itself a merge point
            # (multiple predecessors), the PHI args correspond to the merge
            # block's predecessors, not the direct predecessor.
            effective_preds = block_preds
            edge_target = block.id
            if (
                len(block_preds) == 1
                and len(cfg.predecessors.get(block_preds[0], [])) > 1
            ):
                effective_preds = cfg.predecessors.get(block_preds[0], [])
                edge_target = block_preds[0]
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if (
                    op.kind == "PHI"
                    and op.args
                    and len(op.args) == len(effective_preds)
                    and len(effective_preds) > 1
                ):
                    kept_args = [
                        arg
                        for arg, pred in zip(op.args, effective_preds)
                        if (pred, edge_target) in executable_edges
                    ]
                    normalized_args = kept_args
                    if kept_args and all(
                        isinstance(arg, MoltValue)
                        and isinstance(kept_args[0], MoltValue)
                        and arg.name == kept_args[0].name
                        for arg in kept_args
                    ):
                        normalized_args = [kept_args[0]]
                    if 0 < len(normalized_args) < len(op.args):
                        out.append(
                            MoltOp(
                                kind=op.kind,
                                args=normalized_args,
                                result=op.result,
                                metadata=op.metadata,
                            )
                        )
                        trimmed += len(op.args) - len(normalized_args)
                        continue
                out.append(op)
        return out, trimmed

    def _align_phi_args_to_cfg_predecessors(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        rewrites = 0
        out: list[MoltOp] = []
        for block in cfg.blocks:
            block_preds = cfg.predecessors.get(block.id, [])
            # Look through single-predecessor post-merge blocks to find
            # the effective predecessor count that PHI args should match.
            effective_preds = block_preds
            if (
                len(block_preds) == 1
                and len(cfg.predecessors.get(block_preds[0], [])) > 1
            ):
                effective_preds = cfg.predecessors.get(block_preds[0], [])
            expected = len(effective_preds)
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if op.kind != "PHI" or not op.args:
                    out.append(op)
                    continue
                if expected == 0:
                    out.append(op)
                    continue
                args = list(op.args)
                if len(args) == expected:
                    out.append(op)
                    continue
                if not all(isinstance(arg, MoltValue) for arg in args):
                    out.append(op)
                    continue
                first = cast(MoltValue, args[0])
                all_same = all(
                    isinstance(arg, MoltValue) and arg.name == first.name
                    for arg in args
                )
                if not all_same:
                    out.append(op)
                    continue
                if expected > 0:
                    # Expand to match effective predecessor count, then
                    # collapse identical args back down.
                    expanded = [first for _ in range(expected)]
                    if all(
                        isinstance(a, MoltValue) and a.name == first.name
                        for a in expanded
                    ):
                        normalized = [first]
                    else:
                        normalized = expanded
                    out.append(
                        MoltOp(
                            kind=op.kind,
                            args=normalized,
                            result=op.result,
                            metadata=op.metadata,
                        )
                    )
                    rewrites += abs(len(args) - expected)
                    continue
                out.append(op)
        return out, rewrites

    def _canonicalize_cfg_before_optimization(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0

        current = ops
        total_rewrites = 0
        for _ in range(8):
            round_rewrites = 0
            round_cfg = build_cfg(current)
            if not round_cfg.blocks:
                break

            step_ops, phi_align = self._align_phi_args_to_cfg_predecessors(
                current, round_cfg
            )
            round_rewrites += phi_align

            step_cfg = build_cfg(step_ops)
            if step_cfg.blocks:
                step_ops, ladder_threads = self._normalize_try_except_join_labels(
                    step_ops, cfg=step_cfg
                )
                round_rewrites += ladder_threads

            step_ops, label_prunes, jump_noops = self._prune_dead_labels_and_noop_jumps(
                step_ops
            )
            round_rewrites += label_prunes + jump_noops

            step_ops, structural_prunes = (
                self._canonicalize_structured_regions_pre_sccp(step_ops)
            )
            round_rewrites += structural_prunes

            if step_ops == current:
                break
            total_rewrites += round_rewrites
            current = step_ops

        return current, total_rewrites

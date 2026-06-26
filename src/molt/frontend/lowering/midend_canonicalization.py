"""MidendCanonicalizationMixin: frontend IR alias rewriting, value numbering, and canonicalization state."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, cast

from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
    CFGGraph,
    CanonicalizationState,
    LoopBoundFact,
    MoltOp,
    MoltValue,
    _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY,
    _INLINE_INT_MAX,
    _INLINE_INT_MIN,
)
from molt.frontend.lowering.op_kinds_generated import FRONTEND_EFFECT_CLASS

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class MidendCanonicalizationMixin(_MixinBase):
    def _resolve_alias_value(
        self, value: MoltValue, aliases: dict[str, MoltValue]
    ) -> MoltValue:
        current = value
        visited: set[str] = set()
        while current.name in aliases and current.name not in visited:
            visited.add(current.name)
            current = aliases[current.name]
        return current

    def _rewrite_aliases_in_arg(self, value: Any, aliases: dict[str, MoltValue]) -> Any:
        if isinstance(value, MoltValue):
            return self._resolve_alias_value(value, aliases)
        if isinstance(value, list):
            return [self._rewrite_aliases_in_arg(item, aliases) for item in value]
        if isinstance(value, tuple):
            return tuple(self._rewrite_aliases_in_arg(item, aliases) for item in value)
        if isinstance(value, dict):
            return {
                self._rewrite_aliases_in_arg(k, aliases): self._rewrite_aliases_in_arg(
                    v, aliases
                )
                for k, v in value.items()
            }
        return value

    def _is_canonicalization_barrier_op(self, op_kind: str) -> bool:
        if op_kind in {"RETURN", "RAISE", "RAISE_CAUSE", "RERAISE"}:
            return True
        if op_kind.startswith("EXCEPTION_"):
            return True
        if op_kind.startswith("STATE_"):
            return True
        return False

    def _const_type_tag(self, op: MoltOp) -> int | None:
        if op.kind == "CONST_BOOL":
            return BUILTIN_TYPE_TAGS["bool"]
        if op.kind == "CONST":
            value = op.args[0]
            if isinstance(value, int) and not isinstance(value, bool):
                return BUILTIN_TYPE_TAGS["int"]
        if op.kind == "CONST_BIGINT":
            return BUILTIN_TYPE_TAGS["int"]
        if op.kind == "CONST_FLOAT":
            return BUILTIN_TYPE_TAGS["float"]
        if op.kind == "CONST_STR":
            return BUILTIN_TYPE_TAGS["str"]
        if op.kind == "CONST_BYTES":
            return BUILTIN_TYPE_TAGS["bytes"]
        return None

    def _empty_canonicalization_state(self) -> CanonicalizationState:
        return {
            "aliases": {},
            "const_int_values": {},
            "value_type_tags": {},
            "available_values": {},
            "guard_dict_shapes": {},
            "alias_epochs": {},
            "object_epochs": {},
            "memory_epoch": 0,
        }

    def _clone_canonicalization_state(
        self, state: CanonicalizationState
    ) -> CanonicalizationState:
        cloned: CanonicalizationState = {
            "aliases": state["aliases"].copy(),
            "const_int_values": state["const_int_values"].copy(),
            "value_type_tags": state["value_type_tags"].copy(),
            "available_values": state["available_values"].copy(),
            "guard_dict_shapes": state["guard_dict_shapes"].copy(),
            "alias_epochs": state["alias_epochs"].copy(),
            "object_epochs": state["object_epochs"].copy(),
            "memory_epoch": state["memory_epoch"],
        }
        cached_signature = cast(Any, state).get(
            _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY
        )
        if cached_signature is not None:
            cast(Any, cloned)[_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY] = (
                cached_signature
            )
        return cloned

    def _invalidate_canonicalization_state_signature(
        self, state: CanonicalizationState
    ) -> None:
        cast(Any, state).pop(_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY, None)

    def _const_cache_key_for_op(self, op: MoltOp) -> tuple[Any, ...] | None:
        if op.kind in {"CONST_NONE", "CONST_NOT_IMPLEMENTED", "CONST_ELLIPSIS"}:
            return (op.kind,)
        if op.kind == "CONST_BYTES":
            return ("CONST_BYTES", bytes(op.args[0]))
        if op.kind in {"CONST_BOOL", "CONST_BIGINT", "CONST_FLOAT", "CONST_STR"}:
            value = op.args[0]
            try:
                hash(value)
                normalized = value
            except TypeError:
                normalized = repr(value)
            return (op.kind, normalized)
        if op.kind == "CONST":
            value = op.args[0]
            try:
                hash(value)
                normalized = value
            except TypeError:
                normalized = repr(value)
            return ("CONST", type(value).__name__, normalized)
        return None

    def _op_effect_class(self, op_kind: str) -> str:
        return FRONTEND_EFFECT_CLASS.get(op_kind, "unknown")

    def _is_pure_op_for_global_cse(self, op_kind: str) -> bool:
        return self._op_effect_class(op_kind) == "pure"

    def _is_cse_eligible_op(self, op_kind: str) -> bool:
        return self._op_effect_class(op_kind) in {"pure", "reads_heap"}

    def _normalize_value_operand_key(
        self, value: Any, const_int_values: dict[str, int]
    ) -> tuple[str, Any] | None:
        if not isinstance(value, MoltValue):
            return None
        const_value = const_int_values.get(value.name)
        if const_value is not None:
            return ("const_int", const_value)
        return ("ssa", value.name)

    def _normalize_operand_key_for_value_numbering(
        self, value: Any, const_int_values: dict[str, int]
    ) -> tuple[str, Any] | None:
        if isinstance(value, MoltValue):
            return self._normalize_value_operand_key(value, const_int_values)
        try:
            hash(value)
            return ("const", value)
        except TypeError:
            return ("const_repr", repr(value))

    def _const_type_tag_for_lattice_value(self, value: Any) -> int | None:
        if isinstance(value, bool):
            return BUILTIN_TYPE_TAGS["bool"]
        if isinstance(value, int):
            return BUILTIN_TYPE_TAGS["int"]
        if isinstance(value, float):
            return BUILTIN_TYPE_TAGS["float"]
        if isinstance(value, str):
            return BUILTIN_TYPE_TAGS["str"]
        if isinstance(value, bytes):
            return BUILTIN_TYPE_TAGS["bytes"]
        if isinstance(value, list):
            return BUILTIN_TYPE_TAGS["list"]
        if isinstance(value, tuple):
            return BUILTIN_TYPE_TAGS["tuple"]
        if isinstance(value, dict):
            return BUILTIN_TYPE_TAGS["dict"]
        if isinstance(value, set):
            return BUILTIN_TYPE_TAGS["set"]
        if isinstance(value, frozenset):
            return BUILTIN_TYPE_TAGS["frozenset"]
        if isinstance(value, range):
            return BUILTIN_TYPE_TAGS["range"]
        return None

    def _heap_alias_class_for_read_op(
        self, op: MoltOp, value_type_tags: dict[str, int]
    ) -> str | None:
        if not op.args:
            return None
        primary = op.args[0]
        if not isinstance(primary, MoltValue):
            return "indexable"
        type_tag = value_type_tags.get(primary.name)
        if op.kind == "LEN":
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["frozenset"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            return "indexable"
        if op.kind == "INDEX":
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            return "indexable"
        if op.kind == "CONTAINS":
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["frozenset"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            return "indexable"
        if op.kind in {
            "GET_ATTR",
            "GETATTR",
            "LOAD_ATTR",
            "GETATTR_NAME",
            "HASATTR_NAME",
            "GETATTR_SPECIAL_OBJ",
            "GETATTR_GENERIC_OBJ",
            "GETATTR_GENERIC_PTR",
            "GETATTR_NAME_DEFAULT",
            "GUARDED_GETATTR",
            "MODULE_GET_ATTR",
        }:
            return "attr"
        return "indexable"

    def _is_uncertain_heap_boundary(self, op_kind: str) -> bool:
        return op_kind in {
            "CALL",
            "CALL_INDIRECT",
            "CALL_INTERNAL",
            "INVOKE_FFI",
        }

    def _heap_alias_classes_for_write_op(
        self, op: MoltOp, value_type_tags: dict[str, int]
    ) -> set[str]:
        if op.kind in {
            "DICT_SET",
            "DICT_STR_INT_INC",
            "DICT_SPLIT_COUNT_INT_INC",
            "DICT_SETDEFAULT",
            "DICT_POP",
            "DICT_POPITEM",
            "DICT_CLEAR",
            "DICT_UPDATE",
            "DICT_UPDATE_KWSTAR",
        }:
            return {"dict", "indexable"}
        if op.kind in {
            "LIST_APPEND",
            "LIST_EXTEND",
            "LIST_POP",
            "LIST_REMOVE",
            "LIST_INSERT",
            "LIST_CLEAR",
            "LIST_REVERSE",
        }:
            return {"list", "indexable"}
        if op.kind in {
            "STORE_ATTR",
            "SET_ATTR",
            "SETATTR",
            "SETATTR_INIT",
            "SETATTR_GENERIC_OBJ",
            "SETATTR_GENERIC_PTR",
            "GUARDED_SETATTR",
            "GUARDED_SETATTR_INIT",
            "DEL_ATTR",
            "DELATTR",
            "SETATTR_NAME",
            "DELATTR_NAME",
        }:
            return {"attr"}
        if op.kind in {"STORE_INDEX", "SET_INDEX", "DEL_INDEX"}:
            if not op.args or not isinstance(op.args[0], MoltValue):
                return {"dict", "list", "indexable"}
            type_tag = value_type_tags.get(op.args[0].name)
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return {"dict", "indexable"}
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return {"list", "indexable"}
            return {"dict", "list", "indexable"}
        return {"dict", "list", "indexable", "attr"}

    def _is_heap_read_key(self, key: tuple[Any, ...]) -> bool:
        return bool(key) and key[0] == "READ_HEAP_CLASS"

    def _heap_read_key_class(self, key: tuple[Any, ...]) -> str | None:
        if not self._is_heap_read_key(key):
            return None
        if len(key) < 2:
            return None
        read_class = key[1]
        if not isinstance(read_class, str):
            return None
        return read_class

    def _is_read_key_invalidated_by_alias_classes(
        self, key: tuple[Any, ...], alias_classes: set[str]
    ) -> bool:
        read_class = self._heap_read_key_class(key)
        if read_class is None:
            return False
        if read_class == "immutable_len":
            return False
        if read_class == "indexable":
            return bool(alias_classes.intersection({"indexable", "dict", "list"}))
        return read_class in alias_classes

    def _int_const_from_definition(
        self, name: str, definitions: dict[str, MoltOp]
    ) -> int | None:
        memo: dict[str, int | None] = {}
        visiting: set[str] = set()

        def resolve(value_name: str) -> int | None:
            if value_name in memo:
                return memo[value_name]
            if value_name in visiting:
                memo[value_name] = None
                return None
            visiting.add(value_name)
            op = definitions.get(value_name)
            resolved: int | None = None
            if op is not None:
                if op.kind in {"CONST", "CONST_BIGINT"} and op.args:
                    raw = op.args[0]
                    if isinstance(raw, int) and not isinstance(raw, bool):
                        resolved = raw
                elif op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
                    lhs = op.args[0]
                    rhs = op.args[1]
                    if isinstance(lhs, MoltValue) and isinstance(rhs, MoltValue):
                        lhs_const = resolve(lhs.name)
                        rhs_const = resolve(rhs.name)
                        if lhs_const is not None and rhs_const is not None:
                            if op.kind == "ADD":
                                resolved = lhs_const + rhs_const
                            elif op.kind == "SUB":
                                resolved = lhs_const - rhs_const
                            else:
                                resolved = lhs_const * rhs_const
                elif op.kind == "ABS" and len(op.args) == 1:
                    arg = op.args[0]
                    if isinstance(arg, MoltValue):
                        arg_const = resolve(arg.name)
                        if arg_const is not None:
                            resolved = abs(arg_const)
                elif op.kind == "PHI" and op.args:
                    phi_values: list[int] = []
                    for arg in op.args:
                        if not isinstance(arg, MoltValue):
                            phi_values = []
                            break
                        phi_const = resolve(arg.name)
                        if phi_const is None:
                            phi_values = []
                            break
                        phi_values.append(phi_const)
                    if phi_values and all(v == phi_values[0] for v in phi_values):
                        resolved = phi_values[0]
            visiting.discard(value_name)
            memo[value_name] = resolved
            return resolved

        return resolve(name)

    def _compare_int_truth(self, op_kind: str, lhs: int, rhs: int) -> bool | None:
        if op_kind == "EQ":
            return lhs == rhs
        if op_kind == "NE":
            return lhs != rhs
        if op_kind == "LT":
            return lhs < rhs
        if op_kind == "LE":
            return lhs <= rhs
        if op_kind == "GT":
            return lhs > rhs
        if op_kind == "GE":
            return lhs >= rhs
        return None

    def _detect_induction_step_from_recurrence(
        self, phi_name: str, recurrence: MoltOp, definitions: dict[str, MoltOp]
    ) -> int | None:
        if recurrence.kind not in {"ADD", "SUB"} or len(recurrence.args) != 2:
            return None
        lhs = recurrence.args[0]
        rhs = recurrence.args[1]
        if (
            isinstance(lhs, MoltValue)
            and lhs.name == phi_name
            and isinstance(rhs, MoltValue)
        ):
            rhs_const = self._int_const_from_definition(rhs.name, definitions)
            if rhs_const is None:
                return None
            if recurrence.kind == "ADD":
                return rhs_const
            return -rhs_const
        if (
            isinstance(rhs, MoltValue)
            and rhs.name == phi_name
            and isinstance(lhs, MoltValue)
            and recurrence.kind == "ADD"
        ):
            return self._int_const_from_definition(lhs.name, definitions)
        return None

    def _normalize_compare_for_induction(
        self, compare_op: str, lhs_is_iv: bool
    ) -> str | None:
        if lhs_is_iv:
            if compare_op in {"LT", "LE", "GT", "GE"}:
                return compare_op
            return None
        swapped = {
            "LT": "GT",
            "LE": "GE",
            "GT": "LT",
            "GE": "LE",
        }
        return swapped.get(compare_op)

    def _prove_monotonic_loop_compare(self, fact: LoopBoundFact) -> bool | None:
        start = fact.start
        step = fact.step
        bound = fact.bound
        compare_op = fact.compare_op

        if step == 0:
            return self._compare_int_truth(compare_op, start, bound)

        if step > 0:
            if compare_op == "LT" and start >= bound:
                return False
            if compare_op == "LE" and start > bound:
                return False
            if compare_op == "GT" and start > bound:
                return True
            if compare_op == "GE" and start >= bound:
                return True
            if compare_op == "EQ" and start > bound:
                return False
            if compare_op == "NE" and start > bound:
                return True
            return None

        if compare_op == "LT" and start < bound:
            return True
        if compare_op == "LE" and start <= bound:
            return True
        if compare_op == "GT" and start <= bound:
            return False
        if compare_op == "GE" and start < bound:
            return False
        if compare_op == "EQ" and start < bound:
            return False
        if compare_op == "NE" and start < bound:
            return True
        return None

    def _analyze_loop_bound_facts(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[int, LoopBoundFact]:
        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        loop_bound_facts: dict[int, LoopBoundFact] = {}

        def resolve_affine_iv_term(
            value: MoltValue,
            induction: dict[str, tuple[int, int]],
            *,
            seen: set[str] | None = None,
        ) -> tuple[str, int] | None:
            if value.name in induction:
                return value.name, 0
            if seen is None:
                seen = set()
            if value.name in seen:
                return None
            next_seen = set(seen)
            next_seen.add(value.name)
            def_op = definitions.get(value.name)
            if (
                def_op is None
                or def_op.kind not in {"ADD", "SUB"}
                or len(def_op.args) != 2
            ):
                return None
            lhs = def_op.args[0]
            rhs = def_op.args[1]
            if isinstance(lhs, MoltValue):
                lhs_term = resolve_affine_iv_term(lhs, induction, seen=next_seen)
            else:
                lhs_term = None
            if isinstance(rhs, MoltValue):
                rhs_term = resolve_affine_iv_term(rhs, induction, seen=next_seen)
            else:
                rhs_term = None
            if lhs_term is not None and isinstance(rhs, MoltValue):
                c = self._int_const_from_definition(rhs.name, definitions)
                if c is None:
                    return None
                if def_op.kind == "SUB":
                    c = -c
                return lhs_term[0], lhs_term[1] + c
            if (
                rhs_term is not None
                and isinstance(lhs, MoltValue)
                and def_op.kind == "ADD"
            ):
                c = self._int_const_from_definition(lhs.name, definitions)
                if c is None:
                    return None
                return rhs_term[0], rhs_term[1] + c
            return None

        for loop_start, loop_end in cfg.control.loop_start_to_end.items():
            if loop_end <= loop_start:
                continue

            induction_by_phi: dict[str, tuple[int, int]] = {}
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.kind != "PHI" or not op.args or op.result.name == "none":
                    continue
                phi_name = op.result.name
                start_value: int | None = None
                step_value: int | None = None
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        continue
                    recurrence = definitions.get(arg.name)
                    if recurrence is not None:
                        step = self._detect_induction_step_from_recurrence(
                            phi_name, recurrence, definitions
                        )
                        if step is not None:
                            step_value = step
                            continue
                    start_candidate = self._int_const_from_definition(
                        arg.name, definitions
                    )
                    if start_candidate is not None:
                        start_value = start_candidate
                if step_value is not None and start_value is not None:
                    induction_by_phi[phi_name] = (start_value, step_value)

            if not induction_by_phi:
                continue

            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if (
                    op.kind not in {"LT", "LE", "GT", "GE", "EQ", "NE"}
                    or len(op.args) != 2
                ):
                    continue
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    continue

                iv_name: str | None = None
                bound_value: int | None = None
                normalized_op: str | None = None
                lhs_term = resolve_affine_iv_term(lhs, induction_by_phi)
                rhs_term = resolve_affine_iv_term(rhs, induction_by_phi)
                if lhs_term is not None and rhs_term is None:
                    iv_name = lhs_term[0]
                    rhs_bound = self._int_const_from_definition(rhs.name, definitions)
                    if rhs_bound is None:
                        continue
                    bound_value = rhs_bound - lhs_term[1]
                    normalized_op = self._normalize_compare_for_induction(
                        op.kind, lhs_is_iv=True
                    )
                elif rhs_term is not None and lhs_term is None:
                    iv_name = rhs_term[0]
                    lhs_bound = self._int_const_from_definition(lhs.name, definitions)
                    if lhs_bound is None:
                        continue
                    bound_value = lhs_bound - rhs_term[1]
                    normalized_op = self._normalize_compare_for_induction(
                        op.kind, lhs_is_iv=False
                    )
                else:
                    continue

                if iv_name is None or bound_value is None or normalized_op is None:
                    continue
                start_value, step_value = induction_by_phi[iv_name]
                if op.result.name == "none":
                    continue
                loop_bound_facts[idx] = LoopBoundFact(
                    iv_name=iv_name,
                    start=start_value,
                    step=step_value,
                    bound=bound_value,
                    compare_op=normalized_op,
                    compare_index=idx,
                    compare_result=op.result.name,
                )

        return loop_bound_facts

    def _analyze_affine_loop_compare_truth(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[int, bool]:
        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        compare_truth: dict[int, bool] = {}

        def resolve_affine_iv_term(
            value: MoltValue,
            induction: dict[str, tuple[int, int]],
            *,
            seen: set[str] | None = None,
        ) -> tuple[str, int] | None:
            if value.name in induction:
                return value.name, 0
            if seen is None:
                seen = set()
            if value.name in seen:
                return None
            next_seen = set(seen)
            next_seen.add(value.name)
            def_op = definitions.get(value.name)
            if (
                def_op is None
                or def_op.kind not in {"ADD", "SUB"}
                or len(def_op.args) != 2
            ):
                return None
            lhs = def_op.args[0]
            rhs = def_op.args[1]
            lhs_term = (
                resolve_affine_iv_term(lhs, induction, seen=next_seen)
                if isinstance(lhs, MoltValue)
                else None
            )
            rhs_term = (
                resolve_affine_iv_term(rhs, induction, seen=next_seen)
                if isinstance(rhs, MoltValue)
                else None
            )
            if lhs_term is not None and isinstance(rhs, MoltValue):
                rhs_const = self._int_const_from_definition(rhs.name, definitions)
                if rhs_const is None:
                    return None
                if def_op.kind == "SUB":
                    rhs_const = -rhs_const
                return lhs_term[0], lhs_term[1] + rhs_const
            if (
                rhs_term is not None
                and isinstance(lhs, MoltValue)
                and def_op.kind == "ADD"
            ):
                lhs_const = self._int_const_from_definition(lhs.name, definitions)
                if lhs_const is None:
                    return None
                return rhs_term[0], rhs_term[1] + lhs_const
            return None

        for loop_start, loop_end in cfg.control.loop_start_to_end.items():
            if loop_end <= loop_start:
                continue

            induction_by_phi: dict[str, tuple[int, int]] = {}
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.kind != "PHI" or not op.args or op.result.name == "none":
                    continue
                phi_name = op.result.name
                start_value: int | None = None
                step_value: int | None = None
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        continue
                    recurrence = definitions.get(arg.name)
                    if recurrence is not None:
                        step = self._detect_induction_step_from_recurrence(
                            phi_name, recurrence, definitions
                        )
                        if step is not None:
                            step_value = step
                            continue
                    start_candidate = self._int_const_from_definition(
                        arg.name, definitions
                    )
                    if start_candidate is not None:
                        start_value = start_candidate
                if step_value is not None and start_value is not None:
                    induction_by_phi[phi_name] = (start_value, step_value)

            if not induction_by_phi:
                continue

            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if (
                    op.kind not in {"LT", "LE", "GT", "GE", "EQ", "NE"}
                    or len(op.args) != 2
                ):
                    continue
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    continue
                lhs_term = resolve_affine_iv_term(lhs, induction_by_phi)
                rhs_term = resolve_affine_iv_term(rhs, induction_by_phi)
                if lhs_term is None or rhs_term is None:
                    continue
                if lhs_term[0] != rhs_term[0]:
                    continue
                proven = self._compare_int_truth(op.kind, lhs_term[1], rhs_term[1])
                if isinstance(proven, bool):
                    compare_truth[idx] = proven

        return compare_truth

    def _analyze_loop_induction_steps(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[str, int]:
        induction_steps: dict[str, int] = {}
        for fact in self._analyze_loop_bound_facts(ops, cfg).values():
            induction_steps.setdefault(fact.iv_name, fact.step)
        if induction_steps:
            return induction_steps

        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        for op in ops:
            if op.kind != "PHI" or not op.args or op.result.name == "none":
                continue
            phi_name = op.result.name
            for arg in op.args:
                if not isinstance(arg, MoltValue):
                    continue
                recurrence = definitions.get(arg.name)
                if recurrence is None:
                    continue
                step = self._detect_induction_step_from_recurrence(
                    phi_name, recurrence, definitions
                )
                if step is not None:
                    induction_steps[phi_name] = step
                    break
        return induction_steps

    def _value_number_key_for_op(
        self,
        op: MoltOp,
        const_int_values: dict[str, int],
        value_type_tags: dict[str, int],
        induction_steps: dict[str, int],
        *,
        alias_epochs: dict[str, int],
        object_epochs: dict[str, int],
        memory_epoch: int,
    ) -> tuple[Any, ...] | None:
        if not self._is_cse_eligible_op(op.kind):
            return None
        effect_class = self._op_effect_class(op.kind)

        const_key = self._const_cache_key_for_op(op)
        if const_key is not None:
            return ("CONST",) + const_key

        if op.kind == "IS" and len(op.args) == 2:
            lhs = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs is not None and rhs is not None:
                return ("IS", lhs, rhs)

        if op.kind == "TYPE_OF" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                if effect_class == "reads_heap":
                    return ("READ_HEAP", memory_epoch, "TYPE_OF", arg)
                return ("TYPE_OF", arg)

        if op.kind == "NOT" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                return ("NOT", arg)

        if op.kind == "ABS" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                return ("ABS", arg)

        if op.kind in {"AND", "OR"} and len(op.args) == 2:
            lhs = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs is not None and rhs is not None:
                return ("BOOL_BINOP", op.kind, lhs, rhs)

        if (
            op.kind in {"EQ", "NE", "LT", "LE", "GT", "GE", "STRING_EQ"}
            and len(op.args) == 2
        ):
            lhs_key = self._normalize_operand_key_for_value_numbering(
                op.args[0], const_int_values
            )
            rhs_key = self._normalize_operand_key_for_value_numbering(
                op.args[1], const_int_values
            )
            if lhs_key is None or rhs_key is None:
                return None
            if op.kind in {"EQ", "NE", "STRING_EQ"} and rhs_key < lhs_key:
                lhs_key, rhs_key = rhs_key, lhs_key
            return ("CMP_PURE", op.kind, lhs_key, rhs_key)

        if op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
            lhs_key = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs_key = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs_key is None or rhs_key is None:
                return None

            if op.kind in {"ADD", "MUL"} and rhs_key < lhs_key:
                lhs_key, rhs_key = rhs_key, lhs_key

            lhs = op.args[0]
            rhs = op.args[1]
            if (
                op.kind in {"ADD", "SUB"}
                and isinstance(lhs, MoltValue)
                and isinstance(rhs, MoltValue)
                and lhs.name in induction_steps
                and rhs.name in const_int_values
            ):
                return (
                    "INDUCT_ARITH",
                    op.kind,
                    lhs.name,
                    induction_steps[lhs.name],
                    const_int_values[rhs.name],
                )

            return ("ARITH_PURE", op.kind, lhs_key, rhs_key)
        if effect_class == "reads_heap":
            normalized_args: list[tuple[str, Any]] = []
            for arg in op.args:
                key = self._normalize_operand_key_for_value_numbering(
                    arg, const_int_values
                )
                if key is None:
                    return None
                normalized_args.append(key)
            read_alias_class = self._heap_alias_class_for_read_op(op, value_type_tags)
            if read_alias_class is None:
                return None
            object_epoch = 0
            if op.args and isinstance(op.args[0], MoltValue):
                object_epoch = object_epochs.get(op.args[0].name, 0)
            if read_alias_class == "immutable_len":
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            class_epoch = alias_epochs.get(read_alias_class, 0)
            if read_alias_class in {"dict", "list"}:
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    class_epoch,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            if read_alias_class == "indexable":
                indexable_epoch = alias_epochs.get("indexable", 0)
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    indexable_epoch,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            return (
                "READ_HEAP_CLASS",
                read_alias_class,
                class_epoch,
                object_epoch,
                memory_epoch,
                op.kind,
                tuple(normalized_args),
            )
        return None

    def _kill_value_in_canonicalization_state(
        self, state: CanonicalizationState, name: str
    ) -> None:
        aliases: dict[str, MoltValue] = state["aliases"]
        aliases.pop(name, None)
        stale_aliases = [key for key, value in aliases.items() if value.name == name]
        for key in stale_aliases:
            aliases.pop(key, None)

        state["const_int_values"].pop(name, None)
        state["value_type_tags"].pop(name, None)

        available_values: dict[tuple[Any, ...], MoltValue] = state["available_values"]
        stale_values = [
            key for key, value in available_values.items() if value.name == name
        ]
        for key in stale_values:
            available_values.pop(key, None)

        guard_dict_shapes: dict[str, tuple[str, str]] = state["guard_dict_shapes"]
        guard_dict_shapes.pop(name, None)
        stale_dict_shapes = [
            key
            for key, (dict_type_name, version_name) in guard_dict_shapes.items()
            if dict_type_name == name or version_name == name
        ]
        for key in stale_dict_shapes:
            guard_dict_shapes.pop(key, None)
        object_epochs: dict[str, int] = state["object_epochs"]
        object_epochs.pop(name, None)
        self._invalidate_canonicalization_state_signature(state)

    def _intersect_canonicalization_state(
        self, left: CanonicalizationState, right: CanonicalizationState
    ) -> CanonicalizationState:
        aliases: dict[str, MoltValue] = {}
        for key, left_value in left["aliases"].items():
            right_value = right["aliases"].get(key)
            if (
                isinstance(right_value, MoltValue)
                and right_value.name == left_value.name
            ):
                aliases[key] = left_value

        const_int_values: dict[str, int] = {}
        for key, left_value in left["const_int_values"].items():
            right_value = right["const_int_values"].get(key)
            if isinstance(right_value, int) and right_value == left_value:
                const_int_values[key] = left_value

        value_type_tags: dict[str, int] = {}
        for key, left_value in left["value_type_tags"].items():
            right_value = right["value_type_tags"].get(key)
            if isinstance(right_value, int) and right_value == left_value:
                value_type_tags[key] = left_value

        available_values: dict[tuple[Any, ...], MoltValue] = {}
        for key, left_value in left["available_values"].items():
            right_value = right["available_values"].get(key)
            if (
                isinstance(right_value, MoltValue)
                and right_value.name == left_value.name
            ):
                available_values[key] = left_value

        guard_dict_shapes: dict[str, tuple[str, str]] = {}
        for key, left_value in left["guard_dict_shapes"].items():
            right_value = right["guard_dict_shapes"].get(key)
            if (
                isinstance(right_value, tuple)
                and len(right_value) == 2
                and right_value == left_value
            ):
                guard_dict_shapes[key] = left_value

        alias_epochs: dict[str, int] = {}
        left_alias_epochs = left["alias_epochs"]
        right_alias_epochs = right["alias_epochs"]
        for key in set(left_alias_epochs.keys()).union(right_alias_epochs.keys()):
            alias_epochs[key] = max(
                int(left_alias_epochs.get(key, 0)),
                int(right_alias_epochs.get(key, 0)),
            )

        object_epochs: dict[str, int] = {}
        left_object_epochs = left["object_epochs"]
        right_object_epochs = right["object_epochs"]
        for key in set(left_object_epochs.keys()).union(right_object_epochs.keys()):
            object_epochs[key] = max(
                int(left_object_epochs.get(key, 0)),
                int(right_object_epochs.get(key, 0)),
            )

        return {
            "aliases": aliases,
            "const_int_values": const_int_values,
            "value_type_tags": value_type_tags,
            "available_values": available_values,
            "guard_dict_shapes": guard_dict_shapes,
            "alias_epochs": alias_epochs,
            "object_epochs": object_epochs,
            "memory_epoch": max(left["memory_epoch"], right["memory_epoch"]),
        }

    def _intersect_canonicalization_states(
        self, states: list[CanonicalizationState]
    ) -> CanonicalizationState:
        if not states:
            return self._empty_canonicalization_state()
        merged = self._clone_canonicalization_state(states[0])
        for state in states[1:]:
            merged = self._intersect_canonicalization_state(merged, state)
        return merged

    def _canonicalization_state_signature(
        self, state: CanonicalizationState
    ) -> tuple[
        tuple[tuple[str, str], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[tuple[Any, ...], str], ...],
        tuple[tuple[str, tuple[str, str]], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[str, int], ...],
        int,
    ]:
        cached_signature = cast(Any, state).get(
            _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY
        )
        if cached_signature is not None:
            return cast(
                tuple[
                    tuple[tuple[str, str], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[tuple[Any, ...], str], ...],
                    tuple[tuple[str, tuple[str, str]], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[str, int], ...],
                    int,
                ],
                cached_signature,
            )
        alias_items = tuple(
            sorted((key, value.name) for key, value in state["aliases"].items())
        )
        const_items = tuple(sorted(state["const_int_values"].items()))
        tag_items = tuple(sorted(state["value_type_tags"].items()))
        available_items = tuple(
            sorted(
                (key, value.name) for key, value in state["available_values"].items()
            )
        )
        dict_shape_items = tuple(sorted(state["guard_dict_shapes"].items()))
        alias_epoch_items = tuple(sorted(state["alias_epochs"].items()))
        object_epoch_items = tuple(sorted(state["object_epochs"].items()))
        memory_epoch = state["memory_epoch"]
        signature = (
            alias_items,
            const_items,
            tag_items,
            available_items,
            dict_shape_items,
            alias_epoch_items,
            object_epoch_items,
            memory_epoch,
        )
        cast(Any, state)[_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY] = signature
        return signature

    def _canonicalize_block_with_state(
        self,
        ops: list[MoltOp],
        in_state: CanonicalizationState,
        *,
        induction_steps: dict[str, int],
    ) -> tuple[list[MoltOp], CanonicalizationState]:
        func_stats = self._midend_function_stats()
        state = self._clone_canonicalization_state(in_state)
        aliases: dict[str, MoltValue] = state["aliases"]
        const_int_values: dict[str, int] = state["const_int_values"]
        value_type_tags: dict[str, int] = state["value_type_tags"]
        available_values: dict[tuple[Any, ...], MoltValue] = state["available_values"]
        guard_dict_shapes: dict[str, tuple[str, str]] = state["guard_dict_shapes"]
        alias_epochs: dict[str, int] = state["alias_epochs"]
        object_epochs: dict[str, int] = state["object_epochs"]
        memory_epoch = state["memory_epoch"]
        state_dirty = False

        out: list[MoltOp] = []
        for op in ops:
            canonical_args = [
                self._rewrite_aliases_in_arg(arg, aliases) for arg in op.args
            ]
            canonical_op = MoltOp(
                kind=op.kind,
                args=canonical_args,
                result=op.result,
                metadata=op.metadata,
                source_line=op.source_line,
                col_offset=op.col_offset,
                end_col_offset=op.end_col_offset,
            )

            result_name = canonical_op.result.name
            if result_name != "none":
                self._kill_value_in_canonicalization_state(state, result_name)

            if canonical_op.kind == "PHI" and canonical_op.args:
                phi_args = canonical_op.args
                if all(
                    isinstance(arg, MoltValue) and arg.name == phi_args[0].name
                    for arg in phi_args
                ):
                    shared = self._resolve_alias_value(phi_args[0], aliases)
                    aliases[result_name] = shared
                    state_dirty = True
                    if shared.name in const_int_values:
                        const_int_values[result_name] = const_int_values[shared.name]
                    if shared.name in value_type_tags:
                        value_type_tags[result_name] = value_type_tags[shared.name]
                    continue

            if canonical_op.kind == "GUARD_DICT_SHAPE" and len(canonical_op.args) == 3:
                guarded_obj = canonical_op.args[0]
                dict_type = canonical_op.args[1]
                version = canonical_op.args[2]
                if (
                    isinstance(guarded_obj, MoltValue)
                    and isinstance(dict_type, MoltValue)
                    and isinstance(version, MoltValue)
                ):
                    expected = (dict_type.name, version.name)
                    if guard_dict_shapes.get(guarded_obj.name) == expected:
                        continue

            if canonical_op.kind == "GUARD_TAG" and len(canonical_op.args) == 2:
                guarded = canonical_op.args[0]
                expected = canonical_op.args[1]
                if isinstance(guarded, MoltValue) and isinstance(expected, MoltValue):
                    actual_tag = value_type_tags.get(guarded.name)
                    expected_tag = const_int_values.get(expected.name)
                    if actual_tag is not None and expected_tag == actual_tag:
                        continue

            value_key = self._value_number_key_for_op(
                canonical_op,
                const_int_values,
                value_type_tags,
                induction_steps,
                alias_epochs=alias_epochs,
                object_epochs=object_epochs,
                memory_epoch=memory_epoch,
            )
            effect_class = self._op_effect_class(canonical_op.kind)
            if (
                effect_class == "reads_heap"
                and value_key is not None
                and result_name != "none"
            ):
                func_stats["cse_readheap_attempted"] += 1
            if value_key is not None and result_name != "none":
                cached = available_values.get(value_key)
                if cached is not None:
                    shared = self._resolve_alias_value(cached, aliases)
                    aliases[result_name] = shared
                    self.midend_stats["gvn_hits"] += 1
                    if effect_class == "reads_heap":
                        func_stats["cse_readheap_accepted"] += 1
                    if shared.name in const_int_values:
                        const_int_values[result_name] = const_int_values[shared.name]
                    if shared.name in value_type_tags:
                        value_type_tags[result_name] = value_type_tags[shared.name]
                    continue
                if effect_class == "reads_heap":
                    func_stats["cse_readheap_rejected"] += 1

            # Constant-fold arithmetic: when both operands are known
            # constants, compute the result.  If it overflows the
            # 47-bit signed inline range, replace with CONST_BIGINT
            # to prevent Cranelift 0.130 constant-folding miscompilation.
            _folded_to_bigint = False
            if (
                canonical_op.kind in {"ADD", "SUB", "MUL", "POW"}
                and len(canonical_op.args) == 2
            ):
                lhs, rhs = canonical_op.args
                if isinstance(lhs, MoltValue) and isinstance(rhs, MoltValue):
                    lhs_const = const_int_values.get(lhs.name)
                    rhs_const = const_int_values.get(rhs.name)
                    if lhs_const is not None and rhs_const is not None:
                        if canonical_op.kind == "ADD":
                            folded = lhs_const + rhs_const
                        elif canonical_op.kind == "SUB":
                            folded = lhs_const - rhs_const
                        elif canonical_op.kind == "MUL":
                            folded = lhs_const * rhs_const
                        else:
                            # POW – only fold for small non-negative exponents
                            # to avoid float results and unbounded computation.
                            if 0 <= rhs_const <= 64:
                                folded = lhs_const**rhs_const
                            else:
                                folded = None
                        if folded is None:
                            # Skip folding (e.g. negative exponent)
                            out.append(canonical_op)
                            continue
                        if not (_INLINE_INT_MIN <= folded <= _INLINE_INT_MAX):
                            canonical_op = MoltOp(
                                kind="CONST_BIGINT",
                                args=[str(folded)],
                                result=canonical_op.result,
                                source_line=canonical_op.source_line,
                                col_offset=canonical_op.col_offset,
                                end_col_offset=canonical_op.end_col_offset,
                            )
                            _folded_to_bigint = True
                        elif canonical_op.kind == "POW":
                            canonical_op = MoltOp(
                                kind="CONST",
                                args=[folded],
                                result=canonical_op.result,
                                source_line=canonical_op.source_line,
                                col_offset=canonical_op.col_offset,
                                end_col_offset=canonical_op.end_col_offset,
                            )
                        const_int_values[result_name] = folded
                        state_dirty = True

            # Constant-fold bitwise operations: when both operands are
            # known integer constants, compute the result at compile time.
            if (
                not _folded_to_bigint
                and canonical_op.kind
                in {
                    "BIT_AND",
                    "BIT_OR",
                    "BIT_XOR",
                    "LSHIFT",
                    "RSHIFT",
                    "INVERT",
                }
                and len(canonical_op.args) >= 1
            ):
                args = canonical_op.args
                if canonical_op.kind == "INVERT" and len(args) == 1:
                    arg = args[0]
                    if isinstance(arg, MoltValue):
                        arg_const = const_int_values.get(arg.name)
                        if arg_const is not None:
                            folded_bw = ~arg_const
                            if _INLINE_INT_MIN <= folded_bw <= _INLINE_INT_MAX:
                                const_int_values[result_name] = folded_bw
                                state_dirty = True
                elif len(args) == 2:
                    lhs_bw, rhs_bw = args
                    if isinstance(lhs_bw, MoltValue) and isinstance(rhs_bw, MoltValue):
                        lc = const_int_values.get(lhs_bw.name)
                        rc = const_int_values.get(rhs_bw.name)
                        if lc is not None and rc is not None:
                            if canonical_op.kind == "BIT_AND":
                                folded_bw = lc & rc
                            elif canonical_op.kind == "BIT_OR":
                                folded_bw = lc | rc
                            elif canonical_op.kind == "BIT_XOR":
                                folded_bw = lc ^ rc
                            elif canonical_op.kind == "LSHIFT":
                                folded_bw = lc << rc if 0 <= rc <= 128 else None
                            elif canonical_op.kind == "RSHIFT":
                                folded_bw = lc >> rc if 0 <= rc <= 128 else None
                            else:
                                folded_bw = None
                            if folded_bw is not None:
                                if not (
                                    _INLINE_INT_MIN <= folded_bw <= _INLINE_INT_MAX
                                ):
                                    canonical_op = MoltOp(
                                        kind="CONST_BIGINT",
                                        args=[str(folded_bw)],
                                        result=canonical_op.result,
                                        source_line=canonical_op.source_line,
                                        col_offset=canonical_op.col_offset,
                                        end_col_offset=canonical_op.end_col_offset,
                                    )
                                    _folded_to_bigint = True
                                const_int_values[result_name] = folded_bw
                                state_dirty = True

            out.append(canonical_op)

            if not _folded_to_bigint and canonical_op.kind == "CONST":
                value = canonical_op.args[0]
                if isinstance(value, int) and not isinstance(value, bool):
                    const_int_values[result_name] = value
                    state_dirty = True
            elif canonical_op.kind == "ABS" and len(canonical_op.args) == 1:
                arg = canonical_op.args[0]
                if isinstance(arg, MoltValue):
                    arg_const = const_int_values.get(arg.name)
                    if arg_const is not None:
                        const_int_values[result_name] = abs(arg_const)
                        state_dirty = True
            elif canonical_op.kind == "GUARD_TAG" and len(canonical_op.args) == 2:
                guarded, expected = canonical_op.args
                if isinstance(guarded, MoltValue) and isinstance(expected, MoltValue):
                    expected_tag = const_int_values.get(expected.name)
                    if expected_tag is not None:
                        value_type_tags[guarded.name] = expected_tag
                        state_dirty = True
            elif (
                canonical_op.kind == "GUARD_DICT_SHAPE" and len(canonical_op.args) == 3
            ):
                guarded_obj, dict_type, version = canonical_op.args
                if (
                    isinstance(guarded_obj, MoltValue)
                    and isinstance(dict_type, MoltValue)
                    and isinstance(version, MoltValue)
                ):
                    guard_dict_shapes[guarded_obj.name] = (dict_type.name, version.name)
                    state_dirty = True
            type_tag = self._const_type_tag(canonical_op)
            if type_tag is None and result_name != "none":
                if canonical_op.kind in {
                    "NOT",
                    "IS",
                    "AND",
                    "OR",
                    "EQ",
                    "NE",
                    "LT",
                    "LE",
                    "GT",
                    "GE",
                    "STRING_EQ",
                    "ISINSTANCE",
                    "EXCEPTION_MATCH_BUILTIN",
                }:
                    type_tag = BUILTIN_TYPE_TAGS["bool"]
                elif canonical_op.kind in {"LEN", "TYPE_OF"}:
                    type_tag = BUILTIN_TYPE_TAGS["int"]
                elif canonical_op.kind == "ABS" and len(canonical_op.args) == 1:
                    abs_arg = canonical_op.args[0]
                    if isinstance(abs_arg, MoltValue):
                        abs_arg_tag = value_type_tags.get(abs_arg.name)
                        if abs_arg_tag in {
                            BUILTIN_TYPE_TAGS["int"],
                            BUILTIN_TYPE_TAGS["float"],
                        }:
                            type_tag = abs_arg_tag
                elif canonical_op.kind == "DICT_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["dict"]
                elif canonical_op.kind == "LIST_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["list"]
                elif canonical_op.kind == "TUPLE_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["tuple"]
                elif canonical_op.kind == "SET_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["set"]
                elif canonical_op.kind == "FROZENSET_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["frozenset"]
                elif canonical_op.kind == "RANGE_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["range"]
            if type_tag is not None and result_name != "none":
                value_type_tags[result_name] = type_tag
                state_dirty = True
            if canonical_op.kind == "IS" and result_name != "none":
                value_type_tags[result_name] = BUILTIN_TYPE_TAGS["bool"]
                state_dirty = True
            if value_key is not None and result_name != "none":
                available_values[value_key] = canonical_op.result
                state_dirty = True

            if self._is_canonicalization_barrier_op(canonical_op.kind):
                aliases.clear()
                const_int_values.clear()
                value_type_tags.clear()
                available_values.clear()
                guard_dict_shapes.clear()
                state_dirty = True

            if effect_class == "writes_heap":
                write_alias_classes = self._heap_alias_classes_for_write_op(
                    canonical_op, value_type_tags
                )
                if self._is_uncertain_heap_boundary(canonical_op.kind):
                    memory_epoch += 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_heap_read_key(key)
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                    for alias_class in sorted(alias_epochs):
                        alias_epochs[alias_class] = alias_epochs.get(alias_class, 0) + 1
                    guard_dict_shapes.clear()
                    state_dirty = True
                    continue
                if canonical_op.args and isinstance(canonical_op.args[0], MoltValue):
                    obj_name = canonical_op.args[0].name
                    object_epochs[obj_name] = object_epochs.get(obj_name, 0) + 1
                    state_dirty = True
                if write_alias_classes:
                    for alias_class in sorted(write_alias_classes):
                        alias_epochs[alias_class] = alias_epochs.get(alias_class, 0) + 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_read_key_invalidated_by_alias_classes(
                            key, write_alias_classes
                        )
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                else:
                    memory_epoch += 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_heap_read_key(key)
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                guard_dict_shapes.clear()
                state_dirty = True

        state["alias_epochs"] = alias_epochs
        state["object_epochs"] = object_epochs
        state["memory_epoch"] = memory_epoch
        if state_dirty:
            self._invalidate_canonicalization_state_signature(state)
        return out, state

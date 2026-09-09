"""Short-circuit value and condition lowering.

A syntax condition consumes truth, whereas a value expression returns the
selected operand unchanged. Tested values carry an observed truth only where
CPython's target-version lowering permits it: explicit comparison/conditional
value boundaries, and pre-3.14 nested BoolOps, retain their observable retest.
Flat boolean/comparison chains are emitted iteratively, with one merge authority
for synchronous PHI/COPY and suspension-safe asynchronous scratch storage.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal

from molt.frontend._types import MoltOp, MoltValue, ScratchCell
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object

_ConditionMode = Literal["value", "condition", "tested"]


@dataclass(frozen=True)
class _ConditionBindingState:
    unbound: frozenset[str]
    provenance: dict[str, frozenset[str]]
    locals: dict[str, MoltValue | None]
    hints: tuple[dict[str, str | None], ...]


@dataclass
class _ConditionMerge:
    cells: tuple[ScratchCell, ...]
    copies: tuple[MoltValue, ...]
    entry: _ConditionBindingState
    true_state: _ConditionBindingState | None = None


class ConditionFlowMixin(_MixinBase):
    def _capture_condition_bindings(
        self, names: tuple[str, ...]
    ) -> _ConditionBindingState:
        return _ConditionBindingState(
            frozenset(self.unbound_check_names),
            dict(self.imported_module_provenance),
            {name: self.locals.get(name) for name in names},
            tuple(
                {name: hints.get(name) for name in names}
                for hints in (
                    self.boxed_local_hints,
                    self.async_public_hints,
                    self.async_internal_hints,
                )
            ),
        )

    def _restore_condition_bindings(self, state: _ConditionBindingState) -> None:
        self.unbound_check_names = set(state.unbound)
        self.imported_module_provenance = dict(state.provenance)
        for name, value in state.locals.items():
            if value is None:
                self.locals.pop(name, None)
            else:
                self.locals[name] = value
            # Branch-local shape facts are not facts about the merged binding.
            self.container_elem_hints.pop(name, None)
            self.bytearray_len_hints.pop(name, None)
        for hints, saved in zip(
            (
                self.boxed_local_hints,
                self.async_public_hints,
                self.async_internal_hints,
            ),
            state.hints,
            strict=True,
        ):
            for name, hint in saved.items():
                if hint is None:
                    hints.pop(name, None)
                else:
                    hints[name] = hint

    def _new_condition_merge(
        self, width: int, names: tuple[str, ...]
    ) -> _ConditionMerge:
        entry = self._capture_condition_bindings(names)
        self.control_flow_depth += 1
        if self.is_async():
            return _ConditionMerge(
                tuple(self._new_scratch_cell() for _ in range(width)), (), entry
            )
        copies: list[MoltValue] = []
        if not self.enable_phi:
            for _ in range(width):
                value = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=value))
                copies.append(value)
        return _ConditionMerge((), tuple(copies), entry)

    def _condition_else(self, merge: _ConditionMerge) -> None:
        merge.true_state = self._capture_condition_bindings(tuple(merge.entry.locals))
        self._restore_condition_bindings(merge.entry)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))

    def _store_condition_branch(
        self, merge: _ConditionMerge, values: tuple[MoltValue, ...]
    ) -> tuple[MoltValue, ...]:
        if merge.cells:
            for cell, value in zip(merge.cells, values, strict=True):
                self._store_scratch_cell(cell, value)
            return values
        if merge.copies:
            for target, value in zip(merge.copies, values, strict=True):
                self.emit(MoltOp(kind="COPY", args=[value], result=target))
            return values
        # Branch-local definitions are required even for an unchanged operand:
        # the backend must identify the predecessor owning each PHI input.
        aliases: list[MoltValue] = []
        for value in values:
            alias = MoltValue(self.next_var(), type_hint=value.type_hint)
            self.emit(MoltOp(kind="IDENTITY_ALIAS", args=[value], result=alias))
            aliases.append(alias)
        return tuple(aliases)

    def _finish_condition_merge(
        self,
        merge: _ConditionMerge,
        true_values: tuple[MoltValue, ...],
        false_values: tuple[MoltValue, ...],
    ) -> tuple[MoltValue, ...]:
        true_state = merge.true_state
        if true_state is None:
            raise AssertionError("condition merge requires both predecessor states")
        false_state = self._capture_condition_bindings(tuple(merge.entry.locals))
        local_values: dict[str, MoltValue | None] = {}
        for name, true_value in true_state.locals.items():
            false_value = false_state.locals[name]
            selected = true_value if true_value is not None else false_value
            if selected is not None:
                hint = (
                    true_value.type_hint
                    if true_value is not None
                    and false_value is not None
                    and true_value.type_hint == false_value.type_hint
                    else "Any"
                )
                selected = MoltValue(selected.name, type_hint=hint)
            local_values[name] = selected
        self._restore_condition_bindings(
            _ConditionBindingState(
                true_state.unbound | false_state.unbound,
                self._join_imported_module_provenance(
                    true_state.provenance, false_state.provenance
                ),
                local_values,
                tuple(
                    {
                        name: hint if hint == false_hints[name] else None
                        for name, hint in true_hints.items()
                    }
                    for true_hints, false_hints in zip(
                        true_state.hints, false_state.hints, strict=True
                    )
                ),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.control_flow_depth -= 1
        values: list[MoltValue] = []
        for index, (true, false) in enumerate(
            zip(true_values, false_values, strict=True)
        ):
            hint = true.type_hint if true.type_hint == false.type_hint else "Any"
            if merge.cells:
                value = self._consume_scratch_cell(merge.cells[index])
                value.type_hint = hint
            elif merge.copies:
                value = merge.copies[index]
                value.type_hint = hint
            else:
                value = MoltValue(self.next_var(), type_hint=hint)
                self.emit(MoltOp(kind="PHI", args=[true, false], result=value))
            values.append(value)
        return tuple(values)

    def _condition_result(
        self,
        value: MoltValue,
        mode: _ConditionMode,
        *,
        known_truth: bool | None = None,
    ) -> tuple[MoltValue, ...]:
        if mode == "value":
            return (value,)
        if known_truth is None:
            truth = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="BOOL", args=[value], result=truth))
        else:
            truth = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[known_truth], result=truth))
        return (value, truth) if mode == "tested" else (truth,)

    def _emit_condition(self, node: ast.expr) -> MoltValue:
        """Consume a syntax condition once and return an exact bool."""
        return self._emit_expression_flow(node, "condition")[0]

    def _emit_expression_flow(
        self, node: ast.expr, mode: _ConditionMode
    ) -> tuple[MoltValue, ...]:
        names = set(self._collect_namedexpr_names(node))
        self._prepare_mutable_control_flow_bindings(names)
        depth = self.control_flow_depth
        try:
            return self._emit_expression_flow_prepared(node, mode)
        finally:
            self.control_flow_depth = depth
            self._evict_module_control_flow_bindings(names)

    def _emit_expression_flow_prepared(
        self, node: ast.expr, mode: _ConditionMode
    ) -> tuple[MoltValue, ...]:
        if mode == "tested" and (
            isinstance(node, (ast.Compare, ast.IfExp))
            or (isinstance(node, ast.BoolOp) and self.target_python < (3, 14))
        ):
            # These are real value boundaries in CPython, not redundant truth
            # callbacks. Direct nested BoolOps changed in 3.14; comparison and
            # conditional-expression boundaries did not.
            value = self._emit_expression_flow_prepared(node, "value")[0]
            return self._condition_result(value, mode)
        if (
            mode == "condition"
            and isinstance(node, ast.UnaryOp)
            and isinstance(node.op, ast.Not)
        ):
            operand = self._emit_expression_flow_prepared(node.operand, "condition")[0]
            return (self._emit_not(operand),)
        if isinstance(node, ast.BoolOp):
            return self._emit_boolean_flow(node, mode)
        if isinstance(node, ast.Compare):
            return self._emit_comparison_flow(node, mode)
        if isinstance(node, ast.IfExp):
            return self._emit_conditional_expression_flow(node, mode)
        value = self.visit(node)
        if value is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported conditional operand"
            )
        return self._condition_result(value, mode)

    def _emit_boolean_flow(
        self, node: ast.BoolOp, mode: _ConditionMode
    ) -> tuple[MoltValue, ...]:
        if not node.values:
            raise FrontendRejection(
                Diagnostic.INTERNAL_INVARIANT, "Empty bool op is not supported"
            )
        if not isinstance(node.op, (ast.And, ast.Or)):
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM, "Unsupported boolean operator"
            )
        is_and = isinstance(node.op, ast.And)
        names = tuple(self._collect_namedexpr_names(node))
        frames: list[tuple[_ConditionMerge, tuple[MoltValue, ...]]] = []
        for operand in node.values[:-1]:
            observed = self._emit_expression_flow_prepared(
                operand, "condition" if mode == "condition" else "tested"
            )
            merge = self._new_condition_merge(2 if mode == "tested" else 1, names)
            stop = self._emit_not(observed[-1]) if is_and else observed[-1]
            self.emit(MoltOp(kind="IF", args=[stop], result=MoltValue("none")))
            # Emit the stopped value before the continuing arm's suspension
            # labels. It is not live across a yield on the other runtime path.
            stopped = self._store_condition_branch(
                merge,
                self._condition_result(observed[0], mode, known_truth=not is_and),
            )
            self._condition_else(merge)
            frames.append((merge, stopped))
        result = self._emit_expression_flow_prepared(node.values[-1], mode)
        for merge, stopped in reversed(frames):
            continued = self._store_condition_branch(merge, result)
            result = self._finish_condition_merge(merge, stopped, continued)
        return result

    def _emit_comparison_flow(
        self, node: ast.Compare, mode: _ConditionMode
    ) -> tuple[MoltValue, ...]:
        names = tuple(self._collect_namedexpr_names(node))
        left = self.visit(node.left)
        if left is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported compare left operand"
            )
        frames: list[tuple[_ConditionMerge, tuple[MoltValue, ...]]] = []
        result: tuple[MoltValue, ...] = ()
        for index, (op, comparator) in enumerate(
            zip(node.ops, node.comparators, strict=True)
        ):
            left_cell = None
            if self.is_async() and self._expr_may_yield(comparator):
                left_cell = self._new_scratch_cell(left, type_hint=left.type_hint)
            right = self.visit(comparator)
            if right is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported compare right operand"
                )
            if left_cell is not None:
                left = self._consume_scratch_cell(left_cell)
            comparison = self._emit_compare_op(op, left, right)
            if index == len(node.ops) - 1:
                result = self._condition_result(comparison, mode)
                break
            merge = self._new_condition_merge(2 if mode == "tested" else 1, names)
            stop = self._emit_not(comparison)
            self.emit(MoltOp(kind="IF", args=[stop], result=MoltValue("none")))
            stopped = self._store_condition_branch(
                merge, self._condition_result(comparison, mode, known_truth=False)
            )
            self._condition_else(merge)
            frames.append((merge, stopped))
            left = right
        for merge, stopped in reversed(frames):
            continued = self._store_condition_branch(merge, result)
            result = self._finish_condition_merge(merge, stopped, continued)
        if not result:
            raise FrontendRejection(
                Diagnostic.INTERNAL_INVARIANT, "Empty comparison is not supported"
            )
        return result

    def _emit_conditional_expression_flow(
        self, node: ast.IfExp, mode: _ConditionMode
    ) -> tuple[MoltValue, ...]:
        condition = self._emit_expression_flow_prepared(node.test, "condition")[0]
        names = tuple(self._collect_namedexpr_names(node))
        merge = self._new_condition_merge(2 if mode == "tested" else 1, names)
        self.emit(MoltOp(kind="IF", args=[condition], result=MoltValue("none")))
        true_values = self._store_condition_branch(
            merge, self._emit_expression_flow_prepared(node.body, mode)
        )
        self._condition_else(merge)
        false_values = self._store_condition_branch(
            merge, self._emit_expression_flow_prepared(node.orelse, mode)
        )
        return self._finish_condition_merge(merge, true_values, false_values)

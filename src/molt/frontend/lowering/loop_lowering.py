"""LoopLoweringMixin: loop iteration, range lowering, and loop guard custody.

Move-only extraction from frontend/__init__.py. This lowering authority owns
iter/range/for/while emission, loop-body control-flow snapshots, loop orelse,
static-live branch emission, loop guard hoisting/invalidation, and specialized
loop fast paths shared by statement, comprehension, call, and analysis visitors.
A fast path reads each binding once, before the loop, where Python reads it
inside the loop, and trusts cached facts about the binding only where the
binding analysis proves that source read clean. A fused loop runs a prefix of
the ordinary loop, in bounded chunks, only where its runtime kernel or guard
proves, on the values the loop reads, that the chunk runs no Python code. Each
chunk leaves exactly the bindings and values the loop leaves after those items,
published before the chunk loop's back edge observes pending asynchronous work,
and the next chunk rereads them. The ordinary loop then continues from that
state on the same iterator or index, so nothing is evaluated twice.
"""

from __future__ import annotations

import ast
from typing import Callable

from molt.compiler_analysis.python_binding_facts import UNBOUND_IDENTITY
from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import LoopScope, MoltOp, MoltValue, ScratchCell
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection


class LoopLoweringMixin(GeneratorMixinBase):
    # Fused-loop reductions: each takes ``(it, acc, target)``, the loop's own
    # iterator and the current accumulator and loop target, and returns
    # ``(result, last, count, more)`` from an exact runtime kernel that consumes
    # at most one bounded chunk of the iterator.
    _VECTOR_REDUCTION_OPS = {
        "sum": "VEC_SUM",
        "prod": "VEC_PROD",
        "min": "VEC_MIN",
        "max": "VEC_MAX",
    }
    # A fused byte fill writes at most this many bytes per chunk.
    _BYTEARRAY_FILL_CHUNK = 1 << 20

    def _iterable_is_indexable(self, iterable: MoltValue | None) -> bool:
        if iterable is None:
            return False
        return iterable.type_hint in {
            "list",
            "tuple",
            "range",
            "memoryview",
        }

    def _iterable_is_indexable_for_loop(self, iterable: MoltValue | None) -> bool:
        if iterable is None:
            return False
        if not self._iterable_is_indexable(iterable):
            return False
        # List iteration must observe mutations (e.g., append during iteration).
        return iterable.type_hint != "list"

    def _emit_iter_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | ScratchCell | None = None,
    ) -> None:
        target = node.target
        item_hint = self._iteration_element_hint(node, iterable) or "Any"
        if self.is_async():
            iter_obj = self._emit_iter_new(iterable)
            iter_slot = self._new_async_internal_slot()
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", iter_slot, iter_obj],
                    result=MoltValue("none"),
                )
            )
            guard_map = self._emit_hoisted_loop_guards(node.body)
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            iter_val = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", iter_slot],
                    result=iter_val,
                )
            )
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            pair = self._emit_iter_next_checked(iter_val)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_TRUE",
                    args=[done],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
            self._emit_assign_target(target, item, None)
            scope = self._visit_loop_body(
                node.body, guard_map, loop_break_flag=loop_break_flag
            )
            if scope.needs_latch:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._emit_loop_exit(scope)
            return
        guard_map = (
            {}
            if self.current_func_name == "molt_main"
            else self._emit_hoisted_loop_guards(node.body)
        )

        def emit_loop_body() -> None:
            iter_obj = self._emit_iter_new(iterable)
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))

            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            pair = self._emit_iter_next_checked(iter_obj)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_TRUE",
                    args=[done],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
            self._emit_assign_target(target, item, None)
            scope = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            if scope.needs_latch:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._emit_loop_exit(scope)

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return

        emit_loop_body()

    def _emit_index_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | ScratchCell | None = None,
    ) -> None:
        target = node.target
        item_hint = self._iteration_element_hint(node, iterable) or "Any"
        if self.is_async():
            seq_slot = self._new_async_internal_slot()
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", seq_slot, iterable],
                    result=MoltValue("none"),
                )
            )
            length_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[iterable], result=length_val))
            length_slot = self._new_async_internal_slot()
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", length_slot, length_val],
                    result=MoltValue("none"),
                )
            )
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            idx_slot = self._new_async_internal_slot()
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", idx_slot, zero],
                    result=MoltValue("none"),
                )
            )
            guard_map = self._emit_hoisted_loop_guards(node.body)
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", idx_slot],
                    result=idx,
                )
            )
            seq_val = MoltValue(self.next_var(), type_hint=iterable.type_hint)
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", seq_slot],
                    result=seq_val,
                )
            )
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", length_slot],
                    result=length,
                )
            )
            cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[idx, length], result=cond))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[seq_val, idx], result=item))
            self._emit_assign_target(target, item, None)
            scope = self._visit_loop_body(
                node.body, guard_map, loop_break_flag=loop_break_flag
            )
            if scope.needs_latch:
                idx_after = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="LOAD_CLOSURE",
                        args=["self", idx_slot],
                        result=idx_after,
                    )
                )
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx_after, one], result=next_idx))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", idx_slot, next_idx],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._emit_loop_exit(scope)
            return
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_loop_body() -> None:
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[iterable], result=length))

            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LOOP_INDEX_START", args=[zero], result=idx))
            cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[idx, length], result=cond))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[iterable, idx], result=item))
            self._emit_assign_target(target, item, None)
            scope = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            if scope.needs_latch:
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
                self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._emit_loop_exit(scope)

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return

        emit_loop_body()

    def _parse_range_call(
        self, node: ast.AST
    ) -> tuple[MoltValue, MoltValue, MoltValue] | None:
        """The bounds of a call the binding analysis proves is builtin
        ``range`` with one to three positional arguments, as exact ints.

        Every argument is evaluated, in order; then, as ``range()`` does, each
        given bound converts through ``operator.index`` (start, stop, step),
        raising ``range()``'s own TypeError, and a missing start or step is
        0 or 1. The zero-step check follows in the consumer. ``None``, with
        nothing evaluated, for any other expression.
        """
        if not isinstance(node, ast.Call):
            return None
        if self._specializable_builtin_name(node) != "range":
            return None
        if not 1 <= len(node.args) <= 3 or node.keywords:
            return None
        if any(isinstance(arg, ast.Starred) for arg in node.args):
            return None
        values: list[MoltValue] = []
        for arg in node.args:
            value = self.visit(arg)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported range() argument"
                )
            values.append(value)
        bounds = [
            self._emit_range_bound(arg, value) for arg, value in zip(node.args, values)
        ]
        if len(bounds) == 1:
            start = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start))
            bounds.insert(0, start)
        if len(bounds) == 2:
            step = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step))
            bounds.append(step)
        start, stop, step = bounds
        return start, stop, step

    def _emit_range_bound(self, node: ast.expr, value: MoltValue) -> MoltValue:
        """``operator.index(value)``: ``range()``'s conversion of one bound to
        an exact int. A value the analysis proves an exact int converts to
        itself, and a bool literal to its int; anything else converts here,
        once, which may call ``__index__``."""
        if self._builtin_exact_type_from_expr(node) == "int":
            return value
        exact = MoltValue(self.next_var(), type_hint="int")
        if isinstance(node, ast.Constant) and isinstance(node.value, bool):
            self.emit(MoltOp(kind="CONST", args=[int(node.value)], result=exact))
        else:
            self.emit(MoltOp(kind="OPERATOR_INDEX", args=[value], result=exact))
        return exact

    def _emit_range_obj_from_args(
        self, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="range")
        self.emit(MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=res))
        return res

    def _emit_range_step_zero_guard(
        self, step: MoltValue, step_const: int | None
    ) -> None:
        if step_const is not None and step_const != 0:
            return
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        is_zero = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[step, zero], result=is_zero))
        self.emit(MoltOp(kind="IF", args=[is_zero], result=MoltValue("none")))
        err_val = self._emit_exception_new(
            "ValueError", "range() arg 3 must not be zero"
        )
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_range_loop(
        self,
        node: ast.For,
        start: MoltValue,
        stop: MoltValue,
        step: MoltValue,
        loop_break_flag: int | ScratchCell | None = None,
    ) -> None:
        target = node.target
        if self.is_async():
            range_obj = MoltValue(self.next_var(), type_hint="range")
            self.emit(
                MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=range_obj)
            )
            self._emit_iter_loop(node, range_obj, loop_break_flag=loop_break_flag)
            return None
        step_const = self.const_ints.get(step.name)
        self._emit_range_step_zero_guard(step, step_const)
        guard_map = self._emit_hoisted_loop_guards(node.body)
        simple_name_target = isinstance(target, ast.Name)

        def emit_range_loop_body() -> None:
            if step_const is not None and step_const != 0:
                with self._suppress_check_exception(emit_on_exit=False):
                    self.emit(
                        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none"))
                    )
                    idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
                    cond = MoltValue(self.next_var(), type_hint="bool")
                    if step_const > 0:
                        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
                    else:
                        self.emit(MoltOp(kind="LT", args=[stop, idx], result=cond))
                    self.emit(
                        MoltOp(
                            kind="LOOP_BREAK_IF_FALSE",
                            args=[cond],
                            result=MoltValue("none"),
                        )
                    )
                    if simple_name_target:
                        self._emit_assign_target(target, idx, None)
                if not simple_name_target:
                    self._emit_assign_target(target, idx, None)
                scope = self._visit_loop_body(
                    node.body, None, loop_break_flag=loop_break_flag
                )
                if scope.needs_latch:
                    with self._suppress_check_exception(emit_on_exit=False):
                        next_idx = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
                        self.emit(
                            MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx)
                        )
                        self.emit(
                            MoltOp(
                                kind="LOOP_CONTINUE", args=[], result=MoltValue("none")
                            )
                        )
                self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
                self._emit_loop_exit(scope)
                return None
            with self._suppress_check_exception(emit_on_exit=False):
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                zero = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                step_pos = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[zero, step], result=step_pos))
            self.emit(MoltOp(kind="IF", args=[step_pos], result=MoltValue("none")))
            with self._suppress_check_exception(emit_on_exit=False):
                self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
                idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
                cond = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
                self.emit(
                    MoltOp(
                        kind="LOOP_BREAK_IF_FALSE",
                        args=[cond],
                        result=MoltValue("none"),
                    )
                )
                if simple_name_target:
                    self._emit_assign_target(target, idx, None)
            if not simple_name_target:
                self._emit_assign_target(target, idx, None)
            scope = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            if scope.needs_latch:
                with self._suppress_check_exception(emit_on_exit=False):
                    next_idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
                    self.emit(
                        MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx)
                    )
                    self.emit(
                        MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                    )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._emit_loop_exit(scope)
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            with self._suppress_check_exception(emit_on_exit=False):
                step_neg = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[step, zero], result=step_neg))
            self.emit(MoltOp(kind="IF", args=[step_neg], result=MoltValue("none")))
            with self._suppress_check_exception(emit_on_exit=False):
                self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
                idx_neg = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx_neg))
                cond_neg = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[stop, idx_neg], result=cond_neg))
                self.emit(
                    MoltOp(
                        kind="LOOP_BREAK_IF_FALSE",
                        args=[cond_neg],
                        result=MoltValue("none"),
                    )
                )
                if simple_name_target:
                    self._emit_assign_target(target, idx_neg, None)
            if not simple_name_target:
                self._emit_assign_target(target, idx_neg, None)
            scope = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            if scope.needs_latch:
                with self._suppress_check_exception(emit_on_exit=False):
                    next_idx_neg = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="ADD", args=[idx_neg, step], result=next_idx_neg)
                    )
                    self.emit(
                        MoltOp(
                            kind="LOOP_INDEX_NEXT", args=[next_idx_neg], result=idx_neg
                        )
                    )
                    self.emit(
                        MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                    )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._emit_loop_exit(scope)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_range_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_range_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return None

        emit_range_loop_body()
        return None

    def _emit_iter_new(self, iterable: MoltValue) -> MoltValue:
        # Internal iterator transports (not arbitrary Python values inferred as
        # iterable) have already executed the protocol's eager ``iter()`` call.
        # Reusing them preserves generator-expression outer-iterator timing and
        # avoids a second observable ``__iter__`` invocation.
        if iterable.type_hint == "iter":
            return iterable
        res = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=res))
        if self.try_end_labels:
            self._emit_raise_if_pending()
        else:
            self._emit_raise_if_pending()
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[res, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        err_val = self._emit_exception_new("TypeError", "object is not iterable")
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_iter_next_checked(self, iter_obj: MoltValue) -> MoltValue:
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        if not self.try_end_labels:
            # Every function now carries a function-level exception label
            # (needs_exception_stack defaults to True), so a pending exception
            # from ITER_NEXT always routes to the function handler via
            # `_emit_raise_if_pending`.  The former `else` branch — which
            # emitted LOOP_BREAK_IF_EXCEPTION for label-less functions — is
            # unreachable and has been removed.  (The LOOP_BREAK_IF_EXCEPTION
            # opcode itself is retained for other emission sites.)
            assert self.function_exception_label is not None, (
                "every function must carry a function-level exception label"
            )
            self._emit_raise_if_pending()
        return pair

    def _emit_layout_guard(self, obj: MoltValue, expected_class: str) -> MoltValue:
        if expected_class == "dict":
            return self._emit_guard_dict_shape(obj)
        class_info = self.classes.get(expected_class)
        if class_info and not class_info.get("static"):
            class_ref = self._load_local_value(expected_class)
            if class_ref is None:
                guard = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=guard))
                return guard
        else:
            class_ref = self._emit_class_ref(expected_class)
        expected_version = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="CONST",
                args=[self.classes[expected_class].get("layout_version", 0)],
                result=expected_version,
            )
        )
        guard = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="GUARD_LAYOUT",
                args=[obj, class_ref, expected_version],
                result=guard,
            )
        )
        return guard

    def _emit_guard_dict_shape(self, obj: MoltValue) -> MoltValue:
        dict_type = self._emit_builtin_type_value("dict")
        expected_version = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="CLASS_VERSION",
                args=[dict_type],
                result=expected_version,
            )
        )
        guard = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="GUARD_DICT_SHAPE",
                args=[obj, dict_type, expected_version],
                result=guard,
            )
        )
        return guard

    def _loop_guard_assumption(self, obj_name: str, expected_class: str) -> bool | None:
        for guard_map in reversed(self.loop_guard_assumptions):
            entry = guard_map.get(obj_name)
            if entry is not None:
                class_id, assumption, token = entry
                if class_id == expected_class and token == self.exact_class_token:
                    return assumption
        return None

    def _push_loop_guard_assumptions(
        self,
        guard_map: dict[str, tuple[str, MoltValue, int]],
        assume_true: bool,
    ) -> None:
        assumptions: dict[str, tuple[str, bool, int]] = {}
        for name, (expected_class, _, _) in guard_map.items():
            # Entering the emitted bool-only guard branch establishes the
            # layout proof at this exact point. Later callbacks advance the
            # token and retire the assumption.
            assumptions[name] = (
                expected_class,
                assume_true,
                self.exact_class_token,
            )
        self.loop_guard_assumptions.append(assumptions)

    def _pop_loop_guard_assumptions(self) -> None:
        if self.loop_guard_assumptions:
            self.loop_guard_assumptions.pop()

    def _loop_guard_for(
        self, obj: MoltValue, expected_class: str, *, obj_name: str | None = None
    ) -> MoltValue | None:
        if not self.loop_layout_guards:
            return None
        name = obj_name or obj.name
        if self._exact_class_for_name(name) != expected_class:
            return None
        guard_map = self.loop_layout_guards[-1]
        cached = guard_map.get(name)
        if cached is not None:
            class_id, guard, token = cached
            if class_id == expected_class and token == self.exact_class_token:
                return guard
        guard = self._emit_layout_guard(obj, expected_class)
        guard_map[name] = (expected_class, guard, self.exact_class_token)
        return guard

    def _invalidate_loop_guard(self, name: str) -> None:
        for guard_map in self.loop_layout_guards:
            guard_map.pop(name, None)

    def _invalidate_loop_guards_for_class(self, class_name: str) -> None:
        for guard_map in self.loop_layout_guards:
            stale = [key for key, entry in guard_map.items() if entry[0] == class_name]
            for key in stale:
                guard_map.pop(key, None)

    def _prepare_exact_class_loop_entry(self, body: list[ast.stmt]) -> None:
        """Establish the loop-carried exact-fact fixed point before lowering."""
        if self.is_async() or not self._loop_body_preserves_exact_class_lifetimes(body):
            self._expire_exact_class_facts()

    def _emit_hoisted_loop_guards(
        self, body: list[ast.stmt]
    ) -> dict[str, tuple[str, MoltValue, int]]:
        if self.is_async() or not self._loop_body_preserves_exact_class_lifetimes(body):
            return {}
        candidates = self._collect_loop_guard_candidates(body)
        if not candidates:
            return {}
        guard_map: dict[str, tuple[str, MoltValue, int]] = {}
        for name, expected_class in sorted(candidates.items()):
            # The guarded object is the binding's current value on loop entry;
            # a name with no visible binding here is not guarded.
            obj = self._load_local_value(name)
            if obj is None:
                continue
            guard = self._emit_layout_guard(obj, expected_class)
            guard_map[name] = (expected_class, guard, self.exact_class_token)
        return {
            name: entry
            for name, entry in guard_map.items()
            if entry[2] == self.exact_class_token
        }

    def _emit_guard_map_condition(
        self, guard_map: dict[str, tuple[str, MoltValue, int]]
    ) -> MoltValue:
        condition: MoltValue | None = None
        for _, (_, guard, _) in sorted(guard_map.items()):
            if condition is None:
                condition = guard
                continue
            combined = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="AND", args=[condition, guard], result=combined))
            condition = combined
        if condition is None:
            condition = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=condition))
        return condition

    def _emit_aiter(self, iterable: MoltValue) -> MoltValue:
        # Even an already-acquired async iterator may return a different object
        # from its next __aiter__ call. Only an explicit prepared-iterator path
        # (the generator expression .0 binding) may skip acquisition.
        res = MoltValue(self.next_var(), type_hint="async_iter")
        self.emit(MoltOp(kind="AITER", args=[iterable], result=res))
        return res

    def _emit_for_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | ScratchCell | None = None,
    ) -> None:
        if self._iterable_is_indexable_for_loop(iterable):
            self._emit_index_loop(node, iterable, loop_break_flag=loop_break_flag)
        else:
            self._emit_iter_loop(node, iterable, loop_break_flag=loop_break_flag)

    def _load_loop_target_value(self, target: ast.Name) -> MoltValue | None:
        """The loop target's value before the loop, its missing sentinel when
        unbound. A fused loop binds its target once, after the fact, instead of
        once per item; its kernel admits that only when releasing this value
        runs no Python code (no finalizer can observe the reorder). ``None``,
        with nothing emitted, when no binding is visible here."""
        return self._load_local_value(target.id, guard_unbound=False)

    def _name_read_definitely_bound(self, node: ast.Name) -> bool:
        """The binding analysis proves this read finds its name bound, so it
        cannot raise; with no intervening store it may move to an earlier point
        without a visible difference.

        A callback may rebind a frame's own fast local (PEP 667) but never
        delete it, so its binding identities decide. A global, a class
        namespace entry or a cell a callback may delete, and the analysis keeps
        an expired binding's identities, so such a read must also be clean.
        """
        if self.python_binding_index is None:
            return False
        fact = self.python_binding_index.expression_fact(node)
        if fact is None or fact.identities & UNBOUND_IDENTITY:
            return False
        fast_local = (
            fact.name_lookup == "lexical"
            and node.id not in self.closure_locals
            and node.id not in self.free_vars
        )
        return fast_local or fact.binding_invalidated is False

    def _emit_tuple_item(
        self, pair: MoltValue, index: int, type_hint: str
    ) -> MoltValue:
        position = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[index], result=position))
        item = MoltValue(self.next_var(), type_hint=type_hint)
        self.emit(MoltOp(kind="INDEX", args=[pair, position], result=item))
        return item

    def _emit_is_exact_builtin(self, value: MoltValue, type_name: str) -> MoltValue:
        """``type(value) is <builtin type_name>``: true for an exact instance, not
        a subclass instance. Runs no Python code."""
        actual = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="TYPE_OF", args=[value], result=actual))
        expected = self._emit_builtin_type_value(type_name)
        exact = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[actual, expected], result=exact))
        return exact

    def _emit_fused_branches(
        self,
        cond: MoltValue,
        emit_then: Callable[[], None],
        emit_else: Callable[[], None] | None = None,
    ) -> None:
        """``IF cond: emit_then ELSE: emit_else END_IF`` for branches a fused
        lowering builds, with ``visit_If``'s flow bookkeeping: a name bound on
        only one path stays possibly unbound after the join (so a later read
        still raises UnboundLocalError where the code would), and exact-class
        facts join over both paths."""
        self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
        exact_entry = self._snapshot_live_exact_bindings()
        exact_entry_token = self.exact_class_token
        unbound_snapshot = set(self.unbound_check_names)
        self.control_flow_depth += 1
        try:
            emit_then()
            then_exact = self._snapshot_live_exact_bindings()
            then_exact_token = self.exact_class_token
            then_unbound = set(self.unbound_check_names)
            if emit_else is None:
                self.unbound_check_names = unbound_snapshot
                else_exact = exact_entry
                else_exact_token = exact_entry_token
            else:
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.unbound_check_names = set(unbound_snapshot)
                self.exact_locals = dict(exact_entry)
                self.exact_class_token = exact_entry_token
                emit_else()
                else_exact = self._snapshot_live_exact_bindings()
                else_exact_token = self.exact_class_token
                self.unbound_check_names = then_unbound | set(self.unbound_check_names)
        finally:
            self.control_flow_depth -= 1
        self.exact_locals = self._join_exact_binding_states(
            then_exact,
            then_exact_token,
            else_exact,
            else_exact_token,
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_iterable_vector_reduction(
        self,
        node: ast.For,
        iterable: MoltValue,
        *,
        loop_break_flag: int | ScratchCell | None,
    ) -> bool:
        """``for x in seq: acc += x`` (``*=`` and the min/max updates alike)
        over the loop's evaluated list, tuple or range, whose type hint only
        selects the fused op: a fused prefix of the loop, then the loop.

        The loop's iterator is acquired once, where the loop acquires it. Each
        pass of a chunk loop hands the kernel that iterator, the accumulator and
        the loop target, reread from their bindings, which a signal handler or
        pending call run at the previous back edge may have rebound; the kernel
        consumes at most one bounded chunk of items whose update runs no Python
        code and returns ``(result, last, count, more)``. After a nonempty chunk
        the loop target and then the accumulator are stored, in the order the
        loop releases them, before the back edge observes pending work. The
        chunk loop ends at the iterator's end or at the first item the kernel
        does not admit, which it leaves unconsumed; the ordinary loop then
        continues on the same iterator, finding the rest or its end. False,
        with nothing observable emitted, for any other loop.
        """
        reduction = self._match_vector_reduction_loop(node)
        if reduction is None:
            reduction = self._match_vector_minmax_loop(node)
        if (
            reduction is None
            or iterable.type_hint not in {"list", "tuple", "range"}
            or not isinstance(node.target, ast.Name)
        ):
            return False
        acc_name, _, kind = reduction
        target = node.target
        # A binding visible here (possibly unbound: its missing sentinel, which
        # the kernel declines) is readable inside the chunk loop.
        if self._load_loop_target_value(target) is None:
            return False
        if self._load_local_value(acc_name, guard_unbound=False) is None:
            return False
        item_hint = self._iteration_element_hint(node, iterable) or "Any"
        it = self._emit_iter_new(iterable)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        target_old = self._load_local_value(
            target.id, guard_unbound=False, binding_invalidated=True
        )
        acc = self._load_local_value(
            acc_name, guard_unbound=False, binding_invalidated=True
        )
        assert target_old is not None and acc is not None
        outcome = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(
            MoltOp(
                kind=self._VECTOR_REDUCTION_OPS[kind],
                args=[it, acc, target_old],
                result=outcome,
            )
        )
        count = self._emit_tuple_item(outcome, 2, "int")
        consumed = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NE", args=[count, zero], result=consumed))

        def publish() -> None:
            self._emit_assign_target(
                target, self._emit_tuple_item(outcome, 1, item_hint), None
            )
            self._store_local_value(acc_name, self._emit_tuple_item(outcome, 0, "Any"))

        self._emit_fused_branches(consumed, publish)
        more = self._emit_tuple_item(outcome, 3, "bool")
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[more], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self._emit_iter_loop(node, it, loop_break_flag=loop_break_flag)
        return True

    def _prepare_mutable_control_flow_bindings(self, names: set[str]) -> None:
        if self._class_ns_stack:
            # Names bound by the active class body are backed by its namespace
            # dict (STORE_INDEX/INDEX through ``_class_ns_store``/``_class_ns_load``),
            # which is the heap-resident, loop-carried-correct mutable home — the
            # class-scope analogue of the module dict.  They must NOT be promoted
            # into ``module_global_mutations`` (which would leak the binding into
            # the enclosing module namespace and steer bare-name reads through
            # module global lookup)
            # nor boxed into list cells.  Strip them; let any genuine
            # surrounding-scope temps fall through to the normal handling.
            names = {n for n in names if not self._is_class_body_managed_name(n)}
        if not names:
            return
        # In function scope, loop-carried values are handled natively by
        # Cranelift's SSA phi/block-argument mechanism.  Boxing variables
        # into heap-allocated list cells adds ~10 cycles per access and
        # defeats raw_int_shadow optimisation.  Only box at module scope
        # (where there's no SSA) or for closures/nonlocals that truly
        # need heap storage.
        if self.current_func_name != "molt_main" and not self.is_async():
            return
        module_backed: set[str] = set()
        if self.current_func_name == "molt_main":
            # Module-scope control-flow bindings already have a canonical mutable
            # home: the module object. Route bare-name loads through
            # MODULE_GET_GLOBAL instead of synthesizing one-element list cells
            # just to model loop-carried mutation. That keeps module lowering
            # canonical and avoids ad hoc boxed-local indirection for top-level
            # loops.
            module_backed = set(names)
            if module_backed:
                # Only unpublished values need promotion. Replacing an already
                # live entry repeats observable stores and can restore stale SSA
                # state after callbacks. The deferred set owns that distinction.
                self._flush_deferred_module_attrs(module_backed)
                self.module_global_mutations.update(module_backed)
                # Remove from self.locals so visit_Name falls through to
                # the module_global_mutations check (module_get_global).
                # Without this, the cached local SSA variable shadows the
                # module dict, making while loop conditions read stale values.
                for name in module_backed:
                    self.locals.pop(name, None)
        if self.is_async():
            return
        for name in sorted(names - module_backed):
            self._box_local(name)

    def _evict_module_control_flow_bindings(self, names: set[str]) -> None:
        if self.current_func_name != "molt_main" or self.is_async():
            return
        for name in names:
            if name in self.module_global_mutations:
                self.globals.pop(name, None)
                self.locals.pop(name, None)

    def _emit_loop_orelse(
        self, break_cell: ScratchCell, orelse: list[ast.stmt]
    ) -> None:
        break_val = self._load_scratch_cell(break_cell)
        should_run = self._emit_not(break_val)
        self.emit(MoltOp(kind="IF", args=[should_run], result=MoltValue("none")))
        self._visit_block(orelse)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _const_int_from_expr(self, node: ast.expr) -> int | None:
        """An int literal, or the binding analysis's int constant for a read of
        a name: joined over every path and iteration reaching the read, and
        absent once a callback may have rebound the name."""
        if (
            isinstance(node, ast.Constant)
            and isinstance(node.value, int)
            and not isinstance(node.value, bool)
        ):
            return node.value
        if isinstance(node, ast.Name) and self.python_binding_index is not None:
            fact = self.python_binding_index.expression_fact(node)
            value = None if fact is None else fact.static_value
            if isinstance(value, int) and not isinstance(value, bool):
                return value
        return None

    def _unit_increment_read(self, stmt: ast.stmt, name: str) -> ast.Name | None:
        """The read of ``name`` in ``name += 1`` or ``name = name + 1`` (either
        operand order) with an int literal ``1``; ``None`` for anything else."""

        def is_int_one(expr: ast.expr) -> bool:
            return (
                isinstance(expr, ast.Constant)
                and isinstance(expr.value, int)
                and not isinstance(expr.value, bool)
                and expr.value == 1
            )

        if isinstance(stmt, ast.AugAssign):
            target = stmt.target
            if (
                isinstance(target, ast.Name)
                and target.id == name
                and isinstance(stmt.op, ast.Add)
                and is_int_one(stmt.value)
            ):
                return target
            return None
        if not isinstance(stmt, ast.Assign):
            return None
        if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
            return None
        if stmt.targets[0].id != name:
            return None
        if not isinstance(stmt.value, ast.BinOp) or not isinstance(
            stmt.value.op, ast.Add
        ):
            return None
        left = stmt.value.left
        right = stmt.value.right
        if isinstance(left, ast.Name) and left.id == name and is_int_one(right):
            return left
        if isinstance(right, ast.Name) and right.id == name and is_int_one(left):
            return right
        return None

    def _emit_counted_while(
        self,
        index_name: str,
        start: MoltValue,
        bound: int,
        body: list[ast.stmt],
    ) -> None:
        """``while index < bound: body; index += 1`` with the index in an
        induction variable, starting from ``start``, the loop's first read of
        the index. The matcher proved that nothing in the body can rebind the
        index, so every later test and increment read is that variable."""
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        stop = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[bound], result=stop))
        guard_map = self._emit_hoisted_loop_guards(body)
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(index_name, idx)
        scope = self._visit_loop_body(body, guard_map)
        if scope.needs_latch:
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
            self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self._emit_loop_exit(scope)
        self._store_local_value(index_name, idx)

    def _dict_increment_key_is_single_eval_safe(self, key: ast.expr) -> bool:
        if isinstance(key, (ast.Name, ast.Constant)):
            return True
        if not isinstance(key, ast.Attribute) or not isinstance(key.value, ast.Name):
            return False
        class_id = self._exact_class_for_name(key.value.id)
        class_info = self.classes.get(class_id or "")
        return bool(
            class_info
            and class_info.get("dataclass")
            and key.attr in class_info.get("fields", {})
        )

    def _emit_split_dict_increment_for_loop(
        self, node: ast.For, *, loop_break_flag: int | ScratchCell | None
    ) -> bool:
        """``for w in line.split([SEP]): d[w] = d.get(w, 0) + delta`` as one
        kernel when the loop provably runs no Python code; the kernel checks
        that on the values the loop reads and binds ``w`` to the last word.
        Otherwise the ordinary loop runs, calling ``line.split`` itself. The
        matcher proves every name read here once, before the loop, bound and
        unwritten by the loop, so the early reads are unobservable. False, with
        nothing observable emitted, for any other loop."""
        match = self._match_split_dict_increment_for_loop(node)
        if match is None or not isinstance(node.target, ast.Name):
            return False
        dict_read, line_read, sep, delta_expr = match
        target_old = self._load_loop_target_value(node.target)
        if target_old is None:
            return False
        line_obj = self.visit(line_read)
        dict_obj = self.visit(dict_read)
        delta_obj = self.visit(delta_expr)
        if line_obj is None or dict_obj is None or delta_obj is None:
            return False
        outcome = MoltValue(self.next_var(), type_hint="tuple")
        if sep is None:
            self.emit(
                MoltOp(
                    kind="STRING_SPLIT_WS_DICT_INC",
                    args=[line_obj, dict_obj, delta_obj, target_old],
                    result=outcome,
                )
            )
        else:
            sep_obj = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[sep], result=sep_obj))
            self.emit(
                MoltOp(
                    kind="STRING_SPLIT_SEP_DICT_INC",
                    args=[line_obj, sep_obj, dict_obj, delta_obj, target_old],
                    result=outcome,
                )
            )
        ok = self._emit_tuple_item(outcome, 1, "bool")

        def bind_last_word() -> None:
            self._emit_assign_target(
                node.target, self._emit_tuple_item(outcome, 0, "str"), None
            )

        def run_the_loop() -> None:
            iterable = self.visit(node.iter)
            if iterable is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported iterable in for loop"
                )
            self._emit_for_loop(node, iterable, loop_break_flag=loop_break_flag)

        self._emit_fused_branches(ok, bind_last_word, run_the_loop)
        return True

    def _emit_bytearray_fill_while(
        self,
        index: ast.Name,
        bound: int,
        container_read: ast.Name,
        fill: int,
        emit_loop: Callable[[], None],
    ) -> None:
        """``while i < BOUND: buf[i] = FILL; i += 1`` as a fused prefix: chunks
        of at most ``_BYTEARRAY_FILL_CHUNK`` bytes, each written at once when
        it provably runs no Python code: ``buf`` an exact bytearray and ``i``
        an exact int with ``0 <= i < BOUND <= len(buf)``, checked on the values
        the loop reads, each check only once the previous one makes it run no
        Python code. After a chunk ``i`` holds the chunk's end, as the loop
        leaves it, before the chunk loop's back edge observes pending work; the
        next chunk rereads ``i`` and ``buf``, which that work may have
        rebound. The ordinary loop then runs from the current ``i``: its test
        ends it at once after the last chunk, and after a failed check it
        finishes the loop and reports what the loop reports. The container is
        read without raising, since the loop reads it only inside the body."""
        # The test's read of the index is the loop's first read of it.
        start = self._load_local_value(
            index.id,
            binding_invalidated=self._expression_has_invalidated_binding(index),
        )
        container = self._load_local_value(container_read.id, guard_unbound=False)
        if start is None or container is None:
            emit_loop()
            return
        more_slot = f"__molt_fill_more_{self.next_var()}"
        end_slot = f"__molt_fill_end_{self.next_var()}"
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        stop = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[bound], result=stop))
        chunk = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[self._BYTEARRAY_FILL_CHUNK], result=chunk))
        fill_value = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[fill], result=fill_value))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        start = self._load_local_value(
            index.id, guard_unbound=False, binding_invalidated=True
        )
        container = self._load_local_value(
            container_read.id, guard_unbound=False, binding_invalidated=True
        )
        assert start is not None and container is not None
        declined = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=declined))
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[declined],
                result=MoltValue("none"),
                metadata={"var": more_slot},
            )
        )
        exact_container = self._emit_is_exact_builtin(container, "bytearray")
        self.emit(MoltOp(kind="IF", args=[exact_container], result=MoltValue("none")))
        exact_start = self._emit_is_exact_builtin(start, "int")
        self.emit(MoltOp(kind="IF", args=[exact_start], result=MoltValue("none")))
        not_negative = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LE", args=[zero, start], result=not_negative))
        runs = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[start, stop], result=runs))
        length = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[container], result=length))
        in_bounds = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LE", args=[stop, length], result=in_bounds))
        starts_inside = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="AND", args=[not_negative, runs], result=starts_inside))
        admitted = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="AND", args=[starts_inside, in_bounds], result=admitted))
        self.emit(MoltOp(kind="IF", args=[admitted], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[stop],
                result=MoltValue("none"),
                metadata={"var": end_slot},
            )
        )
        chunk_end = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[start, chunk], result=chunk_end))
        short = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[chunk_end, stop], result=short))
        self.emit(MoltOp(kind="IF", args=[short], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[chunk_end],
                result=MoltValue("none"),
                metadata={"var": end_slot},
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        end = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="LOAD_VAR", args=[], result=end, metadata={"var": end_slot})
        )
        self.emit(
            MoltOp(
                kind="BYTEARRAY_FILL_RANGE",
                args=[container, start, end, fill_value],
                result=MoltValue("none"),
            )
        )
        self._store_local_value(index.id, end)
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[short],
                result=MoltValue("none"),
                metadata={"var": more_slot},
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        more = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="LOAD_VAR", args=[], result=more, metadata={"var": more_slot})
        )
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[more], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        emit_loop()

    def _emit_static_if_live_branch(self, branch: list[ast.stmt]) -> None:
        """Emit only the statically-live branch of a constant `if`.

        The dead branch is dropped entirely (CPython parity: its assignments and
        any value/intrinsic references never reach the IR). Live-branch names are
        boxed / module-backed exactly as a normal conditional branch would do, so
        a name assigned only here behaves identically whether or not the fold
        fired.
        """
        if branch and not self.is_async():
            assigned = self._collect_assigned_names(branch)
            if self.current_func_name == "molt_main":
                self._prepare_mutable_control_flow_bindings(assigned)
            else:
                for name in sorted(assigned):
                    if name not in self.scope_assigned or name in self.closure_locals:
                        self._box_local(name)
        self._visit_block(branch)

    def _visit_block(self, body: list[ast.stmt]) -> bool:
        """Return this block's completion while restoring the enclosing flag."""
        prior = self.block_terminated
        self.block_terminated = False
        terminated = False
        for stmt in body:
            self.visit(stmt)
            if self.block_terminated:
                terminated = True
                break
            # Emit a check_exception after each statement to catch any
            # pending exception from the preceding ops.  This uses the
            # same fast inline flag check as all other check_exception
            # sites, avoiding the broken exception_last → is → not → if
            # → raise pattern that produced stale-exception re-raise bugs.
            handler_label: int | None
            if self.try_end_labels:
                handler_label = self.try_end_labels[-1]
            else:
                handler_label = self.function_exception_label
            if handler_label is not None:
                self.emit(
                    MoltOp(
                        kind="CHECK_EXCEPTION",
                        args=[handler_label],
                        result=MoltValue("none"),
                    )
                )
        self.block_terminated = prior
        return terminated

    def _visit_loop_body(
        self,
        body: list[ast.stmt],
        prefill: dict[str, tuple[str, MoltValue, int]] | None = None,
        loop_break_flag: int | ScratchCell | None = None,
    ) -> LoopScope:
        scope = LoopScope(
            break_label=self.next_label(),
            continue_label=self.next_label(),
            try_depth=len(self.try_scopes),
            break_flag=loop_break_flag,
        )
        if not self.is_async():
            guard_map = dict(prefill) if prefill else {}
            self.loop_layout_guards.append(guard_map)
        self.loop_scopes.append(scope)
        # Snapshot unbound_check_names — the loop body may not execute
        # at all (empty range / false initial condition), so any
        # discards inside the body must be reverted on exit.  Inside
        # the body, post-assignment loads still skip the check, which
        # is the source of the per-iter speedup on
        # `obj = Class(...); obj.x = …; obj.y = …` patterns.
        unbound_snapshot = set(self.unbound_check_names)
        try:
            self.control_flow_depth += 1
            try:
                scope.body_terminated = self._visit_block(body)
            finally:
                self.control_flow_depth -= 1
        finally:
            self.unbound_check_names = unbound_snapshot
            self.loop_scopes.pop()
            if not self.is_async():
                self.loop_layout_guards.pop()
        if scope.continue_used:
            self.emit(
                MoltOp(
                    kind="LABEL", args=[scope.continue_label], result=MoltValue("none")
                )
            )
        return scope

    def _emit_loop_exit(self, scope: LoopScope) -> None:
        if scope.break_used:
            self.emit(
                MoltOp(kind="LABEL", args=[scope.break_label], result=MoltValue("none"))
            )

    def _emit_loop_unwind(self) -> list[int]:
        if not self.loop_scopes:
            return []
        max_scopes = len(self.try_scopes)
        loop_depth = self.loop_scopes[-1].try_depth
        if loop_depth >= max_scopes:
            return []
        return self._emit_control_flow_scope_unwind(
            self.try_scopes[loop_depth:max_scopes]
        )

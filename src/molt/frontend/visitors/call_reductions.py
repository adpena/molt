"""Reducer call lowering helpers for ``CallVisitorMixin``.

This is a semantic F2 extraction from the call visitor, not a second dispatch
surface: full-consumption reducers (``sum``) and short-circuit reducers
(``any``/``all``) own their comprehension-fusion invariants here. Builtin
``sum()`` is its own operation, not an explicit ``+=`` loop: CPython compensates
its float additions, so a running total stands in for it only where every item
is an exact int.
"""

from __future__ import annotations

import ast
from typing import (
    cast,
)

from molt.frontend._types import MoltOp, MoltValue
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend._mixin_base import GeneratorMixinBase


class CallReductionMixin(GeneratorMixinBase):
    def _can_inline_sum_genexpr(self, node: ast.GeneratorExp | ast.ListComp) -> bool:
        if not self._comprehension_frame_can_fuse(node):
            return False
        if self.is_async():
            return False
        if not self._can_inline_simple_comp(node.generators, [node.elt]):
            return False
        comp = node.generators[0]
        if self._collect_inline_comp_walrus_names([node.elt], comp.ifs):
            return False
        target_names = set(self._collect_target_names(comp.target))
        lambda_free_vars = self._collect_inline_comp_lambda_free_vars(
            [node.elt], comp.ifs
        )
        return not bool(target_names & lambda_free_vars)

    _EXACT_INT_BINOPS = (
        ast.Add,
        ast.Sub,
        ast.Mult,
        ast.FloorDiv,
        ast.Mod,
        ast.LShift,
        ast.RShift,
        ast.BitOr,
        ast.BitXor,
        ast.BitAnd,
    )

    def _sum_element_is_exact_int(
        self, expr: ast.expr, int_names: frozenset[str]
    ) -> bool:
        """``expr`` yields an exact int or bool, or raises, by its form alone:
        int literals, ``int_names`` (a counted range's items), the exact
        builtin ``len``, identity and membership tests, ``not``, and int-closed
        arithmetic, comparison and selection of such operands. Adding such
        values runs no Python code, and every mode of builtin ``sum()`` adds
        them exactly, so a running int total is ``sum()``'s result."""
        if isinstance(expr, ast.Constant):
            return type(expr.value) in {int, bool}
        if isinstance(expr, ast.Name):
            return expr.id in int_names
        if isinstance(expr, ast.BinOp):
            return (
                isinstance(expr.op, self._EXACT_INT_BINOPS)
                and self._sum_element_is_exact_int(expr.left, int_names)
                and self._sum_element_is_exact_int(expr.right, int_names)
            )
        if isinstance(expr, ast.UnaryOp):
            return isinstance(expr.op, ast.Not) or (
                isinstance(expr.op, (ast.UAdd, ast.USub, ast.Invert))
                and self._sum_element_is_exact_int(expr.operand, int_names)
            )
        if isinstance(expr, ast.Compare):
            return all(
                isinstance(op, (ast.Is, ast.IsNot, ast.In, ast.NotIn))
                for op in expr.ops
            ) or all(
                self._sum_element_is_exact_int(operand, int_names)
                for operand in (expr.left, *expr.comparators)
            )
        if isinstance(expr, ast.BoolOp):
            return all(
                self._sum_element_is_exact_int(value, int_names)
                for value in expr.values
            )
        if isinstance(expr, ast.IfExp):
            return self._sum_element_is_exact_int(
                expr.body, int_names
            ) and self._sum_element_is_exact_int(expr.orelse, int_names)
        if isinstance(expr, ast.Call):
            return self._specializable_builtin_name(expr) == "len"
        return False

    def _try_emit_inline_sum_genexpr(self, node: ast.Call) -> MoltValue | None:
        if (
            len(node.args) != 1
            or node.keywords
            # `sum([x for x in it])` is semantically identical to
            # `sum(x for x in it)` when the elements are exact ints: the list
            # is a throwaway consumed only by `sum`, and adding exact ints runs
            # no code that could observe the interleaving. Do not copy this to
            # eager-vs-lazy reducers.
            or not isinstance(node.args[0], (ast.GeneratorExp, ast.ListComp))
        ):
            return None
        genexpr = node.args[0]
        if not self._can_inline_sum_genexpr(genexpr):
            return None

        comp = genexpr.generators[0]
        target_name, tuple_target_names = self._inline_simple_comp_target(
            comp, "__molt_sum_genexpr_unpack"
        )
        user_target_names = (
            [target_name] if tuple_target_names is None else list(tuple_target_names)
        )

        # Counted-range fast path: when the sole `for` clause iterates a
        # `range(...)` with a simple `Name` target and no `if` filters, lower
        # the loop through the SAME counted-index shape a top-level
        # `for x in range(...)` loop uses so the loop variable is an int-typed
        # counted value (`ScalarKind::Int` -> `RawI64Safe` in
        # `representation_plan`) and `x*x` plus the accumulator raw-lane instead
        # of routing every element through the boxed `iter_next` protocol.
        # Constant-step ranges take the pure `loop_index_start` lane (no range
        # object); every other `range(...)` counts over a materialized range
        # object via `INDEX`, exactly as `_emit_index_loop` does for the
        # statement form. `if`-filtered, tuple-target, and
        # non-`range` generators fall through to the generic iter path below.
        # The outermost iterable belongs to the enclosing scope: Python
        # evaluates it before the generator binds its targets, so it is lowered
        # before they shadow the enclosing bindings.
        range_call = comp.iter
        counted_candidate = (
            not comp.ifs
            and tuple_target_names is None
            and isinstance(range_call, ast.Call)
            and not range_call.keywords
            and 1 <= len(range_call.args) <= 3
            and self._specializable_builtin_name(range_call) == "range"
        )
        # Decide before evaluating anything: the call's own lowering evaluates
        # the iterable exactly once when this path declines.
        int_names = frozenset({target_name}) if counted_candidate else frozenset()
        if not self._sum_element_is_exact_int(genexpr.elt, int_names):
            return None
        counted_range = None
        if counted_candidate:
            counted_range = self._counted_range_args_for_genexpr(comp.iter)
            if counted_range is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported sum range argument"
                )
        iterable_val = None if counted_range is not None else self.visit(comp.iter)
        if counted_range is None and iterable_val is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported sum generator iterable"
            )

        saved_locals = {name: self.locals.get(name) for name in user_target_names}
        saved_boxed = {
            name: self.boxed_locals.pop(name, None) for name in user_target_names
        }
        saved_boxed_hints = {
            name: self.boxed_local_hints.pop(name, None) for name in user_target_names
        }
        outer_comp_shadow_locals = set(self.comp_shadow_locals)
        self.comp_shadow_locals.add(target_name)
        if tuple_target_names is not None:
            self.comp_shadow_locals.update(tuple_target_names)

        if counted_range is not None:
            return self._finish_counted_range_sum_genexpr(
                genexpr,
                target_name,
                user_target_names,
                counted_range,
                saved_locals,
                saved_boxed,
                saved_boxed_hints,
                outer_comp_shadow_locals,
            )

        assert iterable_val is not None
        iter_obj = self._emit_iter_new(iterable_val)
        # `zero`/`one` are loop-invariant index constants for the iter-next pair;
        # emit them in the preheader (real op stream) so they dominate the body.
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))

        # The accumulator is a scalar SSA slot (STORE_VAR/LOAD_VAR), NOT a heap
        # `list` cell. A loop-carried store_var/load_var slot becomes a typed phi
        # at the loop header, which the representation plan promotes to a raw
        # carrier (RawI64Safe) — exactly the any/all reducer's `res_slot` idiom.
        # The seed is the int 0 `sum()` starts from, which is also its result
        # for an empty generator.
        acc_slot = f"__molt_sum_acc_{self.next_var()}"

        # Buffer the loop body so the preheader seed is emitted before
        # `loop_start`; store_var/load_var bind by slot name, so the seed
        # physically preceding loop_start is the only ordering constraint.
        saved_ops = self.current_ops
        body_ops: list[MoltOp] = []
        self.current_ops = body_ops
        try:
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            pair = self._emit_iter_next_checked(iter_obj)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            self.emit(
                MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
            )
            iter_elem_hint = self._iterable_element_hint(iterable_val) or "Any"
            item = MoltValue(self.next_var(), type_hint=iter_elem_hint)
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
            self.locals[target_name] = item
            self._store_comprehension_local_value(target_name, item)
            if tuple_target_names is not None:
                item_vals = [
                    MoltValue(self.next_var(), type_hint="Any")
                    for _ in tuple_target_names
                ]
                self.emit(
                    MoltOp(
                        kind="UNPACK_SEQUENCE",
                        args=[item] + item_vals,
                        result=MoltValue("none"),
                        metadata={"expected_count": len(tuple_target_names)},
                    )
                )
                for tname, item_val in zip(tuple_target_names, item_vals):
                    self._store_comprehension_local_value(tname, item_val)
            for if_node in comp.ifs:
                cond_val = self._emit_condition(if_node)
                not_cond = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[cond_val], result=not_cond))
                self.emit(MoltOp(kind="IF", args=[not_cond], result=MoltValue("none")))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            value = self.visit(genexpr.elt)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE,
                    "Unsupported sum generator expression",
                )
            self._emit_sum_genexpr_add(acc_slot, value)
            for name in user_target_names:
                prior = saved_locals.get(name)
                if prior is not None:
                    self.locals[name] = prior
                else:
                    self.locals.pop(name, None)
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        finally:
            self.current_ops = saved_ops

        return self._finish_sum_genexpr_accumulator(
            body_ops,
            acc_slot=acc_slot,
            user_target_names=user_target_names,
            saved_boxed=saved_boxed,
            saved_boxed_hints=saved_boxed_hints,
            outer_comp_shadow_locals=outer_comp_shadow_locals,
        )

    def _emit_sum_genexpr_add(self, acc_slot: str, value: MoltValue) -> None:
        """``total = total + value`` on the inline running int total."""
        acc_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="LOAD_VAR",
                args=[],
                result=acc_val,
                metadata={"var": acc_slot},
            )
        )
        acc_next = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[acc_val, value], result=acc_next))
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[acc_next],
                result=MoltValue("none"),
                metadata={"var": acc_slot},
            )
        )

    def _finish_sum_genexpr_accumulator(
        self,
        body_ops: list[MoltOp],
        *,
        acc_slot: str,
        user_target_names: list[str],
        saved_boxed: dict[str, MoltValue | None],
        saved_boxed_hints: dict[str, str | None],
        outer_comp_shadow_locals: set[str],
    ) -> MoltValue:
        """Seed the accumulator, splice the buffered loop, and yield the result.

        Shared preheader/epilogue for both the generic iter-protocol lowering
        and the counted-range fast path: the int 0 seed must physically precede
        ``loop_start`` so the loop-carried ``store_var``/``load_var`` phi is an
        int. ``body_ops`` is the buffered loop body captured with that
        constraint in mind.
        """
        seed_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=seed_val))
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[seed_val],
                result=MoltValue("none"),
                metadata={"var": acc_slot},
            )
        )

        # Splice the buffered loop body in after the preheader seed.
        self.current_ops.extend(body_ops)

        result = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="LOAD_VAR",
                args=[],
                result=result,
                metadata={"var": acc_slot},
            )
        )
        for name in user_target_names:
            boxed = saved_boxed.get(name)
            hint = saved_boxed_hints.get(name)
            if boxed is not None:
                self.boxed_locals[name] = boxed
            else:
                self.boxed_locals.pop(name, None)
            if hint is not None:
                self.boxed_local_hints[name] = hint
            else:
                self.boxed_local_hints.pop(name, None)
        self.comp_shadow_locals = outer_comp_shadow_locals
        return result

    def _counted_range_args_for_genexpr(
        self, iter_node: ast.expr
    ) -> tuple[MoltValue, MoltValue, MoltValue, int | None] | None:
        """Return counted-range loop args for a `range(...)` genexpr iterable.

        Yields ``(start, stop, step, step_const)`` when ``iter_node`` is a
        ``range(...)`` call; its bounds are ``range()``'s exact ints. ``step_const``
        is the step when it is a nonzero constant — the precondition for the
        pure ``loop_index_start`` counted-index shape (no range object). Otherwise ``step_const`` is ``None``: the loop still
        counts over a materialized ``range`` object via ``INDEX`` at a counted
        index (an int-typed element that raw-lanes), delegating every range
        semantic — empty ranges, negative/zero step (``ValueError``),
        start/stop/step math — to the runtime range object, exactly as a
        top-level ``for x in range(...)`` loop does through ``_emit_index_loop``.

        Returns ``None`` (fall back to the generic iter-protocol path) only for
        a non-``range`` iterable. ``start``/``stop``/``step`` carrier CONSTs are
        emitted into the current (preheader) op stream as a side effect, exactly
        as the top-level ``for``-loop range lowering parses its args.
        """
        range_args = self._parse_range_call(iter_node)
        if range_args is None:
            return None
        start_val, stop_val, step_val = range_args
        const = self.const_ints.get(step_val.name)
        step_const = const if const is not None and const != 0 else None
        return start_val, stop_val, step_val, step_const

    def _finish_counted_range_sum_genexpr(
        self,
        genexpr: ast.GeneratorExp | ast.ListComp,
        target_name: str,
        user_target_names: list[str],
        counted_range: tuple[MoltValue, MoltValue, MoltValue, int | None],
        saved_locals: dict[str, MoltValue | None],
        saved_boxed: dict[str, MoltValue | None],
        saved_boxed_hints: dict[str, str | None],
        outer_comp_shadow_locals: set[str],
    ) -> MoltValue:
        """Lower `sum(<elt> for x in range(...))` through a counted-index loop.

        The loop variable ``x`` is an int-typed counted index — either the
        ``loop_index_start`` result directly (pure counted lane, when the step
        is a compile-time-known nonzero constant over int bounds) or ``INDEX``
        into a materialized ``range`` object at a counted position (the generic
        range lane, mirroring the top-level ``for``-loop ``_emit_index_loop``).
        Either way ``x`` carries ``ScalarKind::Int``, so the element expression
        and the ``load_var``/``add``/``store_var`` accumulator raw-lane. The
        empty-range result is the int ``0`` seed, as ``sum(range(0)) == 0``.
        """
        start, stop, step, step_const = counted_range

        # The generic range lane materializes the range object in the preheader
        # before the loop is buffered. RANGE_NEW faithfully raises ValueError on
        # a zero step (`range() arg 3 must not be zero`); route that pending
        # exception to the function handler IMMEDIATELY — before LEN and the
        # loop — so the error surfaces exactly as CPython's `range(...)` call
        # does, never after a spurious iteration over an invalid range.
        range_obj: MoltValue | None = None
        if step_const is None:
            range_obj = self._emit_range_obj_from_args(start, stop, step)
            self._emit_raise_if_pending()
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[range_obj], result=length))
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))

        acc_slot = f"__molt_sum_acc_{self.next_var()}"

        # Buffer the loop body so the int 0 seed is emitted before
        # `loop_start` (the seed must physically precede loop_start for an int
        # phi). The range preheader ops (arg CONSTs, RANGE_NEW/LEN) were already
        # emitted above; only the loop itself is buffered here.
        saved_ops = self.current_ops
        body_ops: list[MoltOp] = []
        self.current_ops = body_ops
        try:
            with self._suppress_check_exception(emit_on_exit=False):
                self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
                idx = MoltValue(self.next_var(), type_hint="int")
                if step_const is not None:
                    self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
                    cond = MoltValue(self.next_var(), type_hint="bool")
                    if step_const > 0:
                        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
                    else:
                        self.emit(MoltOp(kind="LT", args=[stop, idx], result=cond))
                    item = idx
                else:
                    assert range_obj is not None
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
                if step_const is None:
                    assert range_obj is not None
                    item = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="INDEX", args=[range_obj, idx], result=item))
            # Bind the comprehension target to the int-typed counted value. `x`
            # is a raw i64 (loop_index_start result, or an int-hinted INDEX into
            # the range), so the element expression and accumulator raw-lane.
            self.locals[target_name] = item
            self._store_comprehension_local_value(target_name, item)
            value = self.visit(genexpr.elt)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE,
                    "Unsupported sum generator expression",
                )
            self._emit_sum_genexpr_add(acc_slot, value)
            for name in user_target_names:
                prior = saved_locals.get(name)
                if prior is not None:
                    self.locals[name] = prior
                else:
                    self.locals.pop(name, None)
            with self._suppress_check_exception(emit_on_exit=False):
                advance = step if step_const is not None else one
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx, advance], result=next_idx))
                self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        finally:
            self.current_ops = saved_ops

        return self._finish_sum_genexpr_accumulator(
            body_ops,
            acc_slot=acc_slot,
            user_target_names=user_target_names,
            saved_boxed=saved_boxed,
            saved_boxed_hints=saved_boxed_hints,
            outer_comp_shadow_locals=outer_comp_shadow_locals,
        )

    def _emit_any_all_call(
        self, func_id: str, node: ast.Call, needs_bind: bool
    ) -> MoltValue:
        inlined = self._try_emit_inline_any_all_genexpr(func_id, node)
        if inlined is not None:
            return inlined

        callee = self._emit_builtin_function(func_id)
        res = MoltValue(self.next_var(), type_hint="bool")
        if needs_bind:
            callargs = self._emit_call_args_builder(node)
            self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
        else:
            args = self._emit_call_args(node.args)
            self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
        return res

    def _try_emit_inline_any_all_genexpr(
        self, func_id: str, node: ast.Call
    ) -> MoltValue | None:
        is_any = func_id == "any"
        if (
            len(node.args) != 1
            or node.keywords
            or not isinstance(node.args[0], ast.GeneratorExp)
        ):
            return None
        genexpr = node.args[0]
        if not self._can_inline_any_all_genexpr(genexpr):
            return None

        comp = genexpr.generators[0]
        target = cast(ast.Name, comp.target)
        target_name = target.id
        iterable_val = self.visit(comp.iter)
        if iterable_val is None:
            return None
        iter_obj = self._emit_iter_new(iterable_val)
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[not is_any], result=res))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        res_slot = f"__molt_{func_id}_result_{self.next_var()}"
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[res],
                result=MoltValue("none"),
                metadata={"var": res_slot},
            )
        )

        cell = self._load_boxed_cell(target_name)
        saved_cell_val: MoltValue | None = None
        if cell is not None:
            saved_cell_val = self._emit_cell_get(cell)

        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        iter_elem_hint = self._iterable_element_hint(iterable_val) or "Any"
        item = MoltValue(self.next_var(), type_hint=iter_elem_hint)
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))

        old_local = self.locals.get(target_name)
        target_in_scope_assigned = target_name in self.scope_assigned
        target_in_unbound_check = target_name in self.unbound_check_names
        if target_in_scope_assigned:
            self.scope_assigned.discard(target_name)
        if target_in_unbound_check:
            self.unbound_check_names.discard(target_name)
        self.locals[target_name] = item
        if cell is not None:
            self._emit_cell_set(cell, item)
        # The filters and the element read the target as the iteration value,
        # never as the enclosing function's same-named binding or its storage.
        outer_comp_shadow_locals = set(self.comp_shadow_locals)
        self.comp_shadow_locals.add(target_name)

        for if_node in comp.ifs:
            cond_val = self._emit_condition(if_node)
            not_cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[cond_val], result=not_cond))
            self.emit(MoltOp(kind="IF", args=[not_cond], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        elt_val = self.visit(genexpr.elt)
        neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[elt_val], result=neg))
        truth = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[neg], result=truth))
        if is_any:
            self.emit(MoltOp(kind="IF", args=[truth], result=MoltValue("none")))
            terminal_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=terminal_val))
        else:
            self.emit(MoltOp(kind="IF", args=[neg], result=MoltValue("none")))
            terminal_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=terminal_val))
        self.emit(
            MoltOp(
                kind="STORE_VAR",
                args=[terminal_val],
                result=MoltValue("none"),
                metadata={"var": res_slot},
            )
        )
        self.emit(MoltOp(kind="LOOP_BREAK", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.comp_shadow_locals = outer_comp_shadow_locals

        if old_local is not None:
            self.locals[target_name] = old_local
        else:
            self.locals.pop(target_name, None)
        if target_in_scope_assigned:
            self.scope_assigned.add(target_name)
        if target_in_unbound_check:
            self.unbound_check_names.add(target_name)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        if cell is not None and saved_cell_val is not None:
            self._emit_cell_set(cell, saved_cell_val)
            self._emit_drop_owned_value(saved_cell_val)

        final_res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="LOAD_VAR",
                args=[],
                result=final_res,
                metadata={"var": res_slot},
            )
        )
        return final_res

    def _emit_sum_call(
        self, func_id: str, node: ast.Call, needs_bind: bool
    ) -> MoltValue:
        if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
            kw.arg is None for kw in node.keywords
        ):
            callee = self._emit_builtin_function(func_id)
            res = MoltValue(self.next_var(), type_hint="Any")
            if needs_bind:
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            else:
                args = self._emit_call_args(node.args)
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
            return res
        if not node.args:
            return self._emit_type_error_value(
                "sum expected at least 1 argument, got 0"
            )
        if len(node.args) > 2:
            return self._emit_type_error_value(
                f"sum expected at most 2 arguments, got {len(node.args)}"
            )
        if len(node.args) == 1 and not node.keywords:
            inline_sum = self._try_emit_inline_sum_genexpr(node)
            if inline_sum is not None:
                return inline_sum

        start_expr = None
        has_start = False
        if len(node.args) == 2:
            start_expr = node.args[1]
            has_start = True
        for keyword in node.keywords:
            if keyword.arg != "start":
                msg = f"sum() got an unexpected keyword argument '{keyword.arg}'"
                return self._emit_type_error_value(msg)
            if has_start:
                return self._emit_type_error_value(
                    "sum() got multiple values for argument 'start'"
                )
            start_expr = keyword.value
            has_start = True

        iterable = self.visit(node.args[0])
        if iterable is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported sum iterable"
            )
        if start_expr is None:
            start_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
        else:
            start_val = self.visit(start_expr)
            if start_val is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported sum start value"
                )
        callee = self._emit_builtin_function(func_id)
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[callee, iterable, start_val], result=res)
        )
        return res

"""Reducer call lowering helpers for ``CallVisitorMixin``.

This is a semantic F2 extraction from the call visitor, not a second dispatch
surface: full-consumption reducers (``sum``) and short-circuit reducers
(``any``/``all``) own their comprehension-fusion invariants here.
"""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    cast,
)

from molt.frontend._types import MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallReductionMixin(_MixinBase):
    def _can_inline_sum_genexpr(self, node: ast.GeneratorExp | ast.ListComp) -> bool:
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

    @staticmethod
    def _sum_add_result_hint(acc: MoltValue, value: MoltValue) -> str:
        if acc.type_hint == "float" or value.type_hint == "float":
            return "float"
        if acc.type_hint in {"bool", "int"} and value.type_hint in {"bool", "int"}:
            return "int"
        return "Any"

    def _try_emit_inline_sum_genexpr(self, node: ast.Call) -> MoltValue | None:
        if (
            len(node.args) != 1
            or node.keywords
            # `sum([x for x in it])` is semantically identical to
            # `sum(x for x in it)`: the list is a throwaway consumed only by
            # `sum`, and `sum` fully consumes its argument with no
            # short-circuit. Do not copy this to eager-vs-lazy reducers.
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

        iterable_val = self.visit(comp.iter)
        if iterable_val is None:
            self.comp_shadow_locals = outer_comp_shadow_locals
            for name, boxed in saved_boxed.items():
                if boxed is not None:
                    self.boxed_locals[name] = boxed
            for name, hint in saved_boxed_hints.items():
                if hint is not None:
                    self.boxed_local_hints[name] = hint
            return None
        iter_obj = self._emit_iter_new(iterable_val)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))

        start_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
        acc_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[start_val], result=acc_cell))

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
                MoltValue(self.next_var(), type_hint="Any") for _ in tuple_target_names
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
            cond_val = self.visit(if_node)
            not_cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[cond_val], result=not_cond))
            self.emit(MoltOp(kind="IF", args=[not_cond], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        value = self.visit(genexpr.elt)
        if value is None:
            raise NotImplementedError("Unsupported sum generator expression")
        acc_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[acc_cell, zero], result=acc_val))
        acc_next = MoltValue(
            self.next_var(),
            type_hint=self._sum_add_result_hint(acc_val, cast(MoltValue, value)),
        )
        self.emit(MoltOp(kind="ADD", args=[acc_val, value], result=acc_next))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[acc_cell, zero, acc_next],
                result=MoltValue("none"),
            )
        )
        for name in user_target_names:
            prior = saved_locals.get(name)
            if prior is not None:
                self.locals[name] = prior
            else:
                self.locals.pop(name, None)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[acc_cell, zero], result=result))
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
        if (
            len(genexpr.generators) != 1
            or genexpr.generators[0].is_async
            or not isinstance(genexpr.generators[0].target, ast.Name)
        ):
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
            save_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=save_idx))
            saved_cell_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[cell, save_idx], result=saved_cell_val))

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
            box_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=box_idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, box_idx, item],
                    result=MoltValue("none"),
                )
            )

        for if_node in comp.ifs:
            cond_val = self.visit(if_node)
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
            post_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=post_idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, post_idx, saved_cell_val],
                    result=MoltValue("none"),
                )
            )

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
            return self._emit_type_error_value("sum expected at least 1 argument, got 0")
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
            raise NotImplementedError("Unsupported sum iterable")
        if start_expr is None:
            start_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
        else:
            start_val = self.visit(start_expr)
            if start_val is None:
                raise NotImplementedError("Unsupported sum start value")
        callee = self._emit_builtin_function(func_id)
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[callee, iterable, start_val], result=res)
        )
        return res

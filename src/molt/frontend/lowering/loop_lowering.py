"""LoopLoweringMixin: loop iteration, range lowering, and loop guard custody.

Move-only extraction from frontend/__init__.py. This lowering authority owns
iter/range/for/while emission, loop-body control-flow snapshots, loop orelse,
static-live branch emission, loop guard hoisting/invalidation, and specialized
loop fast paths shared by statement, comprehension, call, and analysis visitors.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING

from molt.frontend._types import MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class LoopLoweringMixin(_MixinBase):
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

    def _range_start_expr(self, node: ast.expr) -> ast.expr | None:
        if isinstance(node, ast.Constant):
            if isinstance(node.value, int) and node.value > 0:
                return node
            return None
        if isinstance(node, ast.Name):
            return node
        return None

    def _subscript_matches(self, node: ast.expr, seq_name: str, idx_name: str) -> bool:
        if not isinstance(node, ast.Subscript):
            return False
        if not isinstance(node.value, ast.Name) or node.value.id != seq_name:
            return False
        if isinstance(node.slice, ast.Name) and node.slice.id == idx_name:
            return True
        return False

    def _emit_iter_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
        item_hint = self._iterable_element_hint(iterable) or "Any"
        if self.is_async():
            iter_obj = self._emit_iter_new(iterable)
            iter_slot = self._async_local_offset(f"__for_iter_{len(self.async_locals)}")
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
            body_terminated = self._visit_loop_body(
                node.body, guard_map, loop_break_flag=loop_break_flag
            )
            if not body_terminated:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
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
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            if not body_terminated:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

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
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
        item_hint = self._iterable_element_hint(iterable) or "Any"
        if self.is_async():
            seq_slot = self._async_local_offset(f"__for_seq_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", seq_slot, iterable],
                    result=MoltValue("none"),
                )
            )
            length_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[iterable], result=length_val))
            length_slot = self._async_local_offset(
                f"__for_len_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", length_slot, length_val],
                    result=MoltValue("none"),
                )
            )
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            idx_slot = self._async_local_offset(f"__for_idx_{len(self.async_locals)}")
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
            self.async_index_loop_stack.append(idx_slot)
            body_terminated = self._visit_loop_body(
                node.body, guard_map, loop_break_flag=loop_break_flag
            )
            self.async_index_loop_stack.pop()
            if not body_terminated:
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
            self.range_loop_stack.append((idx, one))
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            self.range_loop_stack.pop()
            if not body_terminated:
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
                self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

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
    ) -> tuple[MoltValue, MoltValue, MoltValue, bool] | None:
        if not isinstance(node, ast.Call):
            return None
        if not isinstance(node.func, ast.Name) or node.func.id != "range":
            return None
        if len(node.args) > 3:
            return None
        if node.keywords:
            return None
        start_val: MoltValue | None = None
        stop_val: MoltValue | None = None
        step_val: MoltValue | None = None
        pos_params: list[str] = []
        if len(node.args) == 1:
            pos_params = ["stop"]
        elif len(node.args) == 2:
            pos_params = ["start", "stop"]
        elif len(node.args) == 3:
            pos_params = ["start", "stop", "step"]
        for param, arg in zip(pos_params, node.args):
            val = self.visit(arg)
            if val is None:
                return None
            if param == "start":
                if start_val is not None:
                    return None
                start_val = val
            elif param == "stop":
                if stop_val is not None:
                    return None
                stop_val = val
            else:
                if step_val is not None:
                    return None
                step_val = val
        if stop_val is None:
            return None
        if start_val is None:
            start_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
        if step_val is None:
            step_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step_val))
        int_like = {"int", "bool"}
        lowerable = {
            start_val.type_hint,
            stop_val.type_hint,
            step_val.type_hint,
        }.issubset(int_like)
        return start_val, stop_val, step_val, lowerable

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
        loop_break_flag: int | str | None = None,
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
                self.range_loop_stack.append((idx, step))
                body_terminated = self._visit_loop_body(
                    node.body, None, loop_break_flag=loop_break_flag
                )
                self.range_loop_stack.pop()
                if not body_terminated:
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
                    self.emit(
                        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none"))
                    )
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
            self.range_loop_stack.append((idx, step))
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            self.range_loop_stack.pop()
            if not body_terminated:
                with self._suppress_check_exception(emit_on_exit=False):
                    next_idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
                    self.emit(
                        MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx)
                    )
                    self.emit(
                        MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                    )
                    self.emit(
                        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none"))
                    )
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
            self.range_loop_stack.append((idx_neg, step))
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            self.range_loop_stack.pop()
            if not body_terminated:
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
                    self.emit(
                        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none"))
                    )
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
        res = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=res))
        if self.try_end_labels:
            self._emit_raise_if_pending()
        else:
            self._emit_raise_if_pending(emit_exit=True)
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
            self._emit_raise_if_pending(emit_exit=True)
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
            if entry and entry[0] == expected_class:
                return entry[1]
        return None

    def _push_loop_guard_assumptions(
        self,
        guard_map: dict[str, tuple[str, MoltValue]],
        assume_true: bool,
    ) -> None:
        assumptions: dict[str, tuple[str, bool]] = {}
        for name, (expected_class, _) in guard_map.items():
            assumptions[name] = (expected_class, assume_true)
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
        if self.exact_locals.get(name) != expected_class:
            return None
        guard_map = self.loop_layout_guards[-1]
        cached = guard_map.get(name)
        if cached and cached[0] == expected_class:
            return cached[1]
        guard = self._emit_layout_guard(obj, expected_class)
        guard_map[name] = (expected_class, guard)
        return guard

    def _invalidate_loop_guard(self, name: str) -> None:
        for guard_map in self.loop_layout_guards:
            guard_map.pop(name, None)

    def _invalidate_loop_guards_for_class(self, class_name: str) -> None:
        for guard_map in self.loop_layout_guards:
            stale = [
                key for key, (klass, _) in guard_map.items() if klass == class_name
            ]
            for key in stale:
                guard_map.pop(key, None)

    def _emit_hoisted_loop_guards(
        self, body: list[ast.stmt]
    ) -> dict[str, tuple[str, MoltValue]]:
        if self.is_async():
            return {}
        candidates = self._collect_loop_guard_candidates(body)
        if not candidates:
            return {}
        guard_map: dict[str, tuple[str, MoltValue]] = {}
        for name, expected_class in sorted(candidates.items()):
            obj = self._load_local_value(name)
            if obj is None:
                obj = self.locals.get(name) or self.globals.get(name)
            if obj is None:
                continue
            guard = self._emit_layout_guard(obj, expected_class)
            guard_map[name] = (expected_class, guard)
        return guard_map

    def _emit_guard_map_condition(
        self, guard_map: dict[str, tuple[str, MoltValue]]
    ) -> MoltValue:
        condition: MoltValue | None = None
        for _, (_, guard) in sorted(guard_map.items()):
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
        if iterable.type_hint in {
            "list",
            "tuple",
            "dict",
            "range",
            "iter",
            "generator",
        }:
            return self._emit_iter_new(iterable)
        res = MoltValue(self.next_var(), type_hint="async_iter")
        self.emit(MoltOp(kind="AITER", args=[iterable], result=res))
        return res

    def _emit_for_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        if self._iterable_is_indexable_for_loop(iterable):
            self._emit_index_loop(node, iterable, loop_break_flag=loop_break_flag)
        else:
            self._emit_iter_loop(node, iterable, loop_break_flag=loop_break_flag)

    def _prepare_mutable_control_flow_bindings(self, names: set[str]) -> None:
        if self._class_ns_stack:
            # Names bound by the active class body are backed by its namespace
            # dict (STORE_INDEX/INDEX through ``_class_ns_store``/``_class_ns_load``),
            # which is the heap-resident, loop-carried-correct mutable home — the
            # class-scope analogue of the module dict.  They must NOT be promoted
            # into ``module_global_mutations`` (which would leak the binding into
            # the enclosing module namespace and steer reads to MODULE_GET_ATTR)
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
            # home: the module object. Route loads through MODULE_GET_ATTR instead
            # of synthesizing one-element list cells just to model loop-carried
            # mutation. That keeps module lowering canonical and avoids ad hoc
            # boxed-local indirection for top-level loops.
            module_backed = {name for name in names if not name.startswith("__molt_")}
            if module_backed:
                # Flush any values that were previously assigned (before
                # this loop) into the module dict.  Without this, a
                # variable assigned before the loop and then mutated inside
                # the loop would lose its initial value when module_get_attr
                # reads find nothing in the module dict.
                for name in sorted(module_backed):
                    # Skip variables already flushed to the module dict
                    # by an enclosing loop.  Re-flushing would overwrite
                    # the current dynamic value with the stale SSA value
                    # from the original definition, resetting accumulators
                    # on every outer loop iteration.
                    if name in self.module_global_mutations:
                        continue
                    existing = self.globals.get(name)
                    if existing is None:
                        existing = self.locals.get(name)
                    if existing is not None and self.module_obj is not None:
                        self._emit_module_attr_set_on(self.module_obj, name, existing)
                self.module_global_mutations.update(module_backed)
                # Remove from self.locals so visit_Name falls through to
                # the module_global_mutations check (module_get_attr).
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

    def _emit_loop_orelse(self, break_name: str, orelse: list[ast.stmt]) -> None:
        break_val = self._load_local_value(break_name)
        if break_val is None and break_name in self.module_global_mutations:
            break_val = self._emit_module_attr_get(break_name)
        if break_val is None:
            raise NotImplementedError("for-else break flag not initialized")
        should_run = self._emit_not(break_val)
        self.emit(MoltOp(kind="IF", args=[should_run], result=MoltValue("none")))
        self._visit_block(orelse)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _const_int_from_expr(self, node: ast.expr) -> int | None:
        if (
            isinstance(node, ast.Constant)
            and isinstance(node.value, int)
            and not isinstance(node.value, bool)
        ):
            return node.value
        if isinstance(node, ast.Name):
            value = self.locals.get(node.id)
            if value is None and self.current_func_name == "molt_main":
                value = self.globals.get(node.id)
            if value is not None:
                return self.const_ints.get(value.name)
        return None

    def _const_int_for_local(self, name: str) -> int | None:
        value = self.locals.get(name)
        if value is None:
            return 0
        return self.const_ints.get(value.name)

    def _is_unit_increment(self, stmt: ast.stmt, name: str) -> bool:
        if isinstance(stmt, ast.AugAssign):
            if isinstance(stmt.target, ast.Name) and stmt.target.id == name:
                return (
                    isinstance(stmt.op, ast.Add)
                    and isinstance(stmt.value, ast.Constant)
                    and stmt.value.value == 1
                )
            return False
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return False
            if stmt.targets[0].id != name:
                return False
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return False
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and left.id == name
                and isinstance(right, ast.Constant)
                and right.value == 1
            ):
                return True
            if (
                isinstance(right, ast.Name)
                and right.id == name
                and isinstance(left, ast.Constant)
                and left.value == 1
            ):
                return True
        return False

    def _emit_counted_while(
        self, index_name: str, bound: int, body: list[ast.stmt]
    ) -> None:
        start = self._load_local_value(index_name)
        if start is None:
            start = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        stop = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[bound], result=stop))
        guard_map = self._emit_hoisted_loop_guards(body)
        self._push_loop_static_class_refs(body)
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(index_name, idx)
        # For module-level code, also sync to the module namespace so
        # that module_get_attr reads inside the loop body see the current
        # counter value (not the initial value from before the loop).
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[index_name], result=key))
            self.emit(
                MoltOp(
                    kind="MODULE_SET_ATTR",
                    args=[self.module_obj, key, idx],
                    result=MoltValue("none"),
                )
            )
        body_terminated = self._visit_loop_body(body, guard_map)
        if not body_terminated:
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
            self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self._pop_loop_static_class_refs()
        self._store_local_value(index_name, idx)
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            key2 = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[index_name], result=key2))
            self.emit(
                MoltOp(
                    kind="MODULE_SET_ATTR",
                    args=[self.module_obj, key2, idx],
                    result=MoltValue("none"),
                )
            )

    def _dict_increment_key_is_single_eval_safe(self, key: ast.expr) -> bool:
        if isinstance(key, (ast.Name, ast.Constant)):
            return True
        if not isinstance(key, ast.Attribute) or not isinstance(key.value, ast.Name):
            return False
        obj_name = key.value.id
        obj_value = self.locals.get(obj_name)
        if obj_value is None and self.current_func_name == "molt_main":
            obj_value = self.globals.get(obj_name)
        class_id = self.exact_locals.get(obj_name)
        if class_id is None and obj_value is not None:
            class_id = self.boxed_local_hints.get(obj_name) or obj_value.type_hint
        class_info = self.classes.get(class_id or "")
        return bool(
            class_info
            and class_info.get("dataclass")
            and key.attr in class_info.get("fields", {})
        )

    def _emit_split_dict_increment_for_loop(self, node: ast.For) -> bool:
        match = self._match_split_dict_increment_for_loop(node)
        if match is None:
            return False
        dict_expr, line_expr, sep_expr, delta_expr = match
        dict_obj = self.visit(dict_expr)
        line_obj = self.visit(line_expr)
        delta_obj = self.visit(delta_expr)
        if dict_obj is None or line_obj is None or delta_obj is None:
            return False
        # Keep split+count lanes guarded so deopt/profile tooling can track
        # dict-shape assumptions explicitly.
        self._emit_guard_dict_shape(dict_obj)
        pair = MoltValue(self.next_var(), type_hint="tuple")
        if sep_expr is None:
            self.emit(
                MoltOp(
                    kind="STRING_SPLIT_WS_DICT_INC",
                    args=[line_obj, dict_obj, delta_obj],
                    result=pair,
                )
            )
        else:
            sep_obj = self.visit(sep_expr)
            if sep_obj is None:
                return False
            self.emit(
                MoltOp(
                    kind="STRING_SPLIT_SEP_DICT_INC",
                    args=[line_obj, sep_obj, dict_obj, delta_obj],
                    result=pair,
                )
            )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        last_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=last_val))
        has_any = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=has_any))
        self.emit(MoltOp(kind="IF", args=[has_any], result=MoltValue("none")))
        self._emit_assign_target(node.target, last_val, None)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if node.orelse:
            self._visit_block(node.orelse)
        return True

    def _is_taq_header_guard(self, stmt: ast.stmt) -> str | None:
        if not isinstance(stmt, ast.If):
            return None
        if stmt.orelse:
            return None
        if not isinstance(stmt.test, ast.Name):
            return None
        if len(stmt.body) != 2:
            return None
        assign, cont = stmt.body
        if not isinstance(cont, ast.Continue):
            return None
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        if assign.targets[0].id != stmt.test.id:
            return None
        if (
            not isinstance(assign.value, ast.Constant)
            or assign.value.value is not False
        ):
            return None
        return stmt.test.id

    def _emit_taq_ingest_loop_body(
        self,
        body: list[ast.stmt],
    ) -> bool:
        match = self._match_taq_ingest_loop_body(body)
        if match is None:
            return False
        header_name, data_name, line_name, _split_name, bucket_expr = match
        if header_name is not None:
            header_val = self._load_local_value(header_name)
            if header_val is None:
                header_val = self.locals.get(header_name) or self.globals.get(
                    header_name
                )
            if header_val is None:
                return False
            self.emit(MoltOp(kind="IF", args=[header_val], result=MoltValue("none")))
            header_false = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=header_false))
            self._emit_assign_target(
                ast.Name(id=header_name, ctx=ast.Store()),
                header_false,
                None,
            )
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        data_val = self._load_local_value(data_name)
        if data_val is None:
            data_val = self.locals.get(data_name) or self.globals.get(data_name)
        if data_val is None:
            return False
        line_val = self._load_local_value(line_name)
        if line_val is None:
            line_val = self.locals.get(line_name) or self.globals.get(line_name)
        if line_val is None:
            return False
        bucket_val = self.visit(bucket_expr)
        if bucket_val is None:
            return False
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="TAQ_INGEST_LINE",
                args=[data_val, line_val, bucket_val],
                result=res,
            )
        )
        return True

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
                module_backed = {n for n in assigned if not n.startswith("__molt_")}
                if module_backed:
                    for name in sorted(module_backed):
                        existing = self.globals.get(name)
                        if existing is None:
                            existing = self.locals.get(name)
                        if existing is not None and self.module_obj is not None:
                            self._emit_module_attr_set_on(
                                self.module_obj, name, existing
                            )
                    self.module_global_mutations.update(module_backed)
                for name in sorted(assigned - module_backed):
                    self._box_local(name)
            else:
                for name in sorted(assigned):
                    if name not in self.scope_assigned or name in self.closure_locals:
                        self._box_local(name)
        self._visit_block(branch)

    def _visit_block(self, body: list[ast.stmt]) -> bool:
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
        prefill: dict[str, tuple[str, MoltValue]] | None = None,
        loop_break_flag: int | str | None = None,
    ) -> bool:
        if not self.is_async() and self._emit_taq_ingest_loop_body(body):
            return True
        if not self.is_async():
            guard_map = dict(prefill) if prefill else {}
            self.loop_layout_guards.append(guard_map)
        self.loop_break_flags.append(loop_break_flag)
        self.loop_try_depths.append(len(self.try_scopes))
        terminated = False
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
                terminated = self._visit_block(body)
            finally:
                self.control_flow_depth -= 1
        finally:
            self.unbound_check_names = unbound_snapshot
            self.loop_break_flags.pop()
            self.loop_try_depths.pop()
            if not self.is_async():
                self.loop_layout_guards.pop()
        return terminated

    def _emit_loop_unwind(self) -> list[int]:
        if not self.loop_try_depths:
            return []
        max_scopes = len(self.try_scopes)
        loop_depth = self.loop_try_depths[-1]
        if loop_depth >= max_scopes:
            return []
        return self._emit_control_flow_scope_unwind(
            self.try_scopes[loop_depth:max_scopes]
        )

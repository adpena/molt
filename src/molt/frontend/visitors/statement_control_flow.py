"""ControlFlowStatementVisitorMixin: synchronous control-flow statements.

Move-only extraction from frontend/__init__.py. Covers if/with/loop/try/raise,
assert, break, and continue lowering. Async control flow lives in
AsyncGenVisitorMixin.
"""

from __future__ import annotations

import ast

from typing import TYPE_CHECKING

from molt.frontend._types import (
    ActiveException,
    MoltOp,
    MoltValue,
    TryScope,
)
from molt.compiler_analysis.static_truth import static_if_live_branch

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ControlFlowStatementVisitorMixin(_MixinBase):
    def visit_If(self, node: ast.If) -> None:
        static_branch = static_if_live_branch(node)
        if static_branch is not None:
            self._emit_static_if_live_branch(static_branch)
            return None
        if not self.is_async():
            assigned = self._collect_assigned_names(node.body + node.orelse)
            assigned |= set(self._collect_namedexpr_names(node.test))
            if self.current_func_name == "molt_main":
                # Module-scope if-branch bindings use the module dict as their
                # mutable store instead of synthesising boxed-local cells.
                module_backed = {n for n in assigned if not n.startswith("__molt_")}
                if module_backed:
                    # Flush any values that were previously assigned (before
                    # this if-block) into the module dict.  Without this,
                    # a variable assigned unconditionally *before* the if,
                    # then conditionally reassigned *inside* the if, would
                    # lose its initial value: the pre-if store only wrote to
                    # self.globals (SSA cache), and the post-if eviction
                    # removes the cache entry, so MODULE_GET_ATTR would find
                    # nothing in the module dict.
                    #
                    # IMPORTANT: skip the flush for variables already tracked
                    # in module_global_mutations — they were flushed by a
                    # parent for/while loop's _prepare_mutable_control_flow_bindings
                    # and re-flushing the stale SSA value would overwrite the
                    # loop-carried accumulation on every iteration.
                    for name in sorted(module_backed):
                        if name in self.module_global_mutations:
                            continue
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
        cond = self.visit(node.test)
        self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
        self.control_flow_depth += 1
        # Snapshot unbound_check_names on flow entry so per-branch
        # discards don't leak into the post-merge state — only names
        # discarded in EVERY path can stay discarded after the merge.
        unbound_snapshot = set(self.unbound_check_names)
        try:
            self._visit_block(node.body)
            then_unbound = set(self.unbound_check_names)
            if node.orelse:
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.unbound_check_names = set(unbound_snapshot)
                self._visit_block(node.orelse)
                else_unbound = set(self.unbound_check_names)
                # Names discarded in BOTH branches stay discarded;
                # names discarded in only one go back to checked.
                self.unbound_check_names = then_unbound | else_unbound
            else:
                # if-only: the else path is implicit (no statements),
                # so it can't add discards.  Restore to snapshot —
                # any discards in `body` may not have happened.
                self.unbound_check_names = unbound_snapshot
        finally:
            self.control_flow_depth -= 1
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        # Evict module_global_mutations names from the locals/globals cache so
        # subsequent loads go through MODULE_GET_ATTR instead of reusing a
        # value that was only assigned in one branch.
        if self.current_func_name == "molt_main" and not self.is_async():
            assigned = self._collect_assigned_names(node.body + node.orelse)
            for name in assigned:
                if name in self.module_global_mutations:
                    self.globals.pop(name, None)
                    self.locals.pop(name, None)
        return None

    def visit_With(self, node: ast.With) -> None:
        if len(node.items) != 1:
            nested = ast.With(
                items=node.items[1:],
                body=node.body,
                type_comment=None,
            )
            ast.copy_location(nested, node)
            outer = ast.With(
                items=[node.items[0]],
                body=[nested],
                type_comment=node.type_comment,
            )
            ast.copy_location(outer, node)
            return self.visit_With(outer)

        item = node.items[0]
        ctx_val = self.visit(item.context_expr)
        if ctx_val is None:
            self._bridge_fallback(
                node,
                "with",
                impact="high",
                alternative="use contextlib.nullcontext for now",
                detail="context expression did not lower",
            )
            return None

        ctx_name = f"__molt_with_ctx_{self.next_label()}"
        self._store_local_value(ctx_name, ctx_val)
        ctx_mark = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONTEXT_DEPTH", args=[], result=ctx_mark))
        ctx_mark_offset = None
        if self.is_async():
            ctx_mark_name = f"__ctx_mark_{len(self.async_locals)}"
            ctx_mark_offset = self._async_local_offset(ctx_mark_name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", ctx_mark_offset, ctx_mark],
                    result=MoltValue("none"),
                )
            )
        scope = TryScope(
            ctx_mark=ctx_mark,
            finalbody=None,
            ctx_mark_offset=ctx_mark_offset,
            try_start_has_handler_value=False,
        )
        self.try_scopes.append(scope)
        ctx_ref = self._load_local_value(ctx_name) or ctx_val
        enter_hint = (
            ctx_val.type_hint
            if ctx_val.type_hint in {"file_text", "file_bytes"}
            else "Any"
        )
        enter_val = MoltValue(self.next_var(), type_hint=enter_hint)
        self.emit(MoltOp(kind="CONTEXT_ENTER", args=[ctx_ref], result=enter_val))
        self._emit_raise_if_pending()
        if item.optional_vars is not None:
            self._emit_assign_target(item.optional_vars, enter_val, None)
        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_exc_label = self.next_label()
        try_done_label = self.next_label()
        scope.handler_label = try_exc_label
        scope.done_label = try_done_label
        self.try_end_labels.append(try_exc_label)
        self.emit(
            MoltOp(
                kind="TRY_START",
                args=[],
                result=MoltValue("none"),
                metadata={"try_region_id": try_exc_label},
            )
        )
        self.context_depth += 1
        self.control_flow_depth += 1
        # See _visit_loop_body for the unbound-check snapshot rationale.
        # `with` blocks may exit via exception before any assignment,
        # so post-block code can't rely on body-internal discards.
        unbound_snapshot = set(self.unbound_check_names)
        try:
            self._visit_block(node.body)
        finally:
            self.unbound_check_names = unbound_snapshot
            self.control_flow_depth -= 1
            self.context_depth -= 1
        self.try_end_labels.pop()
        # End the protected body region before issuing the normal __exit__ call.
        # If __exit__ itself raises, it should propagate directly rather than
        # re-entering the with-exception cleanup path and double-consuming context.
        self.emit(
            MoltOp(
                kind="TRY_END",
                args=[],
                result=MoltValue("none"),
                metadata={"try_region_id": try_exc_label},
            )
        )
        none_exit = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exit))
        exit_ok = MoltValue(self.next_var(), type_hint="Any")
        ctx_ref = self._load_local_value(ctx_name) or ctx_val
        self.emit(
            MoltOp(kind="CONTEXT_EXIT", args=[ctx_ref, none_exit], result=exit_ok)
        )
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        self.emit(MoltOp(kind="JUMP", args=[try_done_label], result=MoltValue("none")))
        self.emit(MoltOp(kind="LABEL", args=[try_exc_label], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="TRY_END",
                args=[],
                result=MoltValue("none"),
                metadata={"try_region_id": try_exc_label},
            )
        )
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)
        self.try_handler_scopes.append(scope)

        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
        pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))

        self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="EXCEPTION_CONTEXT_SET",
                args=[exc_val],
                result=MoltValue("none"),
            )
        )
        exit_res = MoltValue(self.next_var(), type_hint="Any")
        ctx_ref = self._load_local_value(ctx_name) or ctx_val
        self.emit(MoltOp(kind="CONTEXT_EXIT", args=[ctx_ref, exc_val], result=exit_res))
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        not_res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[exit_res], result=not_res))
        is_truthy = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[not_res], result=is_truthy))
        self.emit(MoltOp(kind="IF", args=[is_truthy], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self._emit_raise_if_pending()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        exit_ok = MoltValue(self.next_var(), type_hint="Any")
        ctx_ref = self._load_local_value(ctx_name) or ctx_val
        self.emit(MoltOp(kind="CONTEXT_EXIT", args=[ctx_ref, none_val], result=exit_ok))
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.emit(MoltOp(kind="LABEL", args=[try_done_label], result=MoltValue("none")))
        self.try_handler_scopes.pop()
        self.try_scopes.pop()
        self.try_suppress_depth = prior_suppress
        return None

    def visit_For(self, node: ast.For) -> None:
        if self._emit_split_dict_increment_for_loop(node):
            return None
        break_name = None
        if node.orelse:
            while True:
                candidate = f"__molt_for_break_{self.loop_break_counter}"
                self.loop_break_counter += 1
                if (
                    candidate not in self.locals
                    and candidate not in self.globals
                    and candidate not in self.boxed_locals
                ):
                    break_name = candidate
                    break
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            self._store_local_value(break_name, break_init)
            if not self.is_async():
                self._box_local(break_name)
        matmul_match = (
            self._match_matmul_loop(node) if isinstance(node.target, ast.Name) else None
        )
        if matmul_match is not None:
            out_name, a_name, b_name = matmul_match
            a_val = self.locals.get(a_name) or self.globals.get(a_name)
            b_val = self.locals.get(b_name) or self.globals.get(b_name)
            if a_val is None or b_val is None:
                raise NotImplementedError("Matmul operands must be simple locals")
            a_hint = self.boxed_local_hints.get(a_name, a_val.type_hint)
            b_hint = self.boxed_local_hints.get(b_name, b_val.type_hint)
            if a_hint == "buffer2d" and b_hint == "buffer2d":
                a_arg = self._load_local_value(a_name) or a_val
                b_arg = self._load_local_value(b_name) or b_val
                res = MoltValue(self.next_var(), type_hint="buffer2d")
                self.emit(
                    MoltOp(kind="BUFFER2D_MATMUL", args=[a_arg, b_arg], result=res)
                )
                self._store_local_value(out_name, res)
                if break_name is not None:
                    self._emit_loop_orelse(break_name, node.orelse)
                return None
        target_names = self._collect_target_names(node.target)
        if not target_names:
            raise NotImplementedError("Only name/tuple/list for targets are supported")
        for name in target_names:
            self.exact_locals.pop(name, None)
        assigned = self._collect_assigned_names(node.body)
        assigned.update(target_names)
        self._prepare_mutable_control_flow_bindings(assigned)
        reduction = None
        # The vector/reduction fast paths (VEC_SUM/VEC_PROD/VEC_MIN/VEC_MAX)
        # collapse an accumulator loop into a single op and elide the
        # per-iteration loop-target binding — sound only when that target's
        # final value is dead after the loop.  Inside a class body the loop
        # target is ALWAYS observable (it is bound into the class namespace, and
        # may be read, ``del``'d, or end up as a class attribute), so these
        # rewrites would drop a required binding.  Disable them for class-body
        # loops; the ordinary loop lowering (which binds the target into the
        # namespace each iteration) is used instead.  (P0 #50.)
        if (
            not self.is_async()
            and isinstance(node.target, ast.Name)
            and not self._class_ns_stack
        ):
            reduction = self._match_indexed_vector_reduction_loop(node)
            if reduction is None:
                reduction = self._match_indexed_vector_minmax_loop(node)
            if reduction is None:
                reduction = self._match_iter_vector_reduction_loop(node)
            if reduction is None:
                reduction = self._match_iter_vector_minmax_loop(node)
        if reduction is not None:
            acc_name, seq_name, kind, start_expr = reduction
            if seq_name in assigned:
                reduction = None
            else:
                seq_val = self.locals.get(seq_name) or self.globals.get(seq_name)
                if seq_val and seq_val.type_hint in {"list", "tuple", "range"}:
                    acc_val = self._load_local_value(acc_name)
                    if acc_val is not None:
                        seq_arg = seq_val
                        args = [seq_arg, acc_val]
                        vec_kind: str | None = None
                        elem_hint = self._container_elem_hint(seq_val)
                        acc_num_hint = self._reduction_acc_numeric_hint(
                            acc_name, acc_val
                        )
                        if seq_val.type_hint == "range":
                            if kind == "sum" and start_expr is None:
                                if acc_num_hint == "float":
                                    vec_kind = "VEC_SUM_FLOAT_RANGE_ITER"
                                else:
                                    vec_kind = "VEC_SUM_INT_RANGE_ITER"
                                if self.type_hint_policy == "trust":
                                    vec_kind = f"{vec_kind}_TRUSTED"
                        else:
                            if kind == "sum" and acc_num_hint == "float":
                                if elem_hint in {None, "int", "float"}:
                                    vec_kind = "VEC_SUM_FLOAT"
                                    if start_expr is not None:
                                        vec_kind = "VEC_SUM_FLOAT_RANGE"
                                    if self.type_hint_policy == "trust":
                                        vec_kind = f"{vec_kind}_TRUSTED"
                            else:
                                vec_kind = {
                                    "sum": "VEC_SUM_INT",
                                    "prod": "VEC_PROD_INT",
                                    "min": "VEC_MIN_INT",
                                    "max": "VEC_MAX_INT",
                                }.get(kind, "VEC_SUM_INT")
                            if (
                                kind == "prod"
                                and elem_hint == "int"
                                and not self._is_flat_list_int_container(seq_val)
                            ):
                                seq_arg = self._emit_intarray_from_seq(seq_val)
                                args[0] = seq_arg
                            if (
                                start_expr is not None
                                and vec_kind is not None
                                and vec_kind.startswith("VEC_")
                                and "FLOAT" not in vec_kind
                            ):
                                vec_kind = f"{vec_kind}_RANGE"
                            if (
                                self.type_hint_policy == "trust"
                                and elem_hint == "int"
                                and vec_kind is not None
                                and "FLOAT" not in vec_kind
                            ):
                                vec_kind = f"{vec_kind}_TRUSTED"
                        if vec_kind is None:
                            pass
                        else:
                            zero = MoltValue(self.next_var(), type_hint="int")
                            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                            one = MoltValue(self.next_var(), type_hint="int")
                            self.emit(MoltOp(kind="CONST", args=[1], result=one))
                            pair = MoltValue(self.next_var(), type_hint="tuple")
                            if start_expr is not None:
                                start_val = self.visit(start_expr)
                                if start_val is None:
                                    raise NotImplementedError(
                                        "Unsupported range start for vector reduction"
                                    )
                                args.append(start_val)
                            self.emit(MoltOp(kind=vec_kind, args=args, result=pair))
                            sum_hint = (
                                "float"
                                if vec_kind is not None and "FLOAT" in vec_kind
                                else "int"
                            )
                            sum_val = MoltValue(self.next_var(), type_hint=sum_hint)
                            self.emit(
                                MoltOp(kind="INDEX", args=[pair, zero], result=sum_val)
                            )
                            ok_val = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(kind="INDEX", args=[pair, one], result=ok_val)
                            )
                            self.emit(
                                MoltOp(
                                    kind="IF", args=[ok_val], result=MoltValue("none")
                                )
                            )
                            self._store_local_value(acc_name, sum_val)
                            self.emit(
                                MoltOp(kind="ELSE", args=[], result=MoltValue("none"))
                            )
                            range_args = self._parse_range_call(node.iter)
                            if range_args is not None:
                                start, stop, step, lowerable = range_args
                                if lowerable:
                                    self._emit_range_loop(
                                        node,
                                        start,
                                        stop,
                                        step,
                                        loop_break_flag=break_name,
                                    )
                                else:
                                    iterable = self._emit_range_obj_from_args(
                                        start, stop, step
                                    )
                                    self._emit_for_loop(
                                        node, iterable, loop_break_flag=break_name
                                    )
                            else:
                                iterable = self._load_local_value(seq_name) or seq_val
                                self._emit_for_loop(
                                    node, iterable, loop_break_flag=break_name
                                )
                            self.emit(
                                MoltOp(kind="END_IF", args=[], result=MoltValue("none"))
                            )
                            if break_name is not None:
                                self._emit_loop_orelse(break_name, node.orelse)
                            return None
        range_args = self._parse_range_call(node.iter)
        if range_args is not None:
            start, stop, step, lowerable = range_args
            if lowerable:
                # Vector reductions elide the per-iteration loop-target binding;
                # in a class body that target must persist into the namespace.
                # (P0 #50.)
                _skip_vec = self.is_async() or bool(self._class_ns_stack)
                vector_info = (
                    None if _skip_vec else self._match_vector_reduction_loop(node)
                )
                minmax_info = (
                    None if _skip_vec else self._match_vector_minmax_loop(node)
                )
                if vector_info is None:
                    vector_info = minmax_info
                if vector_info:
                    acc_name, item_name, kind = vector_info
                    target_id = (
                        node.target.id if isinstance(node.target, ast.Name) else None
                    )
                    if (
                        kind == "sum"
                        and target_id is not None
                        and item_name == target_id
                    ):
                        acc_val = self._load_local_value(acc_name)
                        if acc_val is not None:
                            seq_arg = self._emit_range_obj_from_args(start, stop, step)
                            acc_num_hint = self._reduction_acc_numeric_hint(
                                acc_name, acc_val
                            )
                            vec_kind = "VEC_SUM_INT_RANGE_ITER"
                            if acc_num_hint == "float":
                                vec_kind = "VEC_SUM_FLOAT_RANGE_ITER"
                            if self.type_hint_policy == "trust":
                                vec_kind = f"{vec_kind}_TRUSTED"
                            zero = MoltValue(self.next_var(), type_hint="int")
                            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                            one = MoltValue(self.next_var(), type_hint="int")
                            self.emit(MoltOp(kind="CONST", args=[1], result=one))
                            pair = MoltValue(self.next_var(), type_hint="tuple")
                            self.emit(
                                MoltOp(
                                    kind=vec_kind,
                                    args=[seq_arg, acc_val],
                                    result=pair,
                                )
                            )
                            sum_hint = "float" if "FLOAT" in vec_kind else "int"
                            sum_val = MoltValue(self.next_var(), type_hint=sum_hint)
                            self.emit(
                                MoltOp(kind="INDEX", args=[pair, zero], result=sum_val)
                            )
                            ok_val = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(kind="INDEX", args=[pair, one], result=ok_val)
                            )
                            self.emit(
                                MoltOp(
                                    kind="IF", args=[ok_val], result=MoltValue("none")
                                )
                            )
                            self._store_local_value(acc_name, sum_val)
                            self.emit(
                                MoltOp(kind="ELSE", args=[], result=MoltValue("none"))
                            )
                            self._emit_range_loop(
                                node,
                                start,
                                stop,
                                step,
                                loop_break_flag=break_name,
                            )
                            self.emit(
                                MoltOp(kind="END_IF", args=[], result=MoltValue("none"))
                            )
                            if break_name is not None:
                                self._emit_loop_orelse(break_name, node.orelse)
                            return None
                self._emit_range_loop(
                    node, start, stop, step, loop_break_flag=break_name
                )
                if break_name is not None:
                    self._emit_loop_orelse(break_name, node.orelse)
                return None
            iterable = self._emit_range_obj_from_args(start, stop, step)
        else:
            iterable = None
        if iterable is None:
            iterable = self.visit(node.iter)
        if iterable is None:
            raise NotImplementedError("Unsupported iterable in for loop")
        # Vector reductions elide the per-iteration loop-target binding; in a
        # class body that target must persist into the namespace.  (P0 #50.)
        _skip_vec = self.is_async() or bool(self._class_ns_stack)
        vector_info = None if _skip_vec else self._match_vector_reduction_loop(node)
        minmax_info = None if _skip_vec else self._match_vector_minmax_loop(node)
        if vector_info is None:
            vector_info = minmax_info
        if (
            vector_info
            and iterable.type_hint in {"list", "tuple", "range"}
            and self._iterable_is_indexable(iterable)
        ):
            acc_name, _, kind = vector_info
            acc_val = self._load_local_value(acc_name)
            if acc_val is not None:
                seq_arg = iterable
                vec_kind: str | None = None
                acc_num_hint = self._reduction_acc_numeric_hint(acc_name, acc_val)
                if iterable.type_hint == "range":
                    if kind == "sum":
                        if acc_num_hint == "float":
                            vec_kind = "VEC_SUM_FLOAT_RANGE_ITER"
                        else:
                            vec_kind = "VEC_SUM_INT_RANGE_ITER"
                        if self.type_hint_policy == "trust":
                            vec_kind = f"{vec_kind}_TRUSTED"
                else:
                    elem_hint = self._container_elem_hint(iterable)
                    if kind == "sum" and acc_num_hint == "float":
                        if elem_hint in {None, "int", "float"}:
                            vec_kind = "VEC_SUM_FLOAT"
                            if self.type_hint_policy == "trust":
                                vec_kind = f"{vec_kind}_TRUSTED"
                    else:
                        vec_kind = {
                            "sum": "VEC_SUM_INT",
                            "prod": "VEC_PROD_INT",
                            "min": "VEC_MIN_INT",
                            "max": "VEC_MAX_INT",
                        }.get(kind, "VEC_SUM_INT")
                        if (
                            kind == "prod"
                            and elem_hint == "int"
                            and not self._is_flat_list_int_container(iterable)
                        ):
                            seq_arg = self._emit_intarray_from_seq(iterable)
                        if self.type_hint_policy == "trust" and elem_hint == "int":
                            vec_kind = f"{vec_kind}_TRUSTED"
                if vec_kind is not None:
                    zero = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                    one = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[1], result=one))
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind=vec_kind, args=[seq_arg, acc_val], result=pair)
                    )
                    sum_hint = (
                        "float"
                        if vec_kind is not None and "FLOAT" in vec_kind
                        else "int"
                    )
                    sum_val = MoltValue(self.next_var(), type_hint=sum_hint)
                    self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=sum_val))
                    ok_val = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="INDEX", args=[pair, one], result=ok_val))
                    self.emit(
                        MoltOp(kind="IF", args=[ok_val], result=MoltValue("none"))
                    )
                    self._store_local_value(acc_name, sum_val)
                    self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                    self._emit_for_loop(node, iterable, loop_break_flag=break_name)
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    if break_name is not None:
                        self._emit_loop_orelse(break_name, node.orelse)
                    return None

        self._emit_for_loop(node, iterable, loop_break_flag=break_name)
        if break_name is not None:
            self._emit_loop_orelse(break_name, node.orelse)
        return None

    def visit_While(self, node: ast.While) -> None:
        break_name = None
        if node.orelse:
            while True:
                candidate = f"__molt_while_break_{self.loop_break_counter}"
                self.loop_break_counter += 1
                if (
                    candidate not in self.locals
                    and candidate not in self.globals
                    and candidate not in self.boxed_locals
                ):
                    break_name = candidate
                    break
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            if (
                self.current_func_name == "molt_main"
                and hasattr(self, "module_obj")
                and self.module_obj is not None
            ):
                # At module scope, store the break flag in the module dict
                # instead of a boxed local.  Boxed locals (list cells)
                # suffer from SSA phi corruption in sequential while-else
                # blocks because Cranelift resolves the cell pointer to the
                # entry-block 0-init instead of the actual list_new output.
                # The module dict is on the heap and immune to SSA issues.
                self._emit_module_attr_set_on(self.module_obj, break_name, break_init)
                self.module_global_mutations.add(break_name)
                self.locals.pop(break_name, None)
            else:
                self._store_local_value(break_name, break_init)
                if not self.is_async():
                    self._box_local(break_name)
        counted = (
            None
            if break_name is not None
            or self.current_func_name == "molt_main"
            # In a class body the loop index name must persist into the class
            # namespace; the counted-while fold elides it.  (P0 #50.)
            or self._class_ns_stack
            else self._match_counted_while(node)
        )
        if counted is not None and not self.is_async():
            index_name, bound, body = counted
            bytearray_fill = self._match_bytearray_fill_counted_while(
                index_name, bound, body
            )
            if bytearray_fill is not None:
                container_name, start, stop, fill = bytearray_fill
                container = self._load_local_value(container_name)
                if container is None:
                    raise NotImplementedError("bytearray fill target not initialized")
                start_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[start], result=start_val))
                stop_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[stop], result=stop_val))
                fill_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[fill], result=fill_val))
                self.emit(
                    MoltOp(
                        kind="BYTEARRAY_FILL_RANGE",
                        args=[container, start_val, stop_val, fill_val],
                        result=MoltValue("none"),
                    )
                )
                idx_res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[stop], result=idx_res))
                self._store_local_value(index_name, idx_res)
                return None
            acc_name = self._match_counted_while_sum(index_name, body)
            if acc_name is not None:
                start_val = self._load_local_value(index_name)
                if start_val is None:
                    start_const = 0
                else:
                    start_const = self.const_ints.get(start_val.name)
                acc_val = self._load_local_value(acc_name)
                acc_const = None
                if acc_val is not None:
                    acc_const = self.const_ints.get(acc_val.name)
                if start_const is not None and acc_const is not None:
                    # Guard the empty-loop case: when start_const >= bound the
                    # loop runs zero times, so the accumulator is unchanged. The
                    # arithmetic-series closed form below assumes >=1 iteration;
                    # without this guard span goes negative and the fold emits a
                    # silently-wrong sum (e.g. start=10,bound=5 -> -35 instead of
                    # 0). Mirrors the already-correct const_inc fast path and the
                    # final_index guard just below.
                    if start_const < bound:
                        span = bound - start_const
                        sum_val = span * (start_const + bound - 1) // 2
                    else:
                        sum_val = 0
                    final_val = acc_const + sum_val
                    acc_res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[final_val], result=acc_res))
                    self._store_local_value(acc_name, acc_res)
                    final_index = bound if start_const < bound else start_const
                    idx_res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[final_index], result=idx_res))
                    self._store_local_value(index_name, idx_res)
                    return None
            const_inc = self._match_counted_while_const_increment(body)
            if const_inc is not None:
                acc_name, delta = const_inc
                start_val = self._load_local_value(index_name)
                if start_val is None:
                    start_const = 0
                else:
                    start_const = self.const_ints.get(start_val.name)
                acc_val = self._load_local_value(acc_name)
                acc_const = None
                if acc_val is not None:
                    acc_const = self.const_ints.get(acc_val.name)
                if start_const is not None and acc_const is not None:
                    if start_const < bound:
                        span = bound - start_const
                    else:
                        span = 0
                    final_val = acc_const + span * delta
                    acc_res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[final_val], result=acc_res))
                    self._store_local_value(acc_name, acc_res)
                    final_index = bound if start_const < bound else start_const
                    idx_res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[final_index], result=idx_res))
                    self._store_local_value(index_name, idx_res)
                    return None
            assigned = self._collect_assigned_names(node.body)
            self._prepare_mutable_control_flow_bindings(assigned)
            self._emit_counted_while(index_name, bound, body)
            return None
        assigned = self._collect_assigned_names(node.body)
        assigned |= set(self._collect_namedexpr_names(node.test))
        self._prepare_mutable_control_flow_bindings(assigned)
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_loop_body() -> None:
            self._push_loop_static_class_refs(node.body)
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            cond = self.visit(node.test)
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            self.control_flow_depth += 1
            try:
                body_terminated = self._visit_loop_body(
                    node.body, None, loop_break_flag=break_name
                )
            finally:
                self.control_flow_depth -= 1
            if not body_terminated:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._pop_loop_static_class_refs()

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
            # Re-evict module-backed mutation names (same fix as below)
            if self.current_func_name == "molt_main":
                for name in assigned:
                    if name in self.module_global_mutations:
                        self.locals.pop(name, None)
            if break_name is not None:
                self._emit_loop_orelse(break_name, node.orelse)
            return None

        emit_loop_body()
        # Re-evict module-backed mutation names from self.locals.
        # The loop body may have re-added them via _store_local_value,
        # but post-loop code must read them via module_get_attr to see
        # the correct value (the loop body may not have executed).
        if self.current_func_name == "molt_main":
            for name in assigned:
                if name in self.module_global_mutations:
                    self.locals.pop(name, None)
        if break_name is not None:
            self._emit_loop_orelse(break_name, node.orelse)
        return None

    def visit_Try(self, node: ast.Try) -> None:
        if not node.handlers and not node.finalbody:
            self._bridge_fallback(
                node,
                "try without except",
                impact="high",
                alternative="add an except handler or a finally block",
                detail="try without except/finally is not supported yet",
            )
            return None
        if node.orelse and not node.handlers:
            self._bridge_fallback(
                node,
                "try/finally with else",
                impact="high",
                alternative="move the else body into the try",
                detail="try/else requires an except handler",
            )
            return None
        assigned: set[str] = set()
        if not self.is_async() and self.current_func_name != "molt_main":
            assigned = self._collect_assigned_names([node])
            for name in sorted(assigned):
                if name not in self.scope_assigned or name in self.closure_locals:
                    self._box_local(name)
        elif not self.is_async() and self.current_func_name == "molt_main":
            assigned = self._collect_assigned_names([node])
            self._prepare_mutable_control_flow_bindings(assigned)
        prior_terminated = self.block_terminated
        self.block_terminated = False
        self.control_flow_depth += 1
        # try/except: snapshot unbound_check_names — the body may
        # raise before any internal assignment, so post-block code
        # cannot rely on body-internal discards.  See _visit_loop_body
        # for the full rationale.
        unbound_snapshot_try = set(self.unbound_check_names)

        needs_context_unwind = self._block_needs_context_unwind(node.body)
        ctx_mark: MoltValue | None = None
        ctx_mark_offset = None
        if needs_context_unwind:
            ctx_mark = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONTEXT_DEPTH", args=[], result=ctx_mark))
        if needs_context_unwind and self.is_async():
            ctx_name = f"__ctx_mark_{len(self.async_locals)}"
            ctx_mark_offset = self._async_local_offset(ctx_name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", ctx_mark_offset, ctx_mark],
                    result=MoltValue("none"),
                )
            )
        scope = TryScope(
            ctx_mark=ctx_mark,
            finalbody=node.finalbody,
            ctx_mark_offset=ctx_mark_offset,
            needs_context_unwind=needs_context_unwind,
        )
        self.try_scopes.append(scope)

        if node.handlers and not node.finalbody and not self.is_async():
            self._emit_sync_try_except_split(
                node,
                scope,
                unbound_snapshot_try,
                prior_terminated,
            )
            self._evict_module_control_flow_bindings(assigned)
            return None

        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_exc_label = self.next_label()
        try_join_label = self.next_label()
        try_done_label = self.next_label()
        scope.handler_label = try_exc_label
        scope.done_label = try_done_label
        self.try_end_labels.append(try_exc_label)
        self.emit(
            MoltOp(
                kind="TRY_START",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self._visit_block(node.body)
        body_terminated = self.block_terminated
        self.block_terminated = False
        if not body_terminated:
            self.emit(
                MoltOp(
                    kind="TRY_END",
                    args=[try_exc_label],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(kind="JUMP", args=[try_join_label], result=MoltValue("none"))
            )
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="TRY_END",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.try_end_labels.pop()
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)
        self.try_handler_scopes.append(scope)

        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_join_label],
                result=MoltValue("none"),
            )
        )
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        pending_observer_kind = (
            "EXCEPTION_LAST_PENDING"
            if node.handlers
            else "EXCEPTION_FINALLY_PENDING_OBSERVER"
        )
        self.emit(MoltOp(kind=pending_observer_kind, args=[], result=exc_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
        pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))

        self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
        self._emit_context_unwind_to(scope, exc_val)

        def emit_handlers(handlers: list[ast.ExceptHandler]) -> None:
            if not handlers:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
                return
            handler = handlers[0]
            match_val = self._emit_exception_match(handler, exc_val)
            self.emit(MoltOp(kind="IF", args=[match_val], result=MoltValue("none")))
            exc_slot_offset = None
            if self.is_async():
                exc_slot_name = f"__exc_handler_{len(self.async_locals)}"
                exc_slot_offset = self._async_local_offset(exc_slot_name)
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", exc_slot_offset, exc_val],
                        result=MoltValue("none"),
                    )
                )
            if handler.name:
                if self.current_func_name == "molt_main":
                    self.module_global_mutations.add(handler.name)
                self._store_local_value(handler.name, exc_val)
            exc_entry = ActiveException(
                value=exc_val,
                slot=exc_slot_offset,
                handler_name=handler.name,
                is_handler=True,
                handler_try_depth=len(self.try_end_labels),
            )
            self.active_exceptions.append(exc_entry)
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[exc_val],
                    result=MoltValue("none"),
                )
            )
            self._emit_guarded_body(handler.body, exc_entry)
            handler_terminated = self.block_terminated
            if not handler_terminated:
                self._emit_exception_handler_exit_cleanup(exc_entry)
            self.active_exceptions.pop()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            if len(handlers) > 1:
                emit_handlers(handlers[1:])
            else:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        if node.handlers:
            emit_handlers(node.handlers)

        if node.finalbody:
            if node.handlers:
                final_exc = MoltValue(self.next_var(), type_hint="exception")
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                        args=[],
                        result=final_exc,
                    )
                )
            else:
                final_exc = exc_val
            final_slot = None
            if self.is_async():
                final_slot = self._async_local_offset(
                    f"__final_exc_{len(self.async_locals)}"
                )
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", final_slot, final_exc],
                        result=MoltValue("none"),
                    )
                )
            final_entry = ActiveException(value=final_exc, slot=final_slot)
            self.active_exceptions.append(final_entry)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[final_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self._emit_finalbody(node.finalbody, final_entry, popped_scopes=0)
            none_after = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
            exc_after = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                    args=[],
                    result=exc_after,
                )
            )
            is_none_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after)
            )
            self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
            restored_exc = self._active_exception_value(final_entry)
            is_restore_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS", args=[restored_exc, none_after], result=is_restore_none
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_restore_none], result=MoltValue("none"))
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[restored_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            # Finally raised a new exception -- chain __context__ to the
            # original exception so it is not silently lost (CPython 3.12+).
            _orig_exc = self._active_exception_value(final_entry)
            _orig_is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[_orig_exc, none_after], result=_orig_is_none)
            )
            self.emit(MoltOp(kind="IF", args=[_orig_is_none], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[exc_after, "__context__", _orig_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[exc_after],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.active_exceptions.pop()

        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        if node.orelse:
            if node.finalbody:
                with self._suppress_check_exception(emit_on_exit=False):
                    self._emit_guarded_body(node.orelse, None)
            else:
                self._emit_guarded_body(node.orelse, None)
        if node.finalbody:
            else_final_exc = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                    args=[],
                    result=else_final_exc,
                )
            )
            else_final_slot = None
            if self.is_async():
                else_final_slot = self._async_local_offset(
                    f"__final_else_exc_{len(self.async_locals)}"
                )
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", else_final_slot, else_final_exc],
                        result=MoltValue("none"),
                    )
                )
            else_final_entry = ActiveException(
                value=else_final_exc, slot=else_final_slot
            )
            self.active_exceptions.append(else_final_entry)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[else_final_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self._emit_finalbody(node.finalbody, else_final_entry, popped_scopes=0)
            none_after = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
            else_after = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                    args=[],
                    result=else_after,
                )
            )
            is_none_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[else_after, none_after], result=is_none_after)
            )
            self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
            restored_exc = self._active_exception_value(else_final_entry)
            is_restore_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS", args=[restored_exc, none_after], result=is_restore_none
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_restore_none], result=MoltValue("none"))
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[restored_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            # Finally raised a new exception -- chain __context__ to the
            # original exception so it is not silently lost (CPython 3.12+).
            _orig_exc = self._active_exception_value(else_final_entry)
            _orig_is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[_orig_exc, none_after], result=_orig_is_none)
            )
            self.emit(MoltOp(kind="IF", args=[_orig_is_none], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[else_after, "__context__", _orig_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[else_after],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.active_exceptions.pop()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_done_label],
                result=MoltValue("none"),
            )
        )
        self.try_handler_scopes.pop()
        self.try_suppress_depth = prior_suppress
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True)
        self.try_scopes.pop()
        self.unbound_check_names = unbound_snapshot_try
        self.control_flow_depth -= 1
        self.block_terminated = prior_terminated
        self._evict_module_control_flow_bindings(assigned)
        return None

    def visit_TryStar(self, node: ast.TryStar) -> None:
        if not node.handlers and not node.finalbody:
            self._bridge_fallback(
                node,
                "try* without except",
                impact="high",
                alternative="add an except* handler or a finally block",
                detail="try* without except*/finally is not supported yet",
            )
            return None
        if node.orelse and not node.handlers:
            self._bridge_fallback(
                node,
                "try*/finally with else",
                impact="high",
                alternative="move the else body into the try*",
                detail="try*/else requires an except* handler",
            )
            return None
        if not self.is_async():
            assigned = self._collect_assigned_names([node])
            for name in sorted(assigned):
                if name not in self.scope_assigned or name in self.closure_locals:
                    self._box_local(name)
        prior_terminated = self.block_terminated
        self.block_terminated = False
        self.control_flow_depth += 1
        # try/except*: snapshot unbound_check_names — see visit_Try.
        unbound_snapshot_try_star = set(self.unbound_check_names)

        needs_context_unwind = self._block_needs_context_unwind(node.body)
        ctx_mark: MoltValue | None = None
        ctx_mark_offset = None
        if needs_context_unwind:
            ctx_mark = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONTEXT_DEPTH", args=[], result=ctx_mark))
        if needs_context_unwind and self.is_async():
            ctx_name = f"__ctx_star_{len(self.async_locals)}"
            ctx_mark_offset = self._async_local_offset(ctx_name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", ctx_mark_offset, ctx_mark],
                    result=MoltValue("none"),
                )
            )
        scope = TryScope(
            ctx_mark=ctx_mark,
            finalbody=node.finalbody,
            ctx_mark_offset=ctx_mark_offset,
            needs_context_unwind=needs_context_unwind,
        )
        self.try_scopes.append(scope)

        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_exc_label = self.next_label()
        try_done_label = self.next_label()
        scope.handler_label = try_exc_label
        scope.done_label = try_done_label
        self.try_end_labels.append(try_exc_label)
        self.emit(
            MoltOp(
                kind="TRY_START",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self._visit_block(node.body)
        self.emit(MoltOp(kind="JUMP", args=[try_done_label], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="TRY_END",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.try_end_labels.pop()
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)
        self.try_handler_scopes.append(scope)

        exc_val = MoltValue(self.next_var(), type_hint="exception")
        pending_observer_kind = (
            "EXCEPTION_LAST_PENDING"
            if node.handlers
            else "EXCEPTION_FINALLY_PENDING_OBSERVER"
        )
        self.emit(MoltOp(kind=pending_observer_kind, args=[], result=exc_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
        pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))

        self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
        self._emit_context_unwind_to(scope, exc_val)
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))

        rest_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[exc_val], result=rest_cell))
        raised_list = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=raised_list))
        rest_slot = None
        raised_slot = None
        if self.is_async():
            rest_slot = self._async_local_offset(
                f"__try_star_rest_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", rest_slot, rest_cell],
                    result=MoltValue("none"),
                )
            )
            raised_slot = self._async_local_offset(
                f"__try_star_raised_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", raised_slot, raised_list],
                    result=MoltValue("none"),
                )
            )

        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))

        def load_rest_cell() -> MoltValue:
            if rest_slot is None or not self.is_async():
                return rest_cell
            res = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", rest_slot], result=res))
            return res

        def load_rest_value() -> MoltValue:
            cell = load_rest_cell()
            res = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=res))
            return res

        def store_rest_value(value: MoltValue) -> None:
            cell = load_rest_cell()
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, zero, value],
                    result=MoltValue("none"),
                )
            )

        def load_raised() -> MoltValue:
            if raised_slot is None or not self.is_async():
                return raised_list
            res = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(kind="LOAD_CLOSURE", args=["self", raised_slot], result=res)
            )
            return res

        for handler in node.handlers:
            rest_cur = load_rest_value()
            rest_is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[rest_cur, none_val], result=rest_is_none))
            has_rest = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[rest_is_none], result=has_rest))
            self.emit(MoltOp(kind="IF", args=[has_rest], result=MoltValue("none")))
            if handler.type is None:
                class_val = self._emit_exception_class("BaseException")
            else:
                class_val = self.visit(handler.type)
            if class_val is None:
                self._bridge_fallback(
                    handler,
                    "except* (unsupported handler)",
                    alternative="use a lowered exception name or tuple",
                    detail="handler expression could not be lowered",
                )
            else:
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(
                    MoltOp(
                        kind="EXCEPTIONGROUP_MATCH",
                        args=[rest_cur, class_val],
                        result=pair,
                    )
                )
                match_val = MoltValue(self.next_var(), type_hint="exception")
                self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=match_val))
                new_rest = MoltValue(self.next_var(), type_hint="exception")
                self.emit(MoltOp(kind="INDEX", args=[pair, one], result=new_rest))
                store_rest_value(new_rest)
                match_is_none = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="IS", args=[match_val, none_val], result=match_is_none)
                )
                has_match = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[match_is_none], result=has_match))
                self.emit(MoltOp(kind="IF", args=[has_match], result=MoltValue("none")))
                exc_slot_offset = None
                if self.is_async():
                    exc_slot_name = f"__exc_star_{len(self.async_locals)}"
                    exc_slot_offset = self._async_local_offset(exc_slot_name)
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", exc_slot_offset, match_val],
                            result=MoltValue("none"),
                        )
                    )
                if handler.name:
                    if self.current_func_name == "molt_main":
                        self.module_global_mutations.add(handler.name)
                    self._store_local_value(handler.name, match_val)
                exc_entry = ActiveException(
                    value=match_val,
                    slot=exc_slot_offset,
                    handler_name=handler.name,
                    is_handler=True,
                    handler_try_depth=len(self.try_end_labels),
                )
                self.active_exceptions.append(exc_entry)
                self.emit(
                    MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none"))
                )
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_CONTEXT_SET",
                        args=[match_val],
                        result=MoltValue("none"),
                    )
                )
                self._emit_guarded_body(handler.body, exc_entry)
                handler_terminated = self.block_terminated
                if not handler_terminated:
                    self._emit_exception_handler_exit_cleanup(exc_entry)
                self.active_exceptions.pop()
                raised_exc = MoltValue(self.next_var(), type_hint="exception")
                self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=raised_exc))
                raised_is_none = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="IS",
                        args=[raised_exc, none_val],
                        result=raised_is_none,
                    )
                )
                has_raised = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[raised_is_none], result=has_raised))
                self.emit(
                    MoltOp(kind="IF", args=[has_raised], result=MoltValue("none"))
                )
                raised_target = load_raised()
                self.emit(
                    MoltOp(
                        kind="LIST_APPEND",
                        args=[raised_target, raised_exc],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        rest_final = load_rest_value()
        raised_final = load_raised()
        raised_len = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[raised_final], result=raised_len))
        len_is_zero = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[raised_len, zero], result=len_is_zero))
        self.emit(MoltOp(kind="IF", args=[len_is_zero], result=MoltValue("none")))
        rest_is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[rest_final, none_val], result=rest_is_none))
        self.emit(MoltOp(kind="IF", args=[rest_is_none], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="EXCEPTION_SET_LAST",
                args=[rest_final],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        rest_is_none_raised = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="IS", args=[rest_final, none_val], result=rest_is_none_raised)
        )
        self.emit(
            MoltOp(
                kind="IF",
                args=[rest_is_none_raised],
                result=MoltValue("none"),
            )
        )
        len_is_one = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[raised_len, one], result=len_is_one))
        self.emit(MoltOp(kind="IF", args=[len_is_one], result=MoltValue("none")))
        only_exc = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="INDEX", args=[raised_final, zero], result=only_exc))
        self.emit(
            MoltOp(
                kind="EXCEPTION_SET_LAST",
                args=[only_exc],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        combined = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTIONGROUP_COMBINE",
                args=[raised_final],
                result=combined,
            )
        )
        self.emit(
            MoltOp(
                kind="EXCEPTION_SET_LAST",
                args=[combined],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LIST_APPEND",
                args=[raised_final, rest_final],
                result=MoltValue("none"),
            )
        )
        combined = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTIONGROUP_COMBINE",
                args=[raised_final],
                result=combined,
            )
        )
        self.emit(
            MoltOp(
                kind="EXCEPTION_SET_LAST",
                args=[combined],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        if node.finalbody:
            final_exc = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                    args=[],
                    result=final_exc,
                )
            )
            final_slot = None
            if self.is_async():
                final_slot = self._async_local_offset(
                    f"__final_star_{len(self.async_locals)}"
                )
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", final_slot, final_exc],
                        result=MoltValue("none"),
                    )
                )
            final_entry = ActiveException(value=final_exc, slot=final_slot)
            self.active_exceptions.append(final_entry)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[final_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self._emit_finalbody(node.finalbody, final_entry, popped_scopes=0)
            none_after = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
            exc_after = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                    args=[],
                    result=exc_after,
                )
            )
            is_none_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after)
            )
            self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
            restored_exc = self._active_exception_value(final_entry)
            is_restore_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS", args=[restored_exc, none_after], result=is_restore_none
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_restore_none], result=MoltValue("none"))
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[restored_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            # Finally raised a new exception -- chain __context__ to the
            # original exception so it is not silently lost (CPython 3.12+).
            _orig_exc = self._active_exception_value(final_entry)
            _orig_is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[_orig_exc, none_after], result=_orig_is_none)
            )
            self.emit(MoltOp(kind="IF", args=[_orig_is_none], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[exc_after, "__context__", _orig_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[exc_after],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.active_exceptions.pop()

        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        if node.orelse:
            if node.finalbody:
                with self._suppress_check_exception(emit_on_exit=False):
                    self._emit_guarded_body(node.orelse, None)
            else:
                self._emit_guarded_body(node.orelse, None)
        if node.finalbody:
            else_final_exc = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                    args=[],
                    result=else_final_exc,
                )
            )
            else_final_slot = None
            if self.is_async():
                else_final_slot = self._async_local_offset(
                    f"__final_star_else_exc_{len(self.async_locals)}"
                )
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", else_final_slot, else_final_exc],
                        result=MoltValue("none"),
                    )
                )
            else_final_entry = ActiveException(
                value=else_final_exc, slot=else_final_slot
            )
            self.active_exceptions.append(else_final_entry)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[else_final_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self._emit_finalbody(node.finalbody, else_final_entry, popped_scopes=0)
            none_after = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
            else_after = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_FINALLY_PENDING_OBSERVER",
                    args=[],
                    result=else_after,
                )
            )
            is_none_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[else_after, none_after], result=is_none_after)
            )
            self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
            restored_exc = self._active_exception_value(else_final_entry)
            is_restore_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS", args=[restored_exc, none_after], result=is_restore_none
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_restore_none], result=MoltValue("none"))
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[restored_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            # Finally raised a new exception -- chain __context__ to the
            # original exception so it is not silently lost (CPython 3.12+).
            _orig_exc = self._active_exception_value(else_final_entry)
            _orig_is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[_orig_exc, none_after], result=_orig_is_none)
            )
            self.emit(MoltOp(kind="IF", args=[_orig_is_none], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[else_after, "__context__", _orig_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[else_after],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.active_exceptions.pop()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_done_label],
                result=MoltValue("none"),
            )
        )
        self.try_handler_scopes.pop()
        self.try_suppress_depth = prior_suppress
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True)
        self.try_scopes.pop()
        self.unbound_check_names = unbound_snapshot_try_star
        self.control_flow_depth -= 1
        self.block_terminated = prior_terminated
        return None

    def visit_Raise(self, node: ast.Raise) -> None:
        self.block_terminated = True
        clear_handlers = (
            self.current_func_name == "molt_main"
            and not self.try_end_labels
            and self.try_suppress_depth is None
        )
        if self.try_suppress_depth is None:
            should_exit = True
        else:
            should_exit = len(self.try_end_labels) > self.try_suppress_depth

        def emit_raise_or_defer(exc: MoltValue) -> None:
            if should_exit:
                self.emit(MoltOp(kind="RAISE", args=[exc], result=MoltValue("none")))
            else:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_SET_LAST",
                        args=[exc],
                        result=MoltValue("none"),
                    )
                )

        def emit_exception_value(
            expr: ast.expr, *, allow_none: bool, context: str
        ) -> MoltValue | None:
            if allow_none and isinstance(expr, ast.Constant) and expr.value is None:
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                return none_val
            exc_val = self.visit(expr)
            if exc_val is None:
                self._bridge_fallback(
                    node,
                    f"{context} (unsupported expression)",
                    impact="high",
                    alternative=f"{context} a named exception with a string literal",
                    detail="unsupported raise expression form",
                )
                return None
            return exc_val

        if node.exc is None:
            if self.active_exceptions:
                if clear_handlers:
                    self.emit(
                        MoltOp(
                            kind="EXCEPTION_STACK_CLEAR",
                            args=[],
                            result=MoltValue("none"),
                        )
                    )
                exc_val = self._active_exception_value(self.active_exceptions[-1])
                # Bare `raise` escaping `except ... as NAME` deletes NAME too
                # (the value is already captured in `exc_val`).
                self._emit_escaping_handler_name_deletes()
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                is_none = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
                self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
                err_val = self._emit_exception_new(
                    "RuntimeError", "No active exception to reraise"
                )
                emit_raise_or_defer(err_val)
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                emit_raise_or_defer(exc_val)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                if should_exit:
                    self._emit_raise_exit()
                return None
            if clear_handlers:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_STACK_CLEAR",
                        args=[],
                        result=MoltValue("none"),
                    )
                )
            exc_val = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            err_val = self._emit_exception_new(
                "RuntimeError", "No active exception to reraise"
            )
            emit_raise_or_defer(err_val)
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            emit_raise_or_defer(exc_val)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            if should_exit:
                self._emit_raise_exit()
            return None

        exc_val = emit_exception_value(node.exc, allow_none=False, context="raise")
        if exc_val is None:
            return None
        if clear_handlers:
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_CLEAR",
                    args=[],
                    result=MoltValue("none"),
                )
            )
        if self.active_exceptions:
            context_val = self._active_exception_value(self.active_exceptions[-1])
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[exc_val, "__context__", context_val],
                    result=MoltValue("none"),
                )
            )
        if node.cause is not None:
            cause_val = emit_exception_value(
                node.cause, allow_none=True, context="raise cause"
            )
            if cause_val is None:
                return None
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_CAUSE",
                    args=[exc_val, cause_val],
                    result=MoltValue("none"),
                )
            )
        # A `raise` escaping an `except ... as NAME` handler must delete NAME
        # (CPython's implicit `finally: del NAME` runs on the exception-escape
        # edge too). Context/cause are already captured into `exc_val` above, so
        # dropping the bindings here cannot disturb the in-flight exception.
        self._emit_escaping_handler_name_deletes()
        emit_raise_or_defer(exc_val)
        if should_exit:
            self._emit_raise_exit()
        return None

    def visit_Assert(self, node: ast.Assert) -> None:
        test_val = self.visit(node.test)
        if test_val is None:
            self._bridge_fallback(
                node,
                "assert test expression (unsupported form)",
                impact="medium",
                alternative="assert supported expressions",
                detail="unsupported assert test expression",
            )
            return None

        test_false = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[test_val], result=test_false))
        self.emit(MoltOp(kind="IF", args=[test_false], result=MoltValue("none")))
        if node.msg is None:
            exc_val = self._emit_exception_new_from_args("AssertionError", [])
        else:
            msg_val = self.visit(node.msg)
            if msg_val is None:
                self._bridge_fallback(
                    node,
                    "assert message expression (unsupported form)",
                    impact="low",
                    alternative="assert with supported message expression",
                    detail="unsupported assert message expression",
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                return None
            exc_val = self._emit_exception_new_from_args("AssertionError", [msg_val])
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return None

    def visit_Break(self, node: ast.Break) -> None:
        if self.finally_depth > 0:
            self._emit_syntax_warning(node, "'break' in a 'finally' block")
        if not self.loop_break_flags:
            raise SyntaxError(f"'break' outside loop (line {node.lineno})")
        del node
        if self.loop_break_flags:
            break_slot = self.loop_break_flags[-1]
            if break_slot is not None:
                break_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=break_val))
                if isinstance(break_slot, int):
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", break_slot, break_val],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    self._store_local_value(break_slot, break_val)
        self._emit_exception_handler_exit_cleanup()
        popped_labels = self._emit_loop_unwind()
        try:
            self.emit(MoltOp(kind="LOOP_BREAK", args=[], result=MoltValue("none")))
        finally:
            self._restore_control_flow_unwind_labels(popped_labels)
        self.block_terminated = True
        return None

    def visit_Continue(self, node: ast.Continue) -> None:
        if self.finally_depth > 0:
            self._emit_syntax_warning(node, "'continue' in a 'finally' block")
        if not self.loop_break_flags:
            raise SyntaxError(f"'continue' not properly in loop (line {node.lineno})")
        del node
        self._emit_exception_handler_exit_cleanup()
        popped_labels = self._emit_loop_unwind()
        try:
            if self.async_index_loop_stack:
                idx_slot = self.async_index_loop_stack[-1]
                idx_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="LOAD_CLOSURE",
                        args=["self", idx_slot],
                        result=idx_val,
                    )
                )
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx_val, one], result=next_idx))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", idx_slot, next_idx],
                        result=MoltValue("none"),
                    )
                )
            elif self.range_loop_stack:
                idx, step = self.range_loop_stack[-1]
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
                self.emit(
                    MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=next_idx)
                )
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        finally:
            self._restore_control_flow_unwind_labels(popped_labels)
        self.block_terminated = True
        return None

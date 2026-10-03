"""ControlFlowStatementVisitorMixin: synchronous control-flow statements.

Move-only extraction from frontend/__init__.py. Covers if/with/loop/try/raise,
assert, break, and continue lowering. Async control flow lives in
AsyncGenVisitorMixin.
"""

from __future__ import annotations

import ast
from functools import wraps
from typing import Any, Callable, Concatenate, ParamSpec

from molt.frontend._mixin_base import GeneratorMixinBase
from molt.compiler_analysis.static_truth import static_expression_result
from molt.frontend._types import (
    ActiveException,
    ExactClassFact,
    MoltOp,
    MoltValue,
    ScratchCell,
    SyncContextExit,
    TryScope,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection


_VisitorArgs = ParamSpec("_VisitorArgs")


def _with_module_provenance_loop_flow(
    visitor: Callable[Concatenate[Any, _VisitorArgs], None],
) -> Callable[Concatenate[Any, _VisitorArgs], None]:
    @wraps(visitor)
    def wrapped(
        self: Any, *args: _VisitorArgs.args, **kwargs: _VisitorArgs.kwargs
    ) -> None:
        flow = self._begin_module_provenance_flow(record_exception_prefixes=True)
        try:
            return visitor(self, *args, **kwargs)
        finally:
            self._finish_module_provenance_flow(flow)

    return wrapped


class ControlFlowStatementVisitorMixin(GeneratorMixinBase):
    def _clear_exact_bindings(self, names: set[str]) -> None:
        for name in names:
            self.exact_locals.pop(name, None)

    def _join_exact_binding_states(
        self,
        left: dict[str, ExactClassFact],
        left_token: int,
        right: dict[str, ExactClassFact],
        right_token: int,
    ) -> dict[str, ExactClassFact]:
        token = self._advance_exact_class_token()
        return {
            name: ExactClassFact(fact.class_id, token)
            for name, fact in left.items()
            if (other := right.get(name)) is not None
            and fact.token == left_token
            and other.token == right_token
            and other.class_id == fact.class_id
        }

    def visit_If(self, node: ast.If) -> None:
        decision = static_expression_result(node.test, **self._static_truth_kwargs())
        # Result knowledge is not permission to erase condition evaluation.
        # Required conditions keep the normal IF last-use/ownership boundary;
        # no ad-hoc drop of a possibly borrowed result or extra truth callback.
        if decision.truth is not None:
            if not decision.evaluation_required:
                self._emit_static_if_live_branch(
                    node.body if decision.truth else node.orelse
                )
                return None
            # Keep the original condition and normal ownership lowering while
            # sharing successor reachability with import/metadata consumers.
            # Copy only this control node; never mutate the analyzed AST.
            node = ast.copy_location(
                ast.If(
                    test=node.test,
                    body=node.body if decision.truth else [],
                    orelse=[] if decision.truth else node.orelse,
                ),
                node,
            )
        assigned = self._collect_assigned_names(node.body + node.orelse)
        if not self.is_async():
            assigned |= set(self._collect_namedexpr_names(node.test))
            if self.current_func_name == "molt_main":
                self._prepare_mutable_control_flow_bindings(assigned)
            else:
                for name in sorted(assigned):
                    if name not in self.scope_assigned or name in self.closure_locals:
                        self._box_local(name)
        cond = self._emit_condition(node.test)
        self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
        exact_entry = self._snapshot_live_exact_bindings()
        exact_entry_token = self.exact_class_token
        self.control_flow_depth += 1
        # Snapshot unbound_check_names on flow entry so per-branch
        # discards don't leak into the post-merge state — only names
        # discarded in EVERY path can stay discarded after the merge.
        unbound_snapshot = set(self.unbound_check_names)
        provenance_snapshot = dict(self.imported_module_provenance)
        provenance_flow = self._begin_module_provenance_flow(
            record_exception_prefixes=False
        )
        then_provenance = provenance_snapshot
        else_provenance = provenance_snapshot
        try:
            self._visit_block(node.body)
            then_exact = self._snapshot_live_exact_bindings()
            then_exact_token = self.exact_class_token
            then_unbound = set(self.unbound_check_names)
            then_provenance = dict(self.imported_module_provenance)
            if node.orelse:
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.unbound_check_names = set(unbound_snapshot)
                self.imported_module_provenance = dict(provenance_snapshot)
                self.exact_locals = dict(exact_entry)
                self.exact_class_token = exact_entry_token
                self._visit_block(node.orelse)
                else_exact = self._snapshot_live_exact_bindings()
                else_exact_token = self.exact_class_token
                else_unbound = set(self.unbound_check_names)
                else_provenance = dict(self.imported_module_provenance)
                # Names discarded in BOTH branches stay discarded;
                # names discarded in only one go back to checked.
                self.unbound_check_names = then_unbound | else_unbound
            else:
                # if-only: the else path is implicit (no statements),
                # so it can't add discards.  Restore to snapshot —
                # any discards in `body` may not have happened.
                self.unbound_check_names = unbound_snapshot
                else_exact = exact_entry
                else_exact_token = exact_entry_token
        finally:
            self.control_flow_depth -= 1
        self._finish_module_provenance_flow(
            provenance_flow,
            normal_paths=(then_provenance, else_provenance),
        )
        # The join creates one fresh authority for facts proven on every path.
        self.exact_locals = self._join_exact_binding_states(
            then_exact,
            then_exact_token,
            else_exact,
            else_exact_token,
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        # Evict module_global_mutations names from the locals/globals cache so
        # subsequent bare-name loads go through MODULE_GET_GLOBAL instead of
        # reusing a value that was only assigned in one branch.
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

        ctx_cell = self._new_scratch_cell(ctx_val, type_hint=ctx_val.type_hint)
        action = SyncContextExit(ctx_cell)
        enter_val = self._emit_context_entry(action, ctx_val)
        self._emit_context_body(node, enter_val, action)
        return None

    def visit_For(self, node: ast.For) -> None:
        return self._visit_for(node)

    @_with_module_provenance_loop_flow
    def _visit_for(self, node: ast.For, *, iterator: MoltValue | None = None) -> None:
        self._prepare_exact_class_loop_entry(node.body)
        exact_assigned = self._collect_assigned_names(node.body + node.orelse)
        exact_assigned.update(self._collect_target_names(node.target))
        break_name: ScratchCell | None = None
        if node.orelse:
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            break_name = self._new_scratch_cell(break_init, type_hint="bool")
        target_names = self._collect_target_names(node.target)
        if not target_names:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Only name/tuple/list for targets are supported",
            )
        for name in target_names:
            self.exact_locals.pop(name, None)
        assigned = self._collect_assigned_names(node.body)
        assigned.update(target_names)
        self._prepare_mutable_control_flow_bindings(assigned)
        if iterator is not None:
            # A generator expression acquired its outer iterator in the
            # enclosing frame. Consume that value, never re-evaluate or apply
            # source-expression optimizations to the original iterable here.
            self._emit_iter_loop(node, iterator, loop_break_flag=break_name)
            if break_name is not None:
                self._emit_loop_orelse(break_name, node.orelse)
            self._clear_exact_bindings(exact_assigned)
            return None
        # A fused loop (the VEC_* reductions, the split/count kernel) runs
        # items in a runtime operation, which admits them only when they
        # provably run no Python code, and then binds the loop target once, to
        # the last item's value. That single store stands in for one store per
        # item only in a function body: a class body's namespace may be any
        # mapping whose __setitem__ observes every store (P0 #50), and a
        # module's loop target is a module global, which the reductions do not
        # read. A fused path consumes the loop's own, single evaluation of its
        # iterable.
        fuse = (
            not self.is_async()
            and not self._class_ns_stack
            and self.current_func_name != "molt_main"
        )
        if fuse and self._emit_split_dict_increment_for_loop(
            node, loop_break_flag=break_name
        ):
            if break_name is not None:
                self._emit_loop_orelse(break_name, node.orelse)
            self._clear_exact_bindings(exact_assigned)
            return None
        iterable: MoltValue | None = None
        range_args = self._parse_range_call(node.iter)
        if range_args is None:
            iterable = self.visit(node.iter)
            if iterable is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported iterable in for loop"
                )
        else:
            # The bounds are range()'s exact ints: the loop counts over them.
            start, stop, step = range_args
            self._emit_range_loop(node, start, stop, step, loop_break_flag=break_name)
        if iterable is not None and not (
            fuse
            and self._emit_iterable_vector_reduction(
                node, iterable, loop_break_flag=break_name
            )
        ):
            self._emit_for_loop(node, iterable, loop_break_flag=break_name)
        if break_name is not None:
            self._emit_loop_orelse(break_name, node.orelse)
        self._clear_exact_bindings(exact_assigned)
        return None

    @_with_module_provenance_loop_flow
    def visit_While(self, node: ast.While) -> None:
        self._prepare_exact_class_loop_entry(node.body)
        exact_assigned = self._collect_assigned_names(node.body + node.orelse)
        exact_assigned |= set(self._collect_namedexpr_names(node.test))
        break_name: ScratchCell | None = None
        if node.orelse:
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            break_name = self._new_scratch_cell(break_init, type_hint="bool")
        counted = (
            None
            if break_name is not None
            or self.current_func_name == "molt_main"
            # In a class body the loop index name must persist into the class
            # namespace; the counted-while fold elides it.  (P0 #50.)
            or self._class_ns_stack
            or self.is_async()
            else self._match_counted_while(node)
        )
        if counted is not None:
            index, bound, body = counted
            assigned = self._collect_assigned_names(node.body)
            assigned |= set(self._collect_namedexpr_names(node.test))
            self._prepare_mutable_control_flow_bindings(assigned)
            bytearray_fill = self._match_bytearray_fill_counted_while(
                index, bound, body
            )
            if bytearray_fill is not None:
                container_read, fill = bytearray_fill
                self._emit_bytearray_fill_while(
                    index,
                    bound,
                    container_read,
                    fill,
                    lambda: self._emit_while_loop(node, None, assigned),
                )
                self._clear_exact_bindings(exact_assigned)
                return None
            # The test's read of the index is the loop's first read of it.
            index_value = self._load_local_value(
                index.id,
                binding_invalidated=self._expression_has_invalidated_binding(index),
            )
            if index_value is not None:
                # LOOP_INDEX_START tells the representation plan its operand is
                # an int, so only an exact int start may count; any other start
                # runs the ordinary loop, which re-reads the index itself.
                exact_start = self._emit_is_exact_builtin(index_value, "int")
                self.emit(
                    MoltOp(kind="IF", args=[exact_start], result=MoltValue("none"))
                )
                self._emit_counted_while(index.id, index_value, bound, body)
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self._emit_while_loop(node, None, assigned)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                self._clear_exact_bindings(exact_assigned)
                return None
        assigned = self._collect_assigned_names(node.body)
        assigned |= set(self._collect_namedexpr_names(node.test))
        self._prepare_mutable_control_flow_bindings(assigned)
        self._emit_while_loop(node, break_name, assigned)
        self._clear_exact_bindings(exact_assigned)
        return None

    def _emit_while_loop(
        self,
        node: ast.While,
        break_name: ScratchCell | None,
        assigned: set[str],
    ) -> None:
        """The ordinary ``while`` loop, its ``else`` clause included, after the
        caller prepared the loop's mutable bindings."""
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_loop_body() -> None:
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            cond = self._emit_condition(node.test)
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            self.control_flow_depth += 1
            try:
                loop = self._visit_loop_body(
                    node.body, None, loop_break_flag=break_name
                )
            finally:
                self.control_flow_depth -= 1
            if loop.needs_latch:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self._emit_loop_exit(loop)

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
        else:
            emit_loop_body()
        # Re-evict module-backed mutation names from self.locals.
        # The loop body may have re-added them via _store_local_value,
        # but post-loop code must read them via module_get_global to see
        # the correct value while preserving NameError-on-miss semantics
        # (the loop body may not have executed).
        if self.current_func_name == "molt_main":
            for name in assigned:
                if name in self.module_global_mutations:
                    self.locals.pop(name, None)
        if break_name is not None:
            self._emit_loop_orelse(break_name, node.orelse)

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
        exact_assigned = self._collect_assigned_names([node])
        provenance_flow = self._begin_module_provenance_flow(
            record_exception_prefixes=True
        )
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

        scope = TryScope(
            finalbody=node.finalbody, lexical_loops=tuple(self.loop_scopes)
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
            self._finish_module_provenance_flow(provenance_flow)
            self._clear_exact_bindings(exact_assigned)
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
        body_terminated = self._visit_block(node.body)
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
                exc_slot_offset = self._new_async_internal_slot()
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
                self._clear_import_binding_origin(handler.name)
                self._store_local_value(handler.name, exc_val)
            exc_entry = ActiveException(
                value=exc_val,
                slot=exc_slot_offset,
                handler_name=handler.name,
                is_handler=True,
                scope=scope,
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
            self._emit_guarded_body(handler.body)
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
                final_slot = self._new_async_internal_slot()
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", final_slot, final_exc],
                        result=MoltValue("none"),
                    )
                )
            final_entry = ActiveException(value=final_exc, scope=scope, slot=final_slot)
            self.active_exceptions.append(final_entry)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[final_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self._emit_finalbody(scope)
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
                    self._emit_guarded_body(node.orelse)
            else:
                self._emit_guarded_body(node.orelse)
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
                else_final_slot = self._new_async_internal_slot()
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", else_final_slot, else_final_exc],
                        result=MoltValue("none"),
                    )
                )
            else_final_entry = ActiveException(
                value=else_final_exc, scope=scope, slot=else_final_slot
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
            self._emit_finalbody(scope)
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
        self._emit_raise_if_pending()
        self.try_scopes.pop()
        self.unbound_check_names = unbound_snapshot_try
        self.control_flow_depth -= 1
        self.block_terminated = prior_terminated
        self._evict_module_control_flow_bindings(assigned)
        self._finish_module_provenance_flow(provenance_flow)
        self._clear_exact_bindings(exact_assigned)
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
        exact_assigned = self._collect_assigned_names([node])
        provenance_flow = self._begin_module_provenance_flow(
            record_exception_prefixes=True
        )
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

        scope = TryScope(
            finalbody=node.finalbody, lexical_loops=tuple(self.loop_scopes)
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
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))

        rest_cell = self._emit_cell_new(exc_val)
        raised_list = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=raised_list))
        rest_slot = None
        raised_slot = None
        if self.is_async():
            rest_slot = self._new_async_internal_slot()
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", rest_slot, rest_cell],
                    result=MoltValue("none"),
                )
            )
            raised_slot = self._new_async_internal_slot()
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
            res = MoltValue(self.next_var(), type_hint="cell")
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", rest_slot], result=res))
            return res

        def load_rest_value() -> MoltValue:
            cell = load_rest_cell()
            return self._emit_cell_get(cell, type_hint="exception")

        def store_rest_value(value: MoltValue) -> None:
            cell = load_rest_cell()
            self._emit_cell_set(cell, value)

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
                    exc_slot_offset = self._new_async_internal_slot()
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
                    self._clear_import_binding_origin(handler.name)
                    self._store_local_value(handler.name, match_val)
                exc_entry = ActiveException(
                    value=match_val,
                    slot=exc_slot_offset,
                    handler_name=handler.name,
                    is_handler=True,
                    scope=scope,
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
                self._emit_guarded_body(handler.body)
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
                final_slot = self._new_async_internal_slot()
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", final_slot, final_exc],
                        result=MoltValue("none"),
                    )
                )
            final_entry = ActiveException(value=final_exc, scope=scope, slot=final_slot)
            self.active_exceptions.append(final_entry)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[final_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self._emit_finalbody(scope)
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
                    self._emit_guarded_body(node.orelse)
            else:
                self._emit_guarded_body(node.orelse)
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
                else_final_slot = self._new_async_internal_slot()
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", else_final_slot, else_final_exc],
                        result=MoltValue("none"),
                    )
                )
            else_final_entry = ActiveException(
                value=else_final_exc, scope=scope, slot=else_final_slot
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
            self._emit_finalbody(scope)
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
        self._emit_raise_if_pending()
        self.try_scopes.pop()
        self.unbound_check_names = unbound_snapshot_try_star
        self.control_flow_depth -= 1
        self.block_terminated = prior_terminated
        self._finish_module_provenance_flow(provenance_flow)
        self._clear_exact_bindings(exact_assigned)
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
            if node.exc is not None:
                self.emit(
                    MoltOp(
                        kind="CALL",
                        args=["molt_exception_trace_prepend", exc],
                        result=MoltValue(self.next_var(), type_hint="None"),
                    )
                )
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
            # Runtime handled state is shared with sys.exception(), including
            # dynamically enclosing callers and finally scopes with no pending
            # error. Saved pending-error slots are not a second active authority.
            exc_val = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(kind="CALL", args=["molt_exception_active"], result=exc_val)
            )
            if clear_handlers:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_STACK_CLEAR", args=[], result=MoltValue("none")
                    )
                )
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
            context_val = MoltValue(self.next_var(), type_hint="exception")
            self.emit(
                MoltOp(kind="CALL", args=["molt_exception_active"], result=context_val)
            )
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
        test_val = self._emit_condition(node.test)
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
        if not self.loop_scopes:
            raise SyntaxError(f"'break' outside loop (line {node.lineno})")
        del node
        loop = self.loop_scopes[-1]
        popped_labels = self._emit_loop_unwind()
        try:
            break_slot = loop.break_flag
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
                    self._store_scratch_cell(break_slot, break_val)
            loop.break_used = True
            self.emit(
                MoltOp(kind="JUMP", args=[loop.break_label], result=MoltValue("none"))
            )
        finally:
            self._restore_control_flow_unwind_labels(popped_labels)
        self.block_terminated = True
        return None

    def visit_Continue(self, node: ast.Continue) -> None:
        if self.finally_depth > 0:
            self._emit_syntax_warning(node, "'continue' in a 'finally' block")
        if not self.loop_scopes:
            raise SyntaxError(f"'continue' not properly in loop (line {node.lineno})")
        del node
        loop = self.loop_scopes[-1]
        popped_labels = self._emit_loop_unwind()
        try:
            loop.continue_used = True
            self.emit(
                MoltOp(
                    kind="JUMP", args=[loop.continue_label], result=MoltValue("none")
                )
            )
        finally:
            self._restore_control_flow_unwind_labels(popped_labels)
        self.block_terminated = True
        return None

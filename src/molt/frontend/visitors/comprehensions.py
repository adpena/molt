"""ComprehensionMixin: list/set/dict comprehension + generator-expression
lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py (F1 phase). Covers visit_ListComp,
visit_SetComp, visit_DictComp, and visit_GeneratorExp, plus the owned
comprehension materialization helpers for inline/range/list/set/dict lowering.
Shared pattern recognizers, scope/free-var collectors, and generator framing
remain on sibling mixins and resolve through the MRO via ``self.<method>``.
"""

from __future__ import annotations

import ast
from typing import (
    Any,
    Callable,
    Sequence,
    cast,
)

from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import (
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    MoltOp,
    MoltValue,
    ScratchCell,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.sema import FunctionKind, stateful_function_frame_plan


class ComprehensionMixin(GeneratorMixinBase):
    _list_int_containers: set[str]

    def visit_ListComp(self, node: ast.ListComp) -> Any:
        if not self._comprehension_requires_async(node.generators, [node.elt]):
            simple_range = self._match_simple_range_list_comp(node)
            if simple_range is not None:
                return self._emit_range_list(*simple_range)
            fill_value = self._match_const_int_range_list_comp(node)
            if fill_value is not None:
                return self._emit_const_int_range_list_comp(node, fill_value)
            fill_node = self._match_const_range_list_comp(node)
            if fill_node is not None:
                return self._emit_const_range_list_comp(node, fill_node)
        return self._emit_inline_list_comp(node)

    def visit_SetComp(self, node: ast.SetComp) -> Any:
        return self._emit_inline_set_comp(node)

    def visit_DictComp(self, node: ast.DictComp) -> Any:
        return self._emit_inline_dict_comp(node)

    def visit_GeneratorExp(self, node: ast.GeneratorExp) -> Any:
        async_needed = self._comprehension_requires_async(node.generators, [node.elt])
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        # CPython evaluates AND iterates the outermost iterable at generator
        # construction time; only the loop body and every nested iterable are
        # lazy.  Transport that already-created iterator as the hidden .0
        # parameter through the shared callable's task payload.  Evaluating the
        # outer expression here preserves exception/side-effect ordering and
        # leaves its one-shot temporaries under the enclosing function's
        # DropInsertion authority.
        func_symbol = self._genexpr_symbol()
        poll_func_name = f"{func_symbol}_poll"
        outer = node.generators[0]
        outer_value = self.visit(outer.iter)
        if outer_value is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE,
                "Unsupported generator-expression outer iterable",
            )
        outer_iter = (
            self._emit_aiter(outer_value)
            if outer.is_async
            else self._emit_iter_new(outer_value)
        )
        outer_iter_name = f"__molt_genexpr_outer_iter_{self.genexpr_counter}"
        outer_iter_expr = ast.copy_location(
            ast.Name(id=outer_iter_name, ctx=ast.Load()), outer.iter
        )
        poll_outer = ast.comprehension(
            target=outer.target,
            iter=outer_iter_expr,
            ifs=outer.ifs,
            is_async=outer.is_async,
        )
        poll_node = ast.copy_location(
            ast.GeneratorExp(
                elt=node.elt,
                generators=[poll_outer, *node.generators[1:]],
            ),
            node,
        )
        cell_vars = self._callable_cell_vars(poll_node)
        prev_func = self.current_func_name

        module_namedexpr_targets: set[str] = set()
        if self.current_func_name == "molt_main":
            module_namedexpr_targets = self._collect_namedexpr_targets_comprehension(
                poll_node
            )
            if module_namedexpr_targets:
                self.module_global_mutations.update(module_namedexpr_targets)
                # Invalidate local SSA cache for walrus targets so that
                # subsequent reads at module scope re-read from the module
                # dict (which the genexpr writes to via global_decls).
                # Pop from both locals and globals — the compiler uses
                # whichever it finds first for module-scope name reads.
                for name in module_namedexpr_targets:
                    self.locals.pop(name, None)
                    self.globals.pop(name, None)
                    self.exact_locals.pop(name, None)
                    self.boxed_locals.pop(name, None)
        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(
                self._lexical_dependencies().summary(poll_node).body.lexical
                - {outer_iter_name}
            )
        )
        frame_plan = stateful_function_frame_plan(
            kind=FunctionKind.ASYNC_GENERATOR
            if async_needed
            else FunctionKind.GENERATOR,
            poll_symbol=poll_func_name,
            # CPython passes the eagerly-created outer iterator as the hidden
            # ``.0`` generator-function parameter. Model that as a real task
            # payload parameter, not as a synthetic closure cell: payload
            # layout then stays identical for direct genexprs and the
            # list/set/dict materialization paths on every backend.
            param_count=1,
            has_closure=has_closure,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        yield_stmt = ast.Expr(value=ast.Yield(value=node.elt))
        body = self._build_comprehension_body(poll_node.generators, [yield_stmt])
        assigned = self._collect_assigned_names(body)
        del_targets = self._collect_deleted_names(body)
        prev_state = self._capture_function_state()
        prev_async_context = self.async_context
        self.start_function(
            poll_func_name,
            stateful_frame_plan=frame_plan,
            python_first_arg=outer_iter_name,
            params=["self"],
            compiler_params={"self"},
            type_facts_name=func_symbol,
            needs_return_slot=False,
        )
        self.current_class = None
        self.current_method_first_param = None
        self.async_context = prev_async_context
        self.global_decls = set(module_namedexpr_targets)
        self.del_targets = del_targets
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        self.in_generator = True
        self.async_locals_base = frame_plan.async_locals_base
        if has_closure:
            self.async_closure_offset = frame_plan.async_closure_offset
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        self.async_public_hints[outer_iter_name] = outer_iter.type_hint or "Any"
        self._async_local_offset(outer_iter_name)
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        self._prebox_scope_cell_vars(cell_vars)
        self._publish_python_frame_context()
        self._push_qualname("<genexpr>", True)
        try:
            for stmt in body:
                self.visit(stmt)
        finally:
            self._pop_qualname()
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                closed = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", GEN_CLOSED_OFFSET, closed],
                        result=MoltValue("none"),
                    )
                )
                done = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
                self._emit_return_value(pair)
            self._emit_return_label()
        elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            closed = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", GEN_CLOSED_OFFSET, closed],
                    result=MoltValue("none"),
                )
            )
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
            self._emit_normal_return_terminator(pair)
        self._spill_async_temporaries()
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        callable_val = MoltValue(
            self.next_var(), type_hint=frame_plan.function_type_hint(closure_size)
        )
        if has_closure:
            assert closure_val is not None
        self.emit(
            MoltOp(
                kind="FUNC_NEW_CLOSURE" if has_closure else "FUNC_NEW",
                args=(
                    [poll_func_name, 1, closure_val]
                    if has_closure
                    else [poll_func_name, 1]
                ),
                result=callable_val,
                metadata=frame_plan.callable_task_metadata(closure_size),
            )
        )
        self._emit_function_metadata(
            callable_val,
            code_symbol=poll_func_name,
            name="<genexpr>",
            qualname=self._qualname_for_def("<genexpr>"),
            trace_lineno=node.lineno,
            posonly_params=[".0"],
            pos_or_kw_params=[],
            kwonly_params=[],
            vararg=None,
            varkw=None,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=None,
            execution_kind=frame_plan.kind,
            varnames=self._collect_varnames_for_body(
                posonly_params=[".0"],
                pos_or_kw_params=[],
                kwonly_params=[],
                vararg=None,
                varkw=None,
                body=body,
            ),
            freevars=free_vars,
            cellvars=cell_vars,
        )
        res = MoltValue(self.next_var(), type_hint=frame_plan.result_type_hint)
        self.emit(MoltOp(kind="CALL_FUNC", args=[callable_val, outer_iter], result=res))
        return res

    def _emit_range_list(
        self, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_FROM_RANGE", args=[start, stop, step], result=res))
        # Range always produces int elements.
        if self.current_func_name == "molt_main":
            self.global_elem_hints[res.name] = "int"
        else:
            self.container_elem_hints[res.name] = "int"
        return res

    def _emit_list_int_filled(self, count: MoltValue, fill: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_INT_NEW", args=[count, fill], result=res))
        if self.current_func_name == "molt_main":
            self.global_elem_hints[res.name] = "int"
        else:
            self.container_elem_hints[res.name] = "int"
        self._list_int_containers = getattr(self, "_list_int_containers", set())
        self._list_int_containers.add(res.name)
        return res

    def _emit_list_filled(
        self, count: MoltValue, fill: MoltValue, elem_hint: str | None
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_FILL_NEW", args=[count, fill], result=res))
        if elem_hint and elem_hint not in {"Any", "Unknown"}:
            if self.current_func_name == "molt_main":
                self.global_elem_hints[res.name] = elem_hint
            else:
                self.container_elem_hints[res.name] = elem_hint
        return res

    def _emit_const_int_range_list_comp(
        self, node: ast.ListComp, fill_value: int
    ) -> MoltValue:
        comp = node.generators[0]
        parsed = self._parse_range_call(comp.iter)
        if parsed is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported range in list comprehension",
            )
        start, stop, step, _ = parsed
        range_obj = self._emit_range_obj_from_args(start, stop, step)
        count = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[range_obj], result=count))
        fill = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[fill_value], result=fill))
        return self._emit_list_int_filled(count, fill)

    def _emit_const_range_list_comp(
        self, node: ast.ListComp, fill_node: ast.Constant
    ) -> MoltValue:
        comp = node.generators[0]
        parsed = self._parse_range_call(comp.iter)
        if parsed is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported range in list comprehension",
            )
        start, stop, step, _ = parsed
        range_obj = self._emit_range_obj_from_args(start, stop, step)
        count = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[range_obj], result=count))
        fill = self.visit(fill_node)
        if fill is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported list comprehension fill value",
            )
        elem_hint = fill.type_hint if isinstance(fill, MoltValue) else None
        return self._emit_list_filled(count, fill, elem_hint)

    def _emit_list_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        elem_hint = self._iterable_element_hint(iterable) or "Any"
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
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint=elem_hint)
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(
            MoltOp(kind="LIST_APPEND", args=[res, item], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        if elem_hint not in {"Any", "Unknown"}:
            if self.current_func_name == "molt_main":
                self.global_elem_hints[res.name] = elem_hint
            else:
                self.container_elem_hints[res.name] = elem_hint
        return res

    def _emit_list_from_aiter(self, iterable: MoltValue) -> MoltValue:
        if not self.is_async():
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "async list comprehension outside async context",
            )
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        res_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        iter_val = MoltValue(self.next_var(), type_hint=iter_obj.type_hint)
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", iter_slot],
                result=iter_val,
            )
        )
        sentinel_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_val,
            )
        )
        item_val = self._emit_await_anext(
            iter_val, default_val=sentinel_val, has_default=True
        )
        sentinel_after = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_after,
            )
        )
        is_done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[item_val, sentinel_after], result=is_done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[is_done], result=MoltValue("none"))
        )
        res_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_val,
            )
        )
        self.emit(
            MoltOp(
                kind="LIST_APPEND",
                args=[res_val, item_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        res_final = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_final,
            )
        )
        return res_final

    def _emit_set_from_iter(
        self, iterable: MoltValue, probe: bool = False
    ) -> MoltValue:
        # `probe=True` realizes the operand of a probe-only set operation
        # (intersection/intersection_update/issubset). CPython hashes each
        # element to probe the receiver without inserting into a fresh set, so an
        # unhashable element raises the bare `unhashable type: 'X'` form on every
        # version (no `set element` context, even on 3.14). The Bare-context
        # add op (SET_ADD_PROBE -> molt_set_add_probe) preserves that while still
        # materializing the temporary set molt's algorithm needs.
        add_kind = "SET_ADD_PROBE" if probe else "SET_ADD"
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
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
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(MoltOp(kind=add_kind, args=[res, item], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _emit_set_from_aiter(self, iterable: MoltValue) -> MoltValue:
        if not self.is_async():
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "async set comprehension outside async context",
            )
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
        res_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        iter_val = MoltValue(self.next_var(), type_hint=iter_obj.type_hint)
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", iter_slot],
                result=iter_val,
            )
        )
        sentinel_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_val,
            )
        )
        item_val = self._emit_await_anext(
            iter_val, default_val=sentinel_val, has_default=True
        )
        sentinel_after = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_after,
            )
        )
        is_done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[item_val, sentinel_after], result=is_done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[is_done], result=MoltValue("none"))
        )
        res_val = MoltValue(self.next_var(), type_hint="set")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_val,
            )
        )
        self.emit(
            MoltOp(
                kind="SET_ADD",
                args=[res_val, item_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        res_final = MoltValue(self.next_var(), type_hint="set")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_final,
            )
        )
        return res_final

    def _emit_dict_fill_from_iter(self, target: MoltValue, iterable: MoltValue) -> None:
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
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        # Validate that the yielded item has at least 2 elements before
        # indexing, so non-tuple / short-sequence inputs produce a clear
        # ValueError instead of an opaque crash.
        two = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[2], result=two))
        item_len = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[item], result=item_len))
        item_too_short = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[item_len, two], result=item_too_short))
        self.emit(MoltOp(kind="IF", args=[item_too_short], result=MoltValue("none")))
        err_msg = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(
                kind="CONST_STR",
                args=["dictionary update sequence element has length less than 2"],
                result=err_msg,
            )
        )
        err_exc = self._emit_exception_new("ValueError", err_msg)
        self.emit(MoltOp(kind="RAISE", args=[err_exc], result=MoltValue("none")))
        self._emit_raise_exit()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        key = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item, zero], result=key))
        val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item, one], result=val))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[target, key, val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_dict_fill_from_aiter(
        self, target: MoltValue, iterable: MoltValue
    ) -> MoltValue:
        if not self.is_async():
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "async dict comprehension outside async context",
            )
        target_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", target_slot, target],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        iter_val = MoltValue(self.next_var(), type_hint=iter_obj.type_hint)
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", iter_slot],
                result=iter_val,
            )
        )
        sentinel_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_val,
            )
        )
        item_val = self._emit_await_anext(
            iter_val, default_val=sentinel_val, has_default=True
        )
        sentinel_after = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_after,
            )
        )
        is_done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[item_val, sentinel_after], result=is_done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[is_done], result=MoltValue("none"))
        )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        key = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item_val, zero], result=key))
        val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item_val, one], result=val))
        target_val = MoltValue(self.next_var(), type_hint=target.type_hint or "dict")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", target_slot],
                result=target_val,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[target_val, key, val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        target_final = MoltValue(self.next_var(), type_hint=target.type_hint or "dict")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", target_slot],
                result=target_final,
            )
        )
        return target_final

    def _build_comprehension_body(
        self,
        generators: list[ast.comprehension],
        inner: list[ast.stmt],
    ) -> list[ast.stmt]:
        body: list[ast.stmt] = list(inner)
        for comp in reversed(generators):
            for test in reversed(comp.ifs):
                body = [ast.If(test=test, body=list(body), orelse=[])]
            if comp.is_async:
                body = [
                    ast.AsyncFor(
                        target=comp.target,
                        iter=comp.iter,
                        body=list(body),
                        orelse=[],
                    )
                ]
            else:
                body = [
                    ast.For(
                        target=comp.target,
                        iter=comp.iter,
                        body=list(body),
                        orelse=[],
                    )
                ]
        return body

    def _comprehension_requires_async(
        self,
        generators: Sequence[ast.comprehension],
        exprs: Sequence[ast.AST | None],
    ) -> bool:
        if any(comp.is_async for comp in generators):
            return True
        for comp in generators:
            if self._expr_needs_async(comp.iter):
                return True
            for test in comp.ifs:
                if self._expr_needs_async(test):
                    return True
        for expr in exprs:
            if expr is None:
                continue
            if self._expr_needs_async(expr):
                return True
        return False

    def _inline_simple_comp_exprs(
        self, node: ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[ast.expr]:
        if isinstance(node, ast.DictComp):
            return [node.key, node.value]
        return [node.elt]

    def _comprehension_frame_can_fuse(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> bool:
        """Prove this reducer's frame-eliding optimization preserves ownership."""
        target_names = {
            name
            for comp in node.generators
            for name in self._collect_target_names(comp.target)
        }
        if target_names.intersection(self.comprehension_bindings):
            # Reducer local substitution cannot replace an active scoped slot.
            return False
        if not isinstance(node, ast.GeneratorExp):
            return True
        authority = self._lexical_dependencies()
        names = (
            authority.summary(node).body.lexical | authority.declarations(node).bound
        )
        # A real generator owns .0; counted fusion has no iterator frame.
        # Retain the real frame when class-cell observation can distinguish it.
        return "__class__" not in names

    def _can_inline_simple_comp(
        self,
        generators: list[ast.comprehension],
        exprs: Sequence[ast.AST],
    ) -> bool:
        """Check whether a comprehension can be lowered as an inline loop.

        Requirements: single generator, no async, simple target (Name or a
        flat Tuple of Names), no nested comprehensions in emitted element
        expressions. This is a reducer-fusion shape predicate, not a choice of
        Python executing frame; materialized comprehensions always stay inline.

        Tuple targets such as ``for i, value in enumerate(values)`` are
        accepted: the inline emitter assigns to a temp Name and emits an
        explicit unpack, matching the semantics of CPython's tuple-target
        ``for`` loops without forcing the comprehension onto the
        generator-poll path (which has known Cranelift codegen
        fragility for large surrounding functions).
        """
        if len(generators) != 1:
            return False
        comp = generators[0]
        if comp.is_async:
            return False
        if isinstance(comp.target, ast.Name):
            pass
        elif isinstance(comp.target, ast.Tuple):
            # Only accept flat tuples of plain Name elements (no nested
            # tuples, no Starred/Subscript/Attribute targets).
            if not comp.target.elts:
                return False
            for elt in comp.target.elts:
                if not isinstance(elt, ast.Name):
                    return False
        else:
            return False
        # Reducer fusion currently accepts no nested expression scopes.
        # Their materialization visitor remains fully caller-frame preserving.
        for expr in exprs:
            for child in ast.walk(expr):
                if isinstance(
                    child, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
                ):
                    return False
        return True

    def _inline_simple_comp_target(
        self, comp: ast.comprehension, temp_prefix: str
    ) -> tuple[str, list[str] | None]:
        if isinstance(comp.target, ast.Name):
            return comp.target.id, None
        if isinstance(comp.target, ast.Tuple) and all(
            isinstance(e, ast.Name) for e in comp.target.elts
        ):
            tuple_target_names = [cast(ast.Name, e).id for e in comp.target.elts]
            target_name = f"{temp_prefix}_{self.next_var()}"
            return target_name, tuple_target_names
        raise FrontendRejection(
            Diagnostic.SYNTAX_FORM,
            "Only simple comprehension targets supported",
        )

    def _emit_materialized_comprehension(
        self,
        node: ast.ListComp | ast.SetComp | ast.DictComp,
        *,
        result_type_hint: str,
        result_op: str,
        emit_result_values: Callable[[MoltValue, list[MoltValue]], None],
    ) -> MoltValue:
        """PEP 709: all materialized shapes execute in the enclosing frame.

        Nested/multi-for/async forms share scoped bindings and loop emission;
        none is rewritten into a generator's distinct Python code-object frame.
        """
        exprs = self._inline_simple_comp_exprs(node)
        async_needed = self._comprehension_requires_async(node.generators, exprs)
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        outer = node.generators[0]
        outer_value = self.visit(outer.iter)
        if outer_value is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "Unsupported comprehension iterable"
            )
        outer_iterator = (
            self._emit_aiter(outer_value)
            if outer.is_async
            else self._emit_iter_new(outer_value)
        )
        result = MoltValue(self.next_var(), type_hint=result_type_hint)
        self.emit(MoltOp(kind=result_op, args=[], result=result))
        result_storage = (
            self._new_scratch_cell(result, type_hint=result_type_hint)
            if self.is_async()
            else None
        )
        walrus_names = self._collect_namedexpr_targets_comprehension(node)
        prior_unbound = set(self.unbound_check_names)
        self._prepare_mutable_control_flow_bindings(walrus_names)

        def emit_leaf() -> None:
            values = self._emit_expr_list(exprs)
            current_result = (
                self._load_scratch_cell(result_storage)
                if result_storage is not None
                else result
            )
            emit_result_values(current_result, values)

        try:
            with self._comprehension_scope(node):
                self._emit_comprehension_generators(
                    node.generators,
                    0,
                    outer_iterator,
                    emit_leaf,
                    self._iteration_element_hint(outer, outer_value) or "Any",
                )
        finally:
            # A zero-iteration comprehension cannot prove a walrus bound.
            self.unbound_check_names.update(prior_unbound & walrus_names)
            self._evict_module_control_flow_bindings(walrus_names)
        return (
            self._consume_scratch_cell(result_storage)
            if result_storage is not None
            else result
        )

    def _emit_comprehension_generators(
        self,
        generators: list[ast.comprehension],
        index: int,
        iterator: MoltValue,
        emit_leaf: Callable[[], None],
        item_hint: str,
    ) -> None:
        """Lower one clause, recursively retaining each iterator across awaits."""
        comp = generators[index]
        iterator_storage = (
            self._new_scratch_cell(iterator, type_hint=iterator.type_hint)
            if self.is_async()
            else None
        )
        sentinel_storage: ScratchCell | None = None
        if comp.is_async:
            sentinel = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
            sentinel_storage = self._new_scratch_cell(sentinel, type_hint="list")
        zero = MoltValue(self.next_var(), type_hint="int")
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        current_iterator = (
            self._load_scratch_cell(iterator_storage)
            if iterator_storage is not None
            else iterator
        )
        if sentinel_storage is not None:
            sentinel = self._load_scratch_cell(sentinel_storage)
            item = self._emit_await_anext(
                current_iterator, default_val=sentinel, has_default=True
            )
            sentinel = self._load_scratch_cell(sentinel_storage)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[item, sentinel], result=done))
        else:
            pair = self._emit_iter_next_checked(current_iterator)
            item = MoltValue(self.next_var(), type_hint=item_hint)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        if sentinel_storage is None:
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self._emit_assign_target(comp.target, item, None)
        for condition in comp.ifs:
            condition_value = self._emit_condition(condition)
            self.emit(
                MoltOp(kind="IF", args=[condition_value], result=MoltValue("none"))
            )
        if index + 1 == len(generators):
            emit_leaf()
        else:
            child = generators[index + 1]
            value = self.visit(child.iter)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported comprehension iterable"
                )
            child_iterator = (
                self._emit_aiter(value)
                if child.is_async
                else self._emit_iter_new(value)
            )
            self._emit_comprehension_generators(
                generators,
                index + 1,
                child_iterator,
                emit_leaf,
                self._iteration_element_hint(child, value) or "Any",
            )
        for _ in comp.ifs:
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_inline_list_comp(self, node: ast.ListComp) -> MoltValue:
        def emit_list_value(res: MoltValue, values: list[MoltValue]) -> None:
            elt_val = values[0]
            self.emit(
                MoltOp(
                    kind="LIST_APPEND",
                    args=[res, elt_val],
                    result=MoltValue("none"),
                )
            )
            # Propagate element type hint to the result list.
            elt_hint = elt_val.type_hint if isinstance(elt_val, MoltValue) else None
            if elt_hint and elt_hint not in {"Any", "Unknown"}:
                if self.current_func_name == "molt_main":
                    self.global_elem_hints[res.name] = elt_hint
                else:
                    self.container_elem_hints[res.name] = elt_hint

        return self._emit_materialized_comprehension(
            node,
            result_type_hint="list",
            result_op="LIST_NEW",
            emit_result_values=emit_list_value,
        )

    def _emit_inline_set_comp(self, node: ast.SetComp) -> MoltValue:
        def emit_set_value(res: MoltValue, values: list[MoltValue]) -> None:
            self.emit(
                MoltOp(
                    kind="SET_ADD",
                    args=[res, values[0]],
                    result=MoltValue("none"),
                )
            )

        return self._emit_materialized_comprehension(
            node,
            result_type_hint="set",
            result_op="SET_NEW",
            emit_result_values=emit_set_value,
        )

    def _emit_inline_dict_comp(self, node: ast.DictComp) -> MoltValue:
        def emit_dict_item(res: MoltValue, values: list[MoltValue]) -> None:
            key_val, item_val = values
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res, key_val, item_val],
                    result=MoltValue("none"),
                )
            )

        return self._emit_materialized_comprehension(
            node,
            result_type_hint="dict",
            result_op="DICT_NEW",
            emit_result_values=emit_dict_item,
        )

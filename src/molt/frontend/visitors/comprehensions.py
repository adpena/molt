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
    TYPE_CHECKING,
    Any,
    Callable,
    Sequence,
    cast,
)

from molt.frontend._types import (
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    MoltOp,
    MoltValue,
)
from molt.frontend.sema import FunctionKind, stateful_function_frame_plan

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ComprehensionMixin(_MixinBase):
    def visit_ListComp(self, node: ast.ListComp) -> Any:
        async_needed = self._comprehension_requires_async(node.generators, [node.elt])
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        if not async_needed:
            simple_range = self._match_simple_range_list_comp(node)
            if simple_range is not None:
                start, stop, step = simple_range
                return self._emit_range_list(start, stop, step)
            fill_value = self._match_const_int_range_list_comp(node)
            if fill_value is not None:
                return self._emit_const_int_range_list_comp(node, fill_value)
            fill_node = self._match_const_range_list_comp(node)
            if fill_node is not None:
                return self._emit_const_range_list_comp(node, fill_node)
            if self._can_inline_list_comp(node):
                return self._emit_inline_list_comp(node)
        genexp = ast.GeneratorExp(elt=node.elt, generators=node.generators)
        gen_val = self.visit(genexp)
        if gen_val is None:
            raise NotImplementedError("Unsupported list comprehension")
        if async_needed:
            return self._emit_list_from_aiter(gen_val)
        return self._emit_list_from_iter(gen_val)

    def visit_SetComp(self, node: ast.SetComp) -> Any:
        async_needed = self._comprehension_requires_async(node.generators, [node.elt])
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        if not async_needed and self._can_inline_set_comp(node):
            return self._emit_inline_set_comp(node)
        genexp = ast.GeneratorExp(elt=node.elt, generators=node.generators)
        gen_val = self.visit(genexp)
        if gen_val is None:
            raise NotImplementedError("Unsupported set comprehension")
        if async_needed:
            return self._emit_set_from_aiter(gen_val)
        return self._emit_set_from_iter(gen_val)

    def visit_DictComp(self, node: ast.DictComp) -> Any:
        async_needed = self._comprehension_requires_async(
            node.generators, [node.key, node.value]
        )
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        if not async_needed and self._can_inline_dict_comp(node):
            return self._emit_inline_dict_comp(node)
        pair = ast.Tuple(elts=[node.key, node.value], ctx=ast.Load())
        genexp = ast.GeneratorExp(elt=pair, generators=node.generators)
        gen_val = self.visit(genexp)
        if gen_val is None:
            raise NotImplementedError("Unsupported dict comprehension")
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
        if async_needed:
            res = self._emit_dict_fill_from_aiter(res, gen_val)
        else:
            self._emit_dict_fill_from_iter(res, gen_val)
        return res

    def visit_GeneratorExp(self, node: ast.GeneratorExp) -> Any:
        async_needed = self._comprehension_requires_async(node.generators, [node.elt])
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        # Generator expressions are lazy after the outermost iterable is
        # evaluated. Eager materialisation changes exception and side-effect
        # timing, so genexprs always use the poll-function lowering.
        cell_vars = self._collect_comprehension_cell_vars(node)
        func_symbol = self._genexpr_symbol()
        poll_func_name = f"{func_symbol}_poll"
        prev_func = self.current_func_name
        free_vars: list[str] = []
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        module_namedexpr_targets: set[str] = set()
        if self.current_func_name == "molt_main":
            module_namedexpr_targets = self._collect_namedexpr_targets_comprehension(
                node
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
        free_vars = self._collect_free_vars_comprehension(node)
        if self.current_func_name == "molt_main":
            # CPython resolves module-scope comprehension names as globals, not
            # closure-captured cells. Capturing them here can clobber module
            # bindings after generator execution.
            free_vars = []
        if free_vars:
            if self.current_func_name != "molt_main":
                self.unbound_check_names.update(free_vars)
            for name in free_vars:
                self._box_local(name)
                self.closure_locals.add(name)
            for name in free_vars:
                hint = self.boxed_local_hints.get(name)
                if hint is None:
                    value = self.locals.get(name)
                    if value is not None and value.type_hint:
                        hint = value.type_hint
                free_var_hints[name] = hint or "Any"
            closure_items = self._closure_cells_for(free_vars)
            closure_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val))
            has_closure = True
        frame_plan = stateful_function_frame_plan(
            kind=FunctionKind.GENERATOR,
            poll_symbol=poll_func_name,
            param_count=0,
            has_closure=has_closure,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        yield_stmt = ast.Expr(value=ast.Yield(value=node.elt))
        body = self._build_comprehension_body(node.generators, [yield_stmt])
        assigned = self._collect_assigned_names(body)
        del_targets = self._collect_deleted_names(body)
        prev_state = self._capture_function_state()
        prev_async_context = self.async_context
        self.start_function(
            poll_func_name,
            params=["self"],
            type_facts_name=func_symbol,
            needs_return_slot=False,
        )
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
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        for name in cell_vars:
            self._box_local(name)
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
            self.emit(MoltOp(kind="ret", args=[pair], result=MoltValue("none")))
        self._spill_async_temporaries()
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        res = MoltValue(self.next_var(), type_hint=frame_plan.result_type_hint)
        args: list[MoltValue] = []
        if has_closure and closure_val is not None:
            args.append(closure_val)
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_func_name, closure_size] + args,
                result=res,
                metadata={"task_kind": frame_plan.task_kind},
            )
        )
        if async_needed:
            async_res = MoltValue(self.next_var(), type_hint="async_generator")
            self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[res], result=async_res))
            return async_res
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
            raise NotImplementedError("Unsupported range in list comprehension")
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
            raise NotImplementedError("Unsupported range in list comprehension")
        start, stop, step, _ = parsed
        range_obj = self._emit_range_obj_from_args(start, stop, step)
        count = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[range_obj], result=count))
        fill = self.visit(fill_node)
        if fill is None:
            raise NotImplementedError("Unsupported list comprehension fill value")
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
            raise NotImplementedError("async list comprehension outside async context")
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        res_slot = self._async_local_offset(
            f"__async_list_comp_res_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_list_comp_iter_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._async_local_offset(
            f"__async_list_comp_sentinel_{len(self.async_locals)}"
        )
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
            raise NotImplementedError("async set comprehension outside async context")
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
        res_slot = self._async_local_offset(
            f"__async_set_comp_res_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_set_comp_iter_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._async_local_offset(
            f"__async_set_comp_sentinel_{len(self.async_locals)}"
        )
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
            raise NotImplementedError("async dict comprehension outside async context")
        target_slot = self._async_local_offset(
            f"__async_dict_comp_target_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", target_slot, target],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_dict_comp_iter_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._async_local_offset(
            f"__async_dict_comp_sentinel_{len(self.async_locals)}"
        )
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
        generators: list[ast.comprehension],
        exprs: list[ast.AST | None],
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
    ) -> list[ast.AST]:
        if isinstance(node, ast.DictComp):
            return [node.key, node.value]
        return [node.elt]

    def _can_inline_simple_comp(
        self,
        generators: list[ast.comprehension],
        exprs: Sequence[ast.AST],
    ) -> bool:
        """Check whether a comprehension can be lowered as an inline loop.

        Requirements: single generator, no async, simple target (Name or a
        flat Tuple of Names), no nested comprehensions in emitted element
        expressions. Multi-for comprehensions use the GeneratorExp path, which
        handles walrus scope leaking separately.

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
        # Reject emitted expressions that themselves contain comprehensions
        # (they would require their own generator and cannot be inlined).
        for expr in exprs:
            for child in ast.walk(expr):
                if isinstance(
                    child, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
                ):
                    return False
        return True

    def _can_inline_list_comp(self, node: ast.ListComp) -> bool:
        return self._can_inline_simple_comp(node.generators, [node.elt])

    def _can_inline_set_comp(self, node: ast.SetComp) -> bool:
        return self._can_inline_simple_comp(node.generators, [node.elt])

    def _can_inline_dict_comp(self, node: ast.DictComp) -> bool:
        return self._can_inline_simple_comp(node.generators, [node.key, node.value])

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
        raise NotImplementedError("Only simple comprehension targets supported")

    def _emit_inline_simple_comp(
        self,
        node: ast.ListComp | ast.SetComp | ast.DictComp,
        *,
        result_type_hint: str,
        result_op: str,
        temp_prefix: str,
        emit_result_values: Callable[[MoltValue, list[MoltValue]], None],
    ) -> MoltValue:
        """Emit an inline loop for a simple collection comprehension.

        Avoids generating a generator task, working around a native-backend
        Cranelift code-generation issue where generator poll functions with
        non-trivial element expressions produce corrupted state machines.
        """
        comp = node.generators[0]
        exprs = self._inline_simple_comp_exprs(node)
        target_name, tuple_target_names = self._inline_simple_comp_target(
            comp, temp_prefix
        )
        # Collect walrus (:=) targets in the element expression and
        # filters. These must leak to the enclosing scope per PEP 572.
        walrus_names = self._collect_inline_comp_walrus_names(exprs, comp.ifs)
        # At module scope the single storage authority for a name is the
        # module dict (MODULE_SET_ATTR / MODULE_GET_ATTR), not a boxed
        # function cell: other functions read the global via the module dict,
        # and module-scope SSA refs dangle across chunk boundaries (#45 item
        # 3).  A walrus target that is *also* bound non-comprehensionally (a
        # ``while`` test walrus, a plain assignment, ...) writes through the
        # module dict, so the comprehension must read/write the same dict —
        # boxing it into a transient cell forks storage and the comp reads a
        # stale/None cell instead of the loop-carried value.  Route module
        # walrus targets through the module dict, exactly as the GeneratorExp
        # path already does (visit_GeneratorExp), and box only at non-module
        # scope.
        module_walrus_names: list[str] = []
        if (
            self.current_func_name == "molt_main"
            and getattr(self, "module_obj", None) is not None
        ):
            module_walrus_names = list(walrus_names)
            if module_walrus_names:
                self.module_global_mutations.update(module_walrus_names)
                # A bare module-scope name read is a LOAD_GLOBAL: it must raise
                # NameError (not the AttributeError that MODULE_GET_ATTR yields)
                # when the binding is absent — e.g. ``[acc := acc + i for i in
                # r]`` with no prior ``acc`` reads an unbound name.  del_targets
                # is the carrier for "module name that may be read while unbound
                # → route through MODULE_GET_GLOBAL" (see _collect_deleted_names:
                # it covers except-handler targets too, not only ``del``), so
                # registering the walrus targets there gives the same NameError
                # semantics the GeneratorExp poll-fn path gets via global_decls.
                self.del_targets.update(module_walrus_names)
                # Drop any SSA/cell caches so subsequent reads at module scope
                # re-read from the module dict (the comprehension writes there
                # via _store_local_value's module_global_mutations branch).
                for wname in module_walrus_names:
                    self.locals.pop(wname, None)
                    self.globals.pop(wname, None)
                    self.exact_locals.pop(wname, None)
                    self.boxed_locals.pop(wname, None)
                    self.boxed_local_hints.pop(wname, None)
        # Box walrus targets so their values survive the loop boundary.
        # The boxed cell lives on the heap, so store_index inside the
        # loop persists and index after the loop reads the final value.
        # Module-scope walrus targets are excluded — they live in the module
        # dict (handled above).
        module_walrus_set = set(module_walrus_names)
        for wname in walrus_names:
            if wname in module_walrus_set:
                continue
            if wname not in self.boxed_locals:
                self._box_local(wname)
        # If the element expression or filters contain lambdas that
        # reference the iteration variable, box it so the lambda can
        # capture it as a closure cell.  Without boxing, the iteration
        # variable is a plain SSA local that lambdas can't close over.
        lambda_free_vars = self._collect_inline_comp_lambda_free_vars(exprs, comp.ifs)
        outer_boxed = self.boxed_locals.pop(target_name, None)
        outer_boxed_hint = self.boxed_local_hints.pop(target_name, None)
        # When unpacking a tuple target, the user-named locals are bound
        # inside the loop body too. Save their pre-comp values so we can
        # restore them after the comprehension exits (CPython per-comp
        # scoping: the iteration variables don't leak).
        saved_tuple_locals: dict[str, MoltValue | None] = {}
        saved_tuple_boxed: dict[str, MoltValue | None] = {}
        saved_tuple_boxed_hints: dict[str, str | None] = {}
        tuple_cells: dict[str, MoltValue] = {}
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                saved_tuple_locals[tname] = self.locals.get(tname)
                saved_tuple_boxed[tname] = self.boxed_locals.pop(tname, None)
                saved_tuple_boxed_hints[tname] = self.boxed_local_hints.pop(tname, None)
        comp_cell: MoltValue | None = None
        if target_name in lambda_free_vars:
            missing = MoltValue(self.next_var(), type_hint="missing")
            self.emit(MoltOp(kind="MISSING", args=[], result=missing))
            comp_cell = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[missing], result=comp_cell))
            self.boxed_locals[target_name] = comp_cell
            self.boxed_local_hints[target_name] = "Any"
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                if tname not in lambda_free_vars:
                    continue
                missing = MoltValue(self.next_var(), type_hint="missing")
                self.emit(MoltOp(kind="MISSING", args=[], result=missing))
                tcell = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_NEW", args=[missing], result=tcell))
                tuple_cells[tname] = tcell
                self.boxed_locals[tname] = tcell
                self.boxed_local_hints[tname] = "Any"
        iterable_val = self.visit(comp.iter)
        iter_obj = self._emit_iter_new(iterable_val)
        res = MoltValue(self.next_var(), type_hint=result_type_hint)
        self.emit(MoltOp(kind=result_op, args=[], result=res))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        outer_comp_shadow_locals = set(self.comp_shadow_locals)
        self.comp_shadow_locals.add(target_name)
        if tuple_target_names is not None:
            self.comp_shadow_locals.update(tuple_target_names)
        # If the iteration variable is boxed, save the current cell value so
        # we can restore it after the comprehension (CPython scoping: the comp
        # does not leak its iteration variable into the enclosing scope).
        cell = comp_cell
        saved_cell_val: MoltValue | None = None
        if cell is not None:
            _save_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=_save_idx))
            saved_cell_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(kind="INDEX", args=[cell, _save_idx], result=saved_cell_val)
            )
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
        # Bind the loop variable so the element expression can reference it.
        old_local = self.locals.get(target_name)
        restore_local: MoltValue | None = old_local
        if restore_local is None and target_name not in self.boxed_locals:
            restore_local = MoltValue(self.next_var(), type_hint="missing")
            self.emit(MoltOp(kind="MISSING", args=[], result=restore_local))
        self.locals[target_name] = item
        if target_name not in self.boxed_locals:
            self._store_comprehension_local_value(target_name, item)
        # If the variable is boxed, write through to the cell so that
        # _load_local_value reads the current iteration value.
        if cell is not None:
            _box_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=_box_idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, _box_idx, item],
                    result=MoltValue("none"),
                )
            )
        # If the original target was a tuple, unpack the synthetic temp
        # Name into comprehension-local user names. Do not route this through
        # normal assignment lowering: module-level comprehension targets must
        # not publish module globals, and tuple element names need their own
        # cells when nested lambdas close over them.
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
        # Evaluate optional filter conditions.
        skip_label_needed = bool(comp.ifs)
        if skip_label_needed:
            for if_node in comp.ifs:
                cond_val = self.visit(if_node)
                not_cond = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[cond_val], result=not_cond))
                self.emit(MoltOp(kind="IF", args=[not_cond], result=MoltValue("none")))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        values: list[MoltValue] = []
        for expr in exprs:
            value = self.visit(expr)
            if value is None:
                raise NotImplementedError("Unsupported comprehension expression")
            values.append(cast(MoltValue, value))
        emit_result_values(res, values)
        # Restore the previous binding (if any). When this comprehension
        # created a closure-owned cell, never write the outer value through
        # _store_local_value while boxed_locals[target_name] still points at
        # the comprehension cell; late-bound lambdas must retain the final
        # iteration value in that cell.
        if restore_local is not None and cell is None:
            self._store_local_value(target_name, restore_local)
        if old_local is not None:
            self.locals[target_name] = old_local
        else:
            self.locals.pop(target_name, None)
        # Restore any user-named locals bound by tuple-unpacking the
        # iteration value.
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                prior = saved_tuple_locals.get(tname)
                if prior is not None:
                    self.locals[tname] = prior
                else:
                    self.locals.pop(tname, None)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        # Post-loop: restore the saved cell value so that the outer scope sees
        # its original value (e.g. ``print(i)`` after the comp returns the
        # outer for-loop's final ``i``, not the comp's last iteration value).
        # Exception: if a lambda inside the comp captures the iteration variable
        # via late binding (``lambda: i``), leave the cell with the final loop
        # value — the closure needs it.  Default-arg capture (``lambda i=i: i``)
        # does NOT appear in lambda_free_vars since ``i`` is a parameter.
        _closure_captured = target_name in lambda_free_vars
        if cell is not None and saved_cell_val is not None and not _closure_captured:
            _post_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=_post_idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, _post_idx, saved_cell_val],
                    result=MoltValue("none"),
                )
            )
        # Sync walrus (:=) targets to the enclosing scope.  The boxed
        # cell was updated inside the loop; read the final value and
        # store it as the local (and module attr at module scope) so
        # subsequent code sees the walrus assignment.
        # Module-scope walrus targets are skipped: they have no boxed cell
        # (they store straight to the module dict each iteration via
        # _store_local_value's module_global_mutations branch), so the dict
        # already holds the final value and is the authoritative reader.
        for wname in walrus_names:
            if wname in module_walrus_set:
                continue
            wcell = self._load_boxed_cell(wname)
            if wcell is not None:
                _widx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=_widx))
                wval = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="INDEX", args=[wcell, _widx], result=wval))
                self.locals[wname] = wval
                if (
                    self.current_func_name == "molt_main"
                    and hasattr(self, "module_obj")
                    and self.module_obj is not None
                ):
                    self._emit_module_attr_set_on(self.module_obj, wname, wval)
        if comp_cell is not None:
            self.boxed_locals.pop(target_name, None)
            self.boxed_local_hints.pop(target_name, None)
        if outer_boxed is not None:
            self.boxed_locals[target_name] = outer_boxed
            if outer_boxed_hint is not None:
                self.boxed_local_hints[target_name] = outer_boxed_hint
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                self.boxed_locals.pop(tname, None)
                self.boxed_local_hints.pop(tname, None)
                prior_boxed = saved_tuple_boxed.get(tname)
                prior_hint = saved_tuple_boxed_hints.get(tname)
                if prior_boxed is not None:
                    self.boxed_locals[tname] = prior_boxed
                    if prior_hint is not None:
                        self.boxed_local_hints[tname] = prior_hint
        self.comp_shadow_locals = outer_comp_shadow_locals
        return res

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

        return self._emit_inline_simple_comp(
            node,
            result_type_hint="list",
            result_op="LIST_NEW",
            temp_prefix="__molt_listcomp_unpack",
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

        return self._emit_inline_simple_comp(
            node,
            result_type_hint="set",
            result_op="SET_NEW",
            temp_prefix="__molt_setcomp_unpack",
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

        return self._emit_inline_simple_comp(
            node,
            result_type_hint="dict",
            result_op="DICT_NEW",
            temp_prefix="__molt_dictcomp_unpack",
            emit_result_values=emit_dict_item,
        )

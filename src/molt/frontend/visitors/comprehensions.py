"""ComprehensionMixin: list/set/dict comprehension + generator-expression
lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py (F1 phase). Covers visit_ListComp,
visit_SetComp, visit_DictComp, and visit_GeneratorExp. The shared comprehension
helpers (``_comprehension_requires_async``, ``_can_inline_*``, ``_emit_*``, the
``_match_*`` range recognizers, scope/free-var collectors, generator framing)
remain in SimpleTIRGenerator and resolve through the MRO via ``self.<method>``.
"""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    MoltOp,
    MoltValue,
)

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
        if has_closure:
            self.async_closure_offset = GEN_CONTROL_SIZE
            self.async_locals_base = GEN_CONTROL_SIZE + 8
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        else:
            self.async_locals_base = GEN_CONTROL_SIZE
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
        payload_slots = 1 if has_closure else 0
        closure_size = self._task_closure_size(payload_slots, include_gen_control=True)
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        res = MoltValue(self.next_var(), type_hint="generator")
        args: list[MoltValue] = []
        if has_closure and closure_val is not None:
            args.append(closure_val)
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_func_name, closure_size] + args,
                result=res,
                metadata={"task_kind": "generator"},
            )
        )
        if async_needed:
            async_res = MoltValue(self.next_var(), type_hint="async_generator")
            self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[res], result=async_res))
            return async_res
        return res

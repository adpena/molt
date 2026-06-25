"""FunctionVisitorMixin: function, lambda, and return lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py. Covers visit_FunctionDef,
visit_Lambda, and visit_Return. Async function/generator visitor methods live in
``async_gen.py``; semantic function-shape facts come from ``frontend.sema``.
"""

from __future__ import annotations

import ast

from typing import TYPE_CHECKING

from molt.frontend._types import (
    FuncInfo,
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    MoltOp,
    MoltValue,
    _MOLT_CLOSURE_PARAM,
)
from molt.frontend.sema import (
    expression_contains_yield,
    function_contains_yield,
    signature_contains_yield,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class FunctionVisitorMixin(_MixinBase):
    def visit_Return(self, node: ast.Return) -> None:
        if self.finally_depth > 0:
            self._emit_syntax_warning(node, "'return' in a 'finally' block")
        self.block_terminated = True
        if self.in_generator:
            val = self.visit(node.value) if node.value is not None else None
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self._emit_exception_handler_exit_cleanup()
            if self.return_unwind_depth == 0:
                self._emit_raise_if_pending(emit_exit=True)
            if self.return_unwind_depth > 0:
                self.emit(
                    MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none"))
                )
            popped_labels: list[int] = []
            if self.try_scopes:
                popped_labels = self._emit_control_flow_scope_unwind(self.try_scopes)
            try:
                # Return-time context cleanup is handled by per-scope CONTEXT_UNWIND_TO
                # above. A full CONTEXT_UNWIND here can incorrectly unwind caller frames.
                self._emit_restore_exception_stack_depth(exit_baseline=False)
                self._emit_raise_if_pending()
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
                self.emit(MoltOp(kind="TUPLE_NEW", args=[val, done], result=pair))
                self._emit_return_value(pair)
            finally:
                self._restore_control_flow_unwind_labels(popped_labels)
            return None
        val = self.visit(node.value) if node.value else None
        if val is None:
            val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
        self._emit_exception_handler_exit_cleanup()
        _has_exc_stack = self.exception_stack_prev_baseline is not None
        if _has_exc_stack and self.return_unwind_depth == 0:
            self._emit_raise_if_pending(emit_exit=True)
        if _has_exc_stack and self.return_unwind_depth > 0:
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        popped_labels = []
        if self.try_scopes:
            popped_labels = self._emit_control_flow_scope_unwind(self.try_scopes)
        try:
            # Return-time context cleanup is handled by per-scope CONTEXT_UNWIND_TO
            # above. A full CONTEXT_UNWIND here can incorrectly unwind caller frames.
            if _has_exc_stack:
                self._emit_restore_exception_stack_depth(exit_baseline=False)
                self._emit_raise_if_pending()
            self._emit_return_value(val)
        finally:
            self._restore_control_flow_unwind_labels(popped_labels)
        return None

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._maybe_record_local_intrinsic_wrapper(node)
        if self.current_func_name == "molt_main":
            new_globals = self._collect_global_decls(node.body)
            self.module_global_mutations.update(new_globals)
            # Evict cached locals for names declared `global` in this
            # function so that subsequent module-level reads go through
            # module_get_attr and see the mutation.
            for gname in new_globals:
                self.locals.pop(gname, None)
        is_generator = function_contains_yield(node)
        needs_locals_cache = self._function_contains_locals_call(node)
        has_return = self._function_contains_return(node)
        func_name = node.name
        qualname = self._qualname_for_def(func_name)
        if is_generator:
            func_symbol = self._function_symbol(func_name)
            if not self._has_typing_overload_decorator(node):
                self._record_func_default_specs(func_symbol, node.args)
            else:
                return None
            poll_func_name = f"{func_symbol}_poll"
            prev_func = self.current_func_name
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                node.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(node.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if node.args.vararg is not None:
                arg_nodes.append(node.args.vararg)
            arg_nodes.extend(kwonly)
            if node.args.kwarg is not None:
                arg_nodes.append(node.args.kwarg)

            free_vars: list[str] = []
            free_var_hints: dict[str, str] = {}
            closure_val: MoltValue | None = None
            has_closure = False
            if self.current_func_name != "molt_main":
                free_vars = self._collect_free_vars(node)
                if free_vars:
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
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                    )
                    has_closure = True

            func_kind = "GenClosureFunc" if has_closure else "GenFunc"
            payload_slots = len(params) + (1 if has_closure else 0)
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=True
            )
            func_val = MoltValue(
                self.next_var(),
                type_hint=f"{func_kind}:{poll_func_name}:{closure_size}",
            )
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[poll_func_name, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[poll_func_name, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=node.decorator_list,
                args=node.args,
                returns=node.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            varnames = self._collect_varnames_for_body(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=node.body,
            )
            self._emit_function_metadata(
                func_val,
                name=func_name,
                qualname=qualname,
                trace_lineno=node.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=node.args.defaults,
                kw_default_exprs=node.args.kw_defaults,
                docstring=ast.get_docstring(node, clean=False),
                is_generator=True,
                varnames=varnames,
                code_names=self._collect_code_names_for_body(
                    node.body,
                    varnames=varnames,
                    free_vars=free_vars,
                ),
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, node)
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
                if func_name in self.boxed_locals:
                    self._store_local_value(func_name, func_val)
            else:
                self._store_local_value(func_name, func_val)
            self._emit_module_attr_set(func_name, func_val)

            prev_state = self._capture_function_state()
            self.current_class = None
            self.start_function(
                poll_func_name,
                params=["self"],
                type_facts_name=func_name,
                needs_return_slot=has_return,
            )
            self.global_decls = self._collect_global_decls(node.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
            assigned = self._collect_assigned_names(node.body)
            self.del_targets = self._collect_deleted_names(node.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            self.async_public_locals = set(self.scope_assigned) | {
                arg.arg for arg in arg_nodes
            }
            self.async_internal_locals = set()
            self.in_generator = True
            if has_closure:
                self.async_closure_offset = GEN_CONTROL_SIZE
                self.async_locals_base = GEN_CONTROL_SIZE + 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            else:
                self.async_locals_base = GEN_CONTROL_SIZE
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                if self._hints_enabled():
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            if needs_locals_cache:
                self._init_locals_cache_and_pin()
            self._push_qualname(func_name, True)
            try:
                for item in node.body:
                    self.visit(item)
                    if isinstance(item, (ast.Return, ast.Raise)):
                        break
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
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                    )
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
                payload_slots, include_gen_control=True
            )
            gen_public_locals = self._async_locals_public_entries()
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            func_val.type_hint = f"{func_kind}:{poll_func_name}:{closure_size}"
            names_vals: list[MoltValue] = []
            offsets_vals: list[MoltValue] = []
            for local_name, offset in gen_public_locals:
                name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[local_name], result=name_val))
                offset_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                names_vals.append(name_val)
                offsets_vals.append(offset_val)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
            offsets_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=offsets_vals, result=offsets_tuple))
            self.emit(
                MoltOp(
                    kind="GEN_LOCALS_REGISTER",
                    args=[poll_func_name, names_tuple, offsets_tuple],
                    result=MoltValue("none"),
                )
            )
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
                if func_name in self.boxed_locals:
                    self._store_local_value(func_name, func_val)
            else:
                self._store_local_value(func_name, func_val)
            if node.decorator_list:
                decorated = func_val
                for deco in reversed(node.decorator_list):
                    decorator_val = self.visit(deco)
                    if decorator_val is None:
                        raise NotImplementedError("Unsupported decorator")
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[decorator_val, decorated],
                            result=res,
                        )
                    )
                    decorated = res
                func_val = decorated
                if self.current_func_name == "molt_main":
                    self.globals[func_name] = func_val
                    if func_name in self.boxed_locals:
                        self._store_local_value(func_name, func_val)
                else:
                    self._store_local_value(func_name, func_val)
                self._emit_module_attr_set(func_name, func_val)
            return None

        func_name = node.name
        func_symbol = self._function_symbol(func_name)
        if not self._has_typing_overload_decorator(node):
            self._record_func_default_specs(func_symbol, node.args)
        else:
            # Overload stubs are purely for type-checking; the real implementation
            # that follows will compile the body and emit FUNC_NEW.  Skip stub
            # compilation entirely so the backend never sees duplicate function
            # declarations with incompatible signatures.
            return None
        self.funcs_map.setdefault(
            func_symbol,
            FuncInfo(params=[], param_types=[], return_hint=None, ops=[]),
        )
        self.funcs_map[func_symbol]["return_hint"] = self._normalized_return_hint(
            node.returns
        )
        prev_func = self.current_func_name
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(node.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(node.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if node.args.vararg is not None:
            arg_nodes.append(node.args.vararg)
        arg_nodes.extend(kwonly)
        if node.args.kwarg is not None:
            arg_nodes.append(node.args.kwarg)

        needs_locals_cache = self._function_contains_locals_call(node)
        free_vars: list[str] = []
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars(node)
            if free_vars:
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
                self.emit(
                    MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                )
                has_closure = True

        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW", args=[func_symbol, len(params)], result=func_val
                )
            )
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=node.decorator_list,
            args=node.args,
            returns=node.returns,
        ):
            func_spill = self._spill_async_value(
                func_val, f"__func_meta_{len(self.async_locals)}"
            )
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=node.body,
        )
        self._emit_function_metadata(
            func_val,
            name=func_name,
            qualname=qualname,
            trace_lineno=node.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=node.args.defaults,
            kw_default_exprs=node.args.kw_defaults,
            docstring=ast.get_docstring(node, clean=False),
            varnames=varnames,
            code_names=self._collect_code_names_for_body(
                node.body,
                varnames=varnames,
                free_vars=free_vars,
            ),
        )
        is_gpu_kernel = self._has_gpu_kernel_decorator(node)
        # ── @gpu.kernel: mark function IR so the backend routes through GPU pipeline ──
        if is_gpu_kernel:
            self.gpu_kernel_symbols_by_name[func_name] = func_symbol
            gpu_flag = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gpu_flag))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_gpu_kernel__", gpu_flag],
                    result=MoltValue("none"),
                )
            )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        self._emit_function_annotate(func_val, node)
        if self.current_func_name == "molt_main":
            self.globals[func_name] = func_val
            if func_name in self.boxed_locals:
                self._store_local_value(func_name, func_val)
        else:
            self._store_local_value(func_name, func_val)
        self._emit_module_attr_set(func_name, func_val)

        func_params = params
        if has_closure:
            func_params = [_MOLT_CLOSURE_PARAM] + params
        prev_state = self._capture_function_state()
        self.current_class = None
        prev_first_param = self.current_method_first_param
        # Extract type hints from parameter annotations for fast-path codegen.
        _param_type_hints = []
        if self._hints_enabled():
            for arg in arg_nodes:
                hint = (
                    self._annotation_to_hint(arg.annotation) if arg.annotation else None
                )
                _param_type_hints.append(hint or "Any")
        self.start_function(
            func_symbol,
            params=func_params,
            param_types=_param_type_hints if _param_type_hints else None,
            type_facts_name=func_name,
            needs_return_slot=has_return,
            has_exception_handlers=self._body_has_exception_handlers(node.body),
        )
        prev_gpu_kernel_context = self.current_gpu_kernel_context
        self.current_gpu_kernel_context = is_gpu_kernel
        self.current_method_first_param = params[0] if params else None
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = self._collect_global_decls(node.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
        assigned = self._collect_assigned_names(node.body)
        self.del_targets = self._collect_deleted_names(node.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        for arg in arg_nodes:
            hint = None
            if self.type_hint_policy == "ignore" and arg.annotation is not None:
                inferred = self._annotation_to_hint(arg.annotation)
                if inferred is not None and inferred in self.classes:
                    hint = inferred
            if self._hints_enabled():
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
            if hint is None and self._hints_enabled():
                hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "Unknown")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        if not self.is_async():
            self._prebox_scope_cell_vars(body=node.body, arg_nodes=arg_nodes)
            # Only box variables that genuinely need cells (closure-captured).
            # Non-closure locals use store_var/load_var for SSA-visible mutations.
            param_names = {arg.arg for arg in arg_nodes}
            for name in sorted(self.scope_assigned):
                if name in self.closure_locals:
                    self._box_local(name)
                elif name not in param_names:
                    # Initialise non-boxed locals with the missing sentinel so
                    # that every SSA path has a definition (needed for phi merging
                    # and UnboundLocalError detection).
                    init = self._emit_missing_value()
                    self.locals[name] = init
                    self.emit(
                        MoltOp(
                            kind="STORE_VAR",
                            args=[init],
                            result=MoltValue("none"),
                            metadata={"var": name},
                        )
                    )
            # Emit store_var for parameters so the backend has an explicit
            # definition that TIR can track through reassignment.
            for arg in arg_nodes:
                pval = self.locals.get(arg.arg)
                if pval is not None and arg.arg not in self.boxed_locals:
                    self.emit(
                        MoltOp(
                            kind="STORE_VAR",
                            args=[pval],
                            result=MoltValue("none"),
                            metadata={"var": arg.arg},
                        )
                    )
            if needs_locals_cache:
                self._init_locals_cache_and_pin()
        self._push_qualname(func_name, True)
        try:
            for item in node.body:
                self.visit(item)
        finally:
            self._pop_qualname()
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self._emit_return_value(res)
            self._emit_return_label()
        elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            self._emit_return_value(res)
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_gpu_kernel_context = prev_gpu_kernel_context
        self.current_method_first_param = prev_first_param
        if is_gpu_kernel:
            descriptor_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(
                    kind="CONST_STR",
                    args=[
                        self._build_gpu_kernel_descriptor_json(
                            func_symbol=func_symbol, func_name=func_name
                        )
                    ],
                    result=descriptor_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_gpu_descriptor__", descriptor_val],
                    result=MoltValue("none"),
                )
            )
        if node.decorator_list:
            decorated = func_val
            for deco in reversed(node.decorator_list):
                decorator_val = self.visit(deco)
                if decorator_val is None:
                    raise NotImplementedError("Unsupported decorator")
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC", args=[decorator_val, decorated], result=res
                    )
                )
                decorated = res
            func_val = decorated
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
                if func_name in self.boxed_locals:
                    self._store_local_value(func_name, func_val)
            else:
                self._store_local_value(func_name, func_val)
            self._emit_module_attr_set(func_name, func_val)
        return None

    def visit_Lambda(self, node: ast.Lambda) -> MoltValue:
        if expression_contains_yield(node.body):
            func_symbol = self._lambda_symbol()
            poll_func_name = f"{func_symbol}_poll"
            qualname = self._qualname_for_def("<lambda>")
            self._record_func_default_specs(func_symbol, node.args)
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                node.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(node.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if node.args.vararg is not None:
                arg_nodes.append(node.args.vararg)
            arg_nodes.extend(kwonly)
            if node.args.kwarg is not None:
                arg_nodes.append(node.args.kwarg)

            needs_locals_cache = self._expr_contains_locals_call(node.body)
            free_vars: list[str] = []
            free_var_hints: dict[str, str] = {}
            closure_val: MoltValue | None = None
            has_closure = False
            if self.current_func_name != "molt_main":
                free_vars = self._collect_free_vars_expr(node)
            else:
                raw_free = self._collect_free_vars_expr_raw(node)
                free_vars = sorted(
                    name for name in raw_free if name in self.boxed_locals
                )
            if free_vars:
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
                self.emit(
                    MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                )
                has_closure = True

            func_kind = "GenClosureFunc" if has_closure else "GenFunc"
            payload_slots = len(params) + (1 if has_closure else 0)
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=True
            )
            func_val = MoltValue(
                self.next_var(),
                type_hint=f"{func_kind}:{poll_func_name}:{closure_size}",
            )
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[poll_func_name, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[poll_func_name, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=[],
                args=node.args,
                returns=None,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            varnames = self._collect_varnames_for_body(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=[ast.Expr(value=node.body)],
            )
            self._emit_function_metadata(
                func_val,
                name="<lambda>",
                qualname=qualname,
                trace_lineno=node.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=node.args.defaults,
                kw_default_exprs=node.args.kw_defaults,
                docstring=None,
                is_generator=True,
                varnames=varnames,
                code_names=self._collect_code_names_for_body(
                    [ast.Expr(value=node.body)],
                    varnames=varnames,
                    free_vars=free_vars,
                ),
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_first_param = self.current_method_first_param
            self.start_function(
                poll_func_name,
                params=["self"],
                type_facts_name=func_symbol,
                needs_return_slot=False,
            )
            self.current_method_first_param = params[0] if params else None
            assigned = self._collect_assigned_names([ast.Expr(value=node.body)])
            self.global_decls = set()
            self.nonlocal_decls = set()
            self.del_targets = set()
            self.scope_assigned = assigned
            self.unbound_check_names = set(self.scope_assigned)
            self.async_public_locals = set(self.scope_assigned) | {
                arg.arg for arg in arg_nodes
            }
            self.async_internal_locals = set()
            self.in_generator = True
            if has_closure:
                self.async_closure_offset = GEN_CONTROL_SIZE
                self.async_locals_base = GEN_CONTROL_SIZE + 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            else:
                self.async_locals_base = GEN_CONTROL_SIZE
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                if self._hints_enabled():
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            if needs_locals_cache:
                self._init_locals_cache_and_pin()
            self._push_qualname("<lambda>", True)
            try:
                return_node = ast.Return(value=node.body)
                return_node = ast.copy_location(return_node, node.body)
                self.visit(return_node)
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
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                    )
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
                payload_slots, include_gen_control=True
            )
            gen_public_locals = self._async_locals_public_entries()
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_method_first_param = prev_first_param
            func_val.type_hint = f"{func_kind}:{poll_func_name}:{closure_size}"
            names_vals: list[MoltValue] = []
            offsets_vals: list[MoltValue] = []
            for local_name, offset in gen_public_locals:
                name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[local_name], result=name_val))
                offset_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                names_vals.append(name_val)
                offsets_vals.append(offset_val)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
            offsets_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=offsets_vals, result=offsets_tuple))
            self.emit(
                MoltOp(
                    kind="GEN_LOCALS_REGISTER",
                    args=[poll_func_name, names_tuple, offsets_tuple],
                    result=MoltValue("none"),
                )
            )
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )
            return func_val

        func_symbol = self._lambda_symbol()
        qualname = self._qualname_for_def("<lambda>")
        self._record_func_default_specs(func_symbol, node.args)
        self.funcs_map.setdefault(
            func_symbol,
            FuncInfo(params=[], param_types=[], return_hint=None, ops=[]),
        )
        self.funcs_map[func_symbol]["return_hint"] = None
        prev_func = self.current_func_name
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(node.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(node.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if node.args.vararg is not None:
            arg_nodes.append(node.args.vararg)
        arg_nodes.extend(kwonly)
        if node.args.kwarg is not None:
            arg_nodes.append(node.args.kwarg)

        needs_locals_cache = self._expr_contains_locals_call(node.body)
        free_vars: list[str] = []
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars_expr(node)
        else:
            # At module level, only capture variables that are already boxed
            # (e.g., comprehension iteration variables). Module-level names
            # accessed via module dict don't need closure cells.
            raw_free = self._collect_free_vars_expr_raw(node)
            free_vars = sorted(name for name in raw_free if name in self.boxed_locals)
        if free_vars:
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

        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW", args=[func_symbol, len(params)], result=func_val
                )
            )
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=[ast.Expr(value=node.body)],
        )
        self._emit_function_metadata(
            func_val,
            name="<lambda>",
            qualname=qualname,
            trace_lineno=node.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=node.args.defaults,
            kw_default_exprs=node.args.kw_defaults,
            docstring=None,
            varnames=varnames,
            code_names=self._collect_code_names_for_body(
                [ast.Expr(value=node.body)],
                varnames=varnames,
                free_vars=free_vars,
            ),
        )

        func_params = params
        if has_closure:
            func_params = [_MOLT_CLOSURE_PARAM] + params
        prev_state = self._capture_function_state()
        self.current_class = None
        prev_first_param = self.current_method_first_param
        self.start_function(
            func_symbol,
            params=func_params,
            type_facts_name=func_symbol,
            # A lambda body is a single expression and can never contain a
            # try/with statement, so it never pushes the exception-handler
            # stack — only the (always-present) function exception label and
            # post-may-raise checks are needed.
            has_exception_handlers=False,
        )
        self.current_method_first_param = params[0] if params else None
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = set()
        for arg in arg_nodes:
            hint = None
            if self.type_hint_policy == "ignore" and arg.annotation is not None:
                inferred = self._annotation_to_hint(arg.annotation)
                if inferred is not None and inferred in self.classes:
                    hint = inferred
            if self._hints_enabled():
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
            if hint is None and self._hints_enabled():
                hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "Unknown")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        if not self.is_async():
            self._prebox_scope_cell_vars(
                body=[ast.Expr(value=node.body)], arg_nodes=arg_nodes
            )
            # Box ALL scope-assigned variables into cell lists.
            # Cell lists provide correct refcount management (inc_ref/
            # dec_ref in molt_store_index). The TIR backend's Memory SSA
            # rewrite converts cell store_index/index to store_var/load_var
            # for SSA phi visibility when optimization is enabled.
            for name in sorted(self.scope_assigned):
                self._box_local(name)
            for arg in arg_nodes:
                pval = self.locals.get(arg.arg)
                if pval is not None and arg.arg not in self.boxed_locals:
                    self.emit(
                        MoltOp(
                            kind="STORE_VAR",
                            args=[pval],
                            result=MoltValue("none"),
                            metadata={"var": arg.arg},
                        )
                    )
            if needs_locals_cache:
                self._init_locals_cache_and_pin()
        self._push_qualname("<lambda>", True)
        try:
            val = self.visit(node.body)
        finally:
            self._pop_qualname()
        if val is None:
            val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
        # Mirror the non-generator `visit_Return` tail: the lambda body is a
        # single expression whose evaluation may have left a pending exception
        # (e.g. `lambda: int("x")`).  With every function now carrying an
        # exception label (needs_exception_stack=True), route any pending
        # exception to the function handler before returning the value, so the
        # silent-None-return bug class is un-expressible.  A lambda body never
        # contains try/with scopes, so `return_unwind_depth == 0` and there are
        # no `try_scopes` to unwind.
        self._emit_exception_handler_exit_cleanup()
        _has_exc_stack = self.exception_stack_prev_baseline is not None
        if _has_exc_stack:
            self._emit_raise_if_pending(emit_exit=True)
            self._emit_restore_exception_stack_depth(exit_baseline=False)
            self._emit_raise_if_pending()
        self._emit_return_value(val)
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_method_first_param = prev_first_param
        return func_val

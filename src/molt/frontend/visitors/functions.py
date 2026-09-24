"""FunctionVisitorMixin: function, lambda, and return lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py. Covers visit_FunctionDef,
visit_Lambda, and visit_Return. Async function/generator visitor methods live in
``async_gen.py``; semantic function-shape facts come from ``frontend.sema``.
"""

from __future__ import annotations

import ast

from molt.frontend._types import (
    _MOLT_CLOSURE_PARAM,
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    FuncInfo,
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.sema import (
    FunctionKind,
    expression_contains_yield,
    function_contains_yield,
    signature_contains_yield,
    stateful_function_frame_plan,
)
from molt.frontend._mixin_base import GeneratorMixinBase


class FunctionVisitorMixin(GeneratorMixinBase):
    def _is_contextmanager_decorator(self, deco: ast.expr) -> bool:
        if isinstance(deco, ast.Name) and deco.id == "contextmanager":
            return True
        if (
            isinstance(deco, ast.Attribute)
            and isinstance(deco.value, ast.Name)
            and deco.value.id == "contextlib"
            and deco.attr == "contextmanager"
        ):
            return True
        return False

    @staticmethod
    def _is_gpu_kernel_decorator(deco: ast.expr) -> bool:
        """Return True if the decorator is @gpu.kernel."""
        # @gpu.kernel  (attribute form: gpu.kernel)
        if (
            isinstance(deco, ast.Attribute)
            and isinstance(deco.value, ast.Name)
            and deco.value.id == "gpu"
            and deco.attr == "kernel"
        ):
            return True
        # @kernel  (bare name after `from molt.gpu import kernel`)
        if isinstance(deco, ast.Name) and deco.id == "kernel":
            return True
        return False

    def _has_gpu_kernel_decorator(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> bool:
        """Return True if any decorator on *node* is @gpu.kernel."""
        return any(self._is_gpu_kernel_decorator(d) for d in node.decorator_list)

    def visit_Return(self, node: ast.Return) -> None:
        if self.finally_depth > 0:
            self._emit_syntax_warning(node, "'return' in a 'finally' block")
        val = self.visit(node.value) if node.value else None
        if val is None:
            val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
        pending_return = (
            self._new_scratch_cell(val, type_hint=val.type_hint)
            if self.is_async() and self.try_scopes
            else None
        )
        needs_scope_exit = (
            self.in_generator or self.exception_stack_prev_baseline is not None
        )
        if needs_scope_exit and self.return_unwind_depth == 0:
            self._emit_raise_if_pending()
        if needs_scope_exit and self.return_unwind_depth > 0:
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        popped_labels = []
        if self.try_scopes:
            popped_labels = self._emit_control_flow_scope_unwind(
                self.try_scopes, pending_return=pending_return
            )
        try:
            if pending_return is not None:
                val = self._consume_scratch_cell(pending_return)
            # The lexical scope actions above own manager cleanup; restoring
            # exception depth must not consume managers belonging to callers.
            if needs_scope_exit:
                self._emit_restore_exception_stack_depth(exit_baseline=False)
                self._emit_raise_if_pending()
            if self.in_generator:
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
                val = pair
            self._emit_return_value(val)
        finally:
            self._restore_control_flow_unwind_labels(popped_labels)
        # Cleanup emits its own reachable failure continuations. Publish the
        # source transfer only after those nested blocks have finished, as for
        # break/continue, so they cannot reopen fallthrough after this return.
        self.block_terminated = True
        return None

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        if self._class_ns_stack and self._class_ns_stack[-1].class_node is not None:
            self._emit_class_function_definition(self._class_ns_stack[-1], node)
            return None
        if (
            self.current_func_name == "molt_main"
            and node.name in self.module_elided_deleted_funcs
        ):
            return None
        self._maybe_record_local_intrinsic_wrapper(node)
        if self.current_func_name == "molt_main":
            new_globals = self._collect_global_decls(node.body)
            self.module_global_mutations.update(new_globals)
            # Evict cached locals for names declared `global` in this
            # function so that subsequent module-level reads go through
            # module_get_global and see the mutation.
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

            free_vars, free_var_hints, closure_val, has_closure = (
                self._capture_lexical_closure(self._cached_free_vars_raw(node))
            )
            cell_vars = self._callable_cell_vars(node)

            frame_plan = stateful_function_frame_plan(
                kind=FunctionKind.GENERATOR,
                poll_symbol=poll_func_name,
                param_count=len(params),
                has_closure=has_closure,
                gen_control_size=GEN_CONTROL_SIZE,
            )
            closure_size = self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            )
            func_val = MoltValue(
                self.next_var(),
                type_hint=frame_plan.function_type_hint(closure_size),
            )
            function_def = MoltOp(
                kind=(
                    "FUNC_NEW_CLOSURE"
                    if has_closure and closure_val is not None
                    else "FUNC_NEW"
                ),
                args=(
                    [poll_func_name, len(params), closure_val]
                    if has_closure and closure_val is not None
                    else [poll_func_name, len(params)]
                ),
                result=func_val,
            )
            self.emit(function_def)
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=node.decorator_list,
                args=node.args,
                returns=node.returns,
            ):
                func_spill = self._spill_async_value(func_val)
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
                code_symbol=poll_func_name,
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
                execution_kind=FunctionKind.GENERATOR,
                varnames=varnames,
                code_names=self._collect_code_names_for_body(
                    node.body,
                    varnames=varnames,
                    free_vars=free_vars,
                ),
                freevars=free_vars,
                cellvars=cell_vars,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, node)
            self._publish_definition_binding(func_name, func_val)

            prev_state = self._capture_function_state()
            self.current_class = None
            self.start_function(
                poll_func_name,
                stateful_frame_plan=frame_plan,
                python_first_arg=self._python_first_positional_arg(node.args),
                params=["self"],
                compiler_params={"self"},
                type_facts_name=func_name,
                needs_return_slot=has_return,
            )
            self._inherit_free_var_import_resolution(free_vars, prev_state)
            self.global_decls = self._collect_global_decls(node.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
            assigned = self._collect_assigned_names(node.body)
            self.del_targets = self._collect_deleted_names(node.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            self.in_generator = True
            self.async_locals_base = frame_plan.async_locals_base
            if has_closure:
                self.async_closure_offset = frame_plan.async_closure_offset
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            for i, arg in enumerate(arg_nodes):
                self._async_local_offset(arg.arg)
                if self._hints_enabled():
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            self._prebox_scope_cell_vars(cell_vars)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            if needs_locals_cache:
                self._init_locals_cache_and_pin()
            self._publish_python_frame_context()
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
                self._emit_normal_return_terminator(pair)
            self._spill_async_temporaries()
            closure_size = self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            )
            gen_public_locals = self._async_locals_public_entries()
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            # Publish the final spilled-frame extent on the defining op.
            function_def.metadata = {
                **(function_def.metadata or {}),
                **frame_plan.callable_task_metadata(closure_size),
            }
            func_val.type_hint = frame_plan.function_type_hint(closure_size)
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
            if node.decorator_list:
                decorated = func_val
                for deco in reversed(node.decorator_list):
                    decorator_val = self.visit(deco)
                    if decorator_val is None:
                        raise FrontendRejection(
                            Diagnostic.SYNTAX_FORM, "Unsupported decorator"
                        )
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
                self._publish_definition_binding(func_name, func_val)
            self._record_source_app_callable(
                func_name,
                kind=FunctionKind.GENERATOR,
                symbol=poll_func_name,
                decorated=bool(node.decorator_list),
            )
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
            FuncInfo(
                params=[], param_types=[], return_abi="value", return_hint=None, ops=[]
            ),
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
        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(node))
        )
        cell_vars = self._callable_cell_vars(node)

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
            func_spill = self._spill_async_value(func_val)
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
            code_symbol=func_symbol,
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
            freevars=free_vars,
            cellvars=cell_vars,
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
        self._publish_definition_binding(func_name, func_val)

        func_params, parameter_bindings = self._function_transport_params(
            params,
            has_closure=has_closure,
        )
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
            python_first_arg=self._python_first_positional_arg(node.args),
            params=func_params,
            param_types=_param_type_hints if _param_type_hints else None,
            type_facts_name=func_name,
            needs_return_slot=has_return,
            has_exception_handlers=self._body_has_exception_handlers(node.body),
        )
        self._inherit_free_var_import_resolution(free_vars, prev_state)
        self.parameter_bindings = parameter_bindings
        prev_gpu_kernel_context = self.current_gpu_kernel_context
        self.current_gpu_kernel_context = is_gpu_kernel
        self.current_method_first_param = params[0] if params else None
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.compiler_bindings[_MOLT_CLOSURE_PARAM] = MoltValue(
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
            value = self._parameter_value(
                arg.arg,
                type_hint=hint or "Unknown",
            )
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        if not self.is_async():
            self._prebox_scope_cell_vars(cell_vars)
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
        self._publish_python_frame_context()
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
                    raise FrontendRejection(
                        Diagnostic.SYNTAX_FORM, "Unsupported decorator"
                    )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC", args=[decorator_val, decorated], result=res
                    )
                )
                decorated = res
            func_val = decorated
            self._publish_definition_binding(func_name, func_val)
        self._record_source_app_callable(
            func_name,
            kind=FunctionKind.SYNC,
            symbol=func_symbol,
            decorated=bool(node.decorator_list),
        )
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
            free_vars, free_var_hints, closure_val, has_closure = (
                self._capture_lexical_closure(self._cached_free_vars_raw(node))
            )
            cell_vars = self._callable_cell_vars(node)

            frame_plan = stateful_function_frame_plan(
                kind=FunctionKind.GENERATOR,
                poll_symbol=poll_func_name,
                param_count=len(params),
                has_closure=has_closure,
                gen_control_size=GEN_CONTROL_SIZE,
            )
            closure_size = self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            )
            func_val = MoltValue(
                self.next_var(),
                type_hint=frame_plan.function_type_hint(closure_size),
            )
            function_def = MoltOp(
                kind=(
                    "FUNC_NEW_CLOSURE"
                    if has_closure and closure_val is not None
                    else "FUNC_NEW"
                ),
                args=(
                    [poll_func_name, len(params), closure_val]
                    if has_closure and closure_val is not None
                    else [poll_func_name, len(params)]
                ),
                result=func_val,
            )
            self.emit(function_def)
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=[],
                args=node.args,
                returns=None,
            ):
                func_spill = self._spill_async_value(func_val)
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
                code_symbol=poll_func_name,
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
                execution_kind=FunctionKind.GENERATOR,
                varnames=varnames,
                code_names=self._collect_code_names_for_body(
                    [ast.Expr(value=node.body)],
                    varnames=varnames,
                    free_vars=free_vars,
                ),
                freevars=free_vars,
                cellvars=cell_vars,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_first_param = self.current_method_first_param
            self.start_function(
                poll_func_name,
                stateful_frame_plan=frame_plan,
                python_first_arg=self._python_first_positional_arg(node.args),
                params=["self"],
                compiler_params={"self"},
                type_facts_name=func_symbol,
                needs_return_slot=False,
            )
            self._inherit_free_var_import_resolution(free_vars, prev_state)
            self.current_method_first_param = params[0] if params else None
            assigned = self._collect_assigned_names([ast.Expr(value=node.body)])
            self.global_decls = set()
            self.nonlocal_decls = set()
            self.del_targets = set()
            self.scope_assigned = assigned
            self.unbound_check_names = set(self.scope_assigned)
            self.in_generator = True
            self.async_locals_base = frame_plan.async_locals_base
            if has_closure:
                self.async_closure_offset = frame_plan.async_closure_offset
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            for i, arg in enumerate(arg_nodes):
                self._async_local_offset(arg.arg)
                if self._hints_enabled():
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            self._prebox_scope_cell_vars(cell_vars)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            if needs_locals_cache:
                self._init_locals_cache_and_pin()
            self._publish_python_frame_context()
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
                self._emit_normal_return_terminator(pair)
            self._spill_async_temporaries()
            closure_size = self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            )
            gen_public_locals = self._async_locals_public_entries()
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_method_first_param = prev_first_param
            # Publish the final spilled-frame extent on the defining op.
            function_def.metadata = {
                **(function_def.metadata or {}),
                **frame_plan.callable_task_metadata(closure_size),
            }
            func_val.type_hint = frame_plan.function_type_hint(closure_size)
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
            return func_val

        func_symbol = self._lambda_symbol()
        qualname = self._qualname_for_def("<lambda>")
        self._record_func_default_specs(func_symbol, node.args)
        self.funcs_map.setdefault(
            func_symbol,
            FuncInfo(
                params=[], param_types=[], return_abi="value", return_hint=None, ops=[]
            ),
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
        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(node))
        )
        cell_vars = self._callable_cell_vars(node)

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
            code_symbol=func_symbol,
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
            freevars=free_vars,
            cellvars=cell_vars,
        )

        func_params, parameter_bindings = self._function_transport_params(
            params,
            has_closure=has_closure,
        )
        prev_state = self._capture_function_state()
        self.current_class = None
        prev_first_param = self.current_method_first_param
        self.start_function(
            func_symbol,
            python_first_arg=self._python_first_positional_arg(node.args),
            params=func_params,
            type_facts_name=func_symbol,
            # A lambda body is a single expression and can never contain a
            # try/with statement, so it never pushes the exception-handler
            # stack — only the (always-present) function exception label and
            # post-may-raise checks are needed.
            has_exception_handlers=False,
        )
        self._inherit_free_var_import_resolution(free_vars, prev_state)
        self.parameter_bindings = parameter_bindings
        self.current_method_first_param = params[0] if params else None
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.compiler_bindings[_MOLT_CLOSURE_PARAM] = MoltValue(
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
            value = self._parameter_value(
                arg.arg,
                type_hint=hint or "Unknown",
            )
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        if not self.is_async():
            self._prebox_scope_cell_vars(cell_vars)
            # Lambda lowering retains its existing all-local boxing policy,
            # now backed by the runtime's dedicated closure-cell primitive.
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
        self._publish_python_frame_context()
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
            self._emit_raise_if_pending()
            self._emit_restore_exception_stack_depth(exit_baseline=False)
            self._emit_raise_if_pending()
        self._emit_return_value(val)
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_method_first_param = prev_first_param
        return func_val

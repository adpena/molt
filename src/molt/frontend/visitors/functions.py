"""FunctionVisitorMixin: function, lambda, and return lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py. Covers visit_FunctionDef,
visit_Lambda, and visit_Return. Async function/generator visitor methods live in
``async_gen.py``; semantic function-shape facts come from ``frontend.sema``.
"""

from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
)

from molt.compiler_analysis.python_lexical_scope import (
    expression_contains_yield,
    function_contains_yield,
)

import ast
from molt.python_private_names import python_definition_name

from molt.frontend._types import (
    _MOLT_CLOSURE_PARAM,
    GEN_CONTROL_SIZE,
    DeclaredClass,
    FuncInfo,
    MoltOp,
    MoltValue,
    FunctionDefinitionValues,
)
from molt.frontend.lowering.gpu_kernel_descriptor import (
    DESCRIPTOR_PUBLICATION_SYMBOL,
)
from molt.frontend.sema import stateful_function_frame_plan
from molt.compiler_analysis.python_function_kind import FunctionKind
from molt.compiler_analysis.python_identity_catalog_generated import PythonIdentity
from molt.frontend._mixin_base import GeneratorMixinBase


class FunctionVisitorMixin(GeneratorMixinBase):
    def _is_gpu_kernel_decorator(self, deco: ast.expr) -> bool:
        """Publish GPU metadata only for the resolved GPU decorator."""
        return (
            self.python_binding_index is not None
            and self.python_binding_index.expression_is(deco, PythonIdentity.GPU_KERNEL)
        )

    def _has_gpu_kernel_decorator(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> bool:
        """Return True if any decorator on *node* is @gpu.kernel."""
        return any(self._is_gpu_kernel_decorator(d) for d in node.decorator_list)

    def visit_Return(self, node: ast.Return) -> None:
        val = self.visit(node.value) if node.value else None
        if val is None:
            val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
        # RETURN evaluates an independent result before finally/with cleanup
        # and before frame publication/deactivation can release its slot.
        val = self._capture_expression_reference(val)
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
            # A generator's return reports completion at the return terminator.
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

        definition = self._prepare_function_definition(node)
        with self._type_parameter_scope(definition.type_parameter_bindings):
            self._emit_function_definition(node, definition)

    def _emit_function_definition(
        self, node: ast.FunctionDef, definition: FunctionDefinitionValues
    ) -> None:
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
        has_return = self._function_contains_return(node)
        func_name = node.name
        qualname = self._definition_qualname(node)
        if is_generator:
            func_symbol = self._source_definition_symbol(
                node, kind=FunctionKind.GENERATOR
            )
            self._record_func_default_specs(func_symbol, node.args)
            poll_func_name = f"{func_symbol}_poll"
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
            cell_plan = self._callable_cell_plan(node)
            cell_vars = cell_plan.cellvars

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
            name_layout = self._collect_callable_name_layout(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=node.body,
                free_vars=free_vars,
                cell_vars=cell_vars,
            )
            self._emit_function_metadata(
                func_val,
                code_symbol=poll_func_name,
                name=python_definition_name(node),
                qualname=qualname,
                trace_lineno=node.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                defaults=definition.defaults,
                docstring=ast.get_docstring(node, clean=False),
                execution_kind=FunctionKind.GENERATOR,
                varnames=list(name_layout.varnames),
                code_names=list(name_layout.names),
                freevars=free_vars,
                cellvars=cell_vars,
            )
            self._emit_function_annotate(func_val, node, definition.type_params)

            with self._function_state_scope() as prev_state:
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
                self.emit(
                    MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none"))
                )
                self._init_scope_async_locals(arg_nodes)
                self._prebox_scope_cell_vars(
                    cell_plan.captured, private_cells=cell_plan.private
                )
                if self.type_hint_policy == "check":
                    for arg in arg_nodes:
                        hint = self.explicit_type_hints.get(arg.arg)
                        if hint is not None:
                            self._emit_guard_type(
                                MoltValue(arg.arg, type_hint=hint), hint
                            )
                self._publish_python_frame_context()
                self._push_qualname(
                    python_definition_name(node), True, qualname=qualname
                )
                try:
                    for item in node.body:
                        self.visit(item)
                        if isinstance(item, (ast.Return, ast.Raise)):
                            break
                finally:
                    self._pop_qualname()
                self._emit_generator_falloff()
                self._spill_async_temporaries()
                closure_size = self._task_closure_size(
                    frame_plan.payload_slots,
                    include_gen_control=frame_plan.include_gen_control,
                )
                locals_layout = self._stateful_locals_layout(
                    frame_plan, params, free_vars
                )
            # Publish the final spilled-frame extent on the defining op.
            function_def.metadata = {
                **(function_def.metadata or {}),
                **frame_plan.callable_task_metadata(closure_size),
            }
            func_val.type_hint = frame_plan.function_type_hint(closure_size)
            self._emit_stateful_locals_register(locals_layout, poll_func_name)
            func_val = self._apply_function_decorators(func_val, definition)
            self._publish_definition_binding(func_name, func_val)
            self._record_source_app_callable(
                func_name,
                kind=FunctionKind.GENERATOR,
                symbol=poll_func_name,
                decorated=bool(node.decorator_list),
                has_closure=has_closure,
            )
            return None

        func_name = node.name
        func_symbol = self._source_definition_symbol(node)
        self._record_func_default_specs(func_symbol, node.args)
        self.funcs_map.setdefault(
            func_symbol,
            FuncInfo(
                params=[], param_types=[], return_abi="value", return_hint=None, ops=[]
            ),
        )
        if origin := self._gpu_body_origin_candidates.get(id(node)):
            self.funcs_map[func_symbol]["gpu_body_origin"] = dict(origin)
        self.funcs_map[func_symbol]["return_hint"] = self._normalized_return_hint(
            node.returns
        )
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

        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(node))
        )
        cell_plan = self._callable_cell_plan(node)
        cell_vars = cell_plan.cellvars

        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        elif self.current_func_name == "molt_main":
            self._register_source_callable(node, func_symbol)
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
        name_layout = self._collect_callable_name_layout(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=node.body,
            free_vars=free_vars,
            cell_vars=cell_vars,
        )
        code_slots = self._emit_function_metadata(
            func_val,
            code_symbol=func_symbol,
            name=python_definition_name(node),
            qualname=qualname,
            trace_lineno=node.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            defaults=definition.defaults,
            docstring=ast.get_docstring(node, clean=False),
            varnames=list(name_layout.varnames),
            code_names=list(name_layout.names),
            freevars=free_vars,
            cellvars=cell_vars,
        )
        is_gpu_kernel = self._has_gpu_kernel_decorator(node)
        self._emit_function_annotate(func_val, node, definition.type_params)

        func_params, parameter_bindings = self._function_transport_params(
            params,
            has_closure=has_closure,
        )
        with self._function_state_scope() as prev_state:
            # Extract type hints from parameter annotations for fast-path codegen.
            _param_type_hints: list[str] = []
            if self._hints_enabled():
                for arg in arg_nodes:
                    hint = (
                        self._annotation_to_hint(arg.annotation)
                        if arg.annotation
                        else None
                    )
                    wire_hint = self._wire_type_hint(hint) if hint is not None else None
                    _param_type_hints.append(wire_hint or "Any")
            self.start_function(
                func_symbol,
                python_first_arg=self._python_first_positional_arg(node.args),
                params=func_params,
                param_types=_param_type_hints if _param_type_hints else None,
                type_facts_name=func_name,
                needs_return_slot=has_return,
                has_exception_handlers=self._body_has_exception_handlers(node.body),
                code_slots=code_slots,
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
                    if (
                        isinstance(inferred, DeclaredClass)
                        and inferred.class_id in self.classes
                    ):
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
            self._prebox_scope_cell_vars(
                cell_plan.captured, private_cells=cell_plan.private
            )
            # Only box variables that genuinely need cells (closure-captured). Every
            # other local is unbound until its first store, as its home is: a read
            # that may precede that store loads the home, so no missing value seeds
            # its SSA transport.
            for name in sorted(self.scope_assigned):
                if name in self.closure_locals:
                    self._box_local(name)
            self._emit_frame_home_prologue([arg.arg for arg in arg_nodes])
            self._publish_python_frame_context()
            self._push_qualname(python_definition_name(node), True, qualname=qualname)
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
            elif not (
                self.current_ops and self.current_ops[-1].kind == FrontendKinds.ret
            ):
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self._emit_return_value(res)
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
            self._emit_runtime_call(
                DESCRIPTOR_PUBLICATION_SYMBOL,
                [func_val, descriptor_val],
                type_hint="None",
            )
        func_val = self._apply_function_decorators(func_val, definition)
        self._publish_definition_binding(func_name, func_val)
        self._record_source_app_callable(
            func_name,
            kind=FunctionKind.SYNC,
            symbol=func_symbol,
            decorated=bool(node.decorator_list),
            has_closure=has_closure,
        )
        return None

    def visit_Lambda(self, node: ast.Lambda) -> MoltValue:
        if expression_contains_yield(node.body):
            func_symbol = self._lambda_symbol(kind=FunctionKind.GENERATOR)
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

            defaults = self._emit_function_default_values(
                node.args.defaults, node.args.kw_defaults, kwonly_names
            )
            free_vars, free_var_hints, closure_val, has_closure = (
                self._capture_lexical_closure(self._cached_free_vars_raw(node))
            )
            cell_plan = self._callable_cell_plan(node)
            cell_vars = cell_plan.cellvars

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
            name_layout = self._collect_callable_name_layout(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=[ast.Expr(value=node.body)],
                free_vars=free_vars,
                cell_vars=cell_vars,
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
                defaults=defaults,
                docstring=None,
                execution_kind=FunctionKind.GENERATOR,
                varnames=list(name_layout.varnames),
                code_names=list(name_layout.names),
                freevars=free_vars,
                cellvars=cell_vars,
            )

            with self._function_state_scope() as prev_state:
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
                self.emit(
                    MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none"))
                )
                self._init_scope_async_locals(arg_nodes)
                self._prebox_scope_cell_vars(
                    cell_plan.captured, private_cells=cell_plan.private
                )
                if self.type_hint_policy == "check":
                    for arg in arg_nodes:
                        hint = self.explicit_type_hints.get(arg.arg)
                        if hint is not None:
                            self._emit_guard_type(
                                MoltValue(arg.arg, type_hint=hint), hint
                            )
                self._publish_python_frame_context()
                self._push_qualname("<lambda>", True)
                try:
                    return_node = ast.Return(value=node.body)
                    return_node = ast.copy_location(return_node, node.body)
                    self.visit(return_node)
                finally:
                    self._pop_qualname()
                self._emit_generator_falloff()
                self._spill_async_temporaries()
                closure_size = self._task_closure_size(
                    frame_plan.payload_slots,
                    include_gen_control=frame_plan.include_gen_control,
                )
                locals_layout = self._stateful_locals_layout(
                    frame_plan, params, free_vars
                )
            # Publish the final spilled-frame extent on the defining op.
            function_def.metadata = {
                **(function_def.metadata or {}),
                **frame_plan.callable_task_metadata(closure_size),
            }
            func_val.type_hint = frame_plan.function_type_hint(closure_size)
            self._emit_stateful_locals_register(locals_layout, poll_func_name)
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

        defaults = self._emit_function_default_values(
            node.args.defaults, node.args.kw_defaults, kwonly_names
        )
        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(node))
        )
        cell_plan = self._callable_cell_plan(node)
        cell_vars = cell_plan.cellvars

        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        elif self.current_func_name == "molt_main":
            self._register_source_callable(node, func_symbol)
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
        name_layout = self._collect_callable_name_layout(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=[ast.Expr(value=node.body)],
            free_vars=free_vars,
            cell_vars=cell_vars,
        )
        code_slots = self._emit_function_metadata(
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
            defaults=defaults,
            docstring=None,
            varnames=list(name_layout.varnames),
            code_names=list(name_layout.names),
            freevars=free_vars,
            cellvars=cell_vars,
        )

        func_params, parameter_bindings = self._function_transport_params(
            params,
            has_closure=has_closure,
        )
        with self._function_state_scope() as prev_state:
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
                code_slots=code_slots,
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
                    if (
                        isinstance(inferred, DeclaredClass)
                        and inferred.class_id in self.classes
                    ):
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
            self._prebox_scope_cell_vars(
                cell_plan.captured, private_cells=cell_plan.private
            )
            # Lambda lowering retains its existing all-local boxing policy,
            # now backed by the runtime's dedicated closure-cell primitive.
            for name in sorted(self.scope_assigned):
                self._box_local(name)
            self._emit_frame_home_prologue([arg.arg for arg in arg_nodes])
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
        return func_val

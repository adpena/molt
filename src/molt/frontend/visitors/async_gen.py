"""AsyncGenVisitorMixin: async function, async block, await, and yield lowering.

Move-only extraction from frontend/__init__.py. Covers visit_AsyncFunctionDef,
visit_AsyncWith, visit_AsyncFor, visit_Await, visit_Yield, and visit_YieldFrom.
Shared function, control-flow, and async-state helpers continue resolving through
the SimpleTIRGenerator MRO via self.<method>.
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
    GEN_SEND_OFFSET,
    GEN_THROW_OFFSET,
    GEN_YIELD_FROM_OFFSET,
    MoltOp,
    MoltValue,
    _MOLT_CLOSURE_PARAM,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AsyncGenVisitorMixin(_MixinBase):
    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        if self.current_func_name == "molt_main":
            new_globals = self._collect_global_decls(node.body)
            self.module_global_mutations.update(new_globals)
            for gname in new_globals:
                self.locals.pop(gname, None)
        needs_locals_cache = self._function_contains_locals_call(node)
        if self._function_contains_yield(node):
            if self._async_generator_contains_yield_from(node):
                raise SyntaxError("'yield from' inside async function")
            if self._async_generator_contains_return_value(node):
                raise SyntaxError("'return' with value in async generator")
            func_name = node.name
            qualname = self._qualname_for_def(func_name)
            func_symbol = self._function_symbol(func_name)
            if not self._has_typing_overload_decorator(node):
                self._record_func_default_specs(func_symbol, node.args)
            else:
                return None
            poll_func_name = f"{func_symbol}_poll"
            prev_func = self.current_func_name
            has_return = self._function_contains_return(node)
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

            func_kind = "AsyncGenClosureFunc" if has_closure else "AsyncGenFunc"
            payload_slots = len(params) + (1 if has_closure else 0)
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=True
            )
            self.globals[func_name] = MoltValue(
                func_name, type_hint=f"{func_kind}:{poll_func_name}:{closure_size}"
            )

            prev_state = self._capture_function_state()
            self.current_class = None
            prev_first_param = self.current_method_first_param
            self.start_function(
                poll_func_name,
                params=["self"],
                type_facts_name=func_name,
                needs_return_slot=has_return,
            )
            self.current_method_first_param = params[0] if params else None
            self.async_context = True
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
                    if hint is not None:
                        self.async_local_hints[arg.arg] = hint
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
            asyncgen_public_locals = self._async_locals_public_entries()
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_method_first_param = prev_first_param

            func_hint = f"{func_kind}:{poll_func_name}:{closure_size}"
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
                        kind="FUNC_NEW",
                        args=[func_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and self._signature_contains_yield(
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
                is_async_generator=True,
                poll_fn_symbol=poll_func_name,
                varnames=varnames,
                code_names=self._collect_code_names_for_body(
                    node.body,
                    varnames=varnames,
                    free_vars=free_vars,
                ),
            )
            names_vals: list[MoltValue] = []
            offsets_vals: list[MoltValue] = []
            for local_name, offset in asyncgen_public_locals:
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
                    kind="ASYNCGEN_LOCALS_REGISTER",
                    args=[poll_func_name, names_tuple, offsets_tuple],
                    result=MoltValue("none"),
                )
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, node)
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
            self._emit_module_attr_set(func_name, func_val)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            self.current_class = None
            func_params = params
            if has_closure:
                func_params = [_MOLT_CLOSURE_PARAM] + params
            self.start_function(
                func_symbol,
                params=func_params,
                type_facts_name=func_name,
            )
            if has_closure:
                self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                    _MOLT_CLOSURE_PARAM, type_hint="tuple"
                )
            for idx, arg in enumerate(arg_nodes):
                hint = None
                if idx == 0 and arg.arg == "self":
                    hint = None
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
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
            args = [self.locals[arg.arg] for arg in arg_nodes]
            if has_closure:
                args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
            gen_val = MoltValue(self.next_var(), type_hint="generator")
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[poll_func_name, closure_size] + args,
                    result=gen_val,
                    metadata={"task_kind": "generator"},
                )
            )
            res = MoltValue(self.next_var(), type_hint="async_generator")
            self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            if node.decorator_list:
                decorated = func_val
                for deco in reversed(node.decorator_list):
                    decorator_val = self.visit(deco)
                    if decorator_val is None:
                        raise NotImplementedError("Unsupported decorator")
                    res_val = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[decorator_val, decorated],
                            result=res_val,
                        )
                    )
                    decorated = res_val
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
        qualname = self._qualname_for_def(func_name)
        func_symbol = self._function_symbol(func_name)
        if not self._has_typing_overload_decorator(node):
            self._record_func_default_specs(func_symbol, node.args)
        else:
            return None
        poll_func_name = f"{func_symbol}_poll"
        prev_func = self.current_func_name
        has_return = self._function_contains_return(node)
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

        # Add to globals to support calls from other scopes
        func_kind = "AsyncClosureFunc" if has_closure else "AsyncFunc"
        payload_slots = len(params) + (1 if has_closure else 0)
        closure_size = self._task_closure_size(payload_slots, include_gen_control=False)
        self.globals[func_name] = MoltValue(
            func_name, type_hint=f"{func_kind}:{poll_func_name}:{closure_size}"
        )  # Placeholder size

        prev_state = self._capture_function_state()
        self.current_class = None
        prev_first_param = self.current_method_first_param
        self.start_function(
            poll_func_name,
            params=["self"],
            type_facts_name=func_name,
            needs_return_slot=has_return,
        )
        self.current_method_first_param = params[0] if params else None
        self.async_context = True
        self.global_decls = self._collect_global_decls(node.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
        assigned = self._collect_assigned_names(node.body)
        self.del_targets = self._collect_deleted_names(node.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        if has_closure:
            self.async_closure_offset = 0
            self.async_locals_base = 8
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        for i, arg in enumerate(arg_nodes):
            self.async_locals[arg.arg] = self.async_locals_base + i * 8
            if self._hints_enabled():
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
                if hint is not None:
                    self.async_local_hints[arg.arg] = hint
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
        finally:
            self._pop_qualname()
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self._emit_return_value(res)
            self._emit_return_label()
        else:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self._spill_async_temporaries()
        closure_size = self._task_closure_size(payload_slots, include_gen_control=False)
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_method_first_param = prev_first_param
        func_hint = f"{func_kind}:{poll_func_name}:{closure_size}"
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
        if self.in_generator and self._signature_contains_yield(
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
            is_coroutine=True,
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
        closure_size_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[closure_size], result=closure_size_val))
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
        self._emit_module_attr_set(func_name, func_val)

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        self.current_class = None
        func_params = params
        if has_closure:
            func_params = [_MOLT_CLOSURE_PARAM] + params
        self.start_function(
            func_symbol,
            params=func_params,
            type_facts_name=func_name,
        )
        if has_closure:
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        for idx, arg in enumerate(arg_nodes):
            hint = None
            if idx == 0 and arg.arg == "self":
                hint = None
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
                elif hint is None:
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
        args = [self.locals[arg.arg] for arg in arg_nodes]
        if has_closure:
            args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
        res = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_func_name, closure_size] + args,
                result=res,
                metadata={"task_kind": "coroutine"},
            )
        )
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
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

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        if not self.is_async():
            raise NotImplementedError("async with is only supported in async functions")
        if len(node.items) != 1:
            nested = ast.AsyncWith(
                items=node.items[1:],
                body=node.body,
                type_comment=None,
            )
            ast.copy_location(nested, node)
            outer = ast.AsyncWith(
                items=[node.items[0]],
                body=[nested],
                type_comment=node.type_comment,
            )
            ast.copy_location(outer, node)
            return self.visit_AsyncWith(outer)

        item = node.items[0]
        ctx_val = self.visit(item.context_expr)
        if ctx_val is None:
            self._bridge_fallback(
                node,
                "async with",
                impact="high",
                alternative="use contextlib.nullcontext for now",
                detail="context expression did not lower",
            )
            return None

        ctx_slot = self._async_local_offset(
            f"__async_with_ctx_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", ctx_slot, ctx_val],
                result=MoltValue("none"),
            )
        )

        aenter_fn = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_SPECIAL_OBJ",
                args=[ctx_val, "__aenter__"],
                result=aenter_fn,
            )
        )
        aenter_call = self._emit_call_bound_or_func(aenter_fn, [])
        self._emit_raise_if_pending()
        enter_val = self._emit_await_value(aenter_call)
        if item.optional_vars is not None:
            self._emit_assign_target(item.optional_vars, enter_val, None)

        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_end_label = self.next_label()
        self.try_end_labels.append(try_end_label)
        self.emit(
            MoltOp(
                kind="TRY_START",
                args=[],
                result=MoltValue("none"),
                metadata={"try_region_id": try_end_label},
            )
        )
        self.control_flow_depth += 1
        # async-with: see _visit_loop_body for snapshot rationale.
        unbound_snapshot = set(self.unbound_check_names)
        try:
            self._visit_block(node.body)
        finally:
            self.unbound_check_names = unbound_snapshot
            self.control_flow_depth -= 1
        self.try_end_labels.pop()
        self.emit(MoltOp(kind="LABEL", args=[try_end_label], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="TRY_END",
                args=[],
                result=MoltValue("none"),
                metadata={"try_region_id": try_end_label},
            )
        )
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)

        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
        pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))

        ctx_reload = MoltValue(self.next_var(), type_hint=ctx_val.type_hint)
        self.emit(
            MoltOp(kind="LOAD_CLOSURE", args=["self", ctx_slot], result=ctx_reload)
        )
        aexit_fn = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_SPECIAL_OBJ",
                args=[ctx_reload, "__aexit__"],
                result=aexit_fn,
            )
        )

        self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
        exc_slot = self._async_local_offset(
            f"__async_with_exc_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", exc_slot, exc_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="EXCEPTION_CONTEXT_SET",
                args=[exc_val],
                result=MoltValue("none"),
            )
        )
        exc_type = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="TYPE_OF", args=[exc_val], result=exc_type))
        tb_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=tb_val))
        aexit_call = self._emit_call_bound_or_func(
            aexit_fn, [exc_type, exc_val, tb_val]
        )
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        aexit_res = self._emit_await_value(aexit_call, raise_pending=False)
        self._emit_raise_if_pending(emit_exit=True, force_exit=True)
        not_res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[aexit_res], result=not_res))
        is_truthy = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[not_res], result=is_truthy))
        self.emit(MoltOp(kind="IF", args=[is_truthy], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        exc_reload = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", exc_slot],
                result=exc_reload,
            )
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_reload], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True, force_exit=True)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        aexit_call = self._emit_call_bound_or_func(
            aexit_fn, [none_val, none_val, none_val]
        )
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        self._emit_await_value(aexit_call, raise_pending=False)
        self._emit_raise_if_pending(emit_exit=True, force_exit=True)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.try_suppress_depth = prior_suppress
        return None

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        if not self.is_async():
            raise NotImplementedError("async for is only supported in async functions")
        iterable = self.visit(node.iter)
        if iterable is None:
            raise NotImplementedError("Unsupported iterable in async for loop")
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_for_iter_{len(self.async_locals)}"
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
            f"__async_for_sentinel_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        break_slot = None
        if node.orelse:
            break_slot = self._async_local_offset(
                f"__async_for_break_{len(self.async_locals)}"
            )
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", break_slot, break_init],
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
        self._emit_assign_target(node.target, item_val, None)
        guard_map = self._emit_hoisted_loop_guards(node.body)
        body_terminated = self._visit_loop_body(
            node.body, guard_map, loop_break_flag=break_slot
        )
        if not body_terminated:
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        if node.orelse:
            break_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", break_slot],
                    result=break_val,
                )
            )
            should_run = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[break_val], result=should_run))
            self.emit(MoltOp(kind="IF", args=[should_run], result=MoltValue("none")))
            self._visit_block(node.orelse)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return None

    def visit_Await(self, node: ast.Await) -> Any:
        if (
            isinstance(node.value, ast.Call)
            and isinstance(node.value.func, ast.Name)
            and node.value.func.id == "anext"
        ):
            if node.value.keywords or len(node.value.args) not in (1, 2):
                raise NotImplementedError("anext expects 1 or 2 positional arguments")
            iter_obj = self.visit(node.value.args[0])
            if iter_obj is None:
                raise NotImplementedError("Unsupported iterator in anext()")
            has_default = len(node.value.args) == 2
            default_val = self.visit(node.value.args[1]) if has_default else None
            return self._emit_await_anext(
                iter_obj, default_val=default_val, has_default=has_default
            )
        if not self.is_async():
            coro = self.visit(node.value)
            coro = self._emit_awaitable_transform(coro)
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[coro], result=res))
            self._emit_raise_if_pending()
            return res
        self.state_count += 1
        pending_state_id = self.state_count
        self.emit(
            MoltOp(
                kind="STATE_LABEL", args=[pending_state_id], result=MoltValue("none")
            )
        )
        pending_state_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="CONST", args=[pending_state_id], result=pending_state_val)
        )
        awaitable_slot = None
        if self.is_async():
            awaitable_slot = self._async_local_offset(
                f"__await_future_{len(self.async_locals)}"
            )
            awaitable_cached = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", awaitable_slot],
                    result=awaitable_cached,
                )
            )
            none_cached = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_cached))
            is_none_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS",
                    args=[awaitable_cached, none_cached],
                    result=is_none_cached,
                )
            )
            zero_cached = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=zero_cached))
            is_zero_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS",
                    args=[awaitable_cached, zero_cached],
                    result=is_zero_cached,
                )
            )
            is_empty_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="OR",
                    args=[is_none_cached, is_zero_cached],
                    result=is_empty_cached,
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_empty_cached], result=MoltValue("none"))
            )
            awaitable_new = self.visit(node.value)
            awaitable_new = self._emit_awaitable_transform(awaitable_new)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", awaitable_slot, awaitable_new],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            coro = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", awaitable_slot],
                    result=coro,
                )
            )
        result_slot = self._async_local_offset(
            f"__await_result_{len(self.async_locals)}"
        )
        result_slot_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[result_slot], result=result_slot_val))
        self.state_count += 1
        next_state_id = self.state_count
        res_placeholder = MoltValue(self.next_var(), type_hint="Any")
        with self._suppress_check_exception():
            self.emit(
                MoltOp(
                    kind="STATE_TRANSITION",
                    args=[coro, result_slot_val, pending_state_val, next_state_id],
                    result=res_placeholder,
                )
            )
            if awaitable_slot is not None:
                cleared_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", awaitable_slot, cleared_val],
                        result=MoltValue("none"),
                    )
                )
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", result_slot],
                    result=res,
                )
            )
            self._emit_raise_if_pending()
        return res

    def visit_Yield(self, node: ast.Yield) -> Any:
        if not self.in_generator:
            raise NotImplementedError("yield outside of generator")
        if node.value is None:
            value = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=value))
        else:
            value = self.visit(node.value)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=done))
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=[value, done], result=pair))
        self.state_count += 1
        resume_state = self.state_count
        self.emit(
            MoltOp(
                kind="STATE_YIELD",
                args=[pair, resume_state],
                result=MoltValue("none"),
            )
        )
        self._emit_state_yield_resume_entry(resume_state)
        throw_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_THROW_OFFSET],
                result=throw_val,
            )
        )
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[throw_val, none_val], result=is_none))
        not_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=not_none))
        self.emit(MoltOp(kind="IF", args=[not_none], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_THROW_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="RAISE", args=[throw_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_SEND_OFFSET],
                result=res,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_SEND_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        return res

    def visit_YieldFrom(self, node: ast.YieldFrom) -> Any:
        if not self.in_generator:
            raise NotImplementedError("yield from outside of generator")
        iterable = self.visit(node.value)
        if iterable is None:
            raise NotImplementedError("yield from operand unsupported")
        iter_obj = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=iter_obj))
        is_gen = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS_GENERATOR", args=[iter_obj], result=is_gen))
        pair = self._emit_iter_next_checked(iter_obj)
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_YIELD_FROM_OFFSET, iter_obj],
                result=MoltValue("none"),
            )
        )
        iter_slot = None
        is_gen_slot = None
        pair_slot = None
        if self.is_async():
            iter_slot = self._async_local_offset(f"__yf_iter_{len(self.async_locals)}")
            is_gen_slot = self._async_local_offset(
                f"__yf_is_gen_{len(self.async_locals)}"
            )
            pair_slot = self._async_local_offset(f"__yf_pair_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", iter_slot, iter_obj],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", is_gen_slot, is_gen],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )

        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        if iter_slot is not None:
            iter_obj = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", iter_slot],
                    result=iter_obj,
                )
            )
            is_gen = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", is_gen_slot],
                    result=is_gen,
                )
            )
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", pair_slot],
                    result=pair,
                )
            )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        value = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=value))
        yielded = MoltValue(self.next_var(), type_hint="tuple")
        done_false = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=done_false))
        self.emit(MoltOp(kind="TUPLE_NEW", args=[value, done_false], result=yielded))
        self.state_count += 1
        resume_state = self.state_count
        self.emit(
            MoltOp(
                kind="STATE_YIELD",
                args=[yielded, resume_state],
                result=MoltValue("none"),
            )
        )
        self._emit_state_yield_resume_entry(resume_state)
        if iter_slot is not None:
            iter_obj = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", iter_slot],
                    result=iter_obj,
                )
            )
            is_gen = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", is_gen_slot],
                    result=is_gen,
                )
            )
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", pair_slot],
                    result=pair,
                )
            )
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        pending_throw = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_THROW_OFFSET],
                result=pending_throw,
            )
        )
        throw_is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="IS", args=[pending_throw, none_val], result=throw_is_none)
        )
        throw_pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[throw_is_none], result=throw_pending))
        self.emit(MoltOp(kind="IF", args=[throw_pending], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_THROW_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="IF", args=[is_gen], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="GEN_THROW",
                args=[iter_obj, pending_throw],
                result=pair,
            )
        )
        if pair_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="RAISE", args=[pending_throw], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        pending_send = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_SEND_OFFSET],
                result=pending_send,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_SEND_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        send_is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[pending_send, none_val], result=send_is_none))
        self.emit(MoltOp(kind="IF", args=[send_is_none], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        if pair_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_gen], result=MoltValue("none")))
        self.emit(MoltOp(kind="GEN_SEND", args=[iter_obj, pending_send], result=pair))
        if pair_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        err_val = self._emit_exception_new(
            "TypeError", "can't send non-None to a non-generator iterator"
        )
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_YIELD_FROM_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        if pair_slot is not None:
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", pair_slot],
                    result=pair,
                )
            )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=result))
        return result

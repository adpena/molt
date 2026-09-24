"""ClassMethodCompilationMixin: class method/function lowering authority.

Owns descriptor classification, method default specs, sync/generator/async
method compilation, implicit ``__class__`` closure computation, and inline
method body proofs for class definitions. ``classes.py`` keeps class object,
layout, namespace, and dataclass construction authority.
"""

from __future__ import annotations

import ast
from typing import Literal, cast

from molt.frontend._mixin_base import GeneratorMixinBase
from molt.compiler_analysis.python_inlining import (
    inline_expression_is_frame_independent,
)
from molt.frontend._types import (
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    MethodInfo,
    MethodDescriptor,
    MoltOp,
    MoltValue,
    _ClassNsScope,
    _MOLT_CLOSURE_PARAM,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.sema import (
    FunctionKind,
    async_generator_contains_return_value,
    async_generator_contains_yield_from,
    function_contains_yield,
    signature_contains_yield,
    stateful_function_frame_plan,
)


class ClassMethodCompilationMixin(GeneratorMixinBase):
    def _emit_class_function_definition(
        self, scope: _ClassNsScope, item: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> None:
        """Create and publish one method at its actual class-body source point.

        Direct definitions and definitions under control flow share this path.
        Decorators/defaults observe the class mapping; bodies capture the
        enclosing lexical frame and the class's single implicit cell.
        """
        class_node = scope.class_node
        if class_node is None:
            raise FrontendRejection(
                Diagnostic.INTERNAL_INVARIANT, "Method definition has no class owner"
            )
        if self._has_typing_overload_decorator(item):
            return
        decorators: list[tuple[MoltValue, int | None]] = []
        suspend = self.in_generator and signature_contains_yield(
            decorators=item.decorator_list, args=item.args, returns=item.returns
        )
        for expression in item.decorator_list:
            decorator = self.visit(expression)
            if decorator is None:
                raise FrontendRejection(
                    Diagnostic.SYNTAX_FORM, "Unsupported method decorator"
                )
            decorators.append(
                (decorator, self._spill_async_value(decorator) if suspend else None)
            )

        previous_method = scope.methods.get(item.name)
        if isinstance(item, ast.AsyncFunctionDef):
            info = self._compile_class_async_method(class_node, item)
        elif function_contains_yield(item):
            info = self._compile_class_generator_method(class_node, item)
        else:
            info = self._compile_class_method(class_node, item)

        _, _, kwonly, _, _ = self._split_function_args(item.args)
        function = self._emit_function_defaults(
            info["func"],
            item.args.defaults,
            item.args.kw_defaults,
            [argument.arg for argument in kwonly],
        )
        info["func"] = function
        self._emit_function_annotate(function, item)
        value = function
        for decorator, spill in reversed(decorators):
            if spill is not None:
                decorator = self._reload_async_value(spill, decorator.type_hint)
            value = self._emit_call_bound_or_func(decorator, [value])
        info["attr"] = value
        self._class_ns_store(scope, item.name, value)
        self.locals[item.name] = value
        if (
            item.name not in scope.global_names
            and item.name not in scope.nonlocal_names
        ):
            if info["descriptor"] == "property_update" and previous_method is not None:
                previous_method["attr"] = value
                scope.methods[item.name] = previous_method
            else:
                scope.methods[item.name] = info

    def _property_field_from_method(self, node: ast.FunctionDef) -> str | None:
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        if not isinstance(stmt, ast.Return):
            return None
        value = stmt.value
        if not isinstance(value, ast.Attribute):
            return None
        if not isinstance(value.value, ast.Name):
            return None
        if value.value.id != "self":
            return None
        return value.attr

    def _emit_function_defaults(
        self,
        func_val: MoltValue,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        kwonly_params: list[str],
    ) -> MoltValue:
        func_val, defaults_val, kwdefaults_val = self._emit_function_default_values(
            func_val, default_exprs, kw_default_exprs, kwonly_params
        )
        self._emit_runtime_call(
            "molt_function_set_defaults",
            [func_val, defaults_val, kwdefaults_val],
            type_hint="None",
        )
        return func_val

    def _extract_inline_return(
        self, item: ast.FunctionDef, params: list[str]
    ) -> ast.expr | None:
        """Project a complete callback-free, parameter-only return expression."""
        body = item.body
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)
        ):
            body = body[1:]
        if (
            len(body) != 1
            or not isinstance(body[0], ast.Return)
            or body[0].value is None
        ):
            return None
        expression = body[0].value
        return (
            expression
            if inline_expression_is_frame_independent(expression, params)
            else None
        )

    def _extract_inline_init_assigns(
        self, item: ast.FunctionDef, params: list[str]
    ) -> list[tuple[str, ast.expr]] | None:
        """Project callback-free initialization with one first write per field."""
        if not params:
            return None
        body = item.body
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)
        ):
            body = body[1:]
        assigns: list[tuple[str, ast.expr]] = []
        seen: set[str] = set()
        for statement in body:
            if isinstance(statement, ast.Return):
                if statement.value is None or (
                    isinstance(statement.value, ast.Constant)
                    and statement.value.value is None
                ):
                    break
                return None
            if not isinstance(statement, ast.Assign) or len(statement.targets) != 1:
                return None
            target = statement.targets[0]
            if (
                not isinstance(target, ast.Attribute)
                or not isinstance(target.value, ast.Name)
                or target.value.id != params[0]
                or target.attr in seen
                or not inline_expression_is_frame_independent(statement.value, params)
            ):
                return None
            seen.add(target.attr)
            assigns.append((target.attr, statement.value))
        return assigns

    def _class_method_receiver_hint(
        self, class_name: str, descriptor: MethodDescriptor, parameter_index: int
    ) -> str | None:
        """Infer a receiver only when the descriptor proves its binding rule."""
        if parameter_index == 0 and descriptor in {
            "function",
            "classmethod",
            "property",
            "property_update",
        }:
            return class_name
        return None

    def _class_method_descriptor(
        self, class_node: ast.ClassDef, item: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> tuple[MethodDescriptor, Literal["setter", "deleter"] | None]:
        """Project descriptor hints only from proved source bindings.

        Runtime decorator evaluation remains authoritative; this classifier
        never manufactures or applies a descriptor from its spelling.
        """
        if not item.decorator_list:
            if item.name in {"__init_subclass__", "__class_getitem__"}:
                return "classmethod", None
            if item.name == "__new__":
                return "staticmethod", None
            return "function", None
        if len(item.decorator_list) != 1 or self.classes[class_node.name].get(
            "dynamic"
        ):
            return "decorated", None
        decorator = item.decorator_list[0]
        if isinstance(decorator, ast.Name) and decorator.id in {
            "classmethod",
            "staticmethod",
            "property",
        }:
            index = self.python_binding_index
            fact = index.expression_fact(decorator) if index is not None else None
            if (
                fact is not None
                and not fact.binding_invalidated
                and not fact.binding_is_bound
            ):
                return cast(MethodDescriptor, decorator.id), None
        if (
            isinstance(decorator, ast.Attribute)
            and isinstance(decorator.value, ast.Name)
            and decorator.value.id == item.name
            and decorator.attr in {"setter", "deleter"}
        ):
            previous = self.classes[class_node.name]["methods"].get(item.name)
            if previous is not None and previous["descriptor"] == "property":
                return "property_update", decorator.attr
        return "decorated", None

    def _compile_class_generator_method(
        self, class_node: ast.ClassDef, item: ast.FunctionDef
    ) -> MethodInfo:
        method_name = item.name
        descriptor, property_update = self._class_method_descriptor(class_node, item)
        property_field = None
        if descriptor == "property":
            property_field = self._property_field_from_method(item)
        return_hint = self._annotation_to_hint(item.returns)
        if (
            return_hint
            and return_hint[:1] in {"'", '"'}
            and return_hint[-1:] == return_hint[:1]
        ):
            return_hint = return_hint[1:-1]
        if return_hint == "Self":
            return_hint = class_node.name
        method_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
        self._record_func_default_specs(method_symbol, item.args)
        poll_symbol = f"{method_symbol}_poll"
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(item.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(item.args)
        default_specs = self._default_specs_from_args(item.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if item.args.vararg is not None:
            arg_nodes.append(item.args.vararg)
        arg_nodes.extend(kwonly)
        if item.args.kwarg is not None:
            arg_nodes.append(item.args.kwarg)
        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(item))
        )
        cell_vars = self._callable_cell_vars(item)
        has_return = self._function_contains_return(item)
        frame_plan = stateful_function_frame_plan(
            kind=FunctionKind.GENERATOR,
            poll_symbol=poll_symbol,
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
                [poll_symbol, len(params), closure_val]
                if has_closure and closure_val is not None
                else [poll_symbol, len(params)]
            ),
            result=func_val,
        )
        self.emit(function_def)
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=item.decorator_list,
            args=item.args,
            returns=item.returns,
        ):
            func_spill = self._spill_async_value(func_val)
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=item.body,
        )
        self._emit_function_metadata(
            func_val,
            code_symbol=poll_symbol,
            name=method_name,
            qualname=self._qualname_for_def(method_name),
            trace_lineno=item.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=ast.get_docstring(item, clean=False),
            execution_kind=FunctionKind.GENERATOR,
            varnames=varnames,
            freevars=free_vars,
            cellvars=cell_vars,
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        prev_class = self.current_class
        prev_first_param = self.current_method_first_param
        self.current_class = class_node.name
        self.current_method_first_param = params[0] if params else None
        self.start_function(
            poll_symbol,
            stateful_frame_plan=frame_plan,
            python_first_arg=self._python_first_positional_arg(item.args),
            params=["self"],
            compiler_params={"self"},
            type_facts_name=f"{class_node.name}.{method_name}",
            needs_return_slot=has_return,
        )
        self.global_decls = self._collect_global_decls(item.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
        assigned = self._collect_assigned_names(item.body)
        self.del_targets = self._collect_deleted_names(item.body)
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
            hint = self._class_method_receiver_hint(class_node.name, descriptor, i)
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
            if hint is not None:
                self.async_public_hints[arg.arg] = hint
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        self._init_scope_async_locals(arg_nodes)
        self._prebox_scope_cell_vars(cell_vars)
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
        self._publish_python_frame_context()
        self._push_qualname(method_name, True)
        try:
            for stmt in item.body:
                self.visit(stmt)
                if isinstance(stmt, (ast.Return, ast.Raise)):
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
        gen_public_locals = self._async_locals_public_entries()
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_class = prev_class
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
                args=[poll_symbol, names_tuple, offsets_tuple],
                result=MoltValue("none"),
            )
        )
        method_attr = func_val
        return {
            "func": func_val,
            "attr": method_attr,
            "descriptor": descriptor,
            "return_hint": return_hint,
            "param_count": len(params),
            "defaults": default_specs,
            "posonly_count": len(posonly),
            "kwonly_count": len(kwonly),
            "has_vararg": vararg is not None,
            "has_varkw": varkw is not None,
            "has_closure": has_closure,
            "property_field": property_field,
            "property_update": property_update,
        }

    def _compile_class_method(
        self, class_node: ast.ClassDef, item: ast.FunctionDef
    ) -> MethodInfo:
        method_name = item.name
        descriptor, property_update = self._class_method_descriptor(class_node, item)
        property_field = None
        if descriptor == "property":
            property_field = self._property_field_from_method(item)
        return_hint = self._annotation_to_hint(item.returns)
        if (
            return_hint
            and return_hint[:1] in {"'", '"'}
            and return_hint[-1:] == return_hint[:1]
        ):
            return_hint = return_hint[1:-1]
        if return_hint == "Self":
            return_hint = class_node.name
        method_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
        self._record_func_default_specs(method_symbol, item.args)
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(item.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(item.args)
        default_specs = self._default_specs_from_args(item.args)
        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(item))
        )
        cell_vars = self._callable_cell_vars(item)

        func_hint = f"Func:{method_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{method_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[method_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW",
                    args=[method_symbol, len(params)],
                    result=func_val,
                )
            )
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=item.decorator_list,
            args=item.args,
            returns=item.returns,
        ):
            func_spill = self._spill_async_value(func_val)
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=item.body,
        )
        self._emit_function_metadata(
            func_val,
            code_symbol=method_symbol,
            name=method_name,
            qualname=self._qualname_for_def(method_name),
            trace_lineno=item.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=ast.get_docstring(item, clean=False),
            varnames=varnames,
            freevars=free_vars,
            cellvars=cell_vars,
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        prev_class = self.current_class
        prev_first_param = self.current_method_first_param
        self.current_class = class_node.name
        self.current_method_first_param = params[0] if params else None
        method_params, parameter_bindings = self._function_transport_params(
            params,
            has_closure=has_closure,
        )
        self.start_function(
            method_symbol,
            python_first_arg=self._python_first_positional_arg(item.args),
            params=method_params,
            type_facts_name=f"{class_node.name}.{method_name}",
            needs_return_slot=False,
            has_exception_handlers=self._body_has_exception_handlers(item.body),
        )
        self.parameter_bindings = parameter_bindings
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.compiler_bindings[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if item.args.vararg is not None:
            arg_nodes.append(item.args.vararg)
        arg_nodes.extend(kwonly)
        if item.args.kwarg is not None:
            arg_nodes.append(item.args.kwarg)
        self.global_decls = self._collect_global_decls(item.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
        assigned = self._collect_assigned_names(item.body)
        self.del_targets = self._collect_deleted_names(item.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        for idx, arg in enumerate(arg_nodes):
            hint = self._class_method_receiver_hint(class_node.name, descriptor, idx)
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
            value = self._parameter_value(
                arg.arg,
                type_hint=hint or "Unknown",
            )
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in item.args.args:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        self._prebox_scope_cell_vars(cell_vars)
        # Class-method lowering retains its existing all-local boxing policy,
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
        self._publish_python_frame_context()
        self._push_qualname(method_name, True)
        try:
            for stmt in item.body:
                self.visit(stmt)
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
            self._emit_normal_return_terminator(res)
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_class = prev_class
        self.current_method_first_param = prev_first_param
        method_attr = func_val
        # Elide only frames whose entire inline expression is proven unobservable.
        inline_return = None
        inline_init_assigns: list[tuple[str, ast.expr]] | None = None
        if (
            descriptor == "function"
            and not free_vars
            and not (vararg is not None or varkw is not None)
            and not kwonly_names
        ):
            inline_return = self._extract_inline_return(item, params)
            # Detect __init__-style trivially-inlinable bodies — a
            # sequence of `self.attr = <pure expr>` assignments
            # where <pure expr> only references params/constants
            # and the targets are attributes of the first param.
            # The class-instantiation fold (visit_Call) inlines
            # these as a sequence of STORE_ATTR ops directly on
            # the freshly-allocated instance, eliminating the
            # __init__ CALL frame setup that dominates
            # bench_struct's per-iter cost.
            if method_name == "__init__":
                inline_init_assigns = self._extract_inline_init_assigns(item, params)
        return {
            "func": func_val,
            "attr": method_attr,
            "descriptor": descriptor,
            "return_hint": return_hint,
            "param_count": len(params),
            "defaults": default_specs,
            "posonly_count": len(posonly),
            "kwonly_count": len(kwonly),
            "has_vararg": vararg is not None,
            "has_varkw": varkw is not None,
            "has_closure": has_closure,
            "property_field": property_field,
            "property_update": property_update,
            "inline_return": inline_return,
            "inline_params": (
                params
                if (inline_return is not None or inline_init_assigns is not None)
                else None
            ),
            "inline_init_assigns": inline_init_assigns,
        }

    def _compile_class_async_method(
        self, class_node: ast.ClassDef, item: ast.AsyncFunctionDef
    ) -> MethodInfo:
        method_name = item.name
        descriptor, property_update = self._class_method_descriptor(class_node, item)
        is_async_gen = function_contains_yield(item)
        if is_async_gen:
            if async_generator_contains_yield_from(item):
                raise SyntaxError("'yield from' inside async function")
            if async_generator_contains_return_value(item):
                raise SyntaxError("'return' with value in async generator")
            method_name = item.name
            property_field = None
            return_hint = self._annotation_to_hint(item.returns)
            if (
                return_hint
                and return_hint[:1] in {"'", '"'}
                and return_hint[-1:] == return_hint[:1]
            ):
                return_hint = return_hint[1:-1]
            if return_hint == "Self":
                return_hint = class_node.name
            method_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
            poll_symbol = f"{method_symbol}_poll"
            self._record_func_default_specs(poll_symbol, item.args)
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            default_specs = self._default_specs_from_args(item.args)
            free_vars, free_var_hints, closure_val, has_closure = (
                self._capture_lexical_closure(self._cached_free_vars_raw(item))
            )
            cell_vars = self._callable_cell_vars(item)
            has_return = self._function_contains_return(item)
            frame_plan = stateful_function_frame_plan(
                kind=FunctionKind.ASYNC_GENERATOR,
                poll_symbol=poll_symbol,
                param_count=len(params),
                has_closure=has_closure,
                gen_control_size=GEN_CONTROL_SIZE,
            )

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = class_node.name
            self.current_method_first_param = params[0] if params else None
            self.start_function(
                poll_symbol,
                stateful_frame_plan=frame_plan,
                python_first_arg=self._python_first_positional_arg(item.args),
                params=["self"],
                compiler_params={"self"},
                type_facts_name=f"{class_node.name}.{method_name}",
                needs_return_slot=has_return,
            )
            self.async_context = True
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
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
                hint = self._class_method_receiver_hint(class_node.name, descriptor, i)
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                if hint is not None:
                    self.async_public_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            self._prebox_scope_cell_vars(cell_vars)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._publish_python_frame_context()
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
                    if isinstance(stmt, (ast.Return, ast.Raise)):
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
            asyncgen_public_locals = self._async_locals_public_entries()
            closure_size = self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            )
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param

            func_hint = frame_plan.function_type_hint(closure_size)
            func_val = MoltValue(self.next_var(), type_hint=func_hint)
            function_def = MoltOp(
                kind=(
                    "FUNC_NEW_CLOSURE"
                    if has_closure and closure_val is not None
                    else "FUNC_NEW"
                ),
                args=(
                    [poll_symbol, len(params), closure_val]
                    if has_closure and closure_val is not None
                    else [poll_symbol, len(params)]
                ),
                result=func_val,
                metadata=frame_plan.callable_task_metadata(closure_size),
            )
            self.emit(function_def)
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(func_val)
            varnames = self._collect_varnames_for_body(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=item.body,
            )
            self._emit_function_metadata(
                func_val,
                code_symbol=poll_symbol,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=[],
                kw_default_exprs=[],
                docstring=ast.get_docstring(item, clean=False),
                execution_kind=FunctionKind.ASYNC_GENERATOR,
                varnames=varnames,
                freevars=free_vars,
                cellvars=cell_vars,
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
                    args=[poll_symbol, names_tuple, offsets_tuple],
                    result=MoltValue("none"),
                )
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)

            method_attr = func_val
            return {
                "func": func_val,
                "attr": method_attr,
                "descriptor": descriptor,
                "return_hint": return_hint,
                "param_count": len(params),
                "defaults": default_specs,
                "posonly_count": len(posonly),
                "kwonly_count": len(kwonly),
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                "property_field": property_field,
                "property_update": property_update,
            }
        method_name = item.name
        property_field = None
        return_hint = self._annotation_to_hint(item.returns)
        if (
            return_hint
            and return_hint[:1] in {"'", '"'}
            and return_hint[-1:] == return_hint[:1]
        ):
            return_hint = return_hint[1:-1]
        if return_hint == "Self":
            return_hint = class_node.name
        method_symbol = self._function_symbol(f"{class_node.name}_{method_name}")
        poll_symbol = f"{method_symbol}_poll"
        self._record_func_default_specs(poll_symbol, item.args)
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(item.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(item.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if item.args.vararg is not None:
            arg_nodes.append(item.args.vararg)
        arg_nodes.extend(kwonly)
        if item.args.kwarg is not None:
            arg_nodes.append(item.args.kwarg)
        default_specs = self._default_specs_from_args(item.args)
        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(item))
        )
        cell_vars = self._callable_cell_vars(item)
        has_return = self._function_contains_return(item)
        frame_plan = stateful_function_frame_plan(
            kind=FunctionKind.ASYNC,
            poll_symbol=poll_symbol,
            param_count=len(params),
            has_closure=has_closure,
            gen_control_size=GEN_CONTROL_SIZE,
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        prev_class = self.current_class
        prev_first_param = self.current_method_first_param
        self.current_class = class_node.name
        self.current_method_first_param = params[0] if params else None
        self.start_function(
            poll_symbol,
            stateful_frame_plan=frame_plan,
            python_first_arg=self._python_first_positional_arg(item.args),
            params=["self"],
            compiler_params={"self"},
            type_facts_name=f"{class_node.name}.{method_name}",
            needs_return_slot=has_return,
        )
        self.async_context = True
        self.global_decls = self._collect_global_decls(item.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
        assigned = self._collect_assigned_names(item.body)
        self.del_targets = self._collect_deleted_names(item.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        self.async_locals_base = frame_plan.async_locals_base
        if has_closure:
            self.async_closure_offset = frame_plan.async_closure_offset
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        for i, arg in enumerate(arg_nodes):
            self._async_local_offset(arg.arg)
            hint = self._class_method_receiver_hint(class_node.name, descriptor, i)
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
            if hint is not None:
                self.async_public_hints[arg.arg] = hint
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        self._init_scope_async_locals(arg_nodes)
        self._prebox_scope_cell_vars(cell_vars)
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
        self._publish_python_frame_context()
        self._push_qualname(method_name, True)
        try:
            for stmt in item.body:
                self.visit(stmt)
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
            self._emit_normal_return_terminator(res)
        self._spill_async_temporaries()
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.current_class = prev_class
        self.current_method_first_param = prev_first_param

        func_hint = frame_plan.function_type_hint(closure_size)
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        function_def = MoltOp(
            kind=(
                "FUNC_NEW_CLOSURE"
                if has_closure and closure_val is not None
                else "FUNC_NEW"
            ),
            args=(
                [poll_symbol, len(params), closure_val]
                if has_closure and closure_val is not None
                else [poll_symbol, len(params)]
            ),
            result=func_val,
            metadata=frame_plan.callable_task_metadata(closure_size),
        )
        self.emit(function_def)
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=item.decorator_list,
            args=item.args,
            returns=item.returns,
        ):
            func_spill = self._spill_async_value(func_val)
        varnames = self._collect_varnames_for_body(
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            body=item.body,
        )
        self._emit_function_metadata(
            func_val,
            code_symbol=poll_symbol,
            name=method_name,
            qualname=self._qualname_for_def(method_name),
            trace_lineno=item.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=ast.get_docstring(item, clean=False),
            execution_kind=FunctionKind.ASYNC,
            varnames=varnames,
            freevars=free_vars,
            cellvars=cell_vars,
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)

        method_attr = func_val
        return {
            "func": func_val,
            "attr": method_attr,
            "descriptor": descriptor,
            "return_hint": return_hint,
            "param_count": len(params),
            "defaults": default_specs,
            "posonly_count": len(posonly),
            "kwonly_count": len(kwonly),
            "has_vararg": vararg is not None,
            "has_varkw": varkw is not None,
            "has_closure": has_closure,
            "property_field": property_field,
            "property_update": property_update,
        }

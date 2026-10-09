"""AsyncGenVisitorMixin: async function, async block, await, and yield lowering.

Move-only extraction from frontend/__init__.py. Covers visit_AsyncFunctionDef,
visit_AsyncWith, visit_AsyncFor, visit_Await, visit_Yield, and visit_YieldFrom.
Shared function, control-flow, and async-state helpers continue resolving through
the SimpleTIRGenerator MRO via self.<method>.
"""

from __future__ import annotations

from molt.compiler_analysis.python_lexical_scope import (
    function_contains_yield,
)

import ast
from molt.python_private_names import python_definition_name
from collections.abc import Sequence
from typing import (
    Any,
)

from molt.frontend.cfg_analysis import build_cfg
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_FIRST_TRAILING_RESULT_ARG,
)
from molt.frontend._types import (
    GEN_CONTROL_SIZE,
    GEN_SEND_OFFSET,
    GEN_THROW_OFFSET,
    GEN_YIELD_FROM_OFFSET,
    AsyncFrameSlot,
    AsyncFrameSlotRole,
    AsyncContextExit,
    MoltOp,
    MoltValue,
    ScratchCell,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.sema import (
    FunctionKind,
    StatefulFunctionFramePlan,
    StatefulLocalsLayout,
    async_generator_contains_return_value,
    async_generator_contains_yield_from,
    signature_contains_yield,
    stateful_function_frame_plan,
)
from molt.frontend._mixin_base import GeneratorMixinBase


# Names that are frame state rather than SSA values.
_RESUME_LIVENESS_EXEMPT = frozenset({"self", "none", ""})


class _ResumeLiveness:
    """SSA value liveness of one stateful poll, over the frontend CFG.

    The CFG gives STATE_SWITCH a resume edge to every STATE_LABEL, and gives
    loops their back edges and checks their exception edges, so a value is live
    into a label exactly when some path from the label uses it before any op
    defines it again. Bit ``i`` of a block's live set is name ``i``.
    """

    def __init__(self, ops: Sequence[MoltOp]) -> None:
        self.cfg = build_cfg(ops)
        self.names: list[str] = []
        self.def_sites: dict[str, list[int]] = {}
        self.type_hints: dict[str, str] = {}
        index: dict[str, int] = {}

        def bit(value: MoltValue) -> int:
            position = index.get(value.name)
            if position is None:
                position = index[value.name] = len(self.names)
                self.names.append(value.name)
            if value.type_hint:
                self.type_hints.setdefault(value.name, value.type_hint)
            return 1 << position

        blocks = self.cfg.blocks
        uses = [0] * len(blocks)
        defs = [0] * len(blocks)
        for block in blocks:
            used = defined = 0
            for idx in range(block.start, block.end):
                op = ops[idx]
                # The generated field authority names the ops whose trailing
                # args are outputs (UNPACK_SEQUENCE's targets), not operands.
                first_output = SIMPLEIR_FIRST_TRAILING_RESULT_ARG.get(
                    op.kind.lower(), len(op.args)
                )
                for arg in op.args[:first_output]:
                    if (
                        isinstance(arg, MoltValue)
                        and arg.name not in _RESUME_LIVENESS_EXEMPT
                    ):
                        mask = bit(arg)
                        if not defined & mask:
                            used |= mask
                outputs = [op.result, *op.args[first_output:]]
                for output in outputs:
                    if (
                        isinstance(output, MoltValue)
                        and output.name not in _RESUME_LIVENESS_EXEMPT
                    ):
                        defined |= bit(output)
                        self.def_sites.setdefault(output.name, []).append(idx)
            uses[block.id] = used
            defs[block.id] = defined
        live_in = [0] * len(blocks)
        # Reverse op order visits most successors first; iterate to a fixpoint.
        order = sorted(self.cfg.reachable, reverse=True)
        changed = True
        while changed:
            changed = False
            for block_id in order:
                live_out = 0
                for successor in self.cfg.successors[block_id]:
                    live_out |= live_in[successor]
                updated = uses[block_id] | (live_out & ~defs[block_id])
                if updated != live_in[block_id]:
                    live_in[block_id] = updated
                    changed = True
        self._live_in = live_in

    def live_into(self, op_index: int) -> list[str]:
        """The names live on entry to the block that starts at ``op_index``."""
        mask = self._live_in[self.cfg.index_to_block[op_index]]
        names: list[str] = []
        while mask:
            lowest = mask & -mask
            names.append(self.names[lowest.bit_length() - 1])
            mask ^= lowest
        return names


class AsyncGenVisitorMixin(GeneratorMixinBase):
    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        if self._class_ns_stack and self._class_ns_stack[-1].class_node is not None:
            self._emit_class_function_definition(self._class_ns_stack[-1], node)
            return None
        if self.current_func_name == "molt_main":
            new_globals = self._collect_global_decls(node.body)
            self.module_global_mutations.update(new_globals)
            for gname in new_globals:
                self.locals.pop(gname, None)
        if function_contains_yield(node):
            if async_generator_contains_yield_from(node):
                raise SyntaxError("'yield from' inside async function")
            if async_generator_contains_return_value(node):
                raise SyntaxError("'return' with value in async generator")
            func_name = node.name
            qualname = self._definition_qualname(node)
            func_symbol = self._function_symbol(
                func_name, kind=FunctionKind.ASYNC_GENERATOR, reuse_reserved=True
            )
            poll_func_name = f"{func_symbol}_poll"
            if not self._has_typing_overload_decorator(node):
                self._record_func_default_specs(poll_func_name, node.args)
            else:
                return None
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

            free_vars, free_var_hints, closure_val, has_closure = (
                self._capture_lexical_closure(self._cached_free_vars_raw(node))
            )
            cell_plan = self._callable_cell_plan(node)
            cell_vars = cell_plan.cellvars

            frame_plan = stateful_function_frame_plan(
                kind=FunctionKind.ASYNC_GENERATOR,
                poll_symbol=poll_func_name,
                param_count=len(params),
                has_closure=has_closure,
                gen_control_size=GEN_CONTROL_SIZE,
            )
            closure_size = self._task_closure_size(
                frame_plan.payload_slots,
                include_gen_control=frame_plan.include_gen_control,
            )
            self.globals[func_name] = MoltValue(
                func_name,
                type_hint=frame_plan.function_type_hint(closure_size),
            )

            prev_state = self._capture_function_state()
            self.current_class = None
            prev_first_param = self.current_method_first_param
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
            self.current_method_first_param = params[0] if params else None
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
                    if hint is not None:
                        self.async_public_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            self._prebox_scope_cell_vars(
                cell_plan.captured, private_cells=cell_plan.private
            )
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._publish_python_frame_context()
            self._push_qualname(python_definition_name(node), True, qualname=qualname)
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
            locals_layout = self._stateful_locals_layout(frame_plan, params, free_vars)
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
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
                    [poll_func_name, len(params), closure_val]
                    if has_closure and closure_val is not None
                    else [poll_func_name, len(params)]
                ),
                result=func_val,
                metadata=frame_plan.callable_task_metadata(closure_size),
            )
            self.emit(function_def)
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=node.decorator_list,
                args=node.args,
                returns=node.returns,
            ):
                func_spill = self._spill_async_value(func_val)
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
                default_exprs=node.args.defaults,
                kw_default_exprs=node.args.kw_defaults,
                docstring=ast.get_docstring(node, clean=False),
                execution_kind=FunctionKind.ASYNC_GENERATOR,
                varnames=list(name_layout.varnames),
                code_names=list(name_layout.names),
                freevars=free_vars,
                cellvars=cell_vars,
            )
            self._emit_stateful_locals_register(locals_layout, poll_func_name)
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, node)
            self._publish_definition_binding(func_name, func_val)
            if node.decorator_list:
                decorated = func_val
                for deco in reversed(node.decorator_list):
                    decorator_val = self.visit(deco)
                    if decorator_val is None:
                        raise FrontendRejection(
                            Diagnostic.SYNTAX_FORM, "Unsupported decorator"
                        )
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
                self._publish_definition_binding(func_name, func_val)
            self._record_source_app_callable(
                func_name,
                kind=FunctionKind.ASYNC_GENERATOR,
                symbol=poll_func_name,
                decorated=bool(node.decorator_list),
            )
            return None
        func_name = node.name
        qualname = self._definition_qualname(node)
        func_symbol = self._function_symbol(
            func_name, kind=FunctionKind.ASYNC, reuse_reserved=True
        )
        poll_func_name = f"{func_symbol}_poll"
        if not self._has_typing_overload_decorator(node):
            self._record_func_default_specs(poll_func_name, node.args)
        else:
            return None
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

        free_vars, free_var_hints, closure_val, has_closure = (
            self._capture_lexical_closure(self._cached_free_vars_raw(node))
        )
        cell_plan = self._callable_cell_plan(node)
        cell_vars = cell_plan.cellvars

        # Add to globals to support calls from other scopes
        frame_plan = stateful_function_frame_plan(
            kind=FunctionKind.ASYNC,
            poll_symbol=poll_func_name,
            param_count=len(params),
            has_closure=has_closure,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.globals[func_name] = MoltValue(
            func_name,
            type_hint=frame_plan.function_type_hint(closure_size),
        )  # Placeholder size

        prev_state = self._capture_function_state()
        self.current_class = None
        prev_first_param = self.current_method_first_param
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
        self.current_method_first_param = params[0] if params else None
        self.global_decls = self._collect_global_decls(node.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
        assigned = self._collect_assigned_names(node.body)
        self.del_targets = self._collect_deleted_names(node.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
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
                if hint is not None:
                    self.async_public_hints[arg.arg] = hint
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        self._init_scope_async_locals(arg_nodes)
        self._prebox_scope_cell_vars(
            cell_plan.captured, private_cells=cell_plan.private
        )
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
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
        else:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            self._emit_normal_return_terminator(res)
        self._spill_async_temporaries()
        locals_layout = self._stateful_locals_layout(frame_plan, params, free_vars)
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
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
                [poll_func_name, len(params), closure_val]
                if has_closure and closure_val is not None
                else [poll_func_name, len(params)]
            ),
            result=func_val,
            metadata=frame_plan.callable_task_metadata(closure_size),
        )
        self.emit(function_def)
        func_spill = None
        if self.in_generator and signature_contains_yield(
            decorators=node.decorator_list,
            args=node.args,
            returns=node.returns,
        ):
            func_spill = self._spill_async_value(func_val)
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
            default_exprs=node.args.defaults,
            kw_default_exprs=node.args.kw_defaults,
            docstring=ast.get_docstring(node, clean=False),
            execution_kind=FunctionKind.ASYNC,
            varnames=list(name_layout.varnames),
            code_names=list(name_layout.names),
            freevars=free_vars,
            cellvars=cell_vars,
        )
        self._emit_stateful_locals_register(locals_layout, poll_func_name)
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        self._emit_function_annotate(func_val, node)
        self._publish_definition_binding(func_name, func_val)
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
            kind=FunctionKind.ASYNC,
            symbol=poll_func_name,
            decorated=bool(node.decorator_list),
        )
        return None

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        self._require_coroutine_body(node, "'async with'")
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

        aenter_fn = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_SPECIAL_OBJ",
                args=[ctx_val, "__aenter__"],
                result=aenter_fn,
            )
        )
        aexit_fn = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_SPECIAL_OBJ",
                args=[ctx_val, "__aexit__"],
                result=aexit_fn,
            )
        )
        exit_action = AsyncContextExit(self._new_scratch_cell(aexit_fn))
        enter_val = self._emit_context_entry(exit_action, aenter_fn)

        self._emit_context_body(node, enter_val, exit_action)
        return None

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        return self._visit_async_for(node)

    def _visit_async_for(
        self, node: ast.AsyncFor, *, iterator: MoltValue | None = None
    ) -> None:
        self._require_coroutine_body(node, "'async for'")
        self._prepare_exact_class_loop_entry(node.body)
        provenance_flow = self._begin_module_provenance_flow(
            record_exception_prefixes=True
        )
        try:
            return self._visit_async_for_lowering(node, iterator=iterator)
        finally:
            # Async iteration may execute zero times. Join the pre-loop binding
            # state with every loop-body assignment exactly like synchronous
            # for/while lowering, so module provenance is never narrowed to the
            # iteration target's non-module state.
            self._finish_module_provenance_flow(provenance_flow)

    def _visit_async_for_lowering(
        self, node: ast.AsyncFor, *, iterator: MoltValue | None = None
    ) -> None:
        if iterator is None:
            iterable = self.visit(node.iter)
            if iterable is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE,
                    "Unsupported iterable in async for loop",
                )
            iterator = self._emit_aiter(iterable)
        iter_obj = iterator
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
        break_slot = None
        if node.orelse:
            break_slot = self._new_async_internal_slot()
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
        scope = self._visit_loop_body(node.body, guard_map, loop_break_flag=break_slot)
        if scope.needs_latch:
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self._emit_loop_exit(scope)
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
        # Await consumes the result of an ordinary, live Python expression.
        self._require_coroutine_body(node, "'await'")
        return self._emit_await_value(self.visit(node.value))

    def _in_coroutine_body(self) -> bool:
        """Whether the innermost scope is an ``async def`` or async genexpr body.

        Generators also own a stateful frame, and a class body compiles inline in
        its enclosing function, so neither decides where ``await`` may appear.
        """
        if self._class_body_depth > 0:
            return False
        plan = self.funcs_map[self.current_func_name].get("stateful_frame_plan")
        return plan is not None and plan.kind in (
            FunctionKind.ASYNC,
            FunctionKind.ASYNC_GENERATOR,
        )

    def _require_coroutine_body(self, node: ast.AST, construct: str) -> None:
        """Reject an async construct where CPython's compiler rejects it."""
        if self._in_coroutine_body():
            return
        if construct == "'await'" and (
            self._class_body_depth > 0 or self.current_func_name == "molt_main"
        ):
            self._raise_syntax_error("'await' outside function", node)
        self._raise_syntax_error(f"{construct} outside async function", node)

    def visit_Yield(self, node: ast.Yield) -> Any:
        if not self.in_generator:
            raise FrontendRejection(
                Diagnostic.CONTROL_FLOW, "yield outside of generator"
            )
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
        self.emit(
            MoltOp(
                kind="CALL",
                args=["molt_exception_trace_prepend", throw_val],
                result=MoltValue(self.next_var(), type_hint="None"),
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
            raise FrontendRejection(
                Diagnostic.CONTROL_FLOW, "yield from outside of generator"
            )
        iterable = self.visit(node.value)
        if iterable is None:
            raise FrontendRejection(
                Diagnostic.OPERAND_VALUE, "yield from operand unsupported"
            )
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
            iter_slot = self._new_async_internal_slot()
            is_gen_slot = self._new_async_internal_slot()
            pair_slot = self._new_async_internal_slot()
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
        throw = self._emit_intrinsic_function("molt_iterator_throw")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[throw, iter_obj, pending_throw], result=pair)
        )
        if pair_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )
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

        cleared_yield_from = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_yield_from))
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_YIELD_FROM_OFFSET, cleared_yield_from],
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

    def is_async(self) -> bool:
        return (
            self.funcs_map[self.current_func_name].get("stateful_frame_plan")
            is not None
        )

    def _allocate_async_frame_slot(
        self,
        role: AsyncFrameSlotRole,
        *,
        public_name: str | None = None,
    ) -> AsyncFrameSlot:
        if role is AsyncFrameSlotRole.PUBLIC:
            if public_name is None:
                raise ValueError("public async frame slots require a name")
            existing = self.async_locals.get(public_name)
            if existing is not None:
                return existing
        elif public_name is not None:
            raise ValueError("internal and scratch async frame slots are anonymous")
        slot = AsyncFrameSlot(
            offset=self.async_locals_base + len(self.async_frame_slots) * 8,
            role=role,
            public_name=public_name,
        )
        self.async_frame_slots.append(slot)
        if public_name is not None:
            self.async_locals[public_name] = slot
        return slot

    def _async_local_offset(self, name: str) -> int:
        return self._allocate_async_frame_slot(
            AsyncFrameSlotRole.PUBLIC, public_name=name
        ).offset

    def _new_async_internal_slot(self) -> int:
        return self._allocate_async_frame_slot(AsyncFrameSlotRole.INTERNAL).offset

    def _async_binding_slot(self, name: str) -> AsyncFrameSlot:
        public = self.async_locals.get(name)
        if public is not None:
            return public
        params = set(self.funcs_map.get(self.current_func_name, {}).get("params", []))
        if name in self.scope_assigned or name in params:
            return self._allocate_async_frame_slot(
                AsyncFrameSlotRole.PUBLIC,
                public_name=name,
            )
        slot = self.async_internal_bindings.get(name)
        if slot is None:
            slot = self._allocate_async_frame_slot(AsyncFrameSlotRole.INTERNAL)
            self.async_internal_bindings[name] = slot
        return slot

    def _async_spill_slot(self, value_name: str) -> AsyncFrameSlot:
        slot = self.async_spill_slots.get(value_name)
        if slot is None:
            slot = self._allocate_async_frame_slot(AsyncFrameSlotRole.INTERNAL)
            self.async_spill_slots[value_name] = slot
        return slot

    def _stateful_locals_layout(
        self,
        frame_plan: StatefulFunctionFramePlan,
        parameter_names: Sequence[str],
        free_vars: Sequence[str],
    ) -> StatefulLocalsLayout:
        """Freeze the finished poll body's typed public slots.

        Runs while the poll body is still current: its public slots, the
        closure cells its prologue publishes and its closure order all belong
        to that activation. The layout is the one locals authority for
        created and suspended views, inspect helpers and pre-entry frames.
        """
        if self.current_func_name != frame_plan.poll_symbol:
            raise ValueError(
                "stateful locals layout must belong to the active poll body"
            )
        cell_names = [
            name
            for name in self.boxed_locals
            if name in self.async_locals and name not in self.free_vars
        ]
        layout = frame_plan.public_locals_layout(
            public_slots=[
                (name, slot.offset) for name, slot in self.async_locals.items()
            ],
            parameter_names=parameter_names,
            cell_names=cell_names,
            free_vars=free_vars,
        )
        self.funcs_map[frame_plan.poll_symbol]["stateful_locals_layout"] = layout
        return layout

    def _emit_stateful_locals_register(
        self, layout: StatefulLocalsLayout, poll_symbol: str
    ) -> None:
        """Publish ``layout`` through the single runtime registration ABI."""
        name_vals: list[MoltValue] = []
        for local_name in layout.wire_names():
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[local_name], result=name_val))
            name_vals.append(name_val)
        names_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=name_vals, result=names_tuple))
        parameter_count, offsets, cells, closure_offset = layout.wire_layout()
        count_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[parameter_count], result=count_val))
        offset_vals: list[MoltValue] = []
        for offset in offsets:
            offset_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
            offset_vals.append(offset_val)
        offsets_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=offset_vals, result=offsets_tuple))
        cell_vals: list[MoltValue] = []
        for cell in cells:
            cell_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[cell], result=cell_val))
            cell_vals.append(cell_val)
        cells_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=cell_vals, result=cells_tuple))
        if closure_offset is None:
            closure_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=closure_val))
        else:
            closure_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[closure_offset], result=closure_val))
        layout_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(
            MoltOp(
                kind="TUPLE_NEW",
                args=[count_val, offsets_tuple, cells_tuple, closure_val],
                result=layout_tuple,
            )
        )
        self.emit(
            MoltOp(
                kind="STATEFUL_LOCALS_REGISTER",
                args=[poll_symbol, names_tuple, layout_tuple],
                result=MoltValue("none"),
            )
        )

    def _init_scope_async_locals(self, arg_nodes: list[ast.arg]) -> None:
        if not self.scope_assigned:
            return
        arg_names = {arg.arg for arg in arg_nodes}
        for name in sorted(self.scope_assigned):
            if (
                name in arg_names
                or name in self.global_decls
                or name in self.nonlocal_decls
            ):
                continue
            if name in self.async_locals:
                continue
            self._async_local_offset(name)

    def _expr_may_yield(self, node: ast.AST) -> bool:
        return self.is_async() and self._expr_needs_async(node)

    def _expr_needs_async(self, node: ast.AST) -> bool:
        class AsyncVisitor(ast.NodeVisitor):
            def __init__(self) -> None:
                self.needs_async = False

            def visit_Await(self, node: ast.Await) -> None:
                self.needs_async = True

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        visitor = AsyncVisitor()
        visitor.visit(node)
        return visitor.needs_async

    def _spill_async_value(self, value: MoltValue) -> int:
        offset = self._new_async_internal_slot()
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", offset, value],
                result=MoltValue("none"),
            )
        )
        return offset

    def _reload_async_value(self, offset: int, hint: str) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
        return res

    def _spill_async_temporaries(self) -> None:
        """Keep in the frame every value that a resume needs.

        A resume enters the poll at STATE_SWITCH and jumps to a STATE_LABEL,
        skipping every op between them. An SSA value that is live into a label
        is therefore lost unless the frame holds it. Liveness comes from the
        frontend CFG, so it follows every resume, exception and loop back edge:
        a value defined before a loop and used in it is live into each label
        inside the loop, whatever the op order says.

        The pass stores each such value into its slot where the value is
        defined, and reloads it after each label it is live into. A store at
        the definition keeps the slot current on the fall-through into a label
        as well as on a resume. A value of the activation prologue (defined
        before STATE_SWITCH) is stored once, on the initial entry: every resume
        re-runs the prologue, but the frame keeps its first activation's value,
        so the exception-stack baselines restore the depth the frame entered at.
        """
        ops = self.current_ops
        switch_idx = next(
            (idx for idx, op in enumerate(ops) if op.kind == "STATE_SWITCH"), None
        )
        label_indices = [idx for idx, op in enumerate(ops) if op.kind == "STATE_LABEL"]
        if switch_idx is None or not label_indices:
            # Only STATE_SWITCH resumes into a label.
            return
        live = _ResumeLiveness(ops)
        prologue_names: set[str] = set()
        body_names: set[str] = set()
        for name, sites in live.def_sites.items():
            if any(site < switch_idx for site in sites):
                prologue_names.add(name)
            if any(site > switch_idx for site in sites):
                body_names.add(name)
        # One slot cannot hold both the frame's first-activation value and a
        # value the body redefines. No lowering defines one name on both
        # sides of the switch; fail closed if one ever does.
        both_sides = prologue_names & body_names
        label_spills: dict[int, list[str]] = {}
        spill_names: set[str] = set()
        for label_idx in label_indices:
            names = sorted(
                name
                for name in live.live_into(label_idx)
                if name in prologue_names or name in body_names
            )
            mixed = [name for name in names if name in both_sides]
            if mixed:
                raise FrontendRejection(
                    Diagnostic.INTERNAL_INVARIANT,
                    f"{self.current_func_name}: values {mixed} are defined both "
                    "before and after STATE_SWITCH and live into a resume label",
                )
            label_spills[label_idx] = names
            spill_names.update(names)
        if not spill_names:
            return
        # The canonical ordered frame-slot allocator appends each newly
        # discovered typed INTERNAL slot; allocate spill slots in sorted order.
        for name in sorted(spill_names):
            self._async_spill_slot(name)
            hint = live.type_hints.get(name)
            if hint is not None:
                self.async_spill_hints.setdefault(name, hint)

        def after_exception_check(idx: int) -> int:
            # A store must not run while its definition's exception is pending.
            if idx + 1 < len(ops) and ops[idx + 1].kind == "CHECK_EXCEPTION":
                return idx + 1
            return idx

        stores_after: dict[int, list[str]] = {}
        for name in sorted(spill_names):
            if name in prologue_names:
                anchors = [after_exception_check(switch_idx)]
            else:
                anchors = [after_exception_check(site) for site in live.def_sites[name]]
            for anchor in anchors:
                stores_after.setdefault(anchor, []).append(name)
        loads_after: dict[int, list[str]] = {}
        for label_idx, names in label_spills.items():
            anchor = label_idx
            # A generator resume re-enters its try regions before any load.
            while anchor + 1 < len(ops) and ops[anchor + 1].kind == "TRY_START":
                anchor += 1
            loads_after.setdefault(anchor, []).extend(names)

        def slot_value(name: str) -> tuple[int, MoltValue]:
            hint = live.type_hints.get(name, "Unknown")
            return self._async_spill_slot(name).offset, MoltValue(name, type_hint=hint)

        new_ops: list[MoltOp] = []
        for idx, op in enumerate(ops):
            new_ops.append(op)
            for name in loads_after.get(idx, ()):
                offset, value = slot_value(name)
                new_ops.append(
                    MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=value)
                )
            for name in stores_after.get(idx, ()):
                offset, value = slot_value(name)
                new_ops.append(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", offset, value],
                        result=MoltValue("none"),
                    )
                )
        self.current_ops[:] = new_ops

    def _emit_await_anext(
        self,
        iter_obj: MoltValue,
        *,
        default_val: MoltValue | None,
        has_default: bool,
    ) -> MoltValue:
        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        awaitable = MoltValue(self.next_var(), type_hint="Future")
        self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=awaitable))
        if has_default:
            if default_val is None:
                default_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        else:
            default_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        res_cell = self._emit_cell_new(default_val)
        cell_slot: int | None = None
        if self.is_async():
            cell_slot = self._new_async_internal_slot()
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", cell_slot, res_cell],
                    result=MoltValue("none"),
                )
            )
        with self._suppress_check_exception():
            exc_val = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
            pending = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))
            self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
            is_stop = self._emit_builtin_exception_match(exc_val, "StopAsyncIteration")
            self.emit(MoltOp(kind="IF", args=[is_stop], result=MoltValue("none")))
            if not has_default:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
            else:
                self.emit(
                    MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        awaited_val = self._emit_await_value(awaitable, raise_pending=False)
        with self._suppress_check_exception():
            exc_after = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
            none_after = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
            is_none_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after)
            )
            pending_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[is_none_after], result=pending_after))
            self.emit(MoltOp(kind="IF", args=[pending_after], result=MoltValue("none")))
            is_stop_after = self._emit_builtin_exception_match(
                exc_after, "StopAsyncIteration"
            )
            self.emit(MoltOp(kind="IF", args=[is_stop_after], result=MoltValue("none")))
            if not has_default:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none"))
                )
            else:
                self.emit(
                    MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
        if cell_slot is not None:
            res_cell_after = MoltValue(self.next_var(), type_hint="cell")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", cell_slot],
                    result=res_cell_after,
                )
            )
            self._emit_cell_set(res_cell_after, awaited_val)
        else:
            self._emit_cell_set(res_cell, awaited_val)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        if cell_slot is not None:
            res_cell_final = MoltValue(self.next_var(), type_hint="cell")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", cell_slot],
                    result=res_cell_final,
                )
            )
            res = self._emit_cell_get(res_cell_final)
        else:
            res = self._emit_cell_get(res_cell)
        return res

    def _emit_awaitable_transform(self, awaitable: MoltValue) -> MoltValue:
        # One runtime protocol owns type-slot lookup, descriptor binding, and
        # iterator admission for every await expression and yield-from bridge.
        get_awaitable = self._emit_intrinsic_function("molt_get_awaitable")
        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[get_awaitable, awaitable], result=result)
        )
        return result

    def _emit_await_value(
        self, awaitable: MoltValue, *, raise_pending: bool = True
    ) -> MoltValue:
        if not self._in_coroutine_body():
            raise FrontendRejection(
                Diagnostic.CONTROL_FLOW, "await outside async function"
            )
        awaitable_slot = self._new_async_internal_slot()
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
        self.emit(MoltOp(kind="IF", args=[is_empty_cached], result=MoltValue("none")))
        transformed = self._emit_awaitable_transform(awaitable)
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", awaitable_slot, transformed],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
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
        coro = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", awaitable_slot],
                result=coro,
            )
        )
        result_slot = self._allocate_async_frame_slot(AsyncFrameSlotRole.SCRATCH)
        result_storage = ScratchCell(
            value=None, async_slot=result_slot, type_hint="Any"
        )
        result_slot_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="CONST", args=[result_slot.offset], result=result_slot_val)
        )
        self.state_count += 1
        next_state_id = self.state_count
        res_placeholder = MoltValue(self.next_var(), type_hint="Any")
        with self._suppress_check_exception(emit_on_exit=raise_pending):
            self.emit(
                MoltOp(
                    kind="STATE_TRANSITION",
                    args=[coro, result_slot_val, pending_state_val, next_state_id],
                    result=res_placeholder,
                )
            )
            cleared_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", awaitable_slot, cleared_val],
                    result=MoltValue("none"),
                )
            )
            # The transition owns this carrier only until resumption. Loading
            # retains the result before clearing its hidden frame reference.
            res = self._consume_scratch_cell(result_storage)
            if raise_pending:
                self._emit_raise_if_pending()
        return res

    def _emit_state_yield_resume_try_starts(self) -> None:
        if not self.in_generator:
            return
        for scope in self.try_scopes:
            handler_label = scope.handler_label
            if handler_label is None or handler_label not in self.try_end_labels:
                continue
            self.emit(
                MoltOp(
                    kind="TRY_START",
                    args=[handler_label],
                    result=MoltValue("none"),
                )
            )

    def _emit_state_yield_resume_entry(self, state_id: int) -> None:
        self.emit(MoltOp(kind="STATE_LABEL", args=[state_id], result=MoltValue("none")))
        self._emit_state_yield_resume_try_starts()

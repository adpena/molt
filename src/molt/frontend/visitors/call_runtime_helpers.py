"""CallRuntimeHelperMixin: extracted call-lowering authority."""

from __future__ import annotations

import ast
from molt.compiler_analysis.python_call_arguments import call_argument_schedule
from typing import (
    TYPE_CHECKING,
)

from molt.frontend._types import (
    FormatParseState,
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallRuntimeHelperMixin(_MixinBase):
    @staticmethod
    def _is_gpu_intrinsic_call(node: ast.Call) -> str | None:
        """If *node* is a gpu.thread_id() / gpu.block_id() / etc., return the
        intrinsic name (e.g. ``"gpu_thread_id"``).  Otherwise return None."""
        _GPU_INTRINSICS = {
            "thread_id": "gpu_thread_id",
            "block_id": "gpu_block_id",
            "block_dim": "gpu_block_dim",
            "grid_dim": "gpu_grid_dim",
            "barrier": "gpu_barrier",
        }
        # gpu.thread_id()
        if (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "gpu"
            and node.func.attr in _GPU_INTRINSICS
        ):
            return _GPU_INTRINSICS[node.func.attr]
        # bare thread_id() after `from molt.gpu import thread_id`
        if isinstance(node.func, ast.Name) and node.func.id in _GPU_INTRINSICS:
            return _GPU_INTRINSICS[node.func.id]
        return None

    def _emit_gpu_kernel_intrinsic_op(self, gpu_intrinsic: str) -> MoltValue:
        hint = "int" if gpu_intrinsic != "gpu_barrier" else "None"
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind=gpu_intrinsic, args=[], result=res))
        return res

    def _parse_gpu_launch_config_expr(
        self, config_expr: ast.expr
    ) -> tuple[MoltValue, MoltValue] | None:
        default_threads = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[256], result=default_threads))
        if isinstance(config_expr, ast.Tuple):
            if len(config_expr.elts) == 0:
                return None
            grid = self.visit(config_expr.elts[0])
            if grid is None:
                return None
            if len(config_expr.elts) == 1:
                return grid, default_threads
            threads = self.visit(config_expr.elts[1])
            if threads is None:
                return None
            return grid, threads
        grid = self.visit(config_expr)
        if grid is None:
            return None
        return grid, default_threads

    def _lower_gpu_kernel_launch_call(self, node: ast.Call) -> MoltValue | None:
        if not isinstance(node.func, ast.Subscript):
            return None
        base = node.func.value
        if not isinstance(base, ast.Name):
            return None
        if base.id not in self.gpu_kernel_symbols_by_name:
            return None
        launcher = self.visit(base)
        if launcher is None:
            return None
        config = self._parse_gpu_launch_config_expr(node.func.slice)
        if config is None:
            return None
        grid, threads = config
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(
                kind="CALL",
                args=["molt_gpu_kernel_launch", launcher, grid, threads, callargs],
                result=res,
            )
        )
        return res

    def _function_symbol_for_reference(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None:
            return reserved
        return self._function_symbol(name)

    def _function_result_hint(self, func_symbol: str) -> str:
        info = self.funcs_map.get(func_symbol)
        hint = info.get("return_hint") if info is not None else None
        return hint or "Any"

    def _record_container_elem_hint(
        self, target: MoltValue, elem_hint: str | None
    ) -> None:
        elem_map = (
            self.global_elem_hints
            if self.current_func_name == "molt_main"
            else self.container_elem_hints
        )
        if elem_hint and elem_hint not in {"Any", "Unknown", "missing"}:
            elem_map[target.name] = elem_hint
        else:
            elem_map.pop(target.name, None)

    def _remember_bytearray_len_hint(
        self, value: MoltValue, length: int | None
    ) -> None:
        if length is not None and length >= 0:
            self.bytearray_len_hints[value.name] = length
        else:
            self.bytearray_len_hints.pop(value.name, None)

    def _emit_locals_dict(self) -> MoltValue:
        if self.current_func_name == "molt_main":
            return self._emit_globals_dict()
        use_snapshot = self.target_python >= (3, 13)
        if use_snapshot:
            res = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
        else:
            self._init_locals_cache()
            if self.locals_cache_cell is None:
                raise AssertionError("locals cache scratch cell was not initialized")
            res = self._load_scratch_cell(self.locals_cache_cell)
        public_names = set(self.scope_assigned)
        public_names.update(self.async_locals)
        public_names.update(self.parameter_bindings)
        for name in sorted(public_names):
            value = self._load_local_value_unchecked(name)
            if value is None:
                continue
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
            # Update the locals dict without emitting control-flow:
            # - value is `__molt_missing__` => delete key if present
            # - else => set key to value
            self.emit(
                MoltOp(
                    kind="DICT_UPDATE_MISSING",
                    args=[res, key, value],
                    result=MoltValue("none"),
                )
            )
        for name in sorted(self.free_vars):
            if name in public_names:
                continue
            cell = self._load_free_var_cell(name)
            if cell is None:
                continue
            hint = self.free_var_hints.get(name, "Any")
            value = self._emit_cell_get(cell, type_hint=hint)
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
            self.emit(
                MoltOp(
                    kind="DICT_UPDATE_MISSING",
                    args=[res, key, value],
                    result=MoltValue("none"),
                )
            )
        return res

    def _emit_dataclasses_field_call(
        self, module_name: str, node: ast.Call
    ) -> MoltValue:
        if any(kw.arg is None for kw in node.keywords):
            # Try to resolve **kwargs spreads from module-level constant dicts
            expanded: list[ast.keyword] = []
            all_resolved = True
            for kw in node.keywords:
                if (
                    kw.arg is None
                    and isinstance(kw.value, ast.Name)
                    and kw.value.id in self.module_const_dicts
                ):
                    for dk, dv in self.module_const_dicts[kw.value.id].items():
                        expanded.append(
                            ast.keyword(arg=dk, value=ast.Constant(value=dv))
                        )
                elif kw.arg is None:
                    # Dynamic **kwargs — cannot resolve at compile time.
                    # Fall through to emit CALLARGS_EXPAND_KWSTAR at runtime.
                    all_resolved = False
                    break
                else:
                    expanded.append(kw)
            if all_resolved:
                node.keywords = expanded
        if node.args:
            raise FrontendRejection(
                Diagnostic.CALL_SIGNATURE,
                "field does not support positional arguments",
            )
        func_val = self._emit_module_attr_get_on(module_name, "field")
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CALL_BIND", args=[func_val, callargs], result=res))
        return res

    def _emit_exception_new_from_class(
        self, class_val: MoltValue, args: list[MoltValue]
    ) -> MoltValue:
        args_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW_FROM_CLASS",
                args=[class_val, args_val],
                result=exc_val,
            )
        )
        return exc_val

    def _emit_type_error_value(self, message: str, type_hint: str = "Any") -> MoltValue:
        err_val = self._emit_exception_new("TypeError", message)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        res = MoltValue(self.next_var(), type_hint=type_hint)
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
        return res

    def _emit_stop_iteration_from_value(self, value: MoltValue) -> None:
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, none_val], result=is_none))
        # Async/poll-function bodies need a closure-slot result, not a list
        # cell. The cell SSA value can be merged with the entry-block default
        # by Cranelift's loop-header phi resolver, producing
        # store_index(None, ...) crashes (see _emit_guarded_field_get for the
        # full rationale).
        if self.is_async():
            slot = self._new_async_internal_slot()
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, empty_tuple],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            value_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[value], result=value_tuple))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, value_tuple],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            args_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=args_val))
        else:
            # Sync path: a single SSA value updated in both branches.
            args_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=args_val))
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
            self.emit(MoltOp(kind="COPY", args=[empty_tuple], result=args_val))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            value_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[value], result=value_tuple))
            self.emit(MoltOp(kind="COPY", args=[value_tuple], result=args_val))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["StopIteration"], result=kind_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, args_val],
                result=exc_val,
            )
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))

    @staticmethod
    def _call_needs_bind(node: ast.Call) -> bool:
        if node.keywords:
            return True
        return any(isinstance(arg, ast.Starred) for arg in node.args)

    def _emit_call_args_builder(
        self, node: ast.Call, *, evaluated: tuple[MoltValue, ...] | None = None
    ) -> MoltValue:
        # Guarded dispatch shares argument assembly without revisiting source
        # expressions. Pre-evaluation is valid only for flat calls: expansions
        # have interleaved observable work owned by call_argument_schedule.
        if evaluated is not None and (
            len(evaluated) != len(node.args) + len(node.keywords)
            or any(isinstance(argument, ast.Starred) for argument in node.args)
            or any(keyword.arg is None for keyword in node.keywords)
        ):
            raise AssertionError("pre-evaluated call requires flat argument syntax")
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        pending: dict[int, MoltValue] = {}
        for step in call_argument_schedule(node):
            if step.action == "evaluate":
                if evaluated is not None:
                    pending[step.index] = evaluated[step.index]
                    continue
                # Only values actually live across this suspension need storage.
                # Consuming each scratch cell clears its retained frame reference.
                suspends = self.is_async() and self._expr_may_yield(step.expression)
                builder_cell = (
                    self._new_scratch_cell(callargs, type_hint="callargs")
                    if suspends
                    else None
                )
                cells = (
                    {
                        index: self._new_scratch_cell(value, type_hint=value.type_hint)
                        for index, value in pending.items()
                    }
                    if suspends
                    else {}
                )
                value = self.visit(step.expression)
                if value is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE, "Unsupported call argument"
                    )
                if builder_cell is not None:
                    callargs = self._consume_scratch_cell(builder_cell)
                    pending = {
                        index: self._consume_scratch_cell(cell)
                        for index, cell in cells.items()
                    }
                pending[step.index] = value
                continue
            value = pending.pop(step.index)
            if step.materialization == "tuple":
                # CALL_FUNCTION_EX consumes a tuple, unlike list accumulation
                # for mixed stars. The runtime tuple authority owns versioned
                # length-hint callbacks and exact-tuple identity preservation.
                value = self._emit_tuple_from_iter(value)
            args = [callargs, value]
            if step.action == "kw":
                if step.name is None:
                    raise AssertionError("keyword schedule has no name")
                key = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[step.name], result=key))
                args = [callargs, key, value]
            kind = {
                "pos": "CALLARGS_PUSH_POS",
                "star": "CALLARGS_EXPAND_STAR",
                "kw": "CALLARGS_PUSH_KW",
                "kwstar": "CALLARGS_EXPAND_KWSTAR",
            }[step.action]
            self.emit(
                MoltOp(
                    kind=kind,
                    args=args,
                    result=MoltValue(self.next_var(), type_hint="None"),
                )
            )
        if pending:
            raise AssertionError("call argument schedule left unconsumed values")
        return callargs

    def _emit_tuple_from_iter(self, iterable: MoltValue) -> MoltValue:
        constructor = self._emit_builtin_type_value("tuple")
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="CALL_FUNC", args=[constructor, iterable], result=res))
        return res

    def _emit_set_update_from_iter(
        self, target: MoltValue, iterable: MoltValue
    ) -> None:
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
        self.emit(MoltOp(kind="SET_ADD", args=[target, item], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_frozenset_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="frozenset")
        self.emit(MoltOp(kind="FROZENSET_NEW", args=[], result=res))
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
        self.emit(
            MoltOp(kind="FROZENSET_ADD", args=[res, item], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _builtin_str_single_object_arg(self, node: ast.AST) -> ast.AST | None:
        if not isinstance(node, ast.Call):
            return None
        if self._specializable_builtin_name(node) != "str":
            return None
        if len(node.args) + len(node.keywords) != 1:
            return None
        kw_object: ast.AST | None = None
        for keyword in node.keywords:
            if keyword.arg != "object":
                return None
            if kw_object is not None:
                return None
            kw_object = keyword.value
        if node.args:
            return node.args[0]
        return kw_object

    def _lower_string_format_call(
        self, node: ast.Call, format_str: str
    ) -> MoltValue | None:
        if any(isinstance(arg, ast.Starred) for arg in node.args):
            return None
        kw_names: list[str] = []
        for keyword in node.keywords:
            if keyword.arg is None:
                return None
            kw_names.append(keyword.arg)
        if len(set(kw_names)) != len(kw_names):
            return None
        cache_key = (format_str, len(node.args), tuple(sorted(kw_names)))
        tokens = self.format_token_cache.get(cache_key)
        if tokens is None:
            state = FormatParseState()
            try:
                tokens = self._parse_format_tokens(
                    format_str,
                    len(node.args),
                    set(kw_names),
                    state,
                )
            except ValueError as exc:
                err_val = self._emit_exception_new("ValueError", str(exc))
                self.emit(
                    MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none"))
                )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if tokens is None:
                return None
            self.format_token_cache[cache_key] = tokens
        args: list[MoltValue] = []
        for arg in node.args:
            value = self.visit(arg)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported format argument"
                )
            args.append(value)
        kwargs: dict[str, MoltValue] = {}
        for keyword in node.keywords:
            value = self.visit(keyword.value)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported format argument"
                )
            key = keyword.arg
            if key is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported format argument"
                )
            kwargs[key] = value
        return self._emit_format_tokens(tokens, args, kwargs)

    def _emit_dynamic_call(self, node: ast.Call, callee: MoltValue) -> MoltValue:
        # The result authority already proves exactness at this source point.
        # It is independent of whether the callee is a constructor, method or
        # open alias, and never authorizes eliding the retained real callable.
        res_hint = self._builtin_exact_type_from_expr(node) or "Any"
        suspends = self.is_async() and any(
            self._expr_may_yield(expression)
            for expression in [
                *node.args,
                *(keyword.value for keyword in node.keywords),
            ]
        )
        callee_cell = (
            self._new_scratch_cell(callee, type_hint=callee.type_hint)
            if suspends
            else None
        )
        if self._call_needs_bind(node):
            callargs = self._emit_call_args_builder(node)
            if callee_cell is not None:
                callee = self._consume_scratch_cell(callee_cell)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res))
            return res
        args = self._emit_call_args(node.args)
        if callee_cell is not None:
            callee = self._consume_scratch_cell(callee_cell)
        if callee.type_hint.startswith("Func:"):
            func_symbol = callee.type_hint.split(":", 1)[1]
            res = MoltValue(self.next_var(), type_hint=res_hint)
            # A code-symbol hint is not a namespace or callable-identity proof.
            # Reuse guarded object dispatch so its fast path also transports the
            # exact function context; a second globals lookup can be rebound too.
            self.emit(
                MoltOp(
                    kind="CALL_GUARDED",
                    args=[callee] + args,
                    result=res,
                    metadata={"target": func_symbol},
                )
            )
            return res
        # Positional object dispatch already owns callable admission and live
        # defaults, including bound-method self. Only keyword/starred syntax
        # requires a callargs builder, never a lexical hint or caller override.
        res = MoltValue(self.next_var(), type_hint=res_hint)
        self.emit(MoltOp(kind="CALL_FUNC", args=[callee, *args], result=res))
        return res

    def _lower_statistics_slice_call(
        self, func_id: str, node: ast.Call
    ) -> MoltValue | None:
        if func_id not in {"mean", "stdev"}:
            return None
        if node.keywords or len(node.args) != 1:
            return None
        data_arg = node.args[0]
        if not isinstance(data_arg, ast.Subscript):
            return None
        data_slice = data_arg.slice
        if not isinstance(data_slice, ast.Slice):
            return None
        if data_slice.step is not None:
            return None
        seq = self.visit(data_arg.value)
        if seq is None:
            return None
        if data_slice.lower is None:
            start = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
            has_start = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_start))
        else:
            start = self.visit(data_slice.lower)
            if start is None:
                return None
            has_start = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
        if data_slice.upper is None:
            end = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
            has_end = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_end))
        else:
            end = self.visit(data_slice.upper)
            if end is None:
                return None
            has_end = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_end))
        kind = (
            "STATISTICS_MEAN_SLICE" if func_id == "mean" else "STATISTICS_STDEV_SLICE"
        )
        res = MoltValue(self.next_var(), type_hint="float")
        self.emit(
            MoltOp(
                kind=kind,
                args=[seq, start, end, has_start, has_end],
                result=res,
            )
        )
        return res

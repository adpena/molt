"""CallRuntimeHelperMixin: extracted call-lowering authority."""

from __future__ import annotations

import ast
import sys

from typing import (
    TYPE_CHECKING,
)

from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
    FormatParseState,
    MoltOp,
    MoltValue,
    _MOLT_CLOSURE_PARAM,
    _MOLT_LOCALS_CACHE,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallRuntimeHelperMixin(_MixinBase):
    def _emit_nullcontext(self, payload: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="context_manager")
        self.emit(MoltOp(kind="CONTEXT_NULL", args=[payload], result=res))
        return res

    def _emit_closing(self, payload: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="context_manager")
        self.emit(MoltOp(kind="CONTEXT_CLOSING", args=[payload], result=res))
        return res

    def _emit_open_call(self, node: ast.Call) -> MoltValue:
        mode_expr = None
        if len(node.args) > 1:
            mode_expr = node.args[1]
        for kw in node.keywords:
            if kw.arg == "mode" and mode_expr is None:
                mode_expr = kw.value
        mode_hint = None
        if mode_expr is None:
            mode_hint = "file_text"
        elif isinstance(mode_expr, ast.Constant) and isinstance(mode_expr.value, str):
            mode_hint = "file_bytes" if "b" in mode_expr.value else "file_text"
        res = MoltValue(self.next_var(), type_hint=mode_hint or "file")
        callee = self._emit_builtin_function("open")
        callargs = self._emit_call_args_builder(node)
        self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
        return res

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
        use_snapshot = sys.version_info >= (3, 13)
        if use_snapshot:
            res = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
        else:
            res = self._load_local_value_unchecked(_MOLT_LOCALS_CACHE)
            if res is None:
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
                self._store_local_value(_MOLT_LOCALS_CACHE, res)
        for name in sorted(self.locals):
            if name == _MOLT_CLOSURE_PARAM or name.startswith("__molt_"):
                continue
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
            if name in self.locals:
                continue
            if name == _MOLT_CLOSURE_PARAM or name.startswith("__molt_"):
                continue
            cell = self._load_free_var_cell(name)
            if cell is None:
                continue
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            hint = self.free_var_hints.get(name, "Any")
            value = MoltValue(self.next_var(), type_hint=hint)
            self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=value))
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
            raise NotImplementedError("field does not support positional arguments")
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
            slot = self._async_local_offset(
                f"__stop_iter_args_{len(self.async_locals)}"
            )
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

    def _static_expr_type_hint_without_emitting(self, expr: ast.expr) -> str | None:
        if isinstance(expr, ast.List):
            return "list"
        if isinstance(expr, ast.Tuple):
            return "tuple"
        if isinstance(expr, ast.Dict):
            return "dict"
        if isinstance(expr, ast.Set):
            return "set"
        if isinstance(expr, ast.Constant):
            if isinstance(expr.value, str):
                return "str"
            if isinstance(expr.value, bytes):
                return "bytes"
            if isinstance(expr.value, bool):
                return "bool"
            if isinstance(expr.value, int):
                return "int"
            if isinstance(expr.value, float):
                return "float"
            if expr.value is None:
                return "None"
        if not isinstance(expr, ast.Name):
            return None
        if self.is_async() and expr.id in self.async_local_hints:
            return self.async_local_hints[expr.id]
        boxed_hint = self.boxed_local_hints.get(expr.id)
        if boxed_hint is not None:
            return boxed_hint
        local_val = self.locals.get(expr.id)
        if local_val is not None:
            return local_val.type_hint
        global_val = self.globals.get(expr.id)
        if global_val is not None:
            return global_val.type_hint
        return None

    @staticmethod
    def _call_needs_bind(node: ast.Call) -> bool:
        if node.keywords:
            return True
        return any(isinstance(arg, ast.Starred) for arg in node.args)

    def _emit_call_args_builder(self, node: ast.Call) -> MoltValue:
        items: list[tuple[str, ast.expr, str | None]] = []
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                items.append(("star", arg.value, None))
            else:
                items.append(("pos", arg, None))
        for kw in node.keywords:
            if kw.arg is None:
                items.append(("kwstar", kw.value, None))
            else:
                items.append(("kw", kw.value, kw.arg))
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        if not items:
            self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
            return callargs
        values: list[MoltValue] = []
        if not self.is_async():
            for _, expr, _ in items:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported call argument")
                values.append(val)
        else:
            yield_flags = [self._expr_may_yield(expr) for _, expr, _ in items]
            if not any(yield_flags):
                for _, expr, _ in items:
                    val = self.visit(expr)
                    if val is None:
                        raise NotImplementedError("Unsupported call argument")
                    values.append(val)
            else:
                spills: list[tuple[int, int, str]] = []
                for idx, (_, expr, _) in enumerate(items):
                    val = self.visit(expr)
                    if val is None:
                        raise NotImplementedError("Unsupported call argument")
                    values.append(val)
                    if any(yield_flags[idx + 1 :]):
                        slot = self._spill_async_value(
                            val, f"__arg_spill_{len(self.async_locals)}"
                        )
                        spills.append((idx, slot, val.type_hint))
                for idx, slot, hint in spills:
                    values[idx] = self._reload_async_value(slot, hint)
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        for (kind, _, name), val in zip(items, values):
            if kind == "pos":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="CALLARGS_PUSH_POS", args=[callargs, val], result=res)
                )
            elif kind == "star":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_STAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            elif kind == "kw":
                if name is None:
                    raise NotImplementedError("Keyword name is missing")
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_KW",
                        args=[callargs, key_val, val],
                        result=res,
                    )
                )
            elif kind == "kwstar":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_KWSTAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            else:
                raise NotImplementedError("Unknown call argument kind")
        return callargs

    def _emit_print_call_args_builder(self, node: ast.Call) -> tuple[MoltValue, bool]:
        items: list[tuple[str, ast.expr, str | None]] = []
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                items.append(("star", arg.value, None))
            else:
                items.append(("pos", arg, None))
        for kw in node.keywords:
            if kw.arg is None:
                items.append(("kwstar", kw.value, None))
            else:
                items.append(("kw", kw.value, kw.arg))
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        if not items:
            self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
            return callargs, False
        values: list[MoltValue] = []
        saw_name_error = False
        if not self.is_async():
            for _, expr, _ in items:
                val = self.visit(expr)
                if val is None:
                    if isinstance(expr, ast.Name):
                        exc_val = self._emit_exception_new(
                            "NameError", f"name '{expr.id}' is not defined"
                        )
                        self.emit(
                            MoltOp(
                                kind="RAISE",
                                args=[exc_val],
                                result=MoltValue("none"),
                            )
                        )
                        saw_name_error = True
                        val = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                    else:
                        raise NotImplementedError("Unsupported call argument")
                values.append(val)
        else:
            yield_flags = [self._expr_may_yield(expr) for _, expr, _ in items]
            if not any(yield_flags):
                for _, expr, _ in items:
                    val = self.visit(expr)
                    if val is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    values.append(val)
            else:
                spills: list[tuple[int, int, str]] = []
                for idx, (_, expr, _) in enumerate(items):
                    val = self.visit(expr)
                    if val is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    values.append(val)
                    if any(yield_flags[idx + 1 :]):
                        slot = self._spill_async_value(
                            val, f"__arg_spill_{len(self.async_locals)}"
                        )
                        spills.append((idx, slot, val.type_hint))
                for idx, slot, hint in spills:
                    values[idx] = self._reload_async_value(slot, hint)
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        for (kind, _, name), val in zip(items, values):
            if kind == "pos":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="CALLARGS_PUSH_POS", args=[callargs, val], result=res)
                )
            elif kind == "star":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_STAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            elif kind == "kw":
                if name is None:
                    raise NotImplementedError("Keyword name is missing")
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_KW",
                        args=[callargs, key_val, val],
                        result=res,
                    )
                )
            elif kind == "kwstar":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_KWSTAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            else:
                raise NotImplementedError("Unknown call argument kind")
        return callargs, saw_name_error

    def _emit_tuple_from_iter(self, iterable: MoltValue) -> MoltValue:
        items = self._emit_list_from_iter(iterable)
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_FROM_LIST", args=[items], result=res))
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
        if not (isinstance(node.func, ast.Name) and node.func.id == "str"):
            return None
        if len(node.args) > 1:
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
                raise NotImplementedError("Unsupported format argument")
            args.append(value)
        kwargs: dict[str, MoltValue] = {}
        for keyword in node.keywords:
            value = self.visit(keyword.value)
            if value is None:
                raise NotImplementedError("Unsupported format argument")
            key = keyword.arg
            if key is None:
                raise NotImplementedError("Unsupported format argument")
            kwargs[key] = value
        return self._emit_format_tokens(tokens, args, kwargs)

    def _emit_dynamic_call(
        self, node: ast.Call, callee: MoltValue, needs_bind: bool
    ) -> MoltValue:
        res_hint = "Any"
        if callee.type_hint.startswith("BoundMethod:"):
            parts = callee.type_hint.split(":", 2)
            if len(parts) == 3:
                class_name = parts[1]
                method_name = parts[2]
                method_info = (
                    self.classes.get(class_name, {}).get("methods", {}).get(method_name)
                )
                if method_info:
                    return_hint = method_info["return_hint"]
                    # Builtin scalar/container return types must propagate as
                    # type hints — without this, method calls returning `int`
                    # become type-erased `Any`, which forces the lane-inference
                    # pass to fall back to a NaN-boxed (effectively float-coerced)
                    # accumulator in tight loops like
                    # `total += obj.compute(i)`.
                    if return_hint and (
                        return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
                    ):
                        res_hint = return_hint
        if needs_bind:
            callargs = self._emit_call_args_builder(node)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res))
            return res
        if callee.type_hint.startswith("BoundMethod:"):
            args = self._emit_call_args(node.args)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_METHOD", args=[callee] + args, result=res))
            return res
        if callee.type_hint.startswith("Func:"):
            func_symbol = callee.type_hint.split(":", 1)[1]
            args, _ = self._emit_direct_call_args_for_symbol(
                func_symbol, node, func_obj=callee
            )
            if args is None:
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res)
                )
                return res
            func_name = self.func_symbol_names.get(func_symbol)
            if func_name and func_name in self.globals:
                # Devirtualized call: check if callee is the expected function,
                # then call directly by symbol.  Falls back to INVOKE_FFI if
                # the identity check fails (e.g. function was rebound).
                #
                # Both branches write to the same output variable (`res`)
                # so the result is available after END_IF without an
                # intermediate list cell.  The old res_cell + STORE_INDEX
                # pattern broke in WASM because CHECK_EXCEPTION between
                # CALL and STORE_INDEX could skip the store, leaving None.
                expected = self._emit_module_attr_get(func_name)
                matches = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS", args=[callee, expected], result=matches))
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self.emit(MoltOp(kind="IF", args=[matches], result=MoltValue("none")))
                self.emit(MoltOp(kind="CALL", args=[func_symbol] + args, result=res))
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.emit(
                    MoltOp(
                        kind="INVOKE_FFI",
                        args=[callee] + args,
                        result=res,
                    )
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                return res
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL", args=[func_symbol] + args, result=res))
            return res
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint=res_hint)
        self.emit(MoltOp(kind="CALL_INDIRECT", args=[callee, callargs], result=res))
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

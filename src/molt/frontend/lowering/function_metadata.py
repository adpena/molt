"""FunctionMetadataMixin: callable defaults, metadata, and known-function facts.

Move-only extraction from frontend/__init__.py. This lowering authority owns
callable parameter/default shape, function metadata emission, builtin function
metadata construction, and known-module function kind/type-hint lookups shared by
call, function, class, and module visitors.
"""

from __future__ import annotations

import ast
import json
from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
    BUILTIN_FUNC_SPECS,
    GEN_CONTROL_SIZE,
    MOLT_BIND_KIND_OPEN,
    MoltOp,
    MoltValue,
)
from molt.frontend.sema import (
    FunctionKind,
    expression_contains_yield,
    normalize_function_kind,
    stateful_function_frame_plan,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class FunctionMetadataMixin(_MixinBase):
    @staticmethod
    def _default_spec_for_expr(expr: ast.expr) -> dict[str, Any]:
        if isinstance(expr, ast.Constant):
            return {"const": True, "value": expr.value}
        return {"const": False}

    @classmethod
    def _default_specs_from_args(cls, args: ast.arguments) -> list[dict[str, Any]]:
        default_specs = [cls._default_spec_for_expr(expr) for expr in args.defaults]
        if not args.kwonlyargs or not args.kw_defaults:
            return default_specs
        kwonly_names = [arg.arg for arg in args.kwonlyargs]
        kwonly_pairs = list(zip(kwonly_names, args.kw_defaults))
        suffix: list[tuple[str, ast.expr]] = []
        for name, expr in reversed(kwonly_pairs):
            if expr is None:
                break
            suffix.append((name, expr))
        for name, expr in reversed(suffix):
            spec = cls._default_spec_for_expr(expr)
            spec["kwonly"] = True
            spec["name"] = name
            default_specs.append(spec)
        return default_specs

    def _record_func_default_specs(self, func_symbol: str, args: ast.arguments) -> None:
        if args.vararg or args.kwarg:
            # Mark as having vararg/kwarg so the direct-call path knows to
            # fall back to CALL_BIND for proper varargs packing.
            self.func_default_specs[func_symbol] = {"has_vararg": True}
            return
        params = self._function_param_names(args)
        default_specs = self._default_specs_from_args(args)
        self.func_default_specs[func_symbol] = {
            "params": len(params),
            "defaults": default_specs,
            "posonly": len(args.posonlyargs),
            "kwonly": len(args.kwonlyargs),
            "kind": "sync",
            "has_decorators": False,
        }

    def _emit_function_default_values(
        self,
        func_val: MoltValue,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        kwonly_params: list[str],
    ) -> tuple[MoltValue, MoltValue, MoltValue]:
        yield_in_defaults = False
        yield_in_kwdefaults = False
        func_spill: int | None = None
        if self.in_generator:
            yield_in_defaults = any(
                expression_contains_yield(expr) for expr in default_exprs
            )
            yield_in_kwdefaults = any(
                expression_contains_yield(expr)
                for expr in kw_default_exprs
                if expr is not None
            )
            if yield_in_defaults or yield_in_kwdefaults:
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )

        if default_exprs:
            default_vals: list[MoltValue] = []
            for expr in default_exprs:
                val = self.visit(expr)
                if val is None:
                    val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                default_vals.append(val)
            defaults_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(kind="TUPLE_NEW", args=default_vals, result=defaults_tuple)
            )
            if func_spill is not None and yield_in_defaults:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            defaults_val = defaults_tuple
        else:
            defaults_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=defaults_none))
            defaults_val = defaults_none

        if kw_default_exprs and kwonly_params:
            kw_pairs: list[MoltValue] = []
            for name, expr in zip(kwonly_params, kw_default_exprs):
                if expr is None:
                    continue
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                val = self.visit(expr)
                if val is None:
                    val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                kw_pairs.extend([key_val, val])
            if kw_pairs:
                kw_defaults = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=kw_pairs, result=kw_defaults))
                if func_spill is not None and yield_in_kwdefaults:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                kwdefaults_val = kw_defaults
            else:
                kw_defaults_none = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=kw_defaults_none))
                if func_spill is not None and yield_in_kwdefaults:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                kwdefaults_val = kw_defaults_none
        else:
            kw_defaults_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=kw_defaults_none))
            kwdefaults_val = kw_defaults_none
        if func_spill is not None and (yield_in_defaults or yield_in_kwdefaults):
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        return func_val, defaults_val, kwdefaults_val

    def _emit_function_metadata(
        self,
        func_val: MoltValue,
        *,
        name: str,
        qualname: str,
        trace_filename: str | None = None,
        trace_lineno: int | None = None,
        trace_name: str | None = None,
        posonly_params: list[str],
        pos_or_kw_params: list[str],
        kwonly_params: list[str],
        vararg: str | None,
        varkw: str | None,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        docstring: str | None,
        module_override: str | None = None,
        is_coroutine: bool = False,
        is_generator: bool = False,
        is_async_generator: bool = False,
        bind_kind: int | None = None,
        poll_fn_symbol: str | None = None,
        emit_code: bool = True,
        varnames: list[str] | None = None,
        code_names: list[str] | None = None,
    ) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))

        qual_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[qualname], result=qual_val))

        module_name = module_override or self.module_name
        module_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_val))

        arg_name_vals: list[MoltValue] = []
        for param in posonly_params + pos_or_kw_params:
            param_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[param], result=param_val))
            arg_name_vals.append(param_val)
        arg_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=arg_name_vals, result=arg_names_tuple))

        posonly_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[len(posonly_params)], result=posonly_val))

        kwonly_name_vals: list[MoltValue] = []
        for param in kwonly_params:
            param_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[param], result=param_val))
            kwonly_name_vals.append(param_val)
        kwonly_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=kwonly_name_vals, result=kwonly_tuple))

        if vararg is None:
            vararg_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=vararg_val))
        else:
            vararg_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[vararg], result=vararg_val))

        if varkw is None:
            varkw_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=varkw_val))
        else:
            varkw_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[varkw], result=varkw_val))
        func_val, defaults_val, kwdefaults_val = self._emit_function_default_values(
            func_val, default_exprs, kw_default_exprs, kwonly_params
        )

        if bind_kind is not None:
            bind_kind_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[bind_kind], result=bind_kind_val))
        else:
            bind_kind_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=bind_kind_val))

        if docstring is None:
            doc_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=doc_val))
        else:
            doc_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[docstring], result=doc_val))

        code_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=code_val))
        if emit_code:
            filename = trace_filename or self.source_path or "<unknown>"
            trace_label = trace_name or qualname or name
            file_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[filename], result=file_val))
            line_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[int(trace_lineno or 0)],
                    result=line_val,
                )
            )
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[trace_label], result=name_val))
            linetable_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=linetable_val))
            varnames_list = varnames
            if varnames_list is None:
                varnames_list = self._varnames_from_params(
                    posonly_params=posonly_params,
                    pos_or_kw_params=pos_or_kw_params,
                    kwonly_params=kwonly_params,
                    vararg=vararg,
                    varkw=varkw,
                )
            varname_vals: list[MoltValue] = []
            for varname in varnames_list:
                var_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[varname], result=var_val))
                varname_vals.append(var_val)
            varnames_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(kind="TUPLE_NEW", args=varname_vals, result=varnames_tuple)
            )
            code_name_vals: list[MoltValue] = []
            if code_names is not None:
                for code_name in code_names:
                    code_name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(kind="CONST_STR", args=[code_name], result=code_name_val)
                    )
                    code_name_vals.append(code_name_val)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=code_name_vals, result=names_tuple))
            argcount_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[len(posonly_params) + len(pos_or_kw_params)],
                    result=argcount_val,
                )
            )
            posonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[len(posonly_params)], result=posonly_val)
            )
            kwonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[len(kwonly_params)], result=kwonly_val)
            )
            code_val = MoltValue(self.next_var(), type_hint="code")
            self.emit(
                MoltOp(
                    kind="CODE_NEW",
                    args=[
                        file_val,
                        name_val,
                        line_val,
                        linetable_val,
                        varnames_tuple,
                        names_tuple,
                        argcount_val,
                        posonly_val,
                        kwonly_val,
                    ],
                    result=code_val,
                )
            )
            code_symbol = self._code_symbol_for_value(func_val)
            if code_symbol is not None:
                code_id = self._register_code_symbol(code_symbol)
                self.emit(
                    MoltOp(
                        kind="CODE_SLOT_SET",
                        args=[code_val],
                        result=MoltValue("none"),
                        metadata={"code_id": code_id},
                    )
                )
            if poll_fn_symbol is not None:
                self.emit(
                    MoltOp(
                        kind="FN_PTR_CODE_SET",
                        args=[poll_fn_symbol, code_val],
                        result=MoltValue("none"),
                    )
                )

        metadata_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(
            MoltOp(
                kind="TUPLE_NEW",
                args=[
                    name_val,
                    qual_val,
                    module_val,
                    arg_names_tuple,
                    posonly_val,
                    kwonly_tuple,
                    vararg_val,
                    varkw_val,
                    defaults_val,
                    kwdefaults_val,
                    doc_val,
                ],
                result=metadata_tuple,
            )
        )
        init_metadata = self._emit_runtime_function(
            "molt_function_init_metadata_packed", 4
        )
        init_res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[init_metadata, func_val, metadata_tuple, code_val, bind_kind_val],
                result=init_res,
            )
        )

        def set_attr(attr: str, value: MoltValue) -> None:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, attr, value],
                    result=MoltValue("none"),
                )
            )

        if is_coroutine:
            coro_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=coro_val))
            set_attr("__molt_is_coroutine__", coro_val)
        if is_generator:
            gen_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gen_val))
            set_attr("__molt_is_generator__", gen_val)
        if is_async_generator:
            gen_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gen_val))
            set_attr("__molt_is_async_generator__", gen_val)

    def _build_gpu_kernel_descriptor_json(
        self, *, func_symbol: str, func_name: str
    ) -> str:
        func_info = self.funcs_map[func_symbol]
        payload = {
            "schema_version": 1,
            "kind": "molt_gpu_kernel",
            "symbol": func_symbol,
            "name": func_name,
            "params": list(func_info["params"]),
            "ops": self.map_ops_to_json(func_info["ops"], function_name=func_name),
        }
        return json.dumps(payload, sort_keys=True, separators=(",", ":"))

    @staticmethod
    def _split_function_args(
        args: ast.arguments,
    ) -> tuple[list[ast.arg], list[ast.arg], list[ast.arg], str | None, str | None]:
        posonly = list(args.posonlyargs)
        pos_or_kw = list(args.args)
        kwonly = list(args.kwonlyargs)
        vararg = args.vararg.arg if args.vararg else None
        varkw = args.kwarg.arg if args.kwarg else None
        return posonly, pos_or_kw, kwonly, vararg, varkw

    @classmethod
    def _function_param_names(cls, args: ast.arguments) -> list[str]:
        posonly, pos_or_kw, kwonly, vararg, varkw = cls._split_function_args(args)
        names = [arg.arg for arg in posonly + pos_or_kw]
        if vararg is not None:
            names.append(vararg)
        names.extend(arg.arg for arg in kwonly)
        if varkw is not None:
            names.append(varkw)
        return names

    def _lookup_func_defaults(
        self, module_name: str | None, func_id: str
    ) -> dict[str, Any] | None:
        if module_name is None:
            module_name = self.module_name
        normalized = self._normalize_allowlist_module(module_name)
        if normalized is not None:
            module_name = normalized
        module_defaults = self.known_func_defaults.get(module_name)
        if module_defaults is None and module_name == self.module_name:
            module_defaults = self.module_func_defaults
        if module_defaults is None:
            return None
        return module_defaults.get(func_id)

    @staticmethod
    def _normalize_func_kind(kind: object) -> FunctionKind | None:
        return normalize_function_kind(kind)

    def _lookup_func_kind(self, module_name: str | None, func_id: str) -> str | None:
        if module_name is None:
            module_name = self.module_name
        normalized = self._normalize_allowlist_module(module_name)
        if normalized is not None:
            module_name = normalized
        module_kinds = self.known_func_kinds.get(module_name)
        if module_kinds is None and module_name == self.module_name:
            module_kinds = self.module_declared_funcs
        if module_kinds is None:
            return None
        return self._normalize_func_kind(module_kinds.get(func_id))

    def _known_function_symbol_target(self, func_symbol: str) -> tuple[str, str] | None:
        candidate_modules = set(self.known_func_defaults) | set(self.known_func_kinds)
        for raw_module_name in sorted(candidate_modules):
            module_name = (
                self._normalize_allowlist_module(raw_module_name) or raw_module_name
            )
            symbol_prefix = f"{self._sanitize_module_name(module_name)}__"
            if not func_symbol.startswith(symbol_prefix):
                continue
            func_id = func_symbol[len(symbol_prefix) :]
            if (
                self._lookup_func_defaults(module_name, func_id) is not None
                or self._lookup_func_kind(module_name, func_id) is not None
            ):
                return module_name, func_id
        return None

    def _known_module_function_type_hint(
        self, module_name: str | None, func_id: str
    ) -> str | None:
        if module_name is None:
            module_name = self.module_name
        normalized = self._normalize_allowlist_module(module_name)
        if normalized is not None:
            module_name = normalized
        info = self._lookup_func_defaults(module_name, func_id)
        info_kind = self._normalize_func_kind(info.get("kind")) if info else None
        kind = self._lookup_func_kind(module_name, func_id) or info_kind
        if info is None and kind is None:
            return None
        if info is not None and info.get("has_decorators"):
            return None
        kind = kind or FunctionKind.SYNC
        func_symbol = f"{self._sanitize_module_name(module_name)}__{func_id}"
        if kind == FunctionKind.SYNC:
            return f"Func:{func_symbol}"
        total_params = info.get("params") if info is not None else None
        param_count = total_params if isinstance(total_params, int) else 0
        frame_plan = stateful_function_frame_plan(
            kind=kind,
            poll_symbol=f"{func_symbol}_poll",
            param_count=param_count,
            has_closure=False,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        return frame_plan.function_type_hint(closure_size)

    def _emit_builtin_function(self, func_id: str) -> MoltValue:
        spec = BUILTIN_FUNC_SPECS[func_id]
        arity = len(spec.params) + len(spec.pos_or_kw_params) + len(spec.kwonly_params)
        if spec.vararg is not None:
            arity += 1
        func_val = MoltValue(self.next_var(), type_hint="function")
        self.emit(
            MoltOp(
                kind="BUILTIN_FUNC",
                args=[spec.runtime, arity],
                result=func_val,
            )
        )
        self._emit_function_metadata(
            func_val,
            name=func_id,
            qualname=func_id,
            posonly_params=list(spec.params),
            pos_or_kw_params=list(spec.pos_or_kw_params),
            kwonly_params=list(spec.kwonly_params),
            vararg=spec.vararg,
            varkw=None,
            default_exprs=list(spec.defaults),
            kw_default_exprs=list(spec.kw_defaults),
            docstring=None,
            bind_kind=MOLT_BIND_KIND_OPEN if func_id == "open" else None,
            module_override="builtins",
            emit_code=False,
        )
        return func_val

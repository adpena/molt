"""FunctionMetadataMixin: callable defaults, metadata, and known-function facts.

Move-only extraction from frontend/__init__.py. This lowering authority owns
callable parameter/default shape, function metadata emission, builtin function
metadata construction, and known-module function kind/type-hint lookups shared by
call, function, class, and module visitors.
"""

from __future__ import annotations

import ast
import json
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Callable, Protocol, Sequence, TypeVar

from molt.frontend._types import (
    BUILTIN_FUNC_SPECS,
    GEN_CONTROL_SIZE,
    MOLT_BIND_KIND_OPEN,
    MoltOp,
    MoltValue,
    _builtin_func_abi_arity,
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


_MetadataValue = TypeVar("_MetadataValue")


@dataclass(frozen=True)
class MaterializedFunctionMetadata:
    """Runtime-visible callable facts after Python defaults are evaluated."""

    name: str
    qualname: str
    module: str
    posonly_params: tuple[str, ...]
    pos_or_kw_params: tuple[str, ...]
    kwonly_params: tuple[str, ...]
    vararg: str | None
    varkw: str | None
    docstring: str | None
    execution_kind: FunctionKind
    bind_kind: int | None
    code_symbol: str | None
    trace_filename: str
    trace_lineno: int
    trace_name: str
    varnames: tuple[str, ...]
    code_names: tuple[str, ...]
    freevars: tuple[str, ...]
    cellvars: tuple[str, ...]


class FunctionMetadataEmitter(Protocol[_MetadataValue]):
    """Target-independent primitives for canonical function metadata emission."""

    def const_str(self, value: str) -> _MetadataValue: ...

    def const_int(self, value: int) -> _MetadataValue: ...

    def const_none(self) -> _MetadataValue: ...

    def tuple_new(self, values: list[_MetadataValue]) -> _MetadataValue: ...

    def code_new(
        self,
        values: list[_MetadataValue],
    ) -> _MetadataValue: ...

    def code_slot_set(
        self,
        code_symbol: str,
        code: _MetadataValue,
    ) -> None: ...

    def init_metadata(
        self,
        function: _MetadataValue,
        metadata: _MetadataValue,
        code: _MetadataValue,
        bind_kind: _MetadataValue,
    ) -> None: ...


def ordered_function_metadata_values(
    *,
    name: _MetadataValue,
    qualname: _MetadataValue,
    module: _MetadataValue,
    arg_names: _MetadataValue,
    posonly_count: _MetadataValue,
    kwonly_names: _MetadataValue,
    vararg: _MetadataValue,
    varkw: _MetadataValue,
    defaults: _MetadataValue,
    kwdefaults: _MetadataValue,
    doc: _MetadataValue,
    execution_kind: _MetadataValue,
    freevars: _MetadataValue,
    cellvars: _MetadataValue,
) -> tuple[_MetadataValue, ...]:
    """Return runtime function metadata in its single canonical field order."""

    return (
        name,
        qualname,
        module,
        arg_names,
        posonly_count,
        kwonly_names,
        vararg,
        varkw,
        defaults,
        kwdefaults,
        doc,
        execution_kind,
        freevars,
        cellvars,
    )


def emit_materialized_function_metadata(
    emitter: FunctionMetadataEmitter[_MetadataValue],
    *,
    function: _MetadataValue,
    materialize_defaults: Callable[
        [_MetadataValue],
        tuple[_MetadataValue, _MetadataValue, _MetadataValue],
    ],
    metadata: MaterializedFunctionMetadata,
) -> None:
    """Emit the single runtime function-metadata and code-slot wire shape."""

    name_value = emitter.const_str(metadata.name)
    qualname_value = emitter.const_str(metadata.qualname)
    module_value = emitter.const_str(metadata.module)
    arg_names = emitter.tuple_new(
        [
            emitter.const_str(param)
            for param in metadata.posonly_params + metadata.pos_or_kw_params
        ]
    )
    posonly_count = emitter.const_int(len(metadata.posonly_params))
    kwonly_names = emitter.tuple_new(
        [emitter.const_str(param) for param in metadata.kwonly_params]
    )
    vararg = (
        emitter.const_none()
        if metadata.vararg is None
        else emitter.const_str(metadata.vararg)
    )
    varkw = (
        emitter.const_none()
        if metadata.varkw is None
        else emitter.const_str(metadata.varkw)
    )
    function, defaults, kwdefaults = materialize_defaults(function)
    bind_kind = (
        emitter.const_none()
        if metadata.bind_kind is None
        else emitter.const_int(metadata.bind_kind)
    )
    doc = (
        emitter.const_none()
        if metadata.docstring is None
        else emitter.const_str(metadata.docstring)
    )
    freevars = emitter.tuple_new(
        [emitter.const_str(name) for name in metadata.freevars]
    )
    cellvars = emitter.tuple_new(
        [emitter.const_str(name) for name in metadata.cellvars]
    )

    code = emitter.const_none()
    if metadata.code_symbol is not None:
        filename = emitter.const_str(metadata.trace_filename)
        trace_lineno = emitter.const_int(metadata.trace_lineno)
        trace_name = emitter.const_str(metadata.trace_name)
        linetable = emitter.const_none()
        varnames = emitter.tuple_new(
            [emitter.const_str(name) for name in metadata.varnames]
        )
        code_names = emitter.tuple_new(
            [emitter.const_str(name) for name in metadata.code_names]
        )
        code = emitter.code_new(
            [
                filename,
                trace_name,
                trace_lineno,
                linetable,
                varnames,
                code_names,
                emitter.const_int(
                    len(metadata.posonly_params) + len(metadata.pos_or_kw_params)
                ),
                emitter.const_int(len(metadata.posonly_params)),
                emitter.const_int(len(metadata.kwonly_params)),
            ]
        )
        emitter.code_slot_set(metadata.code_symbol, code)

    execution_kind = {
        FunctionKind.SYNC: 0,
        FunctionKind.GENERATOR: 1,
        FunctionKind.ASYNC: 2,
        FunctionKind.ASYNC_GENERATOR: 3,
    }[metadata.execution_kind]
    metadata_tuple = emitter.tuple_new(
        list(
            ordered_function_metadata_values(
                name=name_value,
                qualname=qualname_value,
                module=module_value,
                arg_names=arg_names,
                posonly_count=posonly_count,
                kwonly_names=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                defaults=defaults,
                kwdefaults=kwdefaults,
                doc=doc,
                execution_kind=emitter.const_int(execution_kind),
                freevars=freevars,
                cellvars=cellvars,
            )
        )
    )
    emitter.init_metadata(function, metadata_tuple, code, bind_kind)


class _FrontendFunctionMetadataEmitter(FunctionMetadataEmitter[MoltValue]):
    def __init__(self, owner: _GeneratorProtocol) -> None:
        self._owner = owner

    def _result(self, kind: str, *, type_hint: str, args: list[Any]) -> MoltValue:
        value = MoltValue(self._owner.next_var(), type_hint=type_hint)
        self._owner.emit(MoltOp(kind=kind, args=args, result=value))
        return value

    def const_str(self, value: str) -> MoltValue:
        return self._result("CONST_STR", type_hint="str", args=[value])

    def const_int(self, value: int) -> MoltValue:
        return self._result("CONST", type_hint="int", args=[value])

    def const_none(self) -> MoltValue:
        return self._result("CONST_NONE", type_hint="None", args=[])

    def tuple_new(self, values: list[MoltValue]) -> MoltValue:
        return self._result("TUPLE_NEW", type_hint="tuple", args=list(values))

    def code_new(self, values: list[MoltValue]) -> MoltValue:
        return self._result("CODE_NEW", type_hint="code", args=list(values))

    def code_slot_set(self, code_symbol: str, code: MoltValue) -> None:
        self._owner.emit(
            MoltOp(
                kind="CODE_SLOT_SET",
                args=[code, self._owner._emit_globals_dict()],
                result=MoltValue("none"),
                metadata={"code_id": self._owner._register_code_symbol(code_symbol)},
            )
        )

    def init_metadata(
        self,
        function: MoltValue,
        metadata: MoltValue,
        code: MoltValue,
        bind_kind: MoltValue,
    ) -> None:
        self._owner._emit_runtime_call(
            "molt_function_init_metadata_packed",
            [function, metadata, code, bind_kind],
            type_hint="None",
        )


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
                func_spill = self._spill_async_value(func_val)

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
        code_symbol: str | None,
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
        execution_kind: FunctionKind = FunctionKind.SYNC,
        bind_kind: int | None = None,
        varnames: list[str] | None = None,
        code_names: list[str] | None = None,
        freevars: Sequence[str] = (),
        cellvars: Sequence[str] = (),
    ) -> None:
        varnames_list = varnames
        if varnames_list is None:
            varnames_list = self._varnames_from_params(
                posonly_params=posonly_params,
                pos_or_kw_params=pos_or_kw_params,
                kwonly_params=kwonly_params,
                vararg=vararg,
                varkw=varkw,
            )
        emit_materialized_function_metadata(
            _FrontendFunctionMetadataEmitter(self),
            function=func_val,
            materialize_defaults=lambda function: self._emit_function_default_values(
                function,
                default_exprs,
                kw_default_exprs,
                kwonly_params,
            ),
            metadata=MaterializedFunctionMetadata(
                name=name,
                qualname=qualname,
                module=module_override or self.module_name,
                posonly_params=tuple(posonly_params),
                pos_or_kw_params=tuple(pos_or_kw_params),
                kwonly_params=tuple(kwonly_params),
                vararg=vararg,
                varkw=varkw,
                docstring=docstring,
                execution_kind=execution_kind,
                bind_kind=bind_kind,
                code_symbol=code_symbol,
                trace_filename=trace_filename or self.source_path or "<unknown>",
                trace_lineno=int(trace_lineno or 0),
                trace_name=trace_name or qualname or name,
                varnames=tuple(varnames_list),
                code_names=tuple(code_names or ()),
                freevars=tuple(freevars),
                cellvars=tuple(cellvars),
            ),
        )

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

    def _lookup_func_kind(
        self, module_name: str | None, func_id: str
    ) -> FunctionKind | None:
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

    def _emit_builtin_function(
        self, func_id: str, *, runtime_requirement_bits: int = 0
    ) -> MoltValue:
        spec = BUILTIN_FUNC_SPECS[func_id]
        arity = _builtin_func_abi_arity(spec)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[func_id], result=name_val))
        func_val = MoltValue(self.next_var(), type_hint="function")
        self.emit(
            MoltOp(
                kind="BUILTIN_FUNC",
                args=[spec.runtime, arity, name_val],
                result=func_val,
                metadata={
                    "builtin_name": func_id,
                    **(
                        {"runtime_requirement_bits": runtime_requirement_bits}
                        if runtime_requirement_bits
                        else {}
                    ),
                },
            )
        )
        self._emit_function_metadata(
            func_val,
            code_symbol=None,
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
            freevars=(),
            cellvars=(),
        )
        return func_val

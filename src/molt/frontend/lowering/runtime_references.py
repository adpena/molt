"""RuntimeReferenceMixin: runtime, intrinsic, and builtin reference lowering.

Move-only extraction from frontend/__init__.py. This lowering authority owns
constant/default materialization, runtime function handle construction,
intrinsic lookup/handle calls, builtin type refs, and local intrinsic wrapper
detection shared by import, call, class, expression, and pattern visitors.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Sequence

from molt.frontend._types import (
    _INLINE_INT_MAX,
    _INLINE_INT_MIN,
    _canonical_intrinsic_runtime_name,
    _intrinsic_arity_exact,
    _intrinsic_defaults_exact,
    BUILTIN_EXCEPTION_NAMES,
    BUILTIN_FUNC_SPECS,
    BUILTIN_TYPE_TAGS,
    INTRINSIC_HANDLE_CLASS_CONSTRUCTORS_BY_TYPE,
    IntrinsicHandleClassConstructorSpec,
    MoltOp,
    MoltValue,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class RuntimeReferenceMixin(_MixinBase):
    def _emit_const_value(self, value: object) -> MoltValue:
        if value is None:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            return res
        if value is Ellipsis:
            res = MoltValue(self.next_var(), type_hint="ellipsis")
            self.emit(MoltOp(kind="CONST_ELLIPSIS", args=[], result=res))
            return res
        if value is NotImplemented:
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="CONST_NOT_IMPLEMENTED", args=[], result=res))
            return res
        if isinstance(value, bool):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[value], result=res))
            return res
        if isinstance(value, int):
            if _INLINE_INT_MIN <= value <= _INLINE_INT_MAX:
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[value], result=res))
            else:
                res = MoltValue(self.next_var(), type_hint="bigint")
                self.emit(MoltOp(kind="CONST_BIGINT", args=[str(value)], result=res))
            return res
        if isinstance(value, float):
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[value], result=res))
            return res
        if isinstance(value, complex):
            real = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[value.real], result=real))
            imag = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[value.imag], result=imag))
            has_imag = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_imag))
            res = MoltValue(self.next_var(), type_hint="complex")
            self.emit(
                MoltOp(kind="COMPLEX_FROM_OBJ", args=[real, imag, has_imag], result=res)
            )
            return res
        if isinstance(value, str):
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[value], result=res))
            return res
        if isinstance(value, bytes):
            res = MoltValue(self.next_var(), type_hint="bytes")
            self.emit(MoltOp(kind="CONST_BYTES", args=[value], result=res))
            return res
        raise NotImplementedError(f"Unsupported default literal: {value!r}")

    def _emit_intrinsic_function(self, runtime_name: str) -> MoltValue:
        arity = _intrinsic_arity_exact(runtime_name)
        if arity is None:
            raise KeyError(runtime_name)
        return self._emit_runtime_function_with_defaults(
            _canonical_intrinsic_runtime_name(runtime_name),
            arity,
            _intrinsic_defaults_exact(runtime_name),
        )

    def _emit_optional_intrinsic_lookup_value(self, runtime_name: str) -> MoltValue:
        loader = self._emit_runtime_function("molt_load_intrinsic_runtime", 2)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[runtime_name], result=name_val))
        namespace_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=namespace_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[loader, name_val, namespace_val], result=res)
        )
        return res

    def _emit_runtime_function(self, runtime_name: str, arity: int) -> MoltValue:
        func_val = MoltValue(self.next_var(), type_hint="function")
        self.emit(
            MoltOp(
                kind="BUILTIN_FUNC",
                args=[runtime_name, arity],
                result=func_val,
            )
        )
        return func_val

    def _emit_runtime_function_with_none_defaults(
        self, runtime_name: str, arity: int, *, default_count: int
    ) -> MoltValue:
        return self._emit_runtime_function_with_defaults(
            runtime_name, arity, (None,) * max(0, default_count)
        )

    def _emit_runtime_function_with_defaults(
        self, runtime_name: str, arity: int, defaults: Sequence[object]
    ) -> MoltValue:
        func_val = self._emit_runtime_function(runtime_name, arity)
        if not defaults:
            return func_val
        default_vals = [self._emit_const_value(value) for value in defaults]
        defaults_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=default_vals, result=defaults_tuple))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[func_val, "__defaults__", defaults_tuple],
                result=MoltValue("none"),
            )
        )
        return func_val

    def _intrinsic_handle_class_spec_for_value(
        self, value: MoltValue | None
    ) -> IntrinsicHandleClassConstructorSpec | None:
        if value is None:
            return None
        return INTRINSIC_HANDLE_CLASS_CONSTRUCTORS_BY_TYPE.get(value.type_hint)

    def _emit_intrinsic_handle_class_call(
        self,
        obj: MoltValue,
        spec: IntrinsicHandleClassConstructorSpec,
        intrinsic_name: str,
        args: list[MoltValue],
        *,
        result_hint: str,
    ) -> MoltValue:
        handle = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[obj, spec.handle_attr],
                result=handle,
            )
        )
        intrinsic_func = self._emit_intrinsic_function(intrinsic_name)
        res = MoltValue(self.next_var(), type_hint=result_hint)
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[intrinsic_func, handle] + args,
                result=res,
            )
        )
        return res

    @staticmethod
    def _is_intrinsics_module_name(module_name: str | None) -> bool:
        if module_name is None:
            return False
        return module_name == "_intrinsics" or module_name.endswith("._intrinsics")

    @staticmethod
    def _is_safe_intrinsic_namespace_expr(expr: ast.expr) -> bool:
        if isinstance(expr, ast.Constant) and expr.value is None:
            return True
        if isinstance(expr, ast.Name):
            return True
        return (
            isinstance(expr, ast.Call)
            and isinstance(expr.func, ast.Name)
            and expr.func.id == "globals"
            and not expr.args
            and not expr.keywords
        )

    def _maybe_record_local_intrinsic_wrapper(self, node: ast.FunctionDef) -> None:
        if (
            node.decorator_list
            or node.args.posonlyargs
            or len(node.args.args) not in {1, 2}
            or node.args.vararg is not None
            or node.args.kwonlyargs
            or node.args.kw_defaults
            or node.args.kwarg is not None
        ):
            return
        if len(node.args.args) == 1:
            if node.args.defaults:
                return
        else:
            if len(node.args.defaults) != 1:
                return
            default = node.args.defaults[0]
            if not (isinstance(default, ast.Constant) and default.value is None):
                return
        import_alias = None
        if len(node.body) == 1 and isinstance(node.body[0], ast.Return):
            ret_stmt = node.body[0]
        elif (
            len(node.body) == 2
            and isinstance(node.body[0], ast.ImportFrom)
            and isinstance(node.body[1], ast.Return)
        ):
            import_stmt = node.body[0]
            if not self._is_intrinsics_module_name(import_stmt.module):
                return
            if len(import_stmt.names) != 1:
                return
            alias = import_stmt.names[0]
            if alias.name != "require_intrinsic":
                return
            import_alias = alias.asname or alias.name
            ret_stmt = node.body[1]
        else:
            return
        ret = ret_stmt.value
        if (
            ret is None
            or not isinstance(ret, ast.Call)
            or not isinstance(ret.func, ast.Name)
        ):
            return
        if import_alias is not None:
            if ret.func.id != import_alias:
                return
        else:
            if ret.func.id not in {"require_intrinsic", "_require_intrinsic"}:
                return
            imported_from = self.imported_names.get(ret.func.id)
            if not self._is_intrinsics_module_name(imported_from):
                return
        param_name = node.args.args[0].arg
        namespace_param = node.args.args[1].arg if len(node.args.args) == 2 else None
        if not ret.args or not isinstance(ret.args[0], ast.Name):
            return
        if ret.args[0].id != param_name or len(ret.args) > 2:
            return
        if len(ret.args) == 2:
            namespace_expr = ret.args[1]
            if namespace_param is not None:
                if not (
                    isinstance(namespace_expr, ast.Name)
                    and namespace_expr.id == namespace_param
                ):
                    return
            elif not self._is_safe_intrinsic_namespace_expr(namespace_expr):
                return
        if any(kw.arg is None for kw in ret.keywords):
            return
        for kw in ret.keywords:
            if kw.arg == "name":
                if not isinstance(kw.value, ast.Name) or kw.value.id != param_name:
                    return
            elif kw.arg == "namespace":
                if namespace_param is not None:
                    if not (
                        isinstance(kw.value, ast.Name)
                        and kw.value.id == namespace_param
                    ):
                        return
                elif not self._is_safe_intrinsic_namespace_expr(kw.value):
                    return
            else:
                return
        self.local_intrinsic_wrappers.add(node.name)

    def _name_resolves_to_builtin(self, name: str) -> bool:
        """True if `name` names a builtin type/function/exception.

        Used to keep `del`/`except`-target read routing CPython-faithful for
        names that shadow a builtin: once the module binding is removed, a bare
        read must fall back to the builtin (which the regular `visit_Name`
        resolution materialises statically), not raise NameError.
        """
        return (
            name in BUILTIN_TYPE_TAGS
            or name in BUILTIN_FUNC_SPECS
            or name in BUILTIN_EXCEPTION_NAMES
        )

    def _emit_builtin_type_value(self, type_name: str) -> MoltValue:
        tag_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="CONST", args=[BUILTIN_TYPE_TAGS[type_name]], result=tag_val)
        )
        res = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=res))
        return res

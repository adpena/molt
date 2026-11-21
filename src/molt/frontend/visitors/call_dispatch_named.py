"""CallNamedDispatchMixin: extracted visit_Call dispatch phase."""

from __future__ import annotations

from molt.compiler_analysis.python_imports import is_compiler_intrinsics_module

from molt.compiler_analysis.python_imports import normalize_stdlib_module_name
from molt.compiler_analysis.python_identity_catalog_generated import (
    BUILTIN_NAMES_BY_IDENTITY,
    PythonIdentity,
)

import ast
from typing import (
    Any,
)

from molt.frontend.class_model import (
    ClassView,
    Closure,
    ConstructorPlan,
    MemberKind,
    ReleasePlan,
    Resolved,
    ResolveRefusal,
    Unresolved,
    release_plan,
    resolve,
)
from molt.frontend._types import (
    BoundMethodHint,
    BuiltinLayout,
    ClassId,
    DeclaredClass,
    MoltOp,
    MoltValue,
    TypeHint,
    _intrinsic_arity_exact,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.exception_kinds_generated import (
    BUILTIN_EXCEPTION_CODES,
    BUILTIN_EXCEPTION_LAYOUT_KINDS,
)
from molt.frontend.diagnostics import FrontendRejection


from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED
from molt.frontend._mixin_base import GeneratorMixinBase


class CallNamedDispatchMixin(GeneratorMixinBase):
    def _try_emit_imported_exception_class_constructor(
        self,
        node: ast.Call,
        *,
        func_id: str,
        imported_from: str | None,
    ) -> Any:
        if imported_from is None:
            return CALL_NOT_HANDLED
        original_attr = self._imported_attr_name(func_id)
        class_id = self._class_exports.get((imported_from, original_attr))
        if class_id is None or not self._class_is_exception_subclass(class_id):
            return CALL_NOT_HANDLED
        if node.keywords or any(isinstance(arg, ast.Starred) for arg in node.args):
            return CALL_NOT_HANDLED
        args: list[MoltValue] = []
        for arg in node.args:
            arg_val = self.visit(arg)
            if arg_val is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE,
                    f"{original_attr} with unsupported arg expression",
                    f"{original_attr} with simple arguments",
                    "argument expression could not be lowered",
                )
            args.append(arg_val)
        class_val = self.visit(node.func)
        if class_val is None:
            raise FrontendRejection(
                Diagnostic.CALL_TARGET,
                "Unsupported exception class call target",
            )
        return self._emit_exception_new_from_class(class_val, args)

    def _try_emit_imported_classlike_call_bind(
        self,
        node: ast.Call,
        *,
        func_id: str,
        imported_from: str | None,
    ) -> Any:
        if imported_from is None:
            return CALL_NOT_HANDLED
        original_attr = self._imported_attr_name(func_id)
        if not original_attr[:1].isupper():
            return CALL_NOT_HANDLED
        callee = self.visit(node.func)
        if callee is None:
            raise FrontendRejection(
                Diagnostic.CALL_TARGET,
                "Unsupported imported class-like call target",
            )
        res = MoltValue(self.next_var(), type_hint="Any")
        callargs = self._emit_call_args_builder(node)
        self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
        return res

    def _try_emit_imported_named_call(
        self,
        node: ast.Call,
        *,
        func_id: str,
        imported_from: str | None,
        needs_bind: bool,
    ) -> Any:
        if imported_from is None:
            return CALL_NOT_HANDLED
        if imported_from == "builtins" or is_compiler_intrinsics_module(imported_from):
            return CALL_NOT_HANDLED

        normalized = normalize_stdlib_module_name(imported_from)
        visible_module = normalized or imported_from
        original_attr = self._imported_attr_name(func_id)
        native_call = self._try_emit_published_native_object_call(
            node, visible_module, original_attr
        )
        if native_call is not None:
            return native_call
        imported_exception_ctor = self._try_emit_imported_exception_class_constructor(
            node,
            func_id=func_id,
            imported_from=imported_from,
        )
        if imported_exception_ctor is not CALL_NOT_HANDLED:
            return imported_exception_ctor
        visible_import_authorized = self._should_attempt_runtime_module_import(
            imported_from
        ) or self._is_internal_module(imported_from)
        if not visible_import_authorized:
            return CALL_NOT_HANDLED
        target_module = visible_module

        lowered_imported_call = self._try_emit_imported_module_direct_or_task_call(
            target_module,
            original_attr,
            node,
            needs_bind=needs_bind,
        )
        if lowered_imported_call is not None:
            return lowered_imported_call
        callee = self.visit(node.func)
        if callee is None:
            raise FrontendRejection(Diagnostic.CALL_TARGET, "Unsupported call target")
        return self._emit_dynamic_call(node, callee)

    def _try_emit_named_call(self, node: ast.Call, needs_bind: bool) -> Any:
        if isinstance(node.func, ast.Name):
            func_id = node.func.id
            imported_binding = self.imported_names.get(func_id)
            if (
                imported_binding is None
                and func_id not in self.locals
                and func_id not in self.boxed_locals
            ):
                imported_binding = self.global_imported_names.get(func_id)
            imported_from = imported_binding
            target_info = self.locals.get(func_id)
            if target_info is None:
                target_info = self.globals.get(func_id)
            is_local = func_id in self.locals or func_id in self.boxed_locals
            if self.is_async() and func_id in self.async_locals:
                loaded = self._load_local_value(func_id)
                if loaded is not None:
                    target_info = loaded
                is_local = True
            if is_local and imported_binding is None:
                imported_from = None
            if imported_from == "builtins":
                # Proven shape specialization runs before named dispatch.
                # An import origin never changes the source namespace binding
                # that owns the callable, its rebinding, or its lifetime.
                callee = self.visit(node.func)
                if callee is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_TARGET, "Unsupported imported builtin target"
                    )
                return self._emit_dynamic_call(node, callee)
            if imported_from:
                normalized = normalize_stdlib_module_name(imported_from)
                allowlist_key = normalized or imported_from
                native_call = self._try_emit_published_native_object_call(
                    node, allowlist_key, self._imported_attr_name(func_id)
                )
                if native_call is not None:
                    return native_call
                original_attr = self._imported_attr_name(func_id)
                known_func_hint = self._known_module_function_type_hint(
                    allowlist_key, original_attr
                )
                if known_func_hint is not None:
                    if target_info is None:
                        target_info = MoltValue(func_id, type_hint=known_func_hint)
                    else:
                        target_info.type_hint = known_func_hint
            # Try lowering _intrinsics.require_intrinsic("name") calls to a
            # BUILTIN_FUNC opcode early, before any local-function dispatch
            # path (e.g. a `def _require_intrinsic(...)` defined in an except
            # handler) can intercept the call and produce a CALL_BIND on a
            # never-assigned sentinel local.
            if is_compiler_intrinsics_module(imported_binding) and func_id in {
                "require_intrinsic",
                "_require_intrinsic",
            }:
                lowered_intrinsic_early = self._try_lower_intrinsic_lookup_call(
                    func_id=func_id,
                    imported_from=imported_binding,
                    node=node,
                )
                if lowered_intrinsic_early is not None:
                    return lowered_intrinsic_early
            imported_exception_ctor = (
                self._try_emit_imported_exception_class_constructor(
                    node,
                    func_id=func_id,
                    imported_from=imported_from,
                )
            )
            if imported_exception_ctor is not CALL_NOT_HANDLED:
                return imported_exception_ctor
            imported_classlike_call = self._try_emit_imported_classlike_call_bind(
                node,
                func_id=func_id,
                imported_from=imported_from,
            )
            if imported_classlike_call is not CALL_NOT_HANDLED:
                return imported_classlike_call
            source_symbol = self._source_callable_candidate(node.func)
            if source_symbol is not None:
                target_info = MoltValue(func_id, type_hint=f"Func:{source_symbol}")
            imported_binding_is_live = imported_from is not None and (
                self.current_func_name != "molt_main"
                or func_id not in self.module_global_mutations
                or target_info is not None
            )
            if imported_binding_is_live:
                lowered_imported_call = self._try_emit_imported_named_call(
                    node,
                    func_id=func_id,
                    imported_from=imported_from,
                    needs_bind=needs_bind,
                )
                if lowered_imported_call is not CALL_NOT_HANDLED:
                    return lowered_imported_call
            if (
                self.current_func_name == "molt_main"
                and func_id in self.module_global_mutations
            ):
                callee = self._emit_global_get(func_id)
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[callee, callargs],
                        result=res,
                    )
                )
                return res
            lowered_wrapper_intrinsic = self._try_lower_local_intrinsic_wrapper_call(
                func_id=func_id,
                node=node,
            )
            if lowered_wrapper_intrinsic is not None:
                return lowered_wrapper_intrinsic
            # Every builtin branch below keys on the identity the read is
            # proved to select (HF-776). An unproved read matches none of them
            # and keeps the live read and a dynamic call.
            builtin = self._proven_builtin_identity(node.func)
            exception_name = (
                None if builtin is None else BUILTIN_NAMES_BY_IDENTITY[builtin]
            )
            if (
                exception_name is not None
                and exception_name in BUILTIN_EXCEPTION_CODES
                and self._builtin_exception_is_available(exception_name)
                and all(keyword.arg is not None for keyword in node.keywords)
                and not any(isinstance(arg, ast.Starred) for arg in node.args)
            ):
                # A proven builtin exception class is one construction by class
                # tag (closed world: a builtin class has no writer). The class's
                # own constructor validates the arguments (Unicode, syntax and
                # group errors may refuse); named keywords apply through the
                # layout's constructor keywords in the same runtime call.
                def lower(expression: ast.expr) -> MoltValue:
                    value = self.visit(expression)
                    if value is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            f"{exception_name} with unsupported arg expression",
                            f"{exception_name} with simple literals",
                            "argument expression could not be lowered",
                        )
                    return value

                args = [lower(arg) for arg in node.args]
                if not node.keywords:
                    error = self._emit_exception_new_from_args(exception_name, args)
                    self._emit_raise_if_pending()
                    return error
                # Positional values, then keyword values, in source order.
                kw_values = [lower(keyword.value) for keyword in node.keywords]
                kw_names = [
                    self._emit_const_value(keyword.arg) for keyword in node.keywords
                ]
                tuples = []
                for items in (args, kw_names, kw_values):
                    packed = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(MoltOp(kind="TUPLE_NEW", args=items, result=packed))
                    tuples.append(packed)
                error = self._emit_intrinsic_call(
                    "molt_exception_new_builtin_kw",
                    [
                        self._emit_const_value(BUILTIN_EXCEPTION_CODES[exception_name]),
                        *tuples,
                    ],
                    type_hint="exception",
                )
                # The tag construction builds that builtin (or the builtin
                # OSError subclass errno promotion picks): its layout is fixed.
                self._stamp_exact_class(
                    error,
                    BuiltinLayout(BUILTIN_EXCEPTION_LAYOUT_KINDS[exception_name]),
                    durable=True,
                )
                return error
            if (
                builtin is PythonIdentity.BUILTIN_ABS
                and len(node.args) == 1
                and not node.keywords
            ):
                value = self.visit(node.args[0])
                if value is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "abs expects a lowerable operand",
                    )
                if value.type_hint in {"bool", "int"}:
                    result_hint = "int"
                elif value.type_hint == "float":
                    result_hint = "float"
                else:
                    result_hint = "Any"
                res = MoltValue(self.next_var(), type_hint=result_hint)
                self.emit(MoltOp(kind="ABS", args=[value], result=res))
                return res
            if builtin is PythonIdentity.BUILTIN_GLOBALS:
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"globals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_globals_dict()
            if builtin is PythonIdentity.BUILTIN_LOCALS:
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"locals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_locals_dict()
            if builtin is PythonIdentity.BUILTIN_VARS:
                if (
                    node.keywords
                    or len(node.args) > 1
                    or any(isinstance(arg, ast.Starred) for arg in node.args)
                ):
                    return CALL_NOT_HANDLED
                if not node.args:
                    return self._emit_locals_dict()
                obj = self.visit(node.args[0])
                if obj is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "vars expects a simple object",
                    )
                callee = self._emit_builtin_function("vars")
                res = MoltValue(self.next_var(), type_hint="dict")
                receiver_name = (
                    node.args[0].id if isinstance(node.args[0], ast.Name) else None
                )
                exact_class = self._exact_class_for_value(obj, receiver_name)
                requirement_bits = self._runtime_protected_attribute_requirement_bits(
                    obj,
                    "__dict__",
                    exact_class=exact_class,
                )
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee, obj],
                        result=res,
                        metadata=(
                            {"runtime_requirement_bits": requirement_bits}
                            if requirement_bits
                            else None
                        ),
                    )
                )
                return res
            if builtin is PythonIdentity.BUILTIN_DIR:
                if (
                    node.keywords
                    or len(node.args) > 1
                    or any(isinstance(arg, ast.Starred) for arg in node.args)
                ):
                    return CALL_NOT_HANDLED
                if not node.args:
                    locals_dict = self._emit_locals_dict()
                    keys = MoltValue(self.next_var(), type_hint="dict_keys")
                    self.emit(MoltOp(kind="DICT_KEYS", args=[locals_dict], result=keys))
                    key_none = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_none))
                    reverse_false = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=reverse_false)
                    )
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(
                        MoltOp(
                            kind="CALL",
                            args=["molt_sorted_builtin", keys, key_none, reverse_false],
                            result=res,
                        )
                    )
                    return res
                obj = self.visit(node.args[0])
                if obj is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE, "dir expects a simple object"
                    )
                callee = self._emit_builtin_function("dir")
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee, obj], result=res))
                return res
            if builtin is PythonIdentity.BUILTIN_GETATTR:
                if len(node.args) not in {2, 3} or node.keywords:
                    return CALL_NOT_HANDLED
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "getattr expects object and name",
                    )
                default = None
                if len(node.args) == 3:
                    default = self.visit(node.args[2])
                    if default is None:
                        default = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res_hint: TypeHint = "Any"
                name_lit = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    name_lit = node.args[1].value
                receiver_name = (
                    node.args[0].id if isinstance(node.args[0], ast.Name) else None
                )
                receiver_exact_class = self._exact_class_for_value(obj, receiver_name)
                receiver_module_name = (
                    self._imported_module_binding_target(node.args[0].id)
                    if isinstance(node.args[0], ast.Name)
                    else None
                )
                if name_lit and receiver_exact_class is not None:
                    exact_view = self.classes.get(receiver_exact_class)
                    if exact_view is not None and exact_view.layout.static:
                        if exact_view.layout.field(name_lit) is not None:
                            if exact_view.dataclass is None:
                                return self._emit_guarded_getattr(
                                    obj,
                                    name_lit,
                                    receiver_exact_class,
                                    obj_name=receiver_name,
                                )
                if name_lit:
                    declared = self._declared_class(obj)
                    lookup_class: ClassId | None = (
                        declared.class_id if declared is not None else None
                    )
                    if lookup_class is None or lookup_class not in self.classes:
                        lookup_class = self._class_reference(node.args[0])
                    if lookup_class is not None and lookup_class in self.classes:
                        member, method_class = self._resolve_member(
                            lookup_class, name_lit
                        )
                        if member is not None:
                            if member.kind in (
                                MemberKind.FUNCTION,
                                MemberKind.CLASSMETHOD,
                            ):
                                res_hint = BoundMethodHint(
                                    method_class or lookup_class, name_lit
                                )
                            elif (
                                member.kind is MemberKind.STATICMETHOD
                                and member.symbol is not None
                            ):
                                res_hint = f"Func:{member.symbol}"
                res = MoltValue(self.next_var(), type_hint=res_hint)
                if default is not None:
                    receiver_exact_class = self._exact_class_for_value(
                        obj, receiver_name
                    )
                    self._emit_getattr_name_default(
                        obj,
                        name,
                        default,
                        res,
                        literal_name=name_lit,
                        exact_class=receiver_exact_class,
                        qualified_module_name=receiver_module_name,
                    )
                else:
                    # Without a default an absent attribute raises: its own op,
                    # since GETATTR_NAME_DEFAULT returns any default it is given.
                    self._emit_getattr_name(
                        obj,
                        name,
                        res,
                        literal_name=name_lit,
                        exact_class=receiver_exact_class,
                        qualified_module_name=receiver_module_name,
                    )
                return res
            if builtin is PythonIdentity.BUILTIN_SETATTR:
                if len(node.args) != 3 or node.keywords:
                    return CALL_NOT_HANDLED
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                val = self.visit(node.args[2])
                if obj is None or name is None or val is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "setattr expects object, name, value",
                    )
                attr_name = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    attr_name = node.args[1].value
                if attr_name is not None:
                    obj_name = (
                        node.args[0].id if isinstance(node.args[0], ast.Name) else None
                    )
                    self._emit_attribute_store(
                        obj,
                        node.args[0],
                        obj_name,
                        attr_name,
                        val,
                    )
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="SETATTR_NAME",
                        args=[obj, name, val],
                        result=MoltValue("none"),
                    )
                )
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if builtin is PythonIdentity.BUILTIN_DELATTR:
                if len(node.args) != 2 or node.keywords:
                    return CALL_NOT_HANDLED
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "delattr expects object and name",
                    )
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    attr_name = node.args[1].value
                    receiver_name = (
                        node.args[0].id if isinstance(node.args[0], ast.Name) else None
                    )
                    exact_class = self._exact_class_for_value(obj, receiver_name)
                    declared = self._declared_class(obj)
                    declared_class = (
                        declared.class_id
                        if declared is not None and declared.class_id in self.classes
                        else None
                    )
                    if exact_class is not None:
                        self._record_instance_attr_mutation(exact_class, attr_name)
                    elif declared_class is not None:
                        self._record_instance_attr_mutation(declared_class, attr_name)
                    res = MoltValue(self.next_var(), type_hint="None")
                    attr_name = node.args[1].value
                    if declared_class is not None:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_PTR",
                                args=[obj, attr_name],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_OBJ",
                                args=[obj, attr_name],
                                result=MoltValue("none"),
                            )
                        )
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="DELATTR_NAME",
                        args=[obj, name],
                        result=MoltValue("none"),
                    )
                )
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if builtin is PythonIdentity.BUILTIN_HASATTR:
                if len(node.args) != 2 or node.keywords:
                    return CALL_NOT_HANDLED
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "hasattr expects object and name",
                    )
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="HASATTR_NAME",
                        args=[obj, name],
                        result=res,
                    )
                )
                return res
            if builtin is PythonIdentity.BUILTIN_SUPER:
                if not node.args and not node.keywords:
                    res = MoltValue(self.next_var(), type_hint="super")
                    self.emit(
                        MoltOp(kind="CALL", args=["molt_super_from_frame"], result=res)
                    )
                    return res
                # Constructor diagnostics follow ordinary argument evaluation.
                return CALL_NOT_HANDLED
            if builtin is PythonIdentity.BUILTIN_CLASSMETHOD:
                if len(node.args) != 1 or node.keywords:
                    return CALL_NOT_HANDLED
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "classmethod expects a function",
                    )
                res = MoltValue(self.next_var(), type_hint="classmethod")
                self.emit(MoltOp(kind="CLASSMETHOD_NEW", args=[func_val], result=res))
                return res
            if builtin is PythonIdentity.BUILTIN_STATICMETHOD:
                if len(node.args) != 1 or node.keywords:
                    return CALL_NOT_HANDLED
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "staticmethod expects a function",
                    )
                res = MoltValue(self.next_var(), type_hint="staticmethod")
                self.emit(MoltOp(kind="STATICMETHOD_NEW", args=[func_val], result=res))
                return res
            if builtin is PythonIdentity.BUILTIN_PROPERTY:
                # The fast path builds only a doc-less property whose accessors
                # arrive positionally or by keyword in slot order, so visiting
                # the slots evaluates them in source order. Every other call,
                # including any doc (the constructor owns the getter-doc flag)
                # and every malformed shape, is an ordinary call: CPython's
                # Argument Clinic diagnostics follow argument evaluation.
                accessor_names = ("fget", "fset", "fdel")
                if len(node.args) > len(accessor_names) or any(
                    isinstance(arg, ast.Starred) for arg in node.args
                ):
                    return CALL_NOT_HANDLED
                accessor_exprs: list[ast.expr | None] = list(node.args)
                accessor_exprs.extend([None] * (len(accessor_names) - len(node.args)))
                next_slot = len(node.args)
                for kw in node.keywords:
                    if kw.arg not in accessor_names:
                        return CALL_NOT_HANDLED
                    slot = accessor_names.index(kw.arg)
                    if slot < next_slot:
                        return CALL_NOT_HANDLED
                    accessor_exprs[slot] = kw.value
                    next_slot = slot + 1
                getter_expr, setter_expr, deleter_expr = accessor_exprs
                if getter_expr is None:
                    getter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=getter))
                else:
                    getter = self.visit(getter_expr)
                    if getter is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "property expects a getter",
                        )
                if setter_expr is None:
                    setter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=setter))
                else:
                    setter = self.visit(setter_expr)
                    if setter is None:
                        raise FrontendRejection(
                            Diagnostic.UNSUPPORTED_FEATURE,
                            "property setter unsupported",
                        )
                if deleter_expr is None:
                    deleter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=deleter))
                else:
                    deleter = self.visit(deleter_expr)
                    if deleter is None:
                        raise FrontendRejection(
                            Diagnostic.UNSUPPORTED_FEATURE,
                            "property deleter unsupported",
                        )
                res = MoltValue(self.next_var(), type_hint="property")
                self.emit(
                    MoltOp(
                        kind="PROPERTY_NEW",
                        args=[getter, setter, deleter],
                        result=res,
                    )
                )
                return res
            if builtin is PythonIdentity.BUILTIN_PRINT:
                # Print is an ordinary value-producing builtin call. Its runtime
                # authority owns per-argument writes, conversion failures, stream
                # lookup, keyword binding, and the Python None result.
                callee = self._emit_builtin_function(func_id)
                callargs = self._emit_call_args_builder(node)
                result = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="CALL_BIND", args=[callee, callargs], result=result)
                )
                return result
            # Binding flow names the one class statement the callee reads, or
            # nothing; an import binding resolves through the exporting module.
            class_id = self._class_reference(node.func)
            if class_id is not None and class_id not in self.classes:
                class_id = None
            if (
                class_id is not None
                and imported_from
                and class_id.module != self.module_name
            ):
                class_ref = self._emit_module_attr_get_on(
                    imported_from, self._imported_attr_name(func_id)
                )
                # Dispatch through the imported class object so runtime
                # identity owns constructor semantics.
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[class_ref, callargs],
                        result=res,
                    )
                )
                return res
            if class_id is not None and target_info is not None:
                class_value_name = self.class_value_names.get(class_id)
                if (
                    class_value_name is not None
                    and target_info.name != class_value_name
                ):
                    class_id = None
                elif class_value_name is None and self.current_func_name == "molt_main":
                    class_id = None
            if (
                class_id is not None
                and self.current_func_name == "molt_main"
                and func_id in self.del_targets
                and not self._name_resolves_to_builtin(func_id)
            ):
                # A module-scope class whose name is `del`'d (or shadowed by an
                # `except ... as` target) may be unbound when called; a bare Name
                # read has LOAD_GLOBAL semantics, so resolve through
                # MODULE_GET_GLOBAL (NameError on a missing binding) rather than
                # the static class ref the known-class fast path would otherwise
                # take. Dropping `class_id` routes the callee through the generic
                # Name resolution, which applies the same del-target rule.
                class_id = None
            if class_id is not None:
                class_info = self.classes[class_id]
                if self.current_func_name == "molt_main":
                    # Resolve the class reference through the single audited
                    # static-class resolver, which enforces the chunk-liveness
                    # guard (`__init__.py` `_current_module_static_class_ref`,
                    # lines 4678-4680: `self.globals[class_id].name ==
                    # class_value_name`).  When `molt_main` is split into
                    # multiple `molt_module_chunk_N` functions, a class defined
                    # in chunk N and instantiated in chunk N+M has had its
                    # `class_value_name` SSA value reset out of `self.globals`
                    # at the chunk boundary (`_reset_module_chunk_state`); the
                    # resolver then returns None and we fall back to a
                    # chunk-safe MODULE_GET_GLOBAL re-fetch.  Trusting
                    # `class_value_name` directly here would materialise a
                    # dangling cross-chunk SSA ref that lowering degrades to a
                    # CONST_STR of the variable name, feeding a string where a
                    # type is expected (task #50).  The constructor proof
                    # gate is preserved: inline allocation still fires for
                    # the in-chunk case, because a class with no
                    # `constructor_refusal` always satisfies the
                    # resolver's (superset) layout/decoration/mutation guards.
                    static_class_ref = self._current_module_static_class_ref(class_id)
                    if static_class_ref is not None and isinstance(
                        class_info.constructor, ConstructorPlan
                    ):
                        class_ref = static_class_ref
                    else:
                        class_ref = self._emit_global_get(func_id)
                else:
                    # The call's own read of the class name: its source fact,
                    # frame storage and capture across the arguments.
                    class_ref = self.visit(node.func)
                    if class_ref is None:
                        raise FrontendRejection(
                            Diagnostic.CALL_TARGET, "Unsupported call target"
                        )
                if self._class_is_exception_subclass(class_id):
                    # Exception __init__ is a mutable Python descriptor too.
                    # Runtime class construction owns allocation and binding.
                    self._record_constructor_fallback(
                        node,
                        class_id,
                        "an exception class: the runtime owns its constructor",
                    )
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="exception")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[class_ref, callargs],
                            result=res,
                        )
                    )
                    return res
                constructor = class_info.constructor
                refusal = (
                    None
                    if isinstance(constructor, ConstructorPlan)
                    else constructor.reason
                )
                if refusal is None:
                    constructed = self._emit_inline_construction(
                        node, class_id, class_info, class_ref
                    )
                    if isinstance(constructed, MoltValue):
                        return constructed
                    refusal = constructed
                self._record_constructor_fallback(node, class_id, refusal)
                if class_info.dataclass is not None:
                    # Allocation and the generated method use the ordinary class
                    # protocol. A positional call passes its operands directly,
                    # as the unresolved-callee path does: binding an argument
                    # builder cost one allocation and a bind-cache miss per call.
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if not node.keywords and not any(
                        isinstance(argument, ast.Starred) for argument in node.args
                    ):
                        args = self._emit_call_args(node.args)
                        self.emit(
                            MoltOp(
                                kind="CALL_FUNC", args=[class_ref, *args], result=res
                            )
                        )
                        return res
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[class_ref, callargs], result=res)
                    )
                    return res
                _, new_returns_any = self._class_new_policy(class_id, class_info)
                callargs = self._emit_call_args_builder(node)
                res_hint: TypeHint = (
                    "Any" if new_returns_any else DeclaredClass(class_id)
                )
                res = MoltValue(self.next_var(), type_hint=res_hint)
                metadata = (
                    {"defines_del": True}
                    if not new_returns_any and self._class_defines_finalizer(class_id)
                    else None
                )
                # Route user class construction through the class object so __new__,
                # metaclass __call__, and runtime constructor policy stay coherent.
                #
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[class_ref, callargs],
                        result=res,
                        metadata=metadata,
                    )
                )
                return res

            stateful_result = self._emit_stateful_function_value_call(
                target_info,
                func_id,
                node,
                needs_bind=needs_bind,
            )
            if stateful_result is not None:
                return stateful_result
            if target_info and isinstance(target_info.type_hint, BoundMethodHint):
                callee = self._named_callee_value(node)
                res_hint = "Any"
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                args = self._emit_call_args(node.args)
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(MoltOp(kind="CALL_METHOD", args=[callee] + args, result=res))
                return res

            if (
                target_info
                and isinstance(target_info.type_hint, str)
                and target_info.type_hint.startswith("Func:")
            ):
                target_name = target_info.type_hint.split(":")[1]
                intrinsic_target = _intrinsic_arity_exact(target_name) is not None
                res_hint = (
                    self._function_result_hint(target_name)
                    if intrinsic_target
                    else "Any"
                )
                # A registered intrinsic is called by its symbol; any other
                # call, and one that binds its arguments, reads the name.
                callee = (
                    target_info if intrinsic_target else self._named_callee_value(node)
                )
                direct_ok = intrinsic_target or target_name in self.func_default_specs
                if not direct_ok:
                    func_name = self.func_symbol_names.get(target_name)
                    if func_name and self._lookup_func_defaults(None, func_name):
                        direct_ok = True
                    elif self._known_function_symbol_target(target_name) is not None:
                        direct_ok = True
                if needs_bind or not direct_ok:
                    if intrinsic_target:
                        callee = self._named_callee_value(node)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                # The target hint may be stale. Pass only supplied arguments;
                # runtime callable admission and binding own defaults/shape.
                args = self._emit_call_args(node.args)
                res = MoltValue(self.next_var(), type_hint=res_hint)
                if intrinsic_target:
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="CALL_GUARDED",
                            args=[callee] + args,
                            result=res,
                            metadata={"target": target_name},
                        )
                    )
                return res

            if target_info is not None and func_id in self.locals:
                callee = self.visit(node.func)
                if callee is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_TARGET, "Unsupported call target"
                    )
                if needs_bind:
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                else:
                    if isinstance(
                        callee.type_hint, str
                    ) and callee.type_hint.startswith("Func:"):
                        func_symbol = callee.type_hint.split(":", 1)[1]
                        res_hint = "Any"
                        if func_symbol not in self.func_default_specs:
                            res = MoltValue(self.next_var(), type_hint=res_hint)
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        args = self._emit_call_args(node.args)
                        res = MoltValue(self.next_var(), type_hint=res_hint)
                        self.emit(
                            MoltOp(
                                kind="CALL_GUARDED",
                                args=[callee] + args,
                                result=res,
                                metadata={"target": func_symbol},
                            )
                        )
                        return res
                    if isinstance(
                        callee.type_hint, str
                    ) and callee.type_hint.startswith("ClosureFunc:"):
                        func_symbol = callee.type_hint.split(":", 1)[1]
                        res_hint = "Any"
                        if func_symbol not in self.func_default_specs:
                            res = MoltValue(self.next_var(), type_hint=res_hint)
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        args = self._emit_call_args(node.args)
                        res = MoltValue(self.next_var(), type_hint=res_hint)
                        self.emit(
                            MoltOp(
                                kind="CALL_GUARDED",
                                args=[callee] + args,
                                result=res,
                                metadata={"target": func_symbol},
                            )
                        )
                        return res
                    if imported_from:
                        imported_info = self._lookup_func_defaults(
                            imported_from, func_id
                        )
                        if imported_info is None or imported_info.get("kwonly"):
                            callargs = self._emit_call_args_builder(node)
                            res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                    args = self._emit_call_args(node.args)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            builtin_result = self._try_emit_named_builtin_call(
                node, builtin, needs_bind
            )
            if builtin_result is not CALL_NOT_HANDLED:
                return builtin_result
            lowered_intrinsic = self._try_lower_intrinsic_lookup_call(
                func_id=func_id,
                imported_from=imported_from,
                node=node,
            )
            if lowered_intrinsic is not None:
                return lowered_intrinsic
            if is_compiler_intrinsics_module(imported_from) and func_id in {
                "require_intrinsic",
                "_require_intrinsic",
                "load_intrinsic",
                "_load_intrinsic",
            }:
                callee = self.visit(node.func)
                if callee is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_TARGET, "Unsupported call target"
                    )
                res = MoltValue(self.next_var(), type_hint="Any")
                callargs = self._emit_call_args_builder(node)
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[callee, callargs],
                        result=res,
                    )
                )
                return res
            # Every other name is an ordinary Python binding, including one
            # imported from a module outside the compiled graph. The import
            # statement that bound it runs the import transaction, which
            # raises ImportError when it executes and its module is absent
            # from the binary, as CPython's IMPORT_NAME does. A body that
            # never runs therefore never fails.
            callee = self.visit(node.func)
            if callee is None:
                raise FrontendRejection(
                    Diagnostic.CALL_TARGET, "Unsupported call target"
                )
            return self._emit_dynamic_call(node, callee)
        return CALL_NOT_HANDLED

    def _record_constructor_fallback(
        self, node: ast.Call, class_id: ClassId, reason: str
    ) -> None:
        """Name why a class call keeps the runtime class protocol (AGENTS.md:
        a lost fact never falls to the generic runtime in silence)."""
        self.constructor_fallbacks.setdefault(self.current_func_name, []).append(
            f"{self.module_name}:{node.lineno} {class_id.qualname}(): "
            f"runtime class call: {reason}"
        )

    def _object_new_bound_metadata(
        self, node: ast.Call, class_id: ClassId, class_info: ClassView
    ) -> dict[str, object]:
        """The facts an ``OBJECT_NEW_BOUND`` of the class carries: its static
        payload extent (typed-field analysis; not a placement or lifetime
        proof), its identity, and its instance plan when the layout proves
        one (the allocation then compiles the plan word in). A guarded
        allocation names why in the constructor diagnostics."""
        metadata: dict[str, object] = {
            "class_size_bytes": class_info.layout.size,
            "class_id": class_id,
        }
        plan = release_plan(class_info)
        if isinstance(plan, ReleasePlan):
            metadata["release_plan"] = plan.field_words
        else:
            self.constructor_fallbacks.setdefault(self.current_func_name, []).append(
                f"{self.module_name}:{node.lineno} {class_id.qualname}(): "
                f"guarded allocation: {plan.reason}"
            )
        return metadata

    def _emit_inline_construction(
        self,
        node: ast.Call,
        class_id: ClassId,
        class_info: ClassView,
        class_ref: MoltValue,
    ) -> MoltValue | str:
        """Allocate and initialize a proven class inline, or say why not.

        CPython 3.13 ``CALL_ALLOC_AND_ENTER_INIT`` (call_anatomy.md 4.5,
        "Constructors"). The class proof (``constructor_refusal`` is None)
        already holds: no writer can rebind the class or a member of its MRO,
        the metaclass is ``type`` and ``__new__`` is ``object.__new__``. Here
        the call and ``__init__`` must match exactly: positional arguments
        only, one per parameter after ``self``, and an ``__init__`` that is a
        plain synchronous function whose every return yields None. Then
        ``C(args)`` is ``OBJECT_NEW_BOUND`` of the class's fixed layout and a
        direct call of the compiled ``__init__`` entry with ``self`` as
        argument zero: no argument tuple, no bound method, no runtime class
        call. An exception in ``__init__`` propagates through the call's
        exception edge, which releases the allocation. Any mismatch keeps the
        runtime class call, which owns CPython's arity and return-value
        errors.
        """
        if node.keywords or any(isinstance(a, ast.Starred) for a in node.args):
            return "the call passes keywords or unpacked arguments"
        if self._class_layout_definition(class_id) is None:
            return "no class statement stamps the class identity"
        init_lookup = resolve(self._class_mro(class_id), "__init__", self.classes.get)
        if (
            isinstance(init_lookup, Unresolved)
            and init_lookup.reason is not ResolveRefusal.NOT_FOUND
        ):
            return f"__init__ cannot be resolved: {init_lookup.reason.value}"
        init_member = init_lookup.member if isinstance(init_lookup, Resolved) else None
        init_owner = init_lookup.owner
        if init_member is None:
            if node.args:
                # object.__init__ raises for extra arguments.
                return "object.__init__ receives arguments"
            res = MoltValue(self.next_var(), type_hint=DeclaredClass(class_id))
            self.emit(
                MoltOp(
                    kind="OBJECT_NEW_BOUND",
                    args=[class_ref],
                    result=res,
                    metadata=self._object_new_bound_metadata(
                        node, class_id, class_info
                    ),
                )
            )
            self._stamp_exact_class(res, class_id)
            return res
        if init_member.kind is not MemberKind.FUNCTION:
            return "__init__ is not a plain function"
        if init_member.closure is not Closure.NONE:
            return "__init__ is a closure (zero-argument super() or a free variable)"
        shape = init_member.shape
        if shape is None or shape.vararg or shape.varkw or shape.kwonly:
            return "__init__ takes *args, **kwargs or keyword-only parameters"
        if not init_member.returns_none:
            return "__init__ may return a value other than None"
        getattribute_member, _ = self._resolve_member(class_id, "__getattribute__")
        if getattribute_member is not None:
            return "the class defines __getattribute__"
        if (init_owner or class_id) not in self.classes:
            return "__init__ is inherited from a class outside this module"
        init_symbol = init_member.symbol
        if init_symbol is None or init_symbol not in self.func_symbol_names:
            return "__init__ has no compiled entry"
        expected_positional = shape.params - 1
        if len(node.args) != expected_positional:
            return (
                f"the call passes {len(node.args)} positional arguments and "
                f"__init__ takes {expected_positional} (defaults are not bound "
                "inline)"
            )
        init_args = self._emit_call_args(list(node.args))
        res = MoltValue(self.next_var(), type_hint=DeclaredClass(class_id))
        self.emit(
            MoltOp(
                kind="OBJECT_NEW_BOUND",
                args=[class_ref],
                result=res,
                metadata=self._object_new_bound_metadata(node, class_id, class_info),
            )
        )
        self._stamp_exact_class(res, class_id)
        allocation_token = self.exact_class_token
        inline = init_member.inline
        init_assigns = (
            list(inline.init_assigns)
            if inline is not None and inline.init_assigns is not None
            else None
        )
        inline_params = list(inline.params) if inline is not None else None
        if (
            init_assigns is not None
            and inline_params is not None
            and self.exact_class_token == allocation_token
            and self._try_inline_init_assigns(
                class_id, init_assigns, inline_params, res, init_args
            )
        ):
            # A field-assignment body runs no Python code: the exact class
            # fact survives, so later stores are unguarded.
            self._stamp_exact_class(res, class_id)
            return res
        init_res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(
                kind="CALL",
                args=[init_symbol, res] + init_args,
                result=init_res,
            )
        )
        # ``__init__`` returns None (``returns_none``), so its result needs no
        # check. It is a callback boundary: the value stays exactly ``C`` only
        # when no code in the closed graph can assign ``__class__`` on an
        # instance (``instance_class_writer``); otherwise it is declared ``C``
        # and its field accesses keep their layout guard.
        writer = self._instance_class_writer()
        if writer is None:
            return res
        self.constructor_fallbacks.setdefault(self.current_func_name, []).append(
            f"{self.module_name}:{node.lineno} {class_id.qualname}(): "
            f"result declared, not exact: __class__ may be assigned at {writer}"
        )
        return MoltValue(res.name, type_hint=DeclaredClass(class_id))

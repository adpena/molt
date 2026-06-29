"""CallNamedDispatchMixin: extracted visit_Call dispatch phase."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    BUILTIN_FUNC_SPECS,
    BUILTIN_TYPE_TAGS,
    MOLT_DIRECT_CALLS,
    MOLT_DIRECT_CALL_BIND_ALWAYS,
    MOLT_REEXPORT_FUNCTIONS,
    MoltOp,
    MoltValue,
    _intrinsic_arity_exact,
)
from molt.frontend.sema import (
    FunctionKind,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED

_BUILTINS_IMPORT_ALIAS_CALL_NAMES = frozenset(BUILTIN_FUNC_SPECS) | frozenset(
    {
        "BaseExceptionGroup",
        "ExceptionGroup",
        "abs",
        "aiter",
        "all",
        "anext",
        "any",
        "bool",
        "bytearray",
        "bytes",
        "callable",
        "chr",
        "classmethod",
        "complex",
        "delattr",
        "dict",
        "dir",
        "enumerate",
        "filter",
        "float",
        "frozenset",
        "getattr",
        "globals",
        "hasattr",
        "id",
        "int",
        "isinstance",
        "issubclass",
        "iter",
        "len",
        "list",
        "locals",
        "map",
        "max",
        "memoryview",
        "min",
        "next",
        "object",
        "open",
        "ord",
        "pow",
        "print",
        "property",
        "range",
        "repr",
        "reversed",
        "round",
        "set",
        "setattr",
        "slice",
        "sorted",
        "staticmethod",
        "str",
        "sum",
        "super",
        "tuple",
        "type",
        "vars",
        "zip",
    }
)


class CallNamedDispatchMixin(_MixinBase):
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
        if imported_from == "builtins" or self._is_intrinsics_module_name(
            imported_from
        ):
            return CALL_NOT_HANDLED

        normalized = self._normalize_allowlist_module(imported_from)
        visible_module = normalized or imported_from
        original_attr = self._imported_attr_name(func_id)
        target_module: str | None = None
        direct_registry_authorized = False

        if imported_from == "molt":
            if original_attr in MOLT_DIRECT_CALLS.get("molt", set()):
                target_module = MOLT_REEXPORT_FUNCTIONS.get(original_attr)
                direct_registry_authorized = target_module is not None
        elif (
            normalized in MOLT_DIRECT_CALLS
            and original_attr in MOLT_DIRECT_CALLS[normalized]
        ):
            target_module = normalized
            direct_registry_authorized = True
        elif (
            imported_from in MOLT_DIRECT_CALLS
            and original_attr in MOLT_DIRECT_CALLS[imported_from]
        ):
            target_module = imported_from
            direct_registry_authorized = True

        visible_import_authorized = (
            imported_from in self.stdlib_allowlist
            or (normalized is not None and normalized in self.stdlib_allowlist)
            or self._is_internal_module(imported_from)
            or self._is_known_project_module(imported_from)
        )
        if target_module is None and visible_import_authorized:
            target_module = visible_module
        if target_module is None:
            return CALL_NOT_HANDLED

        force_bind = original_attr[
            :1
        ].isupper() or original_attr in MOLT_DIRECT_CALL_BIND_ALWAYS.get(
            target_module, set()
        )
        lowered_imported_call = self._try_emit_imported_module_direct_or_task_call(
            target_module,
            original_attr,
            node,
            imported_from=imported_from,
            normalized=normalized,
            needs_bind=needs_bind,
            force_bind=force_bind,
            direct_registry_authorized=direct_registry_authorized,
        )
        if lowered_imported_call is not None:
            return lowered_imported_call
        if visible_import_authorized:
            callee = self.visit(node.func)
            if callee is None:
                raise NotImplementedError("Unsupported call target")
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
        return CALL_NOT_HANDLED

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
            intrinsic_global_symbol = self.module_intrinsic_globals.get(func_id)
            target_info = self.locals.get(func_id)
            if target_info is None and intrinsic_global_symbol is not None:
                target_info = MoltValue(
                    func_id, type_hint=f"Func:{intrinsic_global_symbol}"
                )
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
                imported_attr = self._imported_attr_name(func_id)
                if (
                    imported_attr is not None
                    and imported_attr in _BUILTINS_IMPORT_ALIAS_CALL_NAMES
                ):
                    func_id = imported_attr
            if imported_from:
                normalized = self._normalize_allowlist_module(imported_from)
                allowlist_key = normalized or imported_from
                if func_id == "field" and allowlist_key == "dataclasses":
                    return self._emit_dataclasses_field_call(allowlist_key, node)
                if allowlist_key == "statistics" and func_id in {"mean", "stdev"}:
                    lowered_stats = self._lower_statistics_slice_call(func_id, node)
                    if lowered_stats is not None:
                        return lowered_stats
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
            if self._is_intrinsics_module_name(imported_binding) and func_id in {
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
            if (
                target_info is None
                and self.current_func_name != "molt_main"
                and self.module_declared_funcs.get(func_id) == FunctionKind.SYNC
            ):
                func_symbol = self._function_symbol_for_reference(func_id)
                target_info = MoltValue(func_id, type_hint=f"Func:{func_symbol}")
            lowered_wrapper_intrinsic = self._try_lower_local_intrinsic_wrapper_call(
                func_id=func_id,
                node=node,
            )
            if lowered_wrapper_intrinsic is not None:
                return lowered_wrapper_intrinsic
            if imported_from:
                target_module = self._normalize_allowlist_module(imported_from)
                if target_module is None:
                    target_module = imported_from
                original_attr = self._imported_attr_name(func_id)
                lowered_handle_ctor = self._try_emit_intrinsic_handle_class_constructor(
                    target_module,
                    original_attr,
                    node,
                )
                if lowered_handle_ctor is not None:
                    return lowered_handle_ctor
                lowered_imported_call = self._try_emit_imported_named_call(
                    node,
                    func_id=func_id,
                    imported_from=imported_from,
                    needs_bind=needs_bind,
                )
                if lowered_imported_call is not CALL_NOT_HANDLED:
                    return lowered_imported_call
            if func_id in {"BaseExceptionGroup", "ExceptionGroup"}:
                if node.keywords:
                    self._bridge_fallback(
                        node,
                        f"{func_id} with keywords",
                        impact="medium",
                        alternative=f"{func_id} with positional arguments only",
                        detail="keywords are not supported for exception constructors",
                    )
                    return None
                args: list[MoltValue] = []
                for arg in node.args:
                    arg_val = self.visit(arg)
                    if arg_val is None:
                        self._bridge_fallback(
                            node,
                            f"{func_id} with unsupported arg expression",
                            impact="medium",
                            alternative=f"{func_id} with simple literals",
                            detail="argument expression could not be lowered",
                        )
                        return None
                    args.append(arg_val)
                class_val = self._emit_exception_class(func_id)
                return self._emit_exception_new_from_class(class_val, args)
            if func_id in {
                "BaseException",
                "Exception",
                "KeyError",
                "IndexError",
                "ValueError",
                "TypeError",
                "RuntimeError",
                "StopIteration",
            }:
                if node.keywords or any(isinstance(a, ast.Starred) for a in node.args):
                    pass  # fall through to generic call handler
                else:
                    args: list[MoltValue] = []
                    for arg in node.args:
                        arg_val = self.visit(arg)
                        if arg_val is None:
                            self._bridge_fallback(
                                node,
                                f"{func_id} with unsupported arg expression",
                                impact="medium",
                                alternative=f"{func_id} with simple literals",
                                detail="argument expression could not be lowered",
                            )
                            return None
                        args.append(arg_val)
                    return self._emit_exception_new_from_args(func_id, args)
            if func_id == "abs" and len(node.args) == 1 and not node.keywords:
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("abs expects a lowerable operand")
                if value.type_hint in {"bool", "int"}:
                    result_hint = "int"
                elif value.type_hint == "float":
                    result_hint = "float"
                else:
                    result_hint = "Any"
                res = MoltValue(self.next_var(), type_hint=result_hint)
                self.emit(MoltOp(kind="ABS", args=[value], result=res))
                return res
            if func_id == "globals":
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"globals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_globals_dict()
            if func_id == "locals":
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"locals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_locals_dict()
            if func_id == "vars":
                if node.keywords:
                    return self._emit_type_error_value(
                        "vars() takes no keyword arguments", "dict"
                    )
                if len(node.args) > 1:
                    msg = f"vars() takes at most 1 argument ({len(node.args)} given)"
                    return self._emit_type_error_value(msg, "dict")
                if not node.args:
                    return self._emit_locals_dict()
                obj = self.visit(node.args[0])
                if obj is None:
                    raise NotImplementedError("vars expects a simple object")
                callee = self._emit_builtin_function("vars")
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee, obj], result=res))
                return res
            if func_id == "dir":
                if node.keywords:
                    return self._emit_type_error_value(
                        "dir() takes no keyword arguments", "list"
                    )
                if len(node.args) > 1:
                    msg = f"dir() takes at most 1 argument ({len(node.args)} given)"
                    return self._emit_type_error_value(msg, "list")
                if not node.args:
                    locals_dict = self._emit_locals_dict()
                    keys = MoltValue(self.next_var(), type_hint="dict_keys")
                    self.emit(MoltOp(kind="DICT_KEYS", args=[locals_dict], result=keys))
                    callee = self._emit_builtin_function("sorted")
                    key_none = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_none))
                    reverse_false = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=reverse_false)
                    )
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[callee, keys, key_none, reverse_false],
                            result=res,
                        )
                    )
                    return res
                obj = self.visit(node.args[0])
                if obj is None:
                    raise NotImplementedError("dir expects a simple object")
                callee = self._emit_builtin_function("dir")
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee, obj], result=res))
                return res
            if func_id == "getattr":
                if len(node.args) not in {2, 3} or node.keywords:
                    raise NotImplementedError("getattr expects 2 or 3 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("getattr expects object and name")
                res_hint = "Any"
                name_lit = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    name_lit = node.args[1].value
                if name_lit and obj.type_hint in self.classes:
                    class_info = self.classes[obj.type_hint]
                    if not class_info.get("dynamic"):
                        field_map = class_info.get("fields", {})
                        if name_lit in field_map:
                            if class_info.get("dataclass"):
                                idx_val = MoltValue(self.next_var(), type_hint="int")
                                self.emit(
                                    MoltOp(
                                        kind="CONST",
                                        args=[field_map[name_lit]],
                                        result=idx_val,
                                    )
                                )
                                res = MoltValue(self.next_var())
                                self.emit(
                                    MoltOp(
                                        kind="DATACLASS_GET",
                                        args=[obj, idx_val],
                                        result=res,
                                    )
                                )
                                return res
                            else:
                                obj_name = None
                                assume_exact = False
                                if isinstance(node.args[0], ast.Name):
                                    obj_name = node.args[0].id
                                    assume_exact = (
                                        self.exact_locals.get(obj_name) == obj.type_hint
                                    )
                                return self._emit_guarded_getattr(
                                    obj,
                                    name_lit,
                                    obj.type_hint,
                                    assume_exact=assume_exact,
                                    obj_name=obj_name,
                                )
                if name_lit:
                    class_name = None
                    if obj.type_hint in self.classes:
                        class_name = obj.type_hint
                    elif isinstance(node.args[0], ast.Name):
                        if node.args[0].id in self.classes:
                            class_name = node.args[0].id
                    if class_name:
                        method_info, method_class = self._resolve_method_info(
                            class_name, name_lit
                        )
                        if method_info:
                            descriptor = method_info["descriptor"]
                            if descriptor in {"function", "classmethod"}:
                                method_owner = method_class or class_name
                                res_hint = f"BoundMethod:{method_owner}:{name_lit}"
                            elif descriptor == "staticmethod":
                                res_hint = method_info["func"].type_hint
                res = MoltValue(self.next_var(), type_hint=res_hint)
                if len(node.args) == 3:
                    default = self.visit(node.args[2])
                    if default is None:
                        default = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                    self.emit(
                        MoltOp(
                            kind="GETATTR_NAME_DEFAULT",
                            args=[obj, name, default],
                            result=res,
                        )
                    )
                else:
                    default = self._emit_missing_value()
                    self.emit(
                        MoltOp(
                            kind="GETATTR_NAME_DEFAULT",
                            args=[obj, name, default],
                            result=res,
                        )
                    )
                return res
            if func_id == "setattr":
                if len(node.args) != 3 or node.keywords:
                    raise NotImplementedError("setattr expects 3 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                val = self.visit(node.args[2])
                if obj is None or name is None or val is None:
                    raise NotImplementedError("setattr expects object, name, value")
                attr_name = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    attr_name = node.args[1].value
                if attr_name:
                    obj_name = None
                    exact_class = None
                    if isinstance(node.args[0], ast.Name):
                        obj_name = node.args[0].id
                        exact_class = self.exact_locals.get(obj_name)
                    if exact_class is not None:
                        self._record_instance_attr_mutation(exact_class, attr_name)
                    elif obj.type_hint in self.classes:
                        self._record_instance_attr_mutation(obj.type_hint, attr_name)
                if (
                    isinstance(node.args[1], ast.Constant)
                    and isinstance(node.args[1].value, str)
                    and obj.type_hint in self.classes
                ):
                    attr_name = node.args[1].value
                    class_info = self.classes[obj.type_hint]
                    if not class_info.get("dynamic"):
                        field_map = class_info.get("fields", {})
                        if attr_name in field_map:
                            if class_info.get("dataclass"):
                                idx_val = MoltValue(self.next_var(), type_hint="int")
                                self.emit(
                                    MoltOp(
                                        kind="CONST",
                                        args=[field_map[attr_name]],
                                        result=idx_val,
                                    )
                                )
                                self.emit(
                                    MoltOp(
                                        kind="DATACLASS_SET",
                                        args=[obj, idx_val, val],
                                        result=MoltValue("none"),
                                    )
                                )
                                res = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=res)
                                )
                            else:
                                res = MoltValue(self.next_var(), type_hint="None")
                                if self._class_attr_is_data_descriptor(
                                    obj.type_hint, attr_name
                                ):
                                    self.emit(
                                        MoltOp(
                                            kind="SETATTR_GENERIC_PTR",
                                            args=[obj, attr_name, val],
                                            result=res,
                                        )
                                    )
                                else:
                                    assume_exact = (
                                        exact_class is not None
                                        and exact_class == obj.type_hint
                                    )
                                    self._emit_guarded_setattr(
                                        obj,
                                        attr_name,
                                        val,
                                        obj.type_hint,
                                        obj_name=obj_name,
                                        assume_exact=assume_exact,
                                    )
                                    self.emit(
                                        MoltOp(
                                            kind="CONST_NONE",
                                            args=[],
                                            result=res,
                                        )
                                    )
                            return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="SETATTR_NAME",
                        args=[obj, name, val],
                        result=res,
                    )
                )
                return res
            if func_id == "delattr":
                if len(node.args) != 2 or node.keywords:
                    raise NotImplementedError("delattr expects 2 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("delattr expects object and name")
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    attr_name = node.args[1].value
                    exact_class = None
                    if isinstance(node.args[0], ast.Name):
                        exact_class = self.exact_locals.get(node.args[0].id)
                    if exact_class is not None:
                        self._record_instance_attr_mutation(exact_class, attr_name)
                    elif obj.type_hint in self.classes:
                        self._record_instance_attr_mutation(obj.type_hint, attr_name)
                    res = MoltValue(self.next_var(), type_hint="None")
                    attr_name = node.args[1].value
                    if obj.type_hint in self.classes:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_PTR",
                                args=[obj, attr_name],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_OBJ",
                                args=[obj, attr_name],
                                result=res,
                            )
                        )
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="DELATTR_NAME",
                        args=[obj, name],
                        result=res,
                    )
                )
                return res
            if func_id == "hasattr":
                if len(node.args) != 2 or node.keywords:
                    raise NotImplementedError("hasattr expects 2 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("hasattr expects object and name")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="HASATTR_NAME",
                        args=[obj, name],
                        result=res,
                    )
                )
                return res
            if func_id == "super":
                if node.keywords:
                    raise NotImplementedError("super does not support keywords")
                if len(node.args) == 0:
                    # Zero-arg ``super()`` reads the class object from the
                    # implicit ``__class__`` closure cell (filled with the
                    # finished class after the class is built) and binds it to
                    # the method's first parameter — exactly mirroring CPython's
                    # ``__build_class__`` / ``super.__init__`` zero-arg path.
                    # Reading the cell rather than re-deriving the class by
                    # module-attribute name makes ``super()`` correct for
                    # function-local, nested, and module-level classes
                    # (including metaclasses) alike.
                    class_ref = (
                        self._emit_free_var_load("__class__")
                        if "__class__" in self.free_vars
                        else None
                    )
                    if (
                        class_ref is not None
                        and self.current_method_first_param is not None
                    ):
                        obj = self._load_local_value(self.current_method_first_param)
                        if (
                            obj is None
                            and self.current_method_first_param in self.free_vars
                        ):
                            obj = self._emit_free_var_load(
                                self.current_method_first_param
                            )
                        if obj is None:
                            raise NotImplementedError("super() missing method receiver")
                        super_hint = (
                            f"super:{self.current_class}"
                            if self.current_class is not None
                            else "super"
                        )
                        res = MoltValue(self.next_var(), type_hint=super_hint)
                        self.emit(
                            MoltOp(kind="SUPER_NEW", args=[class_ref, obj], result=res)
                        )
                        return res
                    if self.current_method_first_param is None:
                        msg = "super(): no arguments"
                    else:
                        msg = "super(): __class__ cell not found"
                    err_val = self._emit_exception_new("RuntimeError", msg)
                    self.emit(
                        MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none"))
                    )
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                if len(node.args) == 2:
                    type_val = self.visit(node.args[0])
                    obj_val = self.visit(node.args[1])
                    if type_val is None or obj_val is None:
                        raise NotImplementedError("super expects type and object")
                    super_hint = "super"
                    if isinstance(node.args[0], ast.Name):
                        super_hint = f"super:{node.args[0].id}"
                    res = MoltValue(self.next_var(), type_hint=super_hint)
                    self.emit(
                        MoltOp(kind="SUPER_NEW", args=[type_val, obj_val], result=res)
                    )
                    return res
                raise NotImplementedError("super expects 0 or 2 arguments")
            if func_id == "classmethod":
                if len(node.args) != 1 or node.keywords:
                    raise NotImplementedError("classmethod expects 1 argument")
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise NotImplementedError("classmethod expects a function")
                res = MoltValue(self.next_var(), type_hint="classmethod")
                self.emit(MoltOp(kind="CLASSMETHOD_NEW", args=[func_val], result=res))
                return res
            if func_id == "staticmethod":
                if len(node.args) != 1 or node.keywords:
                    raise NotImplementedError("staticmethod expects 1 argument")
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise NotImplementedError("staticmethod expects a function")
                res = MoltValue(self.next_var(), type_hint="staticmethod")
                self.emit(MoltOp(kind="STATICMETHOD_NEW", args=[func_val], result=res))
                return res
            if func_id == "property":
                if any(kw.arg is None for kw in node.keywords):
                    raise NotImplementedError("property does not support **kwargs")
                if len(node.args) > 4:
                    return self._emit_type_error_value(
                        "property expected at most 4 arguments", "property"
                    )
                getter_expr = node.args[0] if len(node.args) > 0 else None
                setter_expr = node.args[1] if len(node.args) > 1 else None
                deleter_expr = node.args[2] if len(node.args) > 2 else None
                doc_expr = node.args[3] if len(node.args) > 3 else None
                for kw in node.keywords:
                    if kw.arg == "fget":
                        if getter_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'fget'",
                                "property",
                            )
                        getter_expr = kw.value
                    elif kw.arg == "fset":
                        if setter_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'fset'",
                                "property",
                            )
                        setter_expr = kw.value
                    elif kw.arg == "fdel":
                        if deleter_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'fdel'",
                                "property",
                            )
                        deleter_expr = kw.value
                    elif kw.arg == "doc":
                        if doc_expr is not None:
                            return self._emit_type_error_value(
                                "property() got multiple values for argument 'doc'",
                                "property",
                            )
                        doc_expr = kw.value
                    else:
                        return self._emit_type_error_value(
                            f"property() got an unexpected keyword argument '{kw.arg}'",
                            "property",
                        )
                if getter_expr is None:
                    getter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=getter))
                else:
                    getter = self.visit(getter_expr)
                    if getter is None:
                        raise NotImplementedError("property expects a getter")
                if setter_expr is None:
                    setter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=setter))
                else:
                    setter = self.visit(setter_expr)
                    if setter is None:
                        raise NotImplementedError("property setter unsupported")
                if deleter_expr is None:
                    deleter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=deleter))
                else:
                    deleter = self.visit(deleter_expr)
                    if deleter is None:
                        raise NotImplementedError("property deleter unsupported")
                res = MoltValue(self.next_var(), type_hint="property")
                self.emit(
                    MoltOp(
                        kind="PROPERTY_NEW",
                        args=[getter, setter, deleter],
                        result=res,
                    )
                )
                if doc_expr is not None:
                    doc_val = self.visit(doc_expr)
                    if doc_val is None:
                        raise NotImplementedError("property doc unsupported")
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_OBJ",
                            args=[res, "__doc__", doc_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            if func_id == "open":
                return self._emit_open_call(node)
            if func_id == "nullcontext":
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if func_id == "closing":
                if len(node.args) != 1:
                    raise NotImplementedError("closing expects 1 argument")
                payload = self.visit(node.args[0])
                return self._emit_closing(payload)
            if func_id == "print":
                needs_bind = self._call_needs_bind(node)
                if needs_bind:
                    callargs, saw_name_error = self._emit_print_call_args_builder(node)
                    if saw_name_error:
                        return None
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                if len(node.args) == 0:
                    self.emit(
                        MoltOp(kind="PRINT_NEWLINE", args=[], result=MoltValue("none"))
                    )
                    return None
                args: list[MoltValue] = []
                saw_name_error = False
                for expr in node.args:
                    arg = self.visit(expr)
                    if arg is None:
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
                            arg = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    args.append(arg)
                if saw_name_error:
                    return None
                if len(args) == 1:
                    self.emit(
                        MoltOp(kind="PRINT", args=[args[0]], result=MoltValue("none"))
                    )
                    return None
                parts = [self._emit_str_from_obj(arg) for arg in args]
                sep = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[" "], result=sep))
                items = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=parts, result=items))
                joined = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_JOIN", args=[sep, items], result=joined))
                self.emit(MoltOp(kind="PRINT", args=[joined], result=MoltValue("none")))
                return None
            elif func_id == "molt_spawn":
                arg = self.visit(node.args[0])
                self.emit(MoltOp(kind="SPAWN", args=[arg], result=MoltValue("none")))
                return None
            elif func_id == "molt_cancel_token_new":
                if node.keywords or len(node.args) > 1:
                    raise NotImplementedError(
                        "molt_cancel_token_new expects 0 or 1 argument"
                    )
                if node.args:
                    parent = self.visit(node.args[0])
                    if parent is None:
                        raise NotImplementedError(
                            "Unsupported parent in molt_cancel_token_new"
                        )
                else:
                    parent = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=parent))
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CANCEL_TOKEN_NEW", args=[parent], result=res))
                return res
            elif func_id == "molt_cancel_token_clone":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_clone expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_CLONE", args=[token], result=res))
                return res
            elif func_id == "molt_cancel_token_drop":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_drop expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_DROP", args=[token], result=res))
                return res
            elif func_id == "molt_cancel_token_cancel":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_cancel expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_CANCEL", args=[token], result=res))
                return res
            elif func_id == "molt_future_cancel":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_future_cancel expects 1 argument")
                future = self.visit(node.args[0])
                if future is None:
                    raise NotImplementedError("Unsupported future")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FUTURE_CANCEL", args=[future], result=res))
                return res
            elif func_id == "molt_future_cancel_msg":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_future_cancel_msg expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                msg = self.visit(node.args[1])
                if future is None or msg is None:
                    raise NotImplementedError("Unsupported future cancel message")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="FUTURE_CANCEL_MSG", args=[future, msg], result=res)
                )
                return res
            elif func_id == "molt_future_cancel_clear":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_future_cancel_clear expects 1 argument"
                    )
                future = self.visit(node.args[0])
                if future is None:
                    raise NotImplementedError("Unsupported future cancel clear")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FUTURE_CANCEL_CLEAR", args=[future], result=res))
                return res
            elif func_id == "molt_promise_new":
                if node.keywords or node.args:
                    raise NotImplementedError("molt_promise_new expects no arguments")
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(MoltOp(kind="PROMISE_NEW", args=[], result=res))
                return res
            elif func_id == "molt_promise_set_result":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_promise_set_result expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                result = self.visit(node.args[1])
                if future is None or result is None:
                    raise NotImplementedError("Unsupported promise set result")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="PROMISE_SET_RESULT", args=[future, result], result=res)
                )
                return res
            elif func_id == "molt_promise_set_exception":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_promise_set_exception expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                exc = self.visit(node.args[1])
                if future is None or exc is None:
                    raise NotImplementedError("Unsupported promise set exception")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="PROMISE_SET_EXCEPTION", args=[future, exc], result=res)
                )
                return res
            elif func_id == "molt_task_register_token_owned":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_task_register_token_owned expects 2 arguments"
                    )
                task = self.visit(node.args[0])
                token = self.visit(node.args[1])
                if task is None or token is None:
                    raise NotImplementedError("Unsupported task token registration")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="TASK_REGISTER_TOKEN_OWNED",
                        args=[task, token],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_cancel_token_is_cancelled":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_is_cancelled expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="CANCEL_TOKEN_IS_CANCELLED", args=[token], result=res)
                )
                return res
            elif func_id == "molt_cancel_token_set_current":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_set_current expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(kind="CANCEL_TOKEN_SET_CURRENT", args=[token], result=res)
                )
                return res
            elif func_id == "molt_cancel_token_get_current":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_cancel_token_get_current expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CANCEL_TOKEN_GET_CURRENT", args=[], result=res))
                return res
            elif func_id == "molt_cancelled":
                if node.keywords or node.args:
                    raise NotImplementedError("molt_cancelled expects no arguments")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CANCELLED", args=[], result=res))
                return res
            elif func_id == "molt_cancel_current":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_cancel_current expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_CURRENT", args=[], result=res))
                return res
            elif func_id == "molt_block_on":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_block_on expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[arg], result=res))
                self._emit_raise_if_pending()
                return res
            elif func_id == "molt_asyncgen_shutdown":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_asyncgen_shutdown expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="ASYNCGEN_SHUTDOWN", args=[], result=res))
                return res
            elif func_id == "molt_async_sleep":
                if node.keywords or len(node.args) > 2:
                    raise NotImplementedError("molt_async_sleep expects 0-2 arguments")
                args = []
                if node.args:
                    delay_val = self.visit(node.args[0])
                    if delay_val is None:
                        raise NotImplementedError(
                            "Unsupported delay in molt_async_sleep"
                        )
                    args.append(delay_val)
                if len(node.args) == 2:
                    result_val = self.visit(node.args[1])
                    if result_val is None:
                        raise NotImplementedError(
                            "Unsupported result in molt_async_sleep"
                        )
                    args.append(result_val)
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="CALL_ASYNC",
                        args=["molt_async_sleep_poll", *args],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_thread_submit":
                if node.keywords or len(node.args) != 3:
                    raise NotImplementedError("molt_thread_submit expects 3 arguments")
                callable_val = self.visit(node.args[0])
                args_val = self.visit(node.args[1])
                kwargs_val = self.visit(node.args[2])
                if callable_val is None or args_val is None or kwargs_val is None:
                    raise NotImplementedError("Unsupported thread submit arguments")
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="THREAD_SUBMIT",
                        args=[callable_val, args_val, kwargs_val],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_chan_new":
                if node.keywords:
                    raise NotImplementedError("molt_chan_new does not support keywords")
                if len(node.args) > 1:
                    raise NotImplementedError("molt_chan_new expects 0 or 1 argument")
                if node.args:
                    capacity = self.visit(node.args[0])
                    if capacity is None:
                        raise NotImplementedError("Unsupported channel capacity")
                else:
                    capacity = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=capacity))
                res = MoltValue(self.next_var(), type_hint="Channel")
                self.emit(MoltOp(kind="CHAN_NEW", args=[capacity], result=res))
                return res
            elif func_id == "molt_chan_send":
                chan = self.visit(node.args[0])
                val = self.visit(node.args[1])
                if not self.is_async():
                    callee = self._emit_builtin_function("molt_chan_send")
                    return self._emit_call_bound_or_func(callee, [chan, val])
                chan_slot = None
                val_slot = None
                chan_for_send = chan
                val_for_send = val
                if self.is_async():
                    chan_slot = self._async_local_offset(
                        f"__chan_send_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, chan],
                            result=MoltValue("none"),
                        )
                    )
                    val_slot = self._async_local_offset(
                        f"__chan_send_val_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", val_slot, val],
                            result=MoltValue("none"),
                        )
                    )
                self.state_count += 1
                pending_state_id = self.state_count
                self.emit(
                    MoltOp(
                        kind="STATE_LABEL",
                        args=[pending_state_id],
                        result=MoltValue("none"),
                    )
                )
                pending_state_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST", args=[pending_state_id], result=pending_state_val
                    )
                )
                if self.is_async() and chan_slot is not None and val_slot is not None:
                    chan_for_send = MoltValue(self.next_var(), type_hint="Channel")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", chan_slot],
                            result=chan_for_send,
                        )
                    )
                    val_for_send = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", val_slot],
                            result=val_for_send,
                        )
                    )
                self.state_count += 1
                next_state_id = self.state_count
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_SEND_YIELD",
                        args=[
                            chan_for_send,
                            val_for_send,
                            pending_state_val,
                            next_state_id,
                        ],
                        result=res,
                    )
                )
                if self.is_async() and chan_slot is not None and val_slot is not None:
                    cleared_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", val_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            elif func_id == "molt_chan_recv":
                chan = self.visit(node.args[0])
                if not self.is_async():
                    callee = self._emit_builtin_function("molt_chan_recv")
                    return self._emit_call_bound_or_func(callee, [chan])
                chan_slot = None
                chan_for_recv = chan
                if self.is_async():
                    chan_slot = self._async_local_offset(
                        f"__chan_recv_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, chan],
                            result=MoltValue("none"),
                        )
                    )
                self.state_count += 1
                pending_state_id = self.state_count
                self.emit(
                    MoltOp(
                        kind="STATE_LABEL",
                        args=[pending_state_id],
                        result=MoltValue("none"),
                    )
                )
                pending_state_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST", args=[pending_state_id], result=pending_state_val
                    )
                )
                if self.is_async() and chan_slot is not None:
                    chan_for_recv = MoltValue(self.next_var(), type_hint="Channel")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", chan_slot],
                            result=chan_for_recv,
                        )
                    )
                self.state_count += 1
                next_state_id = self.state_count
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_RECV_YIELD",
                        args=[chan_for_recv, pending_state_val, next_state_id],
                        result=res,
                    )
                )
                if self.is_async() and chan_slot is not None:
                    cleared_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            elif func_id == "molt_chan_drop":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_chan_drop expects 1 argument")
                chan = self.visit(node.args[0])
                if chan is None:
                    raise NotImplementedError("Unsupported channel handle")
                self.emit(
                    MoltOp(kind="CHAN_DROP", args=[chan], result=MoltValue("none"))
                )
                return None
            original_import_attr = self._imported_attr_name(func_id)
            class_id = None
            if func_id in self.classes:
                class_id = func_id
            elif imported_from and original_import_attr in self.classes:
                class_id = original_import_attr
            if class_id is not None and imported_from:
                class_ref = self._emit_module_attr_get_on(imported_from, class_id)
                # Imported class metadata is keyed by the class' export name, but
                # multiple modules can legally export the same simple class name.
                # Dispatch through the imported class object so runtime identity,
                # not frontend metadata keying, owns constructor semantics.
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
                class_value_name = self.classes[class_id].get("class_value_name")
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
                and class_id in self.del_targets
                and not self._name_resolves_to_builtin(class_id)
            ):
                # A module-scope class whose name is `del`'d (or shadowed by an
                # `except ... as` target) may be unbound when called; a bare Name
                # read has LOAD_GLOBAL semantics, so resolve through
                # MODULE_GET_GLOBAL (NameError on a missing binding) rather than
                # the static class ref / MODULE_GET_ATTR (AttributeError) the
                # known-class fast path would otherwise take.  Dropping `class_id`
                # routes the callee through the generic Name resolution, which
                # applies the same del-target rule.
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
                    # chunk-safe MODULE_GET_ATTR re-fetch.  Trusting
                    # `class_value_name` directly here would materialise a
                    # dangling cross-chunk SSA ref that lowering degrades to a
                    # CONST_STR of the variable name, feeding a string where a
                    # type is expected (task #50).  The `constructor_fold_safe`
                    # gate is preserved: the fast alloc + inlined `__init__`
                    # fold still fires for the in-chunk case, because a live
                    # `constructor_fold_safe` class always satisfies the
                    # resolver's (superset) layout/decoration/mutation guards.
                    static_class_ref = self._current_module_static_class_ref(class_id)
                    if static_class_ref is not None and class_info.get(
                        "constructor_fold_safe"
                    ):
                        class_ref = static_class_ref
                    else:
                        class_ref = self._emit_module_attr_get(class_id)
                else:
                    static_class_ref = self._current_module_static_class_ref(class_id)
                    if static_class_ref is not None:
                        class_ref = static_class_ref
                    else:
                        loop_static_class_ref = self._emit_loop_static_class_ref(
                            class_id
                        )
                        if loop_static_class_ref is not None:
                            class_ref = loop_static_class_ref
                        else:
                            local_class = self._load_local_value(class_id)
                            if local_class is not None:
                                class_ref = local_class
                            else:
                                class_ref = self._emit_module_attr_get(class_id)
                if self._class_is_exception_subclass(class_id, class_info):
                    new_method = class_info.get("methods", {}).get("__new__")
                    if new_method is None:
                        for base_name in class_info.get("mro", [])[1:]:
                            base_info = self.classes.get(base_name)
                            if base_info and base_info.get("methods", {}).get(
                                "__new__"
                            ):
                                new_method = base_info["methods"]["__new__"]
                                break
                    if needs_bind or new_method is not None:
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
                    args = self._emit_call_args(node.args)
                    res = self._emit_exception_new_from_class(class_ref, args)
                    init_method = class_info.get("methods", {}).get("__init__")
                    if init_method is None:
                        for base_name in class_info.get("mro", [])[1:]:
                            base_info = self.classes.get(base_name)
                            if base_info and base_info.get("methods", {}).get(
                                "__init__"
                            ):
                                init_method = base_info["methods"]["__init__"]
                                break
                    if init_method is not None:
                        init_func = init_method["func"]
                        target_name = init_func.type_hint.split(":", 1)[1]
                        init_args = [res] + args
                        if init_method.get("has_closure"):
                            # A closure __init__ (e.g. a bare `super()` body
                            # captures the implicit `__class__` cell) compiles
                            # with the cell as its leading parameter; a
                            # bare-name CALL would omit the cell argument and
                            # mis-match the symbol arity (LLVM verifier
                            # rejects; Cranelift only tolerates it when the
                            # cell is never read). Same invariant as the
                            # method-call fold: closure targets never get the
                            # direct symbol CALL — route through the bound
                            # path, which threads the cell via the function
                            # object.
                            init_func_val = self._emit_class_method_func(
                                class_ref, "__init__"
                            )
                            bound_init = MoltValue(self.next_var(), type_hint="method")
                            self.emit(
                                MoltOp(
                                    kind="BOUND_METHOD_NEW",
                                    args=[init_func_val, res],
                                    result=bound_init,
                                )
                            )
                            callargs = self._emit_call_args_builder(node)
                            init_res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[bound_init, callargs],
                                    result=init_res,
                                )
                            )
                            return res
                        func_obj = None
                        param_count = init_method.get("param_count")
                        defaults = init_method.get("defaults", [])
                        kwonly_count = init_method.get("kwonly_count")
                        positional_limit = None
                        if param_count is not None and isinstance(kwonly_count, int):
                            positional_limit = param_count - kwonly_count
                        if param_count is not None:
                            missing = param_count - len(init_args)
                            # Load __init__ whenever a trailing default is filled:
                            # a const default needs the version stamp for the
                            # `__defaults__`-mutation deopt guard, a non-const
                            # default needs the live read.
                            if 0 < missing <= len(defaults):
                                func_obj = self._emit_class_method_func(
                                    class_ref, "__init__"
                                )
                        init_args = self._apply_default_specs(
                            param_count,
                            defaults,
                            init_args,
                            node,
                            call_name=f"{class_id}.__init__",
                            func_obj=func_obj,
                            positional_limit=positional_limit,
                        )
                        if init_args is None:
                            init_func_val = self._emit_class_method_func(
                                class_ref, "__init__"
                            )
                            bound_init = MoltValue(self.next_var(), type_hint="method")
                            self.emit(
                                MoltOp(
                                    kind="BOUND_METHOD_NEW",
                                    args=[init_func_val, res],
                                    result=bound_init,
                                )
                            )
                            callargs = self._emit_call_args_builder(node)
                            init_res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[bound_init, callargs],
                                    result=init_res,
                                )
                            )
                            return res
                        init_res = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="CALL",
                                args=[target_name] + init_args,
                                result=init_res,
                            )
                        )
                    return res
                if class_info.get("dataclass"):
                    static_dataclass = self._try_emit_static_dataclass_constructor(
                        node,
                        class_id,
                        class_info,
                        class_ref,
                    )
                    if static_dataclass is not None:
                        return static_dataclass
                    field_order = class_info["field_order"]
                    name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(kind="CONST_STR", args=[class_id], result=name_val)
                    )
                    field_name_vals: list[MoltValue] = []
                    for field in field_order:
                        field_val = MoltValue(self.next_var(), type_hint="str")
                        self.emit(
                            MoltOp(kind="CONST_STR", args=[field], result=field_val)
                        )
                        field_name_vals.append(field_val)
                    field_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_name_vals,
                            result=field_names_tuple,
                        )
                    )
                    missing_val = MoltValue(self.next_var(), type_hint="missing")
                    self.emit(MoltOp(kind="MISSING", args=[], result=missing_val))
                    field_values = [missing_val for _ in field_order]
                    values_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_values,
                            result=values_tuple,
                        )
                    )
                    flags = 0
                    if class_info.get("frozen"):
                        flags |= 0x1
                    if class_info.get("eq"):
                        flags |= 0x2
                    if class_info.get("repr"):
                        flags |= 0x4
                    if class_info.get("slots"):
                        flags |= 0x8
                    flags_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[flags], result=flags_val))
                    res = MoltValue(self.next_var(), type_hint=class_id)
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_NEW",
                            args=[name_val, field_names_tuple, values_tuple, flags_val],
                            result=res,
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_SET_CLASS",
                            args=[res, class_ref],
                            result=MoltValue("none"),
                        )
                    )
                    init_func = self._emit_class_method_func(class_ref, "__init__")
                    bound_init = MoltValue(self.next_var(), type_hint="method")
                    self.emit(
                        MoltOp(
                            kind="BOUND_METHOD_NEW",
                            args=[init_func, res],
                            result=bound_init,
                        )
                    )
                    callargs = self._emit_call_args_builder(node)
                    init_res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[bound_init, callargs],
                            result=init_res,
                        )
                    )
                    return res
                _, new_returns_any = self._class_new_policy(class_id, class_info)

                # Phase-1-sibling class-instantiation fold.
                #
                # When a class has a vanilla layout — no metaclass, no
                # constructor-fold safety proving default `object.__new__`, no
                # closures/varargs/kwargs/defaults on __init__, and the call
                # site supplies positional args
                # only — we can replace the `CALL_BIND(class_ref,
                # callargs)` dispatch (which goes through
                # `type.__call__` → `__new__` → bound-method-init →
                # IC dispatch) with a structurally-equivalent two-op
                # sequence: alloc instance, direct CALL to __init__.
                # Targets bench_struct (`Point(0, 0)` per iter), bench_
                # exception_heavy (`ValueError(i)` per iter), and any
                # tight loop instantiating user types.  Each iteration
                # saves a callargs-builder allocation, the IC slot
                # probe, the bound-method allocation that
                # `type.__call__` does on every call, and the sequence
                # of dispatch-step trampolines around `__init__`.
                # Fast constructor folding is only sound when an upstream class
                # analysis records an explicit proof; simple-name class metadata is
                # not enough to bypass runtime type.__call__ semantics.
                constructor_fold_safe = bool(class_info.get("constructor_fold_safe"))
                if (
                    constructor_fold_safe
                    and not class_info.get("dynamic")
                    and not class_info.get("dataclass")
                    and not class_info.get("custom_metaclass")
                    and not node.keywords
                    and all(not isinstance(a, ast.Starred) for a in node.args)
                ):
                    init_info, init_owner = self._resolve_method_info(
                        class_id, "__init__"
                    )
                    # Treat "no __init__ on this class or any base except
                    # object" as an instantiation that can run the alloc
                    # alone — `object.__init__` is a no-op for the no-arg
                    # case, and we'll only fold when the call has no args.
                    init_is_default = init_info is None or init_owner == "object"
                    if init_is_default and len(node.args) == 0:
                        res = MoltValue(self.next_var(), type_hint=class_id)
                        # Carry the static class-instance payload size
                        # (in bytes, header NOT included) so the
                        # backend's escape-analysis-rewritten
                        # `object_new_bound_stack` arm can size the
                        # Cranelift StackSlot at codegen time.  The
                        # heap arm ignores it (sizing happens at
                        # runtime via `class_layout_size`).
                        class_size_bytes = (
                            class_info.get("size", 0) if class_info else 0
                        )
                        self.emit(
                            MoltOp(
                                kind="OBJECT_NEW_BOUND",
                                args=[class_ref],
                                result=res,
                                metadata={"class_size_bytes": class_size_bytes},
                            )
                        )
                        return res
                    if (
                        init_info is not None
                        and init_info.get("descriptor") == "function"
                        and not init_info.get("has_closure")
                        and not init_info.get("has_vararg")
                        and not init_info.get("has_varkw")
                        and not init_info.get("kwonly_count")
                    ):
                        # Defaults are fine ONLY if the call site supplies
                        # all required positional args, so that no
                        # default-spec evaluation is needed at runtime.
                        # The arg-count match below enforces this.
                        # Constructor fold safety has already proven default
                        # `object.__new__` through the full MRO. Keep this arm
                        # focused on the remaining direct-`__init__` contract.
                        getattribute_info, _ = self._resolve_method_info(
                            class_id, "__getattribute__"
                        )
                        if (
                            getattribute_info is None
                            and (init_owner or class_id) in self.classes
                        ):
                            init_func_val = init_info.get("func")
                            if init_func_val is not None and getattr(
                                init_func_val, "type_hint", ""
                            ).startswith("Func:"):
                                init_symbol = init_func_val.type_hint.split(":", 1)[1]
                                if init_symbol in self.func_symbol_names:
                                    param_count = init_info.get("param_count")
                                    if param_count is not None:
                                        expected_positional = param_count - 1
                                        if len(node.args) == expected_positional:
                                            res = MoltValue(
                                                self.next_var(),
                                                type_hint=class_id,
                                            )
                                            # See sibling site above: payload
                                            # size in bytes carried via
                                            # metadata for the stack-alloc
                                            # lowering.
                                            class_size_bytes = (
                                                class_info.get("size", 0)
                                                if class_info
                                                else 0
                                            )
                                            self.emit(
                                                MoltOp(
                                                    kind="OBJECT_NEW_BOUND",
                                                    args=[class_ref],
                                                    result=res,
                                                    metadata={
                                                        "class_size_bytes": class_size_bytes
                                                    },
                                                )
                                            )
                                            init_args = [
                                                self.visit(a) for a in node.args
                                            ]
                                            if not any(a is None for a in init_args):
                                                # Phase 2 sibling — inline
                                                # __init__ body directly when
                                                # it's a sequence of
                                                # `self.attr = expr`
                                                # assignments.  Eliminates
                                                # the per-iter __init__ CALL
                                                # frame setup that dominates
                                                # bench_struct's overhead;
                                                # the substituted body emits
                                                # STORE_ATTR ops on `res`.
                                                init_assigns = init_info.get(
                                                    "inline_init_assigns"
                                                )
                                                inline_params = init_info.get(
                                                    "inline_params"
                                                )
                                                # Fail-closed cross-module gate
                                                # (mirrors _try_inline_method_call):
                                                # an __init__ value-expression that
                                                # reads a defining-module global
                                                # (recorded in inline_free_names)
                                                # must not be spliced into a
                                                # different module's scope, where
                                                # the global would mis-resolve.
                                                init_free_names = init_info.get(
                                                    "inline_free_names"
                                                )
                                                init_cross_module = False
                                                if init_free_names:
                                                    init_owner_module = init_info.get(
                                                        "inline_owner_module"
                                                    )
                                                    init_cross_module = (
                                                        init_owner_module is not None
                                                        and init_owner_module
                                                        != self.module_name
                                                    )
                                                if (
                                                    init_assigns is not None
                                                    and inline_params is not None
                                                    and not init_cross_module
                                                    and self._try_inline_init_assigns(
                                                        init_assigns,
                                                        inline_params,
                                                        res,
                                                        init_args,
                                                    )
                                                ):
                                                    return res
                                                init_res = MoltValue(
                                                    self.next_var(),
                                                    type_hint="None",
                                                )
                                                self.emit(
                                                    MoltOp(
                                                        kind="CALL",
                                                        args=[init_symbol, res]
                                                        + init_args,
                                                        result=init_res,
                                                    )
                                                )
                                                return res

                callargs = self._emit_call_args_builder(node)
                res_hint = "Any" if new_returns_any else class_id
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
            if target_info and str(target_info.type_hint).startswith("BoundMethod:"):
                res_hint = "Any"
                class_name = "Unknown"
                method_name = "method"
                method_info = None
                return_hint = None
                parts = target_info.type_hint.split(":", 2)
                if len(parts) == 3:
                    class_name = parts[1]
                    method_name = parts[2]
                    method_info = (
                        self.classes.get(class_name, {})
                        .get("methods", {})
                        .get(method_name)
                    )
                    if method_info:
                        return_hint = method_info["return_hint"]
                    # Propagate builtin return types (int/float/bool/str/etc),
                    # not just user classes — otherwise method-call results in
                    # tight loops fall back to a NaN-boxed accumulator and the
                    # downstream lane inference forces float arithmetic.
                    if return_hint and (
                        return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
                    ):
                        res_hint = return_hint
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_info, callargs],
                            result=res,
                        )
                    )
                    return res
                args = self._emit_call_args(node.args)
                if method_info:
                    func_obj = None
                    param_count = method_info.get("param_count")
                    defaults = method_info.get("defaults", [])
                    kwonly_count = method_info.get("kwonly_count")
                    positional_limit = None
                    if param_count is not None and isinstance(kwonly_count, int):
                        positional_limit = param_count - kwonly_count
                    if param_count is not None:
                        missing = param_count - (len(args) + 1)
                        # Load the bound method's function whenever a trailing
                        # default is filled: a const default needs the version
                        # stamp for the `__defaults__`-mutation deopt guard, a
                        # non-const default needs the live read.
                        if 0 < missing <= len(defaults):
                            func_obj = self._emit_bound_method_func(target_info)
                    args = self._apply_default_specs(
                        param_count,
                        defaults,
                        args,
                        node,
                        call_name=f"{class_name}.{method_name}",
                        func_obj=func_obj,
                        implicit_self=True,
                        positional_limit=positional_limit,
                    )
                    if args is None:
                        callargs = self._emit_call_args_builder(node)
                        res = MoltValue(self.next_var(), type_hint=res_hint)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[target_info, callargs],
                                result=res,
                            )
                        )
                        return res
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="CALL_METHOD", args=[target_info] + args, result=res)
                )
                return res

            if target_info and str(target_info.type_hint).startswith("Func:"):
                target_name = target_info.type_hint.split(":")[1]
                intrinsic_target = _intrinsic_arity_exact(target_name) is not None
                res_hint = self._function_result_hint(target_name)
                direct_ok = intrinsic_target or target_name in self.func_default_specs
                if not direct_ok:
                    func_name = self.func_symbol_names.get(target_name)
                    if func_name and self._lookup_func_defaults(None, func_name):
                        direct_ok = True
                    elif self._known_function_symbol_target(target_name) is not None:
                        direct_ok = True
                if needs_bind or not direct_ok:
                    callargs = self._emit_call_args_builder(node)
                    callee = target_info
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        callee = self._emit_module_attr_get(func_id)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                args, func_obj = self._emit_direct_call_args_for_symbol(
                    target_name, node
                )
                if args is None:
                    callargs = self._emit_call_args_builder(node)
                    callee = target_info
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        callee = self._emit_module_attr_get(func_id)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint=res_hint)
                if (
                    intrinsic_target
                    or self.is_async()
                    or (
                        isinstance(node.func, ast.Name)
                        and node.func.id in self.stable_module_funcs
                    )
                ):
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                else:
                    callee = func_obj or self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
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
                    raise NotImplementedError("Unsupported call target")
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
                        res_hint = self._function_result_hint(func_symbol)
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
                        args, func_obj = self._emit_direct_call_args_for_symbol(
                            func_symbol, node, func_obj=callee
                        )
                        if args is None:
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
                        res = MoltValue(self.next_var(), type_hint=res_hint)
                        self.emit(
                            MoltOp(
                                kind="CALL_GUARDED",
                                args=[func_obj or callee] + args,
                                result=res,
                                metadata={"target": func_symbol},
                            )
                        )
                        return res
                    if isinstance(
                        callee.type_hint, str
                    ) and callee.type_hint.startswith("ClosureFunc:"):
                        func_symbol = callee.type_hint.split(":", 1)[1]
                        res_hint = self._function_result_hint(func_symbol)
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
                        args, _ = self._emit_direct_call_args_for_symbol(
                            func_symbol, node, func_obj=callee
                        )
                        if args is None:
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
                    if imported_from:
                        args = self._apply_direct_call_defaults(
                            imported_from, func_id, args, node
                        )
                        if args is None:
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
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            builtin_result = self._try_emit_named_builtin_call(
                node, func_id, needs_bind
            )
            if builtin_result is not CALL_NOT_HANDLED:
                return builtin_result
            if imported_from is not None:
                normalized = self._normalize_allowlist_module(imported_from)
            else:
                normalized = None
            lowered_intrinsic = self._try_lower_intrinsic_lookup_call(
                func_id=func_id,
                imported_from=imported_from,
                node=node,
            )
            if lowered_intrinsic is not None:
                return lowered_intrinsic
            if self._is_intrinsics_module_name(imported_from) and func_id in {
                "require_intrinsic",
                "_require_intrinsic",
                "load_intrinsic",
                "_load_intrinsic",
            }:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
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
            if imported_from is None:
                callee = self.visit(node.func)
                if callee is not None:
                    return self._emit_dynamic_call(node, callee, needs_bind)

            suggestion = self._call_allowlist_suggestion(func_id, imported_from)
            if suggestion:
                alternative = f"use {suggestion}"
            else:
                alternative = (
                    "import from an allowlisted module (see docs/spec/"
                    "areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)"
                )
            detail = (
                "Tier 0 only allows direct calls to allowlisted module-level"
                " functions; rebinding/monkey-patching is not observed"
            )
            if suggestion:
                detail = f"{detail}. warning: allowlisted path is {suggestion}"
            if self.fallback_policy == "bridge":
                self.compat.bridge_unavailable(
                    node,
                    f"call to non-allowlisted function '{func_id}'",
                    impact="high",
                    alternative=alternative,
                    detail=detail,
                )
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                res = MoltValue(self.next_var(), type_hint="Any")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            raise self.compat.unsupported(
                node,
                f"call to non-allowlisted function '{func_id}'",
                impact="high",
                alternative=alternative,
                detail=detail,
            )
        return CALL_NOT_HANDLED

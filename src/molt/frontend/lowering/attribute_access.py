"""AttributeAccessMixin: module, object, field, and property access lowering.

Move-only extraction from frontend/__init__.py. This lowering authority owns
module attribute get/set, imported-module attribute mutation tracking,
descriptor detection, guarded object field/property fast paths, and general
attribute load/store emission shared by expression, assignment, class, and call
visitors.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING

from molt.frontend._types import (
    _BUILTIN_FAST_METHODS,
    BUILTIN_TYPE_TAGS,
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.op_kinds_generated import (
    SIMPLEIR_RUNTIME_PROTECTED_ATTRIBUTE_REQUIREMENTS,
    SIMPLEIR_RUNTIME_PROTECTED_ACQUISITION_REQUIREMENTS,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AttributeAccessMixin(_MixinBase):
    def _exact_dataclass_field(
        self,
        obj: MoltValue,
        obj_name: str | None,
        attr: str,
    ) -> tuple[str, int] | None:
        class_id = self._exact_class_for_value(obj, obj_name)
        if class_id is None:
            return None
        class_info = self.classes.get(class_id)
        if class_info is None or not class_info.get("dataclass"):
            return None
        offset = class_info.get("fields", {}).get(attr)
        if not isinstance(offset, int):
            return None
        return class_id, offset

    def _module_can_defer_attrs(self, node: ast.Module) -> bool:
        for current in ast.walk(node):
            if isinstance(
                current,
                (
                    ast.FunctionDef,
                    ast.AsyncFunctionDef,
                    ast.ClassDef,
                    ast.Lambda,
                    ast.ListComp,
                    ast.SetComp,
                    ast.DictComp,
                    ast.GeneratorExp,
                ),
            ):
                return False
            if isinstance(current, ast.Call) and isinstance(current.func, ast.Name):
                if current.func.id in {"globals", "locals", "vars"}:
                    return False
        return True

    def _record_instance_attr_mutation(self, class_name: str, attr: str) -> None:
        if class_name not in self.classes:
            return
        self.instance_attr_mutations.setdefault(class_name, set()).add(attr)

    def _instance_attr_mutated(self, class_name: str, attr: str) -> bool:
        return attr in self.instance_attr_mutations.get(class_name, set())

    def _flush_deferred_module_attrs(self, names: set[str] | None = None) -> None:
        if not self.deferred_module_attrs or self.module_obj is None:
            return
        pending = self.deferred_module_attrs
        if names is not None:
            pending = pending & names
        for name in sorted(pending):
            # Skip variables that are live in the module dict via
            # module_global_mutations (loop-carried variables).
            # Their current value is in the module dict, not in a
            # local SSA variable.  Writing back the stale SSA value
            # would overwrite the accumulated loop result.
            if name in self.module_global_mutations:
                self.deferred_module_attrs.discard(name)
                continue
            val = self._load_local_value(name)
            if val is None:
                val = self.globals.get(name)
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self._emit_module_attr_set_on(self.module_obj, name, val)
            self.deferred_module_attrs.discard(name)

    def _expr_is_data_descriptor(self, expr: ast.expr) -> bool:
        if isinstance(expr, ast.Call) and isinstance(expr.func, ast.Name):
            if expr.func.id == "property":
                return True
            class_info = self.classes.get(expr.func.id)
            if class_info:
                methods = class_info.get("methods", {})
                return "__set__" in methods or "__delete__" in methods
        return False

    def _class_attr_is_data_descriptor(self, class_name: str, attr: str) -> bool:
        class_info = self.classes.get(class_name)
        if not class_info:
            return False
        for mro_name in class_info.get("mro", [class_name]):
            mro_info = self.classes.get(mro_name)
            if not mro_info:
                continue
            class_attrs = mro_info.get("class_attrs", {})
            expr = class_attrs.get(attr)
            if expr is not None and self._expr_is_data_descriptor(expr):
                return True
            method_info = mro_info.get("methods", {}).get(attr)
            if method_info and method_info["descriptor"] == "property":
                return True
        return False

    def _emit_module_attr_set(
        self, name: str, value: MoltValue, *, defer: bool = True
    ) -> None:
        if self.current_func_name != "molt_main" or self.module_obj is None:
            return
        self._record_app_module_store(name, value)
        if (
            defer
            and self.defer_module_attrs
            and name not in self.module_global_mutations
        ):
            self.deferred_module_attrs.add(name)
            return
        if self.defer_module_attrs:
            # Mutation-tracked bindings already belong to the live module
            # dictionary. The deferred flush deliberately skips them, so their
            # actual store must happen here and retire any older queued value.
            self.deferred_module_attrs.discard(name)
        self._emit_module_attr_set_on(self.module_obj, name, value)

    def _emit_module_attr_set_on(
        self, module_val: MoltValue, name: str, value: MoltValue
    ) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )
        # Track the value's type hint so _emit_module_attr_get can propagate
        # it to downstream consumers (enabling fast_int/fast_float paths).
        if (
            isinstance(value, MoltValue)
            and value.type_hint
            and value.type_hint != "Any"
        ):
            self._module_attr_type_hints[name] = value.type_hint

    def _emit_module_attr_get(self, name: str) -> MoltValue:
        # A bare module binding referenced from a Python function belongs to
        # that function object's active globals mapping. MODULE_GET_GLOBAL
        # supplies its builtins fallback and NameError behavior; explicit
        # ``module.attribute`` reads continue through _emit_module_attr_get_on.
        if self._function_needs_frame_trace():
            # A rebound function can supply a different value and type. Lexical
            # module facts do not establish the active namespace's value type.
            return self._emit_global_get(name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(self.module_name)
        # Propagate the last-known type hint for this module attribute.
        # When a module-scope variable was assigned from a typed expression
        # (e.g., count = 0 → int), the MODULE_GET_ATTR result inherits
        # that type so downstream _should_fast_int checks can fire.
        attr_hint = self._module_attr_type_hints.get(name, "Any")
        res = MoltValue(self.next_var(), type_hint=attr_hint)
        self.emit(
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[module_val, name_val],
                result=res,
            )
        )
        return res

    def _record_imported_module_attr_mutation(self, target: ast.Attribute) -> None:
        if not isinstance(target.value, ast.Name):
            return
        module_name = self._imported_module_binding_target(target.value.id)
        if module_name is None:
            return
        mutation = (module_name, target.attr)
        self.imported_module_attr_mutations.add(mutation)
        self.global_imported_module_attr_mutations.add(mutation)

    def _imported_module_attr_is_stable(self, module_name: str, attr: str) -> bool:
        mutation = (module_name, attr)
        return (
            mutation not in self.imported_module_attr_mutations
            and mutation not in self.global_imported_module_attr_mutations
        )

    def _emit_module_attr_set_runtime(self, name: str, value: MoltValue) -> None:
        if self._function_needs_frame_trace():
            globals_dict = self._emit_globals_dict()
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
            self.emit(
                MoltOp(
                    kind="DICT_SET",
                    args=[globals_dict, name_val, value],
                    result=MoltValue("none"),
                )
            )
            return
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(self.module_name)
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )

    def _imported_attr_name(self, bind_name: str) -> str:
        return self.imported_attr_names.get(
            bind_name, self.global_imported_attr_names.get(bind_name, bind_name)
        )

    def _emit_module_attr_get_on(self, module_name: str, name: str) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        metadata = self._runtime_qualified_callable_metadata(module_name, name)
        self.emit(
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[module_val, name_val],
                result=res,
                metadata=metadata,
            )
        )
        return res

    def _emit_module_attr_get_default_on(
        self, module_name: str, name: str, default_val: MoltValue
    ) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self._emit_getattr_name_default(
            module_val,
            name_val,
            default_val,
            res,
            literal_name=name,
            qualified_module_name=module_name,
        )
        return res

    def _runtime_protected_attribute_requirement_bits(
        self,
        obj: MoltValue,
        attr: str | None,
        *,
        exact_class: str | None,
        qualified_module_name: str | None = None,
    ) -> int:
        if exact_class is not None or obj.type_hint.startswith("super"):
            return 0
        if qualified_module_name is not None and attr is not None:
            _, gateway_bits = self._runtime_qualified_callable_requirement(
                qualified_module_name, attr
            )
            if gateway_bits:
                return gateway_bits
        if attr is None:
            return SIMPLEIR_RUNTIME_PROTECTED_ACQUISITION_REQUIREMENTS
        return SIMPLEIR_RUNTIME_PROTECTED_ATTRIBUTE_REQUIREMENTS.get(attr, 0)

    def _emit_getattr_name_default(
        self,
        obj: MoltValue,
        name: MoltValue,
        default: MoltValue,
        result: MoltValue,
        *,
        literal_name: str | None,
        exact_class: str | None = None,
        qualified_module_name: str | None = None,
    ) -> None:
        requirement_bits = self._runtime_protected_attribute_requirement_bits(
            obj,
            literal_name,
            exact_class=exact_class,
            qualified_module_name=qualified_module_name,
        )
        self.emit(
            MoltOp(
                kind="GETATTR_NAME_DEFAULT",
                args=[obj, name, default],
                result=result,
                metadata=(
                    {"runtime_requirement_bits": requirement_bits}
                    if requirement_bits
                    else None
                ),
            )
        )

    def _emit_guarded_setattr(
        self,
        obj: MoltValue,
        attr: str,
        value: MoltValue,
        expected_class: str,
        *,
        obj_name: str | None = None,
    ) -> None:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        class_ref: MoltValue | None = None
        if class_info and self._class_is_exception_subclass(expected_class, class_info):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        # Metaclass __init__ receives `cls` which is a TYPE object, not an
        # instance. Field offsets don't apply — always use generic setattr.
        if class_info and "type" in class_info.get("bases", []):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and attr not in class_info.get("fields", {}):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and not class_info.get("static"):
            class_ref = self._load_local_value(expected_class)
            if class_ref is None:
                if self._exact_class_for_value(
                    obj, obj_name
                ) == expected_class and self._class_layout_stable(expected_class):
                    self.emit(
                        MoltOp(
                            kind="SETATTR",
                            args=[obj, attr, value, expected_class],
                            result=MoltValue("none"),
                        )
                    )
                    return
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value],
                        result=MoltValue("none"),
                    )
                )
                return

        def resolve_class_ref() -> MoltValue:
            nonlocal class_ref
            if class_ref is None:
                class_ref = self._emit_class_ref(expected_class)
            return class_ref

        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            self.emit(
                MoltOp(
                    kind="SETATTR",
                    args=[obj, attr, value, expected_class],
                    result=MoltValue("none"),
                )
            )
            return
        if assumption is False:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if self._class_layout_stable(expected_class):
            if self._exact_class_for_value(obj, obj_name) == expected_class:
                self.emit(
                    MoltOp(
                        kind="SETATTR",
                        args=[obj, attr, value, expected_class],
                        result=MoltValue("none"),
                    )
                )
                return
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            class_ref = resolve_class_ref()
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            self.emit(
                MoltOp(
                    kind="GUARDED_SETATTR",
                    args=[
                        obj,
                        class_ref,
                        expected_version,
                        attr,
                        value,
                        expected_class,
                    ],
                    result=MoltValue("none"),
                )
            )
            return

        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR",
                args=[obj, attr, value, expected_class],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_PTR",
                args=[obj, attr, value],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_guarded_getattr(
        self,
        obj: MoltValue,
        attr: str,
        expected_class: str,
        *,
        obj_name: str | None = None,
    ) -> MoltValue:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        class_ref: MoltValue | None = None
        if class_info and self._class_is_exception_subclass(expected_class, class_info):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        # Metaclass methods operate on TYPE objects — field offsets don't apply.
        if class_info and "type" in class_info.get("bases", []):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if class_info and attr not in class_info.get("fields", {}):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if class_info and not class_info.get("static"):
            class_ref = self._load_local_value(expected_class)
            if class_ref is None:
                if self._exact_class_for_value(
                    obj, obj_name
                ) == expected_class and self._class_layout_stable(expected_class):
                    res = MoltValue(self.next_var())
                    self.emit(
                        MoltOp(
                            kind="GETATTR",
                            args=[obj, attr, expected_class],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_PTR",
                        args=[obj, attr],
                        result=res,
                    )
                )
                return res

        def resolve_class_ref() -> MoltValue:
            nonlocal class_ref
            if class_ref is None:
                class_ref = self._emit_class_ref(expected_class)
            return class_ref

        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, attr, expected_class],
                    result=res,
                )
            )
            return res
        if assumption is False:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if self._class_layout_stable(expected_class):
            if self._exact_class_for_value(obj, obj_name) == expected_class:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR",
                        args=[obj, attr, expected_class],
                        result=res,
                    )
                )
                return res
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            class_ref = resolve_class_ref()
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GUARDED_GETATTR",
                    args=[obj, class_ref, expected_version, attr, expected_class],
                    result=res,
                )
            )
            return res
        return self._emit_guarded_field_get_with_guard(
            obj,
            fast_attr=attr,
            fallback_attr=attr,
            expected_class=expected_class,
            guard=guard,
        )

    def _emit_guarded_field_get_with_guard(
        self,
        obj: MoltValue,
        fast_attr: str,
        fallback_attr: str,
        expected_class: str,
        guard: MoltValue,
    ) -> MoltValue:
        use_phi = self.enable_phi and not self.is_async()
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, fast_attr, expected_class],
                    result=fast_val,
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, fallback_attr],
                    result=slow_val,
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = (
                fast_val.type_hint
                if fast_val.type_hint == slow_val.type_hint
                else "Any"
            )
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        # Non-phi path. Async/poll-function bodies must thread the merged
        # result through a closure slot — the LIST_NEW + STORE_INDEX cell
        # pattern was unsafe because Cranelift's loop-header phi resolver
        # could merge the cell SSA value with the entry-block default
        # (None) on the first iteration, producing store_index(None, ...)
        # crashes.
        if self.is_async():
            slot = self._new_async_internal_slot()
            none_init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_init],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, fast_attr, expected_class],
                    result=fast_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fast_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, fallback_attr],
                    result=slow_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, slow_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = (
                fast_val.type_hint
                if fast_val.type_hint == slow_val.type_hint
                else "Any"
            )
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=merged))
            return merged

        # Sync, non-phi path: a single SSA value updated in both branches.
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=merged))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var())
        self.emit(
            MoltOp(
                kind="GETATTR",
                args=[obj, fast_attr, expected_class],
                result=fast_val,
            )
        )
        self.emit(MoltOp(kind="COPY", args=[fast_val], result=merged))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, fallback_attr],
                result=slow_val,
            )
        )
        self.emit(MoltOp(kind="COPY", args=[slow_val], result=merged))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if fast_val.type_hint == slow_val.type_hint:
            merged.type_hint = fast_val.type_hint
        return merged

    def _emit_guarded_property_get(
        self,
        obj: MoltValue,
        attr: str,
        getter_symbol: str,
        expected_class: str,
        *,
        obj_name: str | None = None,
    ) -> MoltValue:
        guard = self._loop_guard_for(obj, expected_class, obj_name=obj_name)
        if guard is None:
            guard = self._emit_layout_guard(obj, expected_class)
        use_phi = self.enable_phi and not self.is_async()
        # The receiver-layout guard certifies descriptor dispatch, not the
        # returned value. A getter annotation cannot certify its result lane.
        fast_hint = "Any"
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
            self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=slow_val,
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = fast_hint if fast_hint == slow_val.type_hint else "Any"
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        # Non-phi path. See `_emit_guarded_field_get_with_guard` for the full
        # rationale: in poll-function bodies we route the merged result
        # through a closure slot rather than a LIST_NEW + STORE_INDEX cell,
        # which is unsafe under Cranelift's loop-header phi resolver.
        if self.is_async():
            slot = self._new_async_internal_slot()
            none_init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_init],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
            self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fast_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=slow_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, slow_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = fast_hint if fast_hint == slow_val.type_hint else "Any"
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=merged))
            return merged

        # Sync, non-phi path: a single SSA value updated in both branches.
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=merged))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
        self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
        self.emit(MoltOp(kind="COPY", args=[fast_val], result=merged))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, attr],
                result=slow_val,
            )
        )
        self.emit(MoltOp(kind="COPY", args=[slow_val], result=merged))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if fast_hint == slow_val.type_hint:
            merged.type_hint = fast_hint
        return merged

    def _emit_attribute_load(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: str | None,
        *,
        generic: bool = False,
    ) -> MoltValue:
        # Set expression-level col_offset from the Attribute AST node so
        # that get_attr ops carry the correct column range for traceback
        # caret annotations (e.g. `x.upper` not `x.upper()`).
        _prev_expr_col = getattr(self, "_expr_col", None)
        _attr_col = getattr(node, "col_offset", None)
        _attr_end_col = getattr(node, "end_col_offset", None)
        if _attr_col is not None and _attr_end_col is not None:
            self._expr_col = (_attr_col, _attr_end_col)
        try:
            return self._emit_attribute_load_inner(
                node, obj, obj_name, exact_class, generic=generic
            )
        finally:
            self._expr_col = _prev_expr_col

    def _emit_attribute_load_inner(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: str | None,
        *,
        generic: bool = False,
    ) -> MoltValue:
        # Canonical imported-module callable acquisition is a producer fact.
        # It must be stamped before type-driven attribute lowering: a module
        # alias loaded through a function's module globals intentionally has an
        # ``Any`` value hint, yet lexical import resolution still proves the
        # exact sys/inspect binding. Target admission rejects this op itself, so
        # no use-sensitive callable/heap taint lane is necessary downstream.
        runtime_symbol, runtime_requirement_bits = (
            self._runtime_qualified_callable_provenance_for_binding(obj_name, node.attr)
        )
        if runtime_symbol is not None and not generic:
            attr_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.attr], result=attr_name))
            result = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="MODULE_GET_ATTR",
                    args=[obj, attr_name],
                    result=result,
                    metadata={"runtime_symbol": runtime_symbol},
                )
            )
            return result
        protected_requirement_bits = self._runtime_protected_attribute_requirement_bits(
            obj,
            node.attr,
            exact_class=exact_class,
        )
        if generic or runtime_requirement_bits or protected_requirement_bits:
            # Protected runtime callable acquisition is a value capability,
            # not a lexical-import spelling. Unknown receivers may carry a
            # sys/inspect module through conditionals, containers, calls, heap
            # storage, or any other Python value transport. Preserve ordinary
            # generic attribute semantics while stamping the acquisition
            # unless an exact non-module class is constructionally proven.
            result = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=result,
                    metadata={
                        "runtime_requirement_bits": (
                            runtime_requirement_bits | protected_requirement_bits
                        )
                    },
                )
            )
            return result
        class_info = self.classes.get(obj.type_hint)
        if class_info:
            getattribute_info, _ = self._resolve_method_info(
                obj.type_hint, "__getattribute__"
            )
            if getattribute_info:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_PTR",
                        args=[obj, node.attr],
                        result=res,
                    )
                )
                return res
        exact_dataclass_field = self._exact_dataclass_field(obj, obj_name, node.attr)
        if exact_dataclass_field is not None:
            exact_dataclass, field_offset = exact_dataclass_field
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_offset], result=idx_val))
            hint = None
            if self._hints_enabled():
                hint = (
                    self.classes[exact_dataclass].get("field_hints", {}).get(node.attr)
                )
            res = MoltValue(self.next_var(), type_hint=hint or "Unknown")
            self.emit(MoltOp(kind="DATACLASS_GET", args=[obj, idx_val], result=res))
            return res
        if class_info and class_info.get("dataclass"):
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        method_info = None
        method_class = None
        if class_info:
            method_info, method_class = self._resolve_method_info(
                obj.type_hint, node.attr
            )
        is_class_obj = (
            obj_name is not None
            and obj.type_hint == "type"
            and (obj_name in self.classes or obj_name in BUILTIN_TYPE_TAGS)
        )
        if method_info and method_info["descriptor"] == "function" and not is_class_obj:
            if method_class:
                method_owner_info = self.classes.get(method_class)
                if (
                    method_owner_info
                    and method_owner_info.get("module") == self.module_name
                ):
                    method_info = None
            # Avoid binding to same-module class methods directly; class method
            # objects are not guaranteed to be in scope for direct reuse.
        if method_info and method_info["descriptor"] == "function" and not is_class_obj:
            fields = class_info.get("fields", {}) if class_info else {}
            if (
                class_info
                and not class_info.get("dynamic")
                and class_info.get("module") == self.module_name
                and node.attr not in fields
                and not self._instance_attr_mutated(obj.type_hint, node.attr)
            ):
                func_val = method_info["func"]
                if self.current_func_name != "molt_main":
                    class_ref = MoltValue(self.next_var(), type_hint="type")
                    self.emit(MoltOp(kind="TYPE_OF", args=[obj], result=class_ref))
                    func_val = self._emit_class_method_func(class_ref, node.attr)
                class_name = method_class or obj.type_hint
                res = MoltValue(
                    self.next_var(),
                    type_hint=f"BoundMethod:{class_name}:{node.attr}",
                )
                self.emit(
                    MoltOp(
                        kind="BOUND_METHOD_NEW",
                        args=[func_val, obj],
                        result=res,
                    )
                )
                return res
        if (
            method_info
            and method_info["descriptor"] == "property"
            and class_info
            and not class_info.get("dynamic")
        ):
            property_field = method_info.get("property_field")
            if property_field:
                field_map = class_info.get("fields", {})
                if (
                    property_field in field_map
                    and not self._class_attr_is_data_descriptor(
                        obj.type_hint, property_field
                    )
                ):
                    guard = self._loop_guard_for(obj, obj.type_hint, obj_name=obj_name)
                    if guard is None:
                        guard = self._emit_layout_guard(obj, obj.type_hint)
                    return self._emit_guarded_field_get_with_guard(
                        obj,
                        fast_attr=property_field,
                        fallback_attr=node.attr,
                        expected_class=obj.type_hint,
                        guard=guard,
                    )
            getter_symbol = method_info["func"].type_hint.split(":", 1)[1]
            return self._emit_guarded_property_get(
                obj,
                node.attr,
                getter_symbol,
                obj.type_hint,
                obj_name=obj_name,
            )
        if obj.type_hint.startswith("module"):
            attr_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.attr], result=attr_name))
            res = MoltValue(self.next_var(), type_hint="Any")
            module_name = (
                self._imported_module_binding_target(obj_name)
                if obj_name is not None
                else None
            )
            metadata = (
                self._runtime_qualified_callable_metadata(module_name, node.attr)
                if module_name is not None
                else None
            )
            self.emit(
                MoltOp(
                    kind="MODULE_GET_ATTR",
                    args=[obj, attr_name],
                    result=res,
                    metadata=metadata,
                )
            )
            return res
        # Acquired method values use the same exact source-point result as
        # immediate method calls, never an annotation/transport hint.
        receiver_kind = self._builtin_exact_type_from_expr(node.value)
        _fast_methods = _BUILTIN_FAST_METHODS.get(receiver_kind)
        if _fast_methods is not None and node.attr in _fast_methods:
            res = MoltValue(
                self.next_var(),
                type_hint=f"BoundMethod:{receiver_kind}:{node.attr}",
            )
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        expected_class = obj.type_hint if obj.type_hint in self.classes else None
        if expected_class is None:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        if self.classes[expected_class].get("dynamic"):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        field_map = self.classes[expected_class].get("fields", {})
        if node.attr not in field_map:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        if self._class_attr_is_data_descriptor(expected_class, node.attr):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        hint = None
        if self._hints_enabled():
            hint = self.classes[expected_class].get("field_hints", {}).get(node.attr)
        res = self._emit_guarded_getattr(
            obj,
            node.attr,
            expected_class,
            obj_name=obj_name,
        )
        if hint is not None:
            res.type_hint = hint
        return res

    def _emit_attribute_store(
        self,
        obj: MoltValue | None,
        obj_expr: ast.AST | None,
        obj_name: str | None,
        attr: str,
        value_node: MoltValue,
    ) -> None:
        if obj_expr is not None and isinstance(obj_expr, ast.Name):
            class_name = obj_expr.id
            if class_name in self.classes:
                self._invalidate_loop_guards_for_class(class_name)
        # Callers may have evaluated callback-capable operands since first
        # loading the receiver (notably augmented assignment). Re-resolve at
        # the consuming store; a captured class string is never authority.
        exact_class = self._exact_class_for_value(obj, obj_name)
        class_info = None
        if obj is not None:
            class_info = self.classes.get(obj.type_hint)
        if exact_class is not None:
            self._record_instance_attr_mutation(exact_class, attr)
        elif obj is not None and obj.type_hint in self.classes:
            self._record_instance_attr_mutation(obj.type_hint, attr)
        if exact_class is not None and obj is not None:
            exact_info = self.classes.get(exact_class)
            if (
                exact_info
                and not exact_info.get("dynamic")
                and not exact_info.get("dataclass")
            ):
                field_map = exact_info.get("fields", {})
                if attr in field_map and not self._class_attr_is_data_descriptor(
                    exact_class, attr
                ):
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        exact_class,
                        obj_name=obj_name,
                    )
                    return
        exact_dataclass_field = (
            self._exact_dataclass_field(obj, obj_name, attr)
            if obj is not None
            else None
        )
        if exact_dataclass_field is not None:
            _, field_offset = exact_dataclass_field
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_offset], result=idx_val))
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and class_info.get("dataclass"):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        field_map = class_info.get("fields", {}) if class_info else {}
        if obj is not None and obj.type_hint in self.classes:
            if class_info and class_info.get("dynamic"):
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
            elif attr in field_map:
                if self._class_attr_is_data_descriptor(obj.type_hint, attr):
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        obj.type_hint,
                        obj_name=obj_name,
                    )
            else:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
        else:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value_node],
                    result=MoltValue("none"),
                )
            )

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
    _next_ic_index,
    BUILTIN_TYPE_TAGS,
    MoltOp,
    MoltValue,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AttributeAccessMixin(_MixinBase):
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

    def _flush_deferred_module_attrs(self) -> None:
        if not self.deferred_module_attrs or self.module_obj is None:
            return
        for name in sorted(self.deferred_module_attrs):
            # Skip variables that are live in the module dict via
            # module_global_mutations (loop-carried variables).
            # Their current value is in the module dict, not in a
            # local SSA variable.  Writing back the stale SSA value
            # would overwrite the accumulated loop result.
            if name in self.module_global_mutations:
                continue
            val = self._load_local_value(name)
            if val is None:
                val = self.globals.get(name)
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self._emit_module_attr_set_on(self.module_obj, name, val)

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
        if defer and self.defer_module_attrs:
            self.deferred_module_attrs.add(name)
            return
        if not defer and self.defer_module_attrs:
            self.deferred_module_attrs.discard(name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[self.module_obj, name_val, value],
                result=MoltValue("none"),
            )
        )

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

    def _emit_module_attr_get(
        self, name: str, *, effect_proof: str | None = None
    ) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(
                self.module_name, effect_proof=effect_proof
            )
        # Propagate the last-known type hint for this module attribute.
        # When a module-scope variable was assigned from a typed expression
        # (e.g., count = 0 → int), the MODULE_GET_ATTR result inherits
        # that type so downstream _should_fast_int checks can fire.
        attr_hint = self._module_attr_type_hints.get(name, "Any")
        res = MoltValue(self.next_var(), type_hint=attr_hint)
        metadata = {"effect_proof": effect_proof} if effect_proof else None
        self.emit(
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[module_val, name_val],
                result=res,
                metadata=metadata,
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
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, name_val], result=res)
        )
        return res

    def _emit_module_attr_get_default_on(
        self, module_name: str, name: str, default_val: MoltValue
    ) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_NAME_DEFAULT",
                args=[module_val, name_val, default_val],
                result=res,
            )
        )
        return res

    def _emit_guarded_setattr(
        self,
        obj: MoltValue,
        attr: str,
        value: MoltValue,
        expected_class: str,
        *,
        use_init: bool = False,
        assume_exact: bool = False,
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
                if assume_exact and self._class_layout_stable(expected_class):
                    # The caller guarantees the object is an instance of
                    # expected_class (e.g. `self` inside a method body).
                    # Emit a direct field store even when the class_ref is
                    # not available in the current scope (class defined
                    # inside a function).
                    setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
                    self.emit(
                        MoltOp(
                            kind=setattr_kind,
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
            setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
            self.emit(
                MoltOp(
                    kind=setattr_kind,
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
            if assume_exact or self.exact_locals.get(name) == expected_class:
                setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
                self.emit(
                    MoltOp(
                        kind=setattr_kind,
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
            setattr_kind = "GUARDED_SETATTR_INIT" if use_init else "GUARDED_SETATTR"
            self.emit(
                MoltOp(
                    kind=setattr_kind,
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
        setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
        self.emit(
            MoltOp(
                kind=setattr_kind,
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
        assume_exact: bool = False,
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
                    metadata={"ic_index": _next_ic_index()},
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
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        if class_info and not class_info.get("static"):
            class_ref = self._load_local_value(expected_class)
            if class_ref is None:
                if assume_exact and self._class_layout_stable(expected_class):
                    # The caller guarantees the object is an instance of
                    # expected_class (e.g. `self` in a method body or a
                    # freshly created instance in the calling scope).
                    # Use a direct field load.
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
                        metadata={"ic_index": _next_ic_index()},
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
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        if self._class_layout_stable(expected_class):
            if assume_exact or self.exact_locals.get(name) == expected_class:
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
                    metadata={"ic_index": _next_ic_index()},
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
            slot = self._async_local_offset(f"__guarded_field_{len(self.async_locals)}")
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
                    metadata={"ic_index": _next_ic_index()},
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
                metadata={"ic_index": _next_ic_index()},
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
        return_hint: str | None,
        *,
        obj_name: str | None = None,
    ) -> MoltValue:
        guard = self._loop_guard_for(obj, expected_class, obj_name=obj_name)
        if guard is None:
            guard = self._emit_layout_guard(obj, expected_class)
        use_phi = self.enable_phi and not self.is_async()
        fast_hint = return_hint or "Any"
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
                    metadata={"ic_index": _next_ic_index()},
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
            slot = self._async_local_offset(
                f"__guarded_property_{len(self.async_locals)}"
            )
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
                    metadata={"ic_index": _next_ic_index()},
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
                metadata={"ic_index": _next_ic_index()},
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
            return self._emit_attribute_load_inner(node, obj, obj_name, exact_class)
        finally:
            self._expr_col = _prev_expr_col

    def _emit_attribute_load_inner(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: str | None,
    ) -> MoltValue:
        if obj.type_hint.startswith("super"):
            super_class = None
            if obj.type_hint == "super":
                super_class = self.current_class
            else:
                super_class = obj.type_hint.split(":", 1)[1]
            if super_class:
                method_info, method_class = self._resolve_super_method_info(
                    super_class, node.attr
                )
                if method_info and method_info["descriptor"] in {
                    "function",
                    "classmethod",
                }:
                    owner_name = method_class or super_class
                    res = MoltValue(
                        self.next_var(),
                        type_hint=f"BoundMethod:{owner_name}:{node.attr}",
                    )
                    self.emit(
                        MoltOp(
                            kind="GETATTR_GENERIC_OBJ",
                            args=[obj, node.attr],
                            result=res,
                        )
                    )
                    return res
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
                        metadata={"ic_index": _next_ic_index()},
                    )
                )
                return res
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if node.attr not in field_map:
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_OBJ",
                        args=[obj, node.attr],
                        result=res,
                    )
                )
                return res
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_map[node.attr]], result=idx_val))
            hint = None
            if self._hints_enabled():
                hint = class_info.get("field_hints", {}).get(node.attr)
            res = MoltValue(self.next_var(), type_hint=hint or "Unknown")
            self.emit(MoltOp(kind="DATACLASS_GET", args=[obj, idx_val], result=res))
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
                method_info["return_hint"],
                obj_name=obj_name,
            )
        if obj.type_hint.startswith("module"):
            attr_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.attr], result=attr_name))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="MODULE_GET_ATTR",
                    args=[obj, attr_name],
                    result=res,
                )
            )
            return res
        # Fast-path BoundMethod hints for known built-in types.
        # When the receiver type is statically known (e.g. type_hint="str")
        # and the accessed attribute is in the fast-dispatch method table,
        # annotate the result with "BoundMethod:<type>:<method>" so that
        # _emit_dynamic_call emits CALL_METHOD and the native backend's
        # s_value match arm can avoid callargs allocation + IC lookup.
        _fast_methods = _BUILTIN_FAST_METHODS.get(obj.type_hint)
        if _fast_methods is not None and node.attr in _fast_methods:
            res = MoltValue(
                self.next_var(),
                type_hint=f"BoundMethod:{obj.type_hint}:{node.attr}",
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
                    metadata={"ic_index": _next_ic_index()},
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
                    metadata={"ic_index": _next_ic_index()},
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
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        hint = None
        if self._hints_enabled():
            hint = self.classes[expected_class].get("field_hints", {}).get(node.attr)
        assume_exact = exact_class == expected_class if exact_class else False
        res = self._emit_guarded_getattr(
            obj,
            node.attr,
            expected_class,
            assume_exact=assume_exact,
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
        exact_class: str | None,
        attr: str,
        value_node: MoltValue,
    ) -> None:
        if obj_expr is not None and isinstance(obj_expr, ast.Name):
            class_name = obj_expr.id
            if class_name in self.classes:
                self._invalidate_loop_guards_for_class(class_name)
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
                        assume_exact=True,
                    )
                    return
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if attr not in field_map:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
                return
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_map[attr]], result=idx_val))
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
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
                    # Inside a method body, `self` (the first parameter)
                    # is guaranteed to be an instance of the current class.
                    # Mark it as exact so the guarded setattr can use a
                    # direct field store instead of the slow generic path.
                    is_method_self = (
                        self.current_class is not None
                        and obj_expr is not None
                        and isinstance(obj_expr, ast.Name)
                        and obj_expr.id == self.current_method_first_param
                        and obj.type_hint == self.current_class
                    )
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        obj.type_hint,
                        obj_name=obj_name,
                        assume_exact=is_method_self,
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

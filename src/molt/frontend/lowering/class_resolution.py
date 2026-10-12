"""ClassResolutionMixin: static class graph, MRO, and method lookup helpers.

Move-only extraction from frontend/__init__.py. These helpers are shared by
class lowering, call lowering, static analysis, and serialization, so they live
as one class-resolution authority rather than inside a single consumer mixin.
"""

from __future__ import annotations


from molt.frontend._types import ClassInfo, MethodInfo, MoltOp, MoltValue
from molt.frontend.sema import c3_merge
from molt.frontend._mixin_base import GeneratorMixinBase


class ClassResolutionMixin(GeneratorMixinBase):
    def _class_layout_stable(self, class_name: str) -> bool:
        class_info = self.classes.get(class_name)
        if not class_info:
            return False
        if class_info.get("dynamic") or class_info.get("dataclass"):
            return False
        if class_name in self.mutated_classes:
            return False
        return True

    def _emit_class_ref(self, class_name: str) -> MoltValue:
        static_ref = self._current_module_static_class_ref(class_name)
        if static_ref is not None:
            return static_ref
        class_info = self.classes.get(class_name)
        module_name = class_info.get("module") if class_info else None
        if module_name and module_name != self.module_name:
            # Reuse an imported class binding, never import its defining module
            # or invoke module attribute hooks merely to construct a guard.
            binding = next(
                (
                    name
                    for name, origin in self.global_imported_names.items()
                    if origin == module_name
                    and self.global_imported_attr_names.get(name, name) == class_name
                ),
                None,
            )
            if binding is None:
                for name, origin in self.imported_names.items():
                    if (
                        name in self.local_imported_names
                        and origin == module_name
                        and self.imported_attr_names.get(name, name) == class_name
                        and self._active_class_ns_scope(name) is None
                    ):
                        local = self._load_local_value(name, guard_unbound=False)
                        if local is not None:
                            return local
                return self._emit_const_value(None)
            class_name = binding
        elif (
            class_info
            and not class_info.get("static")
            and not self._binding_targets_module_namespace(class_name)
        ):
            if self._active_class_ns_scope(class_name) is None:
                local = self._load_local_value(class_name, guard_unbound=False)
                if local is not None:
                    return local
            return self._emit_const_value(None)
        # These references belong only to synthesized layout guards. A retained
        # instance remains usable after its class binding is deleted/rebound,
        # including while module globals are being retired. Missing identity
        # makes the guard miss; explicit source Names still use _emit_global_get.
        namespace = (
            self._emit_globals_dict()
            if self._function_needs_frame_trace()
            else self._emit_module_globals_dict()
        )
        name = self._emit_const_value(class_name)
        missing = self._emit_const_value(None)
        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="DICT_GET", args=[namespace, name, missing], result=result)
        )
        return result

    def _current_module_static_class_ref(self, class_name: str) -> MoltValue | None:
        if self.current_func_name != "molt_main":
            return None
        if self.module_globals_dict_escaped:
            return None
        if class_name in self.module_global_mutations:
            return None
        if class_name in self.class_definition_pending:
            return None
        class_info = self.classes.get(class_name)
        if class_info is None:
            return None
        if class_info.get("module") != self.module_name:
            return None
        if class_info.get("decorated"):
            return None
        if not self._class_layout_stable(class_name):
            return None
        static_name = class_info.get("class_value_name")
        if not static_name:
            return None
        current = self.globals.get(class_name)
        if current is None or current.name != static_name:
            return None
        return current

    def _emit_class_method_func(
        self, class_obj: MoltValue, method_name: str
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[class_obj, method_name],
                result=res,
            )
        )
        return res

    def _class_mro_names(self, name: str) -> list[str]:
        if name == "object":
            return ["object"]
        info = self.classes.get(name)
        if info is None:
            return [name]
        cached = info.get("mro")
        if cached:
            return cached
        bases = info.get("bases", [])
        seqs = [self._class_mro_names(base) for base in bases]
        seqs.append(list(bases))
        merged = c3_merge(seqs)
        if merged is None:
            mro = [name] + list(bases)
            info["mro"] = mro
            return mro
        mro = [name] + merged
        info["mro"] = mro
        return mro

    def _class_is_exception_subclass(
        self, class_name: str, class_info: ClassInfo
    ) -> bool:
        cached = class_info.get("exception_subclass")
        if cached is not None:
            return cached
        for base_name in self._class_mro_names(class_name)[1:]:
            if (
                self._builtin_exception_is_available(base_name)
                and base_name not in self.classes
            ):
                class_info["exception_subclass"] = True
                return True
            base_info = self.classes.get(base_name)
            if base_info and self._class_is_exception_subclass(base_name, base_info):
                class_info["exception_subclass"] = True
                return True
        class_info["exception_subclass"] = False
        return False

    def _resolve_method_info(
        self, class_name: str, method: str
    ) -> tuple[MethodInfo | None, str | None]:
        if class_name in self.class_definition_pending:
            return None, None
        for name in self._class_mro_names(class_name):
            info = self.classes.get(name)
            if not info:
                continue
            methods = info.get("methods", {})
            class_attrs = info.get("class_attrs", {})
            pending = info.get("pending_methods")
            # Avoid early binding to base methods when the current class
            # defines the method later in the class body.
            if pending and method in pending and method not in methods:
                return None, name
            # Avoid binding to base methods when a class-level assignment
            # overrides the attribute with a non-method value.
            if method in class_attrs and method not in methods:
                return None, name
            if method in methods:
                return methods[method], name
        return None, None

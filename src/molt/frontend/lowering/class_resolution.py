"""ClassResolutionMixin: static class graph, MRO, and method lookup helpers.

Move-only extraction from frontend/__init__.py. These helpers are shared by
class lowering, call lowering, static analysis, and serialization, so they live
as one class-resolution authority rather than inside a single consumer mixin.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from molt.frontend._types import (
    BUILTIN_EXCEPTION_NAMES,
    ClassInfo,
    MethodInfo,
    MoltOp,
    MoltValue,
)
from molt.frontend.sema import c3_merge

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ClassResolutionMixin(_MixinBase):
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
            return self._emit_module_attr_get_on(module_name, class_name)
        return self._emit_module_attr_get(class_name)

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
            if base_name in BUILTIN_EXCEPTION_NAMES and base_name not in self.classes:
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

    def _resolve_super_method_info(
        self, class_name: str, method: str
    ) -> tuple[MethodInfo | None, str | None]:
        mro = self._class_mro_names(class_name)
        found_start = False
        for name in mro:
            if not found_start:
                if name == class_name:
                    found_start = True
                continue
            info = self.classes.get(name)
            if info and "methods" in info and method in info["methods"]:
                return info["methods"][method], name
        return None, None

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

    def _c3_merge(self, seqs: list[list[str]]) -> list[str] | None:
        merged: list[str] = []
        working = [list(seq) for seq in seqs]
        heads = [0] * len(working)
        tail_counts: dict[str, int] = {}
        for seq in working:
            for name in seq[1:]:
                tail_counts[name] = tail_counts.get(name, 0) + 1

        while True:
            remaining = 0
            for idx, seq in enumerate(working):
                if heads[idx] < len(seq):
                    remaining += 1
            if remaining == 0:
                return merged

            candidate: str | None = None
            for idx, seq in enumerate(working):
                head_idx = heads[idx]
                if head_idx >= len(seq):
                    continue
                head = seq[head_idx]
                if tail_counts.get(head, 0) == 0:
                    candidate = head
                    break

            if candidate is None:
                return None

            merged.append(candidate)
            for idx, seq in enumerate(working):
                head_idx = heads[idx]
                if head_idx < len(seq) and seq[head_idx] == candidate:
                    heads[idx] += 1
                    next_head_idx = heads[idx]
                    if next_head_idx < len(seq):
                        next_head = seq[next_head_idx]
                        count = tail_counts.get(next_head, 0)
                        if count <= 1:
                            tail_counts.pop(next_head, None)
                        else:
                            tail_counts[next_head] = count - 1

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
        merged = self._c3_merge(seqs)
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

    def _reachable_base_names(
        self, class_name: str, _seen: set[str] | None = None
    ) -> set[str]:
        """Transitive set of base names reachable from ``class_name`` over the
        static module class graph (best-effort; used only to decide whether an
        un-resolvable class might be a subclass of the fold target)."""
        if _seen is None:
            _seen = set()
        if class_name in _seen:
            return _seen
        _seen.add(class_name)
        defs = self.module_class_bases.get(class_name)
        if not defs:
            return _seen
        for entry in defs:
            for base in entry:
                if base != "<opaque>":
                    self._reachable_base_names(base, _seen)
        return _seen

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

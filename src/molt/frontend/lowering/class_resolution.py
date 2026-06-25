"""ClassResolutionMixin: static class graph, MRO, and method lookup helpers.

Move-only extraction from frontend/__init__.py. These helpers are shared by
class lowering, call lowering, static analysis, and serialization, so they live
as one class-resolution authority rather than inside a single consumer mixin.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from molt.frontend._types import BUILTIN_EXCEPTION_NAMES, ClassInfo, MethodInfo

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ClassResolutionMixin(_MixinBase):
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

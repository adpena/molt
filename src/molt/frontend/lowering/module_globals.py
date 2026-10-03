"""ModuleGlobalsMixin: execution namespaces and frame locals lowering.

Move-only extraction from frontend/__init__.py. This lowering authority owns
owned module operands, frame global get/delete, synthesized ``globals`` and
``locals`` backing dictionaries, and the frame-locals pin used by function,
module, import, annotation, expression, and assignment lowering.
First-class builtins use canonical runtime callable materialization, never
module-local wrappers: their identity is shared and globals follows the caller.
"""

from __future__ import annotations


from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import MoltOp, MoltValue


class ModuleGlobalsMixin(GeneratorMixinBase):
    def _lexical_module_owner(self) -> MoltValue:
        """Use the module execution owner, including an explicit chunk parameter."""
        if self.module_obj is None:
            raise RuntimeError("module execution has no owned namespace operand")
        return self.module_obj

    def _emit_global_namespace_operand(self) -> MoltValue:
        """Select frame-owned globals or the held module execution owner.

        Python function entry consumes FrameInvocationGuard's captured namespace.
        MODULE_GET_GLOBAL and MODULE_DEL_GLOBAL select that active frame before
        considering their optional module operand. No public import lookup is
        part of lexical access, including after deletion or re-import.
        """
        if self._function_needs_frame_trace():
            return self._emit_const_value(None)
        return self._lexical_module_owner()

    def _emit_module_global_del(self, name: str) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self._emit_global_namespace_operand()
        self.emit(
            MoltOp(
                kind="MODULE_DEL_GLOBAL",
                args=[module_val, name_val],
                result=MoltValue("none"),
            )
        )

    def _emit_module_global_del_safe(self, name: str) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self._emit_global_namespace_operand()
        self.emit(
            MoltOp(
                kind="MODULE_DEL_GLOBAL_IF_PRESENT",
                args=[module_val, name_val],
                result=MoltValue("none"),
            )
        )

    def _emit_global_get(self, name: str) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self._emit_global_namespace_operand()
        res = MoltValue(self.next_var(), type_hint="Any")
        module_name = self.imported_names.get(
            name, self.global_imported_names.get(name)
        )
        attr_name = self.imported_attr_names.get(
            name, self.global_imported_attr_names.get(name, name)
        )
        # The active function namespace may differ from the lexical module.
        # Keep live LOAD_GLOBAL and its builtin fallback; possible provenance
        # constrains target admission but never certifies a concrete callable.
        requirement_bits = self._runtime_qualified_callable_requirement_bits(
            "builtins", name
        )
        if not self._local_name_shadows_import_binding(name):
            requirement_bits |= self._runtime_qualified_callable_requirement_bits(
                module_name, attr_name
            )
        self.emit(
            MoltOp(
                kind="MODULE_GET_GLOBAL",
                args=[module_val, name_val],
                result=res,
                metadata=(
                    {"runtime_requirement_bits": requirement_bits}
                    if requirement_bits
                    else None
                ),
            )
        )
        return res

    def _emit_globals_dict(self) -> MoltValue:
        """Return the globals mapping for the active Python execution frame."""
        return self._emit_runtime_call("molt_globals_builtin", [], type_hint="dict")

    def _emit_module_globals_dict(self) -> MoltValue:
        """Return this compilation unit's lexical module dictionary.

        This is the pre-frame bootstrap authority for module code metadata.
        Python execution must use ``_emit_globals_dict`` so rebound function
        objects observe their explicit globals mapping.
        """
        module_val = self._lexical_module_owner()
        dict_name = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__dict__"], result=dict_name))
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, dict_name], result=res)
        )
        return res

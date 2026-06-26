"""ModuleGlobalsMixin: module cache, globals, and frame locals lowering.

Move-only extraction from frontend/__init__.py. This lowering authority owns
module-cache references, module global get/delete, synthesized ``globals`` and
``locals`` backing dictionaries, and the frame-locals pin used by function,
module, import, annotation, expression, and assignment lowering.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from molt.frontend._types import _MOLT_GLOBALS_BUILTIN, MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ModuleGlobalsMixin(_MixinBase):
    def _get_or_emit_module_cache(
        self, module_name: str, *, effect_proof: str | None = None
    ) -> MoltValue:
        """Return a MoltValue for *module_name* from MODULE_CACHE_GET.

        Emits a fresh CONST_STR + MODULE_CACHE_GET pair on every call.  Earlier
        versions cached the MoltValue across the function scope, but state-machine
        lowering (used for module init functions with jumps/labels) can place the
        first MODULE_CACHE_GET in a branch that is skipped when a preceding
        exception redirects the state machine.  Re-emitting the lookup each time
        ensures the local is populated in the state that actually uses it.

        Note: this helper is only appropriate for simple, unconditional MODULE_CACHE_GET
        calls (i.e. for the *current* module or other modules that are guaranteed already
        loaded).  Use ``_emit_module_load`` for modules that may need lazy-initialisation.
        """
        module_name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_name_val))
        module_val = MoltValue(self.next_var(), type_hint="module")
        metadata = {"effect_proof": effect_proof} if effect_proof else None
        self.emit(
            MoltOp(
                kind="MODULE_CACHE_GET",
                args=[module_name_val],
                result=module_val,
                metadata=metadata,
            )
        )
        return module_val

    def _emit_module_global_del(self, name: str) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self.module_obj
        if self.current_func_name != "molt_main" or module_val is None:
            module_val = self._get_or_emit_module_cache(self.module_name)
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
        module_val = self.module_obj
        if self.current_func_name != "molt_main" or module_val is None:
            module_val = self._get_or_emit_module_cache(self.module_name)
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
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(self.module_name)
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="MODULE_GET_GLOBAL", args=[module_val, name_val], result=res)
        )
        return res

    def _emit_globals_dict(self) -> MoltValue:
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(self.module_name)
        dict_name = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__dict__"], result=dict_name))
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, dict_name], result=res)
        )
        return res

    def _emit_globals_builtin_obj(self) -> MoltValue:
        if self.globals_builtin_val is not None:
            return self.globals_builtin_val
        func_symbol = self._function_symbol(_MOLT_GLOBALS_BUILTIN)
        func_val = MoltValue(self.next_var(), type_hint=f"Func:{func_symbol}")
        self.emit(MoltOp(kind="FUNC_NEW", args=[func_symbol, 0], result=func_val))
        self._emit_function_metadata(
            func_val,
            name="globals",
            qualname="globals",
            trace_lineno=None,
            posonly_params=[],
            pos_or_kw_params=[],
            kwonly_params=[],
            vararg=None,
            varkw=None,
            default_exprs=[],
            kw_default_exprs=[],
            docstring="Return the current module globals.",
            module_override="builtins",
        )
        set_builtin = self._emit_builtin_function("_molt_function_set_builtin")
        builtin_res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[set_builtin, func_val], result=builtin_res)
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        self.start_function(
            func_symbol, params=[], type_facts_name=_MOLT_GLOBALS_BUILTIN
        )
        res = self._emit_globals_dict()
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.globals_builtin_val = func_val
        return func_val

    def _ensure_globals_builtin(self) -> None:
        if (
            self.globals_builtin_emitted
            or self.current_func_name != "molt_main"
            or self.module_obj is None
        ):
            return
        func_val = self._emit_globals_builtin_obj()
        self._emit_module_attr_set(_MOLT_GLOBALS_BUILTIN, func_val)
        self.globals_builtin_emitted = True

    def _emit_globals_builtin_ref(self) -> MoltValue:
        if not self.globals_builtin_emitted:
            self._ensure_globals_builtin()
        return self._emit_module_attr_get(_MOLT_GLOBALS_BUILTIN)

    def _init_locals_cache(self) -> None:
        if self.locals_cache_val is not None:
            return
        cache_val = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=cache_val))
        self.locals_cache_val = cache_val

    def _init_locals_cache_and_pin(self) -> None:
        """Allocate the locals cache dict and pin it on the frame stack.

        This should be called from function visitors when the function body
        contains a ``locals()`` call.  It combines ``_init_locals_cache()``
        with the ``FRAME_LOCALS_SET`` emission that was previously done
        unconditionally in ``start_function()``.
        """
        self._init_locals_cache()
        cache_val = self.locals_cache_val
        if cache_val is not None:
            self.emit(
                MoltOp(
                    kind="FRAME_LOCALS_SET",
                    args=[cache_val],
                    result=MoltValue("none"),
                )
            )

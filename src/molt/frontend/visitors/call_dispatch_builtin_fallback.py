"""CallNamedBuiltinFallbackDispatchMixin: named builtin call lowering authority."""

from __future__ import annotations

import ast

from typing import (
    Any,
)

from molt.builtin_callables import BUILTIN_FUNC_SPECS
from molt.compiler_analysis.python_identity_catalog_generated import (
    BUILTIN_NAMES_BY_IDENTITY,
    PythonIdentity,
)
from molt.frontend._types import (
    MoltOp,
    MoltValue,
)


from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED
from molt.frontend._mixin_base import GeneratorMixinBase


class CallNamedBuiltinFallbackDispatchMixin(GeneratorMixinBase):
    def _try_emit_named_builtin_fallback_call(
        self, node: ast.Call, builtin: PythonIdentity, needs_bind: bool
    ) -> Any:
        func_id = BUILTIN_NAMES_BY_IDENTITY[builtin]
        if func_id in BUILTIN_FUNC_SPECS:
            spec = BUILTIN_FUNC_SPECS[func_id]
            # Publication declares special binding protocols. Keep this call's
            # transport conservative; the live callee owns the actual binder.
            needs_bind = needs_bind or (
                (spec.bind_kind is not None and spec.bind_kind.requires_custom_binding)
                or spec.vararg is not None
                or spec.varkw is not None
                or bool(spec.kwonly_params)
            )
            direct = self._try_emit_direct_builtin_call(node, func_id, needs_bind)
            if direct is not CALL_NOT_HANDLED:
                return direct
            callee = self._emit_builtin_function(func_id)
            res = MoltValue(self.next_var(), type_hint="Any")
            if needs_bind:
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            else:
                args = self._emit_call_args(node.args)
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
            return res
        return CALL_NOT_HANDLED

    def _try_emit_direct_builtin_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> MoltValue | object:
        """A proven builtin called with plain positional arguments is one
        direct call of its runtime entry (call_anatomy 4.5a, closed world).

        No function object, no name lookup, no binder: the arguments the
        binder would pass, with each omitted trailing default spelled as the
        binder spells it (the missing sentinel or the declared constant).
        Every other shape keeps the bound call.
        """
        spec = BUILTIN_FUNC_SPECS[func_id]
        if (
            needs_bind
            or node.keywords
            or spec.vararg is not None
            or spec.varkw is not None
            or spec.kwonly_params
            or spec.pos_or_kw_params
        ):
            return CALL_NOT_HANDLED
        required = len(spec.params) - len(spec.defaults)
        supplied = len(node.args)
        if not required <= supplied <= len(spec.params):
            return CALL_NOT_HANDLED
        omitted = spec.defaults[supplied - required :]
        if not all(
            _is_missing_default(default) or isinstance(default, ast.Constant)
            for default in omitted
        ):
            return CALL_NOT_HANDLED
        args = self._emit_call_args(node.args)
        for default in omitted:
            if _is_missing_default(default):
                args.append(self._emit_missing_value())
            else:
                value = self.visit(default)
                if value is None:
                    return CALL_NOT_HANDLED
                args.append(value)
        return self._emit_runtime_call(spec.runtime, args)


def _is_missing_default(default: ast.expr) -> bool:
    return isinstance(default, ast.Name) and default.id == "__molt_missing__"

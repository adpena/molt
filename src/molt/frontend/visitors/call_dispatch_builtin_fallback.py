"""CallNamedBuiltinFallbackDispatchMixin: named builtin call lowering authority."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    BUILTIN_FUNC_SPECS,
    MoltOp,
    MoltValue,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object

from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED


class CallNamedBuiltinFallbackDispatchMixin(_MixinBase):
    def _try_emit_named_builtin_fallback_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        if func_id in BUILTIN_FUNC_SPECS:
            if func_id == "open":
                needs_bind = True
            spec = BUILTIN_FUNC_SPECS[func_id]
            # CALL_FUNC bypasses argument binding; vararg/kwonly builtins must
            # route through CALL_BIND to preserve Python call semantics.
            needs_bind = needs_bind or (
                spec.vararg is not None or bool(spec.kwonly_params)
            )
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

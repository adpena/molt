"""CallNamedBuiltinDispatchMixin: named builtin call lowering orchestrator."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend.visitors.call_dispatch_builtin_constructors import (
    CallNamedBuiltinConstructorDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_builtin_fallback import (
    CallNamedBuiltinFallbackDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_builtin_iter import (
    CallNamedBuiltinIterDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_builtin_scalar import (
    CallNamedBuiltinScalarDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallNamedBuiltinDispatchMixin(
    CallNamedBuiltinScalarDispatchMixin,
    CallNamedBuiltinIterDispatchMixin,
    CallNamedBuiltinConstructorDispatchMixin,
    CallNamedBuiltinFallbackDispatchMixin,
    _MixinBase,
):
    def _try_emit_named_builtin_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        for lower in (
            self._try_emit_named_builtin_scalar_call,
            self._try_emit_named_builtin_iter_call,
            self._try_emit_named_builtin_constructor_call,
            self._try_emit_named_builtin_fallback_call,
        ):
            lowered = lower(node, func_id, needs_bind)
            if lowered is not CALL_NOT_HANDLED:
                return lowered
        return CALL_NOT_HANDLED

"""CallNamedBuiltinDispatchMixin: named builtin call lowering orchestrator."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection

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
        if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
            keyword.arg is None for keyword in node.keywords
        ):
            # Splat cardinality and duplicate/keyword errors belong to the
            # runtime binder. Individual builtin lowerers only see explicit
            # arguments; treating a starred operand as one argument is wrong.
            # Residual user names take the same generic path, without relying
            # on an incomplete builtin-name catalog to establish callability.
            callee = self.visit(node.func)
            if callee is None:
                raise FrontendRejection(
                    Diagnostic.CALL_TARGET, "Unsupported call target"
                )
            return self._emit_dynamic_call(node, callee, True)
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

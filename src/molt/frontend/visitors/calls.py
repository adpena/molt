"""CallVisitorMixin: call-expression lowering orchestrator.

The concrete call-lowering authorities live in sibling mixins. This module owns
only the MRO composition and the small visit_Call phase order.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Any

from molt.frontend.visitors.call_defaults import CallDefaultsMixin
from molt.frontend.visitors.call_dispatch_attribute import CallAttributeDispatchMixin
from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED
from molt.frontend.visitors.call_dispatch_imported import (
    CallImportedAttributeDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_named import CallNamedDispatchMixin
from molt.frontend.visitors.call_dispatch_named_builtins import (
    CallNamedBuiltinDispatchMixin,
)
from molt.frontend.visitors.call_method_dispatch import CallMethodDispatchMixin
from molt.frontend.visitors.call_module_dispatch import CallModuleDispatchMixin
from molt.frontend.visitors.call_reductions import CallReductionMixin
from molt.frontend.visitors.call_runtime_helpers import CallRuntimeHelperMixin

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CallVisitorMixin(
    CallNamedDispatchMixin,
    CallNamedBuiltinDispatchMixin,
    CallImportedAttributeDispatchMixin,
    CallAttributeDispatchMixin,
    CallRuntimeHelperMixin,
    CallMethodDispatchMixin,
    CallModuleDispatchMixin,
    CallDefaultsMixin,
    CallReductionMixin,
    _MixinBase,
):
    def visit_Call(self, node: ast.Call) -> Any:
        gpu_launch = self._lower_gpu_kernel_launch_call(node)
        if gpu_launch is not None:
            return gpu_launch

        gpu_intrinsic = self._is_gpu_intrinsic_call(node)
        if gpu_intrinsic is not None and self.current_gpu_kernel_context:
            return self._emit_gpu_kernel_intrinsic_op(gpu_intrinsic)

        user_method_fold = self._try_emit_user_method_static_call(node)
        if user_method_fold is not None:
            return user_method_fold

        super_fold = self._try_emit_super_static_call(node)
        if super_fold is not None:
            return super_fold

        importlib_literal = self._try_emit_importlib_import_module_literal_call(node)
        if importlib_literal is not None:
            return importlib_literal

        needs_bind = self._call_needs_bind(node)
        attribute_result = self._try_emit_attribute_receiver_call(node, needs_bind)
        if attribute_result is not CALL_NOT_HANDLED:
            return attribute_result

        imported_attribute_result = self._try_emit_imported_attribute_call(
            node, needs_bind
        )
        if imported_attribute_result is not CALL_NOT_HANDLED:
            return imported_attribute_result

        named_result = self._try_emit_named_call(node, needs_bind)
        if named_result is not CALL_NOT_HANDLED:
            return named_result

        callee = self.visit(node.func)
        if callee is None:
            raise NotImplementedError("Unsupported call target")
        return self._emit_dynamic_call(node, callee, needs_bind)

"""CallImportedAttributeDispatchMixin: extracted visit_Call dispatch phase."""

from __future__ import annotations

import ast
from typing import (
    Any,
)

from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection


from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED
from molt.frontend._mixin_base import GeneratorMixinBase


class CallImportedAttributeDispatchMixin(GeneratorMixinBase):
    def _try_emit_imported_attribute_call(
        self, node: ast.Call, needs_bind: bool
    ) -> Any:
        if isinstance(node.func, ast.Attribute):
            module_name = None
            if isinstance(node.func.value, ast.Name):
                module_name = self._imported_module_binding_target(node.func.value.id)
            if module_name:
                func_id = node.func.attr
                normalized = self._normalize_allowlist_module(module_name)
                allowlist_key = normalized or module_name
                if func_id == "field" and allowlist_key == "dataclasses":
                    return self._emit_dataclasses_field_call(allowlist_key, node)
                if self._should_attempt_runtime_module_import(
                    module_name
                ) or self._is_internal_module(module_name):
                    lowered_handle_ctor = (
                        self._try_emit_intrinsic_handle_class_constructor(
                            allowlist_key,
                            func_id,
                            node,
                        )
                    )
                    if lowered_handle_ctor is not None:
                        return lowered_handle_ctor
                    lowered_imported_call = (
                        self._try_emit_imported_module_direct_or_task_call(
                            allowlist_key,
                            func_id,
                            node,
                            needs_bind=needs_bind,
                        )
                    )
                    if lowered_imported_call is not None:
                        return lowered_imported_call
                    callee = self.visit(node.func)
                    if callee is None:
                        raise FrontendRejection(
                            Diagnostic.CALL_TARGET, "Unsupported call target"
                        )
                    return self._emit_dynamic_call(node, callee)
        return CALL_NOT_HANDLED

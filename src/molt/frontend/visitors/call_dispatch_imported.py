"""CallImportedAttributeDispatchMixin: extracted visit_Call dispatch phase."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    MOLT_DIRECT_CALLS,
    MOLT_DIRECT_CALL_BIND_ALWAYS,
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


class CallImportedAttributeDispatchMixin(_MixinBase):
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
                if func_id == "open" and allowlist_key == "builtins":
                    return self._emit_open_call(node)
                enforce_allowlist = (
                    allowlist_key in MOLT_DIRECT_CALLS
                    or allowlist_key in self.stdlib_allowlist
                )
                force_bind = func_id[
                    :1
                ].isupper() or func_id in MOLT_DIRECT_CALL_BIND_ALWAYS.get(
                    allowlist_key, set()
                )
                if (
                    allowlist_key in MOLT_DIRECT_CALLS
                    and func_id in MOLT_DIRECT_CALLS[allowlist_key]
                ):
                    lowered_imported_call = (
                        self._try_emit_imported_module_direct_or_task_call(
                            allowlist_key,
                            func_id,
                            node,
                            imported_from=module_name,
                            normalized=normalized,
                            needs_bind=needs_bind,
                            force_bind=force_bind,
                            direct_registry_authorized=True,
                        )
                    )
                    if lowered_imported_call is not None:
                        return lowered_imported_call
                if (
                    allowlist_key in self.stdlib_allowlist
                    or self._is_internal_module(module_name)
                    or self._is_known_project_module(module_name)
                ):
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
                            imported_from=module_name,
                            normalized=normalized,
                            needs_bind=needs_bind,
                            force_bind=force_bind,
                            direct_registry_authorized=False,
                        )
                    )
                    if lowered_imported_call is not None:
                        return lowered_imported_call
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    res_hint = func_id if func_id in self.classes else "Any"
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                if enforce_allowlist:
                    suggestion = self._call_allowlist_suggestion(func_id, module_name)
                    if suggestion:
                        alternative = f"use {suggestion}"
                    else:
                        alternative = (
                            "import from an allowlisted module (see docs/spec/"
                            "areas/compat/surfaces/stdlib/stdlib_surface_matrix.md)"
                        )
                    detail = (
                        "Tier 0 only allows direct calls to allowlisted module-level"
                        " functions; rebinding/monkey-patching is not observed"
                    )
                    if suggestion:
                        detail = f"{detail}. warning: allowlisted path is {suggestion}"
                    if self.fallback_policy == "bridge":
                        self.compat.bridge_unavailable(
                            node,
                            f"call to non-allowlisted function '{func_id}'",
                            impact="high",
                            alternative=alternative,
                            detail=detail,
                        )
                        callee = self.visit(node.func)
                        if callee is None:
                            raise NotImplementedError("Unsupported call target")
                        res = MoltValue(self.next_var(), type_hint="Any")
                        if needs_bind:
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                        else:
                            args = self._emit_call_args(node.args)
                            self.emit(
                                MoltOp(
                                    kind="INVOKE_FFI",
                                    args=[callee] + args,
                                    result=res,
                                    metadata={"ffi_lane": "bridge"},
                                )
                            )
                        return res
                    raise self.compat.unsupported(
                        node,
                        f"call to non-allowlisted function '{func_id}'",
                        impact="high",
                        alternative=alternative,
                        detail=detail,
                    )
        return CALL_NOT_HANDLED

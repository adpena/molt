"""SerializationExceptionOpsMixin: JSON serialization for exception stack, exception object, control label, file, environment, and stderr ops."""

from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
    FrontendOpKind,
    frontend_wire_kind,
)


import os

from molt.frontend._types import (
    MoltOp,
)
from molt.frontend.lowering.serialization_context import (
    SerializationContext,
    SimpleIROp,
)
from molt.frontend.lowering.try_regions import try_region_id
from molt.frontend._mixin_base import GeneratorMixinBase


_SERIALIZE_EXCEPTION_OP_KINDS_1: frozenset[FrontendOpKind] = frozenset(
    {"TRY_START", "TRY_END"}
)


class SerializationExceptionOpsMixin(GeneratorMixinBase):
    def _serialize_exception_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == FrontendKinds.EXCEPTION_PUSH:
            ctx.json_ops.append({"kind": "exception_push", "out": op.result.name})
        elif op.kind == FrontendKinds.EXCEPTION_POP:
            ctx.json_ops.append({"kind": "exception_pop", "out": op.result.name})
        elif op.kind == FrontendKinds.EXCEPTION_STACK_CLEAR:
            ctx.json_ops.append(
                {"kind": "exception_stack_clear", "out": op.result.name}
            )
        elif op.kind == FrontendKinds.EXCEPTION_STACK_ENTER:
            ctx.json_ops.append(
                {"kind": "exception_stack_enter", "out": op.result.name}
            )
        elif op.kind == FrontendKinds.EXCEPTION_STACK_EXIT:
            ctx.json_ops.append(
                {
                    "kind": "exception_stack_exit",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_STACK_DEPTH:
            ctx.json_ops.append(
                {"kind": "exception_stack_depth", "out": op.result.name}
            )
        elif op.kind == FrontendKinds.EXCEPTION_STACK_SET_DEPTH:
            ctx.json_ops.append(
                {
                    "kind": "exception_stack_set_depth",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_LAST_PENDING:
            ctx.json_ops.append(
                {"kind": "exception_last_pending", "out": op.result.name}
            )
        elif op.kind == FrontendKinds.EXCEPTION_FINALLY_PENDING_OBSERVER:
            ctx.json_ops.append(
                {
                    "kind": "exception_finally_pending_observer",
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_NEW:
            ctx.json_ops.append(
                {
                    "kind": "exception_new",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_NEW_BUILTIN:
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_new_builtin",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                    "s_value": metadata["exception_name"],
                    "value": int(metadata["exception_tag"]),
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_NEW_BUILTIN_EMPTY:
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_new_builtin_empty",
                    "args": [],
                    "out": op.result.name,
                    "s_value": metadata["exception_name"],
                    "value": int(metadata["exception_tag"]),
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_NEW_BUILTIN_ONE:
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_new_builtin_one",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                    "s_value": metadata["exception_name"],
                    "value": int(metadata["exception_tag"]),
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_NEW_FROM_CLASS:
            ctx.json_ops.append(
                {
                    "kind": "exception_new_from_class",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTIONGROUP_MATCH:
            ctx.json_ops.append(
                {
                    "kind": "exceptiongroup_match",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTIONGROUP_COMBINE:
            ctx.json_ops.append(
                {
                    "kind": "exceptiongroup_combine",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_SET_LAST:
            ctx.json_ops.append(
                {
                    "kind": "exception_set_last",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_CONTEXT_SET:
            ctx.json_ops.append(
                {
                    "kind": "exception_context_set",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.EXCEPTION_CLEAR:
            ctx.json_ops.append({"kind": "exception_clear", "out": op.result.name})
        elif op.kind == FrontendKinds.EXCEPTION_CLASS:
            ctx.json_ops.append(
                {
                    "kind": "exception_class",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.RAISE:
            ctx.json_ops.append(
                {
                    "kind": "raise",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind in _SERIALIZE_EXCEPTION_OP_KINDS_1:
            payload: SimpleIROp = {"kind": frontend_wire_kind(op.kind)}
            if try_region_id(op) is not None:
                payload["value"] = self._serialization_control_value(op)
            ctx.json_ops.append(payload)
        elif op.kind == FrontendKinds.LABEL:
            ctx.json_ops.append(
                {"kind": "label", "value": self._serialization_control_value(op)}
            )
        elif op.kind == FrontendKinds.STATE_LABEL:
            ctx.json_ops.append(
                {"kind": "state_label", "value": self._serialization_control_value(op)}
            )
        elif op.kind == FrontendKinds.JUMP:
            ctx.json_ops.append(
                {"kind": "jump", "value": self._serialization_control_value(op)}
            )
        elif op.kind == FrontendKinds.PHI:
            ctx.json_ops.append(
                {
                    "kind": "phi",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CHECK_EXCEPTION:
            ctx.json_ops.append(
                {
                    "kind": "check_exception",
                    "value": self._serialization_control_value(op),
                }
            )
        elif op.kind == FrontendKinds.WARN_STDERR:
            if os.environ.get("MOLT_DEBUG_WARN"):
                import sys as _sys

                print(
                    f"[WARN_SERIALIZE] warn_stderr arg={op.args[0].name}",
                    file=_sys.stderr,
                )
            ctx.json_ops.append(
                {
                    "kind": "warn_stderr",
                    "args": [op.args[0].name],
                }
            )
        else:
            return False
        return True

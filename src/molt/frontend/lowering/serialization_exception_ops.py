"""SerializationExceptionOpsMixin: JSON serialization for exception stack, exception object, control label, file, environment, and stderr ops."""

from __future__ import annotations

import os
from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
    MoltOp,
)
from molt.frontend.lowering.serialization_context import SerializationContext

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class SerializationExceptionOpsMixin(_MixinBase):
    def _serialize_exception_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == "EXCEPTION_PUSH":
            ctx.json_ops.append({"kind": "exception_push", "out": op.result.name})
        elif op.kind == "EXCEPTION_POP":
            ctx.json_ops.append({"kind": "exception_pop", "out": op.result.name})
        elif op.kind == "EXCEPTION_STACK_CLEAR":
            ctx.json_ops.append(
                {"kind": "exception_stack_clear", "out": op.result.name}
            )
        elif op.kind == "EXCEPTION_STACK_ENTER":
            ctx.json_ops.append(
                {"kind": "exception_stack_enter", "out": op.result.name}
            )
        elif op.kind == "EXCEPTION_STACK_EXIT":
            ctx.json_ops.append(
                {
                    "kind": "exception_stack_exit",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_STACK_DEPTH":
            ctx.json_ops.append(
                {"kind": "exception_stack_depth", "out": op.result.name}
            )
        elif op.kind == "EXCEPTION_STACK_SET_DEPTH":
            ctx.json_ops.append(
                {
                    "kind": "exception_stack_set_depth",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_LAST":
            ctx.json_ops.append({"kind": "exception_last", "out": op.result.name})
        elif op.kind == "EXCEPTION_LAST_PENDING":
            ctx.json_ops.append(
                {"kind": "exception_last_pending", "out": op.result.name}
            )
        elif op.kind == "EXCEPTION_FINALLY_PENDING_OBSERVER":
            ctx.json_ops.append(
                {
                    "kind": "exception_finally_pending_observer",
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_NEW":
            ctx.json_ops.append(
                {
                    "kind": "exception_new",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_NEW_BUILTIN":
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_new_builtin",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                    "s_value": metadata.get("exception_name", "Exception"),
                    "value": int(metadata.get("exception_tag", 2)),
                }
            )
        elif op.kind == "EXCEPTION_NEW_BUILTIN_EMPTY":
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_new_builtin_empty",
                    "args": [],
                    "out": op.result.name,
                    "s_value": metadata.get("exception_name", "Exception"),
                    "value": int(metadata.get("exception_tag", 2)),
                }
            )
        elif op.kind == "EXCEPTION_NEW_BUILTIN_ONE":
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "exception_new_builtin_one",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                    "s_value": metadata.get("exception_name", "Exception"),
                    "value": int(metadata.get("exception_tag", 2)),
                }
            )
        elif op.kind == "EXCEPTION_NEW_FROM_CLASS":
            ctx.json_ops.append(
                {
                    "kind": "exception_new_from_class",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTIONGROUP_MATCH":
            ctx.json_ops.append(
                {
                    "kind": "exceptiongroup_match",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTIONGROUP_COMBINE":
            ctx.json_ops.append(
                {
                    "kind": "exceptiongroup_combine",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_SET_CAUSE":
            ctx.json_ops.append(
                {
                    "kind": "exception_set_cause",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_SET_LAST":
            ctx.json_ops.append(
                {
                    "kind": "exception_set_last",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_CONTEXT_SET":
            ctx.json_ops.append(
                {
                    "kind": "exception_context_set",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_CLEAR":
            ctx.json_ops.append({"kind": "exception_clear", "out": op.result.name})
        elif op.kind == "EXCEPTION_KIND":
            ctx.json_ops.append(
                {
                    "kind": "exception_kind",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_CLASS":
            ctx.json_ops.append(
                {
                    "kind": "exception_class",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "EXCEPTION_MESSAGE":
            ctx.json_ops.append(
                {
                    "kind": "exception_message",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "RAISE":
            ctx.json_ops.append(
                {
                    "kind": "raise",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TRY_START":
            payload: dict[str, Any] = {"kind": "try_start"}
            if op.args:
                payload["value"] = self._serialization_control_value(op)
            ctx.json_ops.append(payload)
        elif op.kind == "TRY_END":
            payload: dict[str, Any] = {"kind": "try_end"}
            if op.args:
                payload["value"] = self._serialization_control_value(op)
            ctx.json_ops.append(payload)
        elif op.kind == "LABEL":
            ctx.json_ops.append(
                {"kind": "label", "value": self._serialization_control_value(op)}
            )
        elif op.kind == "STATE_LABEL":
            ctx.json_ops.append(
                {"kind": "state_label", "value": self._serialization_control_value(op)}
            )
        elif op.kind == "JUMP":
            ctx.json_ops.append(
                {"kind": "jump", "value": self._serialization_control_value(op)}
            )
        elif op.kind == "PHI":
            ctx.json_ops.append(
                {
                    "kind": "phi",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CHECK_EXCEPTION":
            ctx.json_ops.append(
                {
                    "kind": "check_exception",
                    "value": self._serialization_control_value(op),
                }
            )
        elif op.kind == "FILE_OPEN":
            ctx.json_ops.append(
                {
                    "kind": "file_open",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FILE_READ":
            ctx.json_ops.append(
                {
                    "kind": "file_read",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FILE_WRITE":
            ctx.json_ops.append(
                {
                    "kind": "file_write",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FILE_CLOSE":
            ctx.json_ops.append(
                {
                    "kind": "file_close",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FILE_FLUSH":
            ctx.json_ops.append(
                {
                    "kind": "file_flush",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ENV_GET":
            ctx.json_ops.append(
                {
                    "kind": "env_get",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "PRINT":
            ctx.json_ops.append(
                {
                    "kind": "print",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                }
            )
        elif op.kind == "PRINT_NEWLINE":
            ctx.json_ops.append({"kind": "print_newline"})
        elif op.kind == "WARN_STDERR":
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

"""SerializationLoopStringAsyncOpsMixin: JSON serialization for index mutation, loop/vector, bytes/string, async, generator, closure, and GPU placeholder ops."""

from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
    WireKinds,
    WireOpKind,
)

from typing import Any

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.serialization_context import (
    SerializationContext,
    SimpleIROp,
)
from molt.frontend._mixin_base import GeneratorMixinBase


_SERIALIZE_LOOP_STRING_ASYNC_OP_KINDS_1: frozenset[WireOpKind] = frozenset(
    {"bool", "int"}
)


class SerializationLoopStringAsyncOpsMixin(GeneratorMixinBase):
    def _serialize_loop_string_async_op(
        self, op: MoltOp, ctx: SerializationContext
    ) -> bool:
        if op.kind == FrontendKinds.STORE_INDEX:
            si_entry: SimpleIROp = {
                "kind": "store_index",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # Flat i64-list storage is tracked structurally from
            # list_int_new; do not encode it as container_type metadata.
            if (
                len(op.args) >= 1
                and isinstance(op.args[0], MoltValue)
                and op.args[0].name in ctx.json_list_int_containers
            ):
                value_hint = (
                    op.args[2].type_hint
                    if len(op.args) >= 3 and isinstance(op.args[2], MoltValue)
                    else None
                )
                if value_hint == WireKinds.INT:
                    if op.result.name != "none":
                        ctx.json_list_int_containers.add(op.result.name)
                else:
                    ctx.json_list_int_containers.discard(op.args[0].name)
                    if op.result.name != "none":
                        ctx.json_list_int_containers.discard(op.result.name)
            elif (
                len(op.args) >= 1
                and isinstance(op.args[0], MoltValue)
                and op.args[0].type_hint == "dict"
            ):
                si_entry["container_type"] = "dict"
            elif (
                len(op.args) >= 1
                and isinstance(op.args[0], MoltValue)
                and op.args[0].type_hint == "list"
            ):
                si_entry["container_type"] = "list"
            ctx.json_ops.append(si_entry)
        elif op.kind == FrontendKinds.DEL_INDEX:
            ctx.json_ops.append(
                {
                    "kind": "del_index",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LOOP_START:
            ctx.json_ops.append({"kind": "loop_start"})
        elif op.kind == FrontendKinds.LOOP_INDEX_START:
            ctx.json_ops.append(
                {
                    "kind": "loop_index_start",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LOOP_INDEX_NEXT:
            ctx.json_ops.append(
                {
                    "kind": "loop_index_next",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LOOP_BREAK_IF_TRUE:
            _lbit_entry: SimpleIROp = {
                "kind": "loop_break_if_true",
                "args": [op.args[0].name],
            }
            _lbit_cond = op.args[0]
            if (
                isinstance(_lbit_cond, MoltValue)
                and _lbit_cond.type_hint in _SERIALIZE_LOOP_STRING_ASYNC_OP_KINDS_1
            ):
                _lbit_entry["type_hint"] = _lbit_cond.type_hint
            ctx.json_ops.append(_lbit_entry)
        elif op.kind == FrontendKinds.LOOP_BREAK_IF_FALSE:
            _lbif_entry: SimpleIROp = {
                "kind": "loop_break_if_false",
                "args": [op.args[0].name],
            }
            _lbif_cond = op.args[0]
            if (
                isinstance(_lbif_cond, MoltValue)
                and _lbif_cond.type_hint in _SERIALIZE_LOOP_STRING_ASYNC_OP_KINDS_1
            ):
                _lbif_entry["type_hint"] = _lbif_cond.type_hint
            ctx.json_ops.append(_lbif_entry)
        elif op.kind == FrontendKinds.LOOP_BREAK:
            ctx.json_ops.append({"kind": "loop_break"})
        elif op.kind == FrontendKinds.LOOP_CONTINUE:
            ctx.json_ops.append({"kind": "loop_continue"})
        elif op.kind == FrontendKinds.LOOP_END:
            ctx.json_ops.append({"kind": "loop_end"})
        elif op.kind == FrontendKinds.VEC_SUM:
            ctx.json_ops.append(
                {
                    "kind": "vec_sum",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.VEC_PROD:
            ctx.json_ops.append(
                {
                    "kind": "vec_prod",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.VEC_MIN:
            ctx.json_ops.append(
                {
                    "kind": "vec_min",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.VEC_MAX:
            ctx.json_ops.append(
                {
                    "kind": "vec_max",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SLICE:
            ctx.json_ops.append(
                {
                    "kind": "slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SLICE_NEW:
            ctx.json_ops.append(
                {
                    "kind": "slice_new",
                    "args": [
                        arg.name if arg is not None else "_molt_none" for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_FIND:
            ctx.json_ops.append(
                {
                    "kind": "bytes_find",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_FIND_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytes_find_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_FIND:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_find",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_FIND_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_find_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_STARTSWITH:
            ctx.json_ops.append(
                {
                    "kind": "bytes_startswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_STARTSWITH_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytes_startswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_STARTSWITH:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_startswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_STARTSWITH_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_startswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_ENDSWITH:
            ctx.json_ops.append(
                {
                    "kind": "bytes_endswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_ENDSWITH_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytes_endswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_ENDSWITH:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_endswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_ENDSWITH_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_endswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_COUNT:
            ctx.json_ops.append(
                {
                    "kind": "bytes_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_COUNT:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_COUNT_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytes_count_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_COUNT_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_count_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STR_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "str_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.OPERATOR_INDEX:
            ctx.json_ops.append(
                {
                    "kind": "operator_index",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.REPR_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "repr_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.ASCII_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "ascii_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_FIND:
            ctx.json_ops.append(
                {
                    "kind": "string_find",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_FIND_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "string_find_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_FORMAT:
            ctx.json_ops.append(
                {
                    "kind": "string_format",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_STARTSWITH:
            ctx.json_ops.append(
                {
                    "kind": "string_startswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_STARTSWITH_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "string_startswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_ENDSWITH:
            ctx.json_ops.append(
                {
                    "kind": "string_endswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_ENDSWITH_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "string_endswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_COUNT:
            ctx.json_ops.append(
                {
                    "kind": "string_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_COUNT_SLICE:
            ctx.json_ops.append(
                {
                    "kind": "string_count_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_JOIN:
            ctx.json_ops.append(
                {
                    "kind": "string_join",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_SPLIT:
            ctx.json_ops.append(
                {
                    "kind": "string_split",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_SPLIT_MAX:
            ctx.json_ops.append(
                {
                    "kind": "string_split_max",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_LOWER:
            ctx.json_ops.append(
                {
                    "kind": "string_lower",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_UPPER:
            ctx.json_ops.append(
                {
                    "kind": "string_upper",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_CAPITALIZE:
            ctx.json_ops.append(
                {
                    "kind": "string_capitalize",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_STRIP:
            ctx.json_ops.append(
                {
                    "kind": "string_strip",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_LSTRIP:
            ctx.json_ops.append(
                {
                    "kind": "string_lstrip",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_RSTRIP:
            ctx.json_ops.append(
                {
                    "kind": "string_rstrip",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_REPLACE:
            ctx.json_ops.append(
                {
                    "kind": "string_replace",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_SPLIT:
            ctx.json_ops.append(
                {
                    "kind": "bytes_split",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_SPLIT_MAX:
            ctx.json_ops.append(
                {
                    "kind": "bytes_split_max",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_SPLIT:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_split",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_SPLIT_MAX:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_split_max",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_REPLACE:
            ctx.json_ops.append(
                {
                    "kind": "bytes_replace",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_REPLACE:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_replace",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STORE_VAR:
            var_name = op.metadata["var"] if op.metadata else ""
            ctx.json_ops.append(
                {
                    "kind": "store_var",
                    "var": var_name,
                    "args": [op.args[0].name],
                }
            )
        elif op.kind == FrontendKinds.LOAD_VAR:
            var_name = op.metadata["var"] if op.metadata else ""
            ctx.json_ops.append(
                {
                    "kind": "load_var",
                    "var": var_name,
                    "out": op.result.name,
                }
            )
            # Propagate list_int container hint through load_var:
            # if the variable being loaded is a list_int container,
            # the output name is also a list_int container.
            if var_name in ctx.json_list_int_containers:
                ctx.json_list_int_containers.add(op.result.name)
        elif op.kind == FrontendKinds.ret:
            ctx.json_ops.append({"kind": "ret", "args": [op.args[0].name]})
        elif op.kind == FrontendKinds.ret_void:
            ctx.json_ops.append({"kind": "ret_void"})
        elif op.kind == FrontendKinds.STATE_SWITCH:
            ctx.json_ops.append({"kind": "state_switch"})
        elif op.kind == FrontendKinds.STATE_TRANSITION:
            if len(op.args) == 3:
                future, pending_state, next_state = op.args
                slot_arg = None
            else:
                future, slot_arg, pending_state, next_state = op.args
            args = [future.name]
            if slot_arg is not None:
                args.append(slot_arg.name)
            args.append(pending_state.name)
            ctx.json_ops.append(
                {
                    "kind": "state_transition",
                    "args": args,
                    "value": next_state,
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STATE_YIELD:
            value, next_state = op.args
            ctx.json_ops.append(
                {
                    "kind": "state_yield",
                    "args": [value.name],
                    "value": next_state,
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STATE_SET:
            (state,) = op.args
            ctx.json_ops.append({"kind": "state_set", "value": state})
        elif op.kind == FrontendKinds.GEN_SEND:
            gen, val = op.args
            ctx.json_ops.append(
                {
                    "kind": "gen_send",
                    "args": [gen.name, val.name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.GEN_CLOSE:
            ctx.json_ops.append(
                {
                    "kind": "gen_close",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.IS_GENERATOR:
            ctx.json_ops.append(
                {
                    "kind": "is_generator",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.IS_CALLABLE:
            ctx.json_ops.append(
                {
                    "kind": "is_callable",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LOAD_CLOSURE:
            self_ptr, offset = op.args
            ctx.json_ops.append(
                {
                    "kind": "closure_load",
                    "args": [self_ptr],
                    "value": offset,
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STORE_CLOSURE:
            self_ptr, offset, val = op.args
            ctx.json_ops.append(
                {
                    "kind": "closure_store",
                    "args": [self_ptr, val.name],
                    "value": offset,
                }
            )
        # ── GPU intrinsic ops ──
        else:
            return False
        return True

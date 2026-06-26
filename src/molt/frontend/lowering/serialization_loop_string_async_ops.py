"""SerializationLoopStringAsyncOpsMixin: JSON serialization for index mutation, loop/vector, bytes/string, async, generator, closure, and GPU placeholder ops."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.serialization_context import SerializationContext

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class SerializationLoopStringAsyncOpsMixin(_MixinBase):
    def _serialize_loop_string_async_op(
        self, op: MoltOp, ctx: SerializationContext
    ) -> bool:
        if op.kind == "STORE_INDEX":
            si_entry: dict[str, Any] = {
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
                if value_hint == "int":
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
        elif op.kind == "DEL_INDEX":
            ctx.json_ops.append(
                {
                    "kind": "del_index",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LOOP_START":
            ctx.json_ops.append({"kind": "loop_start"})
        elif op.kind == "LOOP_INDEX_START":
            ctx.json_ops.append(
                {
                    "kind": "loop_index_start",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LOOP_INDEX_NEXT":
            ctx.json_ops.append(
                {
                    "kind": "loop_index_next",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LOOP_BREAK_IF_TRUE":
            _lbit_entry: dict[str, Any] = {
                "kind": "loop_break_if_true",
                "args": [op.args[0].name],
            }
            _lbit_cond = op.args[0]
            if isinstance(_lbit_cond, MoltValue) and _lbit_cond.type_hint in {
                "int",
                "bool",
            }:
                _lbit_entry["type_hint"] = _lbit_cond.type_hint
            ctx.json_ops.append(_lbit_entry)
        elif op.kind == "LOOP_BREAK_IF_FALSE":
            _lbif_entry: dict[str, Any] = {
                "kind": "loop_break_if_false",
                "args": [op.args[0].name],
            }
            _lbif_cond = op.args[0]
            if isinstance(_lbif_cond, MoltValue) and _lbif_cond.type_hint in {
                "int",
                "bool",
            }:
                _lbif_entry["type_hint"] = _lbif_cond.type_hint
            ctx.json_ops.append(_lbif_entry)
        elif op.kind == "LOOP_BREAK_IF_EXCEPTION":
            # Control op (no value arg) that breaks the loop when a runtime
            # exception is pending.  Lowers to the sacrosanct
            # `molt_exception_pending_fast` flag read in every backend, so
            # it can never be folded/copy-propagated away like a value op.
            ctx.json_ops.append({"kind": "loop_break_if_exception"})
        elif op.kind == "LOOP_BREAK":
            ctx.json_ops.append({"kind": "loop_break"})
        elif op.kind == "LOOP_CONTINUE":
            ctx.json_ops.append({"kind": "loop_continue"})
        elif op.kind == "LOOP_END":
            ctx.json_ops.append({"kind": "loop_end"})
        elif op.kind == "VEC_SUM_INT":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_int",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_INT_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_int_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_INT_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_int_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_INT_RANGE_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_int_range_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_INT_RANGE_ITER":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_int_range_iter",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_INT_RANGE_ITER_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_int_range_iter_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_FLOAT":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_float",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_FLOAT_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_float_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_FLOAT_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_float_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_FLOAT_RANGE_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_float_range_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_FLOAT_RANGE_ITER":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_float_range_iter",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_SUM_FLOAT_RANGE_ITER_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_sum_float_range_iter_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_PROD_INT":
            ctx.json_ops.append(
                {
                    "kind": "vec_prod_int",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_PROD_INT_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_prod_int_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_PROD_INT_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "vec_prod_int_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_PROD_INT_RANGE_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_prod_int_range_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MIN_INT":
            ctx.json_ops.append(
                {
                    "kind": "vec_min_int",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MIN_INT_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_min_int_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MIN_INT_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "vec_min_int_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MIN_INT_RANGE_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_min_int_range_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MAX_INT":
            ctx.json_ops.append(
                {
                    "kind": "vec_max_int",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MAX_INT_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_max_int_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MAX_INT_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "vec_max_int_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "VEC_MAX_INT_RANGE_TRUSTED":
            ctx.json_ops.append(
                {
                    "kind": "vec_max_int_range_trusted",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SLICE":
            ctx.json_ops.append(
                {
                    "kind": "slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SLICE_NEW":
            ctx.json_ops.append(
                {
                    "kind": "slice_new",
                    "args": [
                        arg.name if arg is not None else "_molt_none" for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_FIND":
            ctx.json_ops.append(
                {
                    "kind": "bytes_find",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_FIND_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytes_find_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_FIND":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_find",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_FIND_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_find_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_STARTSWITH":
            ctx.json_ops.append(
                {
                    "kind": "bytes_startswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_STARTSWITH_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytes_startswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_STARTSWITH":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_startswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_STARTSWITH_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_startswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_ENDSWITH":
            ctx.json_ops.append(
                {
                    "kind": "bytes_endswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_ENDSWITH_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytes_endswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_ENDSWITH":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_endswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_ENDSWITH_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_endswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_COUNT":
            ctx.json_ops.append(
                {
                    "kind": "bytes_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_COUNT":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_COUNT_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytes_count_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_COUNT_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_count_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STR_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "str_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "REPR_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "repr_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ASCII_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "ascii_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_FIND":
            ctx.json_ops.append(
                {
                    "kind": "string_find",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_FIND_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "string_find_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_FORMAT":
            ctx.json_ops.append(
                {
                    "kind": "string_format",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BUFFER2D_NEW":
            ctx.json_ops.append(
                {
                    "kind": "buffer2d_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BUFFER2D_GET":
            ctx.json_ops.append(
                {
                    "kind": "buffer2d_get",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BUFFER2D_SET":
            ctx.json_ops.append(
                {
                    "kind": "buffer2d_set",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BUFFER2D_MATMUL":
            ctx.json_ops.append(
                {
                    "kind": "buffer2d_matmul",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_STARTSWITH":
            ctx.json_ops.append(
                {
                    "kind": "string_startswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_STARTSWITH_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "string_startswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_ENDSWITH":
            ctx.json_ops.append(
                {
                    "kind": "string_endswith",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_ENDSWITH_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "string_endswith_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_COUNT":
            ctx.json_ops.append(
                {
                    "kind": "string_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_COUNT_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "string_count_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_JOIN":
            ctx.json_ops.append(
                {
                    "kind": "string_join",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_SPLIT":
            ctx.json_ops.append(
                {
                    "kind": "string_split",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_SPLIT_MAX":
            ctx.json_ops.append(
                {
                    "kind": "string_split_max",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_LOWER":
            ctx.json_ops.append(
                {
                    "kind": "string_lower",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_UPPER":
            ctx.json_ops.append(
                {
                    "kind": "string_upper",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_CAPITALIZE":
            ctx.json_ops.append(
                {
                    "kind": "string_capitalize",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_STRIP":
            ctx.json_ops.append(
                {
                    "kind": "string_strip",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_LSTRIP":
            ctx.json_ops.append(
                {
                    "kind": "string_lstrip",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_RSTRIP":
            ctx.json_ops.append(
                {
                    "kind": "string_rstrip",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_REPLACE":
            ctx.json_ops.append(
                {
                    "kind": "string_replace",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_SPLIT":
            ctx.json_ops.append(
                {
                    "kind": "bytes_split",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_SPLIT_MAX":
            ctx.json_ops.append(
                {
                    "kind": "bytes_split_max",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_SPLIT":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_split",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_SPLIT_MAX":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_split_max",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STATISTICS_MEAN_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "statistics_mean_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STATISTICS_STDEV_SLICE":
            ctx.json_ops.append(
                {
                    "kind": "statistics_stdev_slice",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_REPLACE":
            ctx.json_ops.append(
                {
                    "kind": "bytes_replace",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_REPLACE":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_replace",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ASYNC_BLOCK_ON":
            ctx.json_ops.append(
                {
                    "kind": "block_on",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALL_DUMMY":
            ctx.json_ops.append({"kind": "const", "value": 0, "out": op.result.name})
        elif op.kind == "BRIDGE_UNAVAILABLE":
            ctx.json_ops.append(
                {
                    "kind": "bridge_unavailable",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STORE_VAR":
            var_name = op.metadata["var"] if op.metadata else ""
            ctx.json_ops.append(
                {
                    "kind": "store_var",
                    "var": var_name,
                    "args": [op.args[0].name],
                }
            )
        elif op.kind == "DELETE_VAR":
            var_name = op.metadata["var"] if op.metadata else ""
            ctx.json_ops.append(
                {
                    "kind": "delete_var",
                    "var": var_name,
                    "args": [arg.name for arg in op.args],
                }
            )
        elif op.kind == "LOAD_VAR":
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
        elif op.kind == "ret":
            if ctx.emit_function_frame:
                ctx.json_ops.append({"kind": "trace_exit"})
            ctx.json_ops.append({"kind": "ret", "var": op.args[0].name})
        elif op.kind == "ret_void":
            if ctx.emit_function_frame:
                ctx.json_ops.append({"kind": "trace_exit"})
            ctx.json_ops.append({"kind": "ret_void"})
        elif op.kind == "ALLOC_TASK":
            poll_func = self._require_async_poll_target("ALLOC_TASK", op.args[0])
            size = op.args[1]
            args = op.args[2:]
            task_kind = op.metadata.get("task_kind") if op.metadata else None
            if task_kind not in {"future", "generator", "coroutine"}:
                raise ValueError(
                    f"ALLOC_TASK missing task_kind metadata: {task_kind!r}"
                )
            ctx.json_ops.append(
                {
                    "kind": "alloc_task",
                    "s_value": poll_func,
                    "value": size,
                    "task_kind": task_kind,
                    "args": [arg.name for arg in args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ASYNCGEN_NEW":
            ctx.json_ops.append(
                {
                    "kind": "asyncgen_new",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ASYNCGEN_SHUTDOWN":
            ctx.json_ops.append(
                {
                    "kind": "asyncgen_shutdown",
                    "out": op.result.name,
                }
            )
        elif op.kind == "STATE_SWITCH":
            ctx.json_ops.append({"kind": "state_switch"})
        elif op.kind == "STATE_TRANSITION":
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
        elif op.kind == "STATE_YIELD":
            pair, next_state = op.args
            ctx.json_ops.append(
                {
                    "kind": "state_yield",
                    "args": [pair.name],
                    "value": next_state,
                    "out": op.result.name,
                }
            )
        elif op.kind == "SPAWN":
            ctx.json_ops.append({"kind": "spawn", "args": [op.args[0].name]})
        elif op.kind == "CANCEL_TOKEN_NEW":
            ctx.json_ops.append(
                {
                    "kind": "cancel_token_new",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCEL_TOKEN_CLONE":
            ctx.json_ops.append(
                {
                    "kind": "cancel_token_clone",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCEL_TOKEN_DROP":
            ctx.json_ops.append(
                {
                    "kind": "cancel_token_drop",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCEL_TOKEN_CANCEL":
            ctx.json_ops.append(
                {
                    "kind": "cancel_token_cancel",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FUTURE_CANCEL":
            ctx.json_ops.append(
                {
                    "kind": "future_cancel",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FUTURE_CANCEL_MSG":
            ctx.json_ops.append(
                {
                    "kind": "future_cancel_msg",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FUTURE_CANCEL_CLEAR":
            ctx.json_ops.append(
                {
                    "kind": "future_cancel_clear",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "PROMISE_NEW":
            ctx.json_ops.append(
                {
                    "kind": "promise_new",
                    "out": op.result.name,
                }
            )
        elif op.kind == "PROMISE_SET_RESULT":
            ctx.json_ops.append(
                {
                    "kind": "promise_set_result",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "PROMISE_SET_EXCEPTION":
            ctx.json_ops.append(
                {
                    "kind": "promise_set_exception",
                    "args": [op.args[0].name, op.args[1].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "THREAD_SUBMIT":
            ctx.json_ops.append(
                {
                    "kind": "thread_submit",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TASK_REGISTER_TOKEN_OWNED":
            ctx.json_ops.append(
                {
                    "kind": "task_register_token_owned",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCEL_TOKEN_IS_CANCELLED":
            ctx.json_ops.append(
                {
                    "kind": "cancel_token_is_cancelled",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCEL_TOKEN_SET_CURRENT":
            ctx.json_ops.append(
                {
                    "kind": "cancel_token_set_current",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCEL_TOKEN_GET_CURRENT":
            ctx.json_ops.append(
                {
                    "kind": "cancel_token_get_current",
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCELLED":
            ctx.json_ops.append(
                {
                    "kind": "cancelled",
                    "out": op.result.name,
                }
            )
        elif op.kind == "CANCEL_CURRENT":
            ctx.json_ops.append(
                {
                    "kind": "cancel_current",
                    "out": op.result.name,
                }
            )
        elif op.kind == "CHAN_NEW":
            ctx.json_ops.append(
                {
                    "kind": "chan_new",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CHAN_SEND_YIELD":
            chan, val, pending_state, next_state = op.args
            ctx.json_ops.append(
                {
                    "kind": "chan_send_yield",
                    "args": [chan.name, val.name, pending_state.name],
                    "value": next_state,
                    "out": op.result.name,
                }
            )
        elif op.kind == "CHAN_RECV_YIELD":
            chan, pending_state, next_state = op.args
            ctx.json_ops.append(
                {
                    "kind": "chan_recv_yield",
                    "args": [chan.name, pending_state.name],
                    "value": next_state,
                    "out": op.result.name,
                }
            )
        elif op.kind == "CHAN_DROP":
            ctx.json_ops.append(
                {
                    "kind": "chan_drop",
                    "args": [op.args[0].name],
                }
            )
        elif op.kind == "CALL_ASYNC":
            poll_name = self._require_async_poll_target("CALL_ASYNC", op.args[0])
            payload_args = op.args[1:] if len(op.args) > 1 else []
            ctx.json_ops.append(
                {
                    "kind": "call_async",
                    "s_value": poll_name,
                    "args": [arg.name for arg in payload_args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GEN_SEND":
            gen, val = op.args
            ctx.json_ops.append(
                {
                    "kind": "gen_send",
                    "args": [gen.name, val.name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GEN_THROW":
            gen, val = op.args
            ctx.json_ops.append(
                {
                    "kind": "gen_throw",
                    "args": [gen.name, val.name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "GEN_CLOSE":
            ctx.json_ops.append(
                {
                    "kind": "gen_close",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "IS_GENERATOR":
            ctx.json_ops.append(
                {
                    "kind": "is_generator",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "IS_BOUND_METHOD":
            ctx.json_ops.append(
                {
                    "kind": "is_bound_method",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "IS_CALLABLE":
            ctx.json_ops.append(
                {
                    "kind": "is_callable",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LOAD_CLOSURE":
            self_ptr, offset = op.args
            ctx.json_ops.append(
                {
                    "kind": "closure_load",
                    "args": [self_ptr],
                    "value": offset,
                    "out": op.result.name,
                }
            )
        elif op.kind == "STORE_CLOSURE":
            self_ptr, offset, val = op.args
            ctx.json_ops.append(
                {
                    "kind": "closure_store",
                    "args": [self_ptr, val.name],
                    "value": offset,
                }
            )
        # ── GPU intrinsic ops ──
        elif op.kind in (
            "gpu_thread_id",
            "gpu_block_id",
            "gpu_block_dim",
            "gpu_grid_dim",
            "gpu_barrier",
        ):
            ctx.json_ops.append({"kind": op.kind, "out": op.result.name})
        else:
            return False
        return True

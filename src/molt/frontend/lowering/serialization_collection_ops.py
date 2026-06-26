"""SerializationCollectionOpsMixin: JSON serialization for container construction, dict/set/list, iter, index, and unpack ops."""

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


class SerializationCollectionOpsMixin(_MixinBase):
    def _serialize_collection_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == "LEN":
            len_entry: dict[str, object] = {
                "kind": "len",
                "args": [
                    arg.name if hasattr(arg, "name") else str(arg) for arg in op.args
                ],
                "out": op.result.name,
            }
            len_arg = op.args[0]
            if isinstance(len_arg, MoltValue) and len_arg.type_hint in {
                "list",
                "str",
                "dict",
                "tuple",
                "set",
                "frozenset",
                "bytes",
            }:
                len_entry["container_type"] = len_arg.type_hint
            ctx.json_ops.append(len_entry)
        elif op.kind == "ID":
            ctx.json_ops.append(
                {
                    "kind": "id",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ORD":
            ctx.json_ops.append(
                {
                    "kind": "ord",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ORD_AT":
            ctx.json_ops.append(
                {
                    "kind": "ord_at",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CHR":
            ctx.json_ops.append(
                {
                    "kind": "chr",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
                    ],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALLARGS_NEW":
            ctx.json_ops.append(
                {
                    "kind": "callargs_new",
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALLARGS_PUSH_POS":
            ctx.json_ops.append(
                {
                    "kind": "callargs_push_pos",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALLARGS_PUSH_KW":
            ctx.json_ops.append(
                {
                    "kind": "callargs_push_kw",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALLARGS_EXPAND_STAR":
            ctx.json_ops.append(
                {
                    "kind": "callargs_expand_star",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALLARGS_EXPAND_KWSTAR":
            ctx.json_ops.append(
                {
                    "kind": "callargs_expand_kwstar",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_NEW":
            _list_op: dict[str, Any] = {
                "kind": "list_new",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
                "type_hint": "list",
            }
            # Named-local fact (#58): a container literal bound to a local
            # carries the Python scope boundary for any finalizer-bearing
            # element it absorbs.
            ctx.json_ops.append(self._serialization_carry_bound_local(op, _list_op))
        elif op.kind == "LIST_INT_NEW":
            # Specialized flat i64 list: args are [count, fill_value]
            ctx.json_ops.append(
                {
                    "kind": "list_int_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
            ctx.json_list_int_containers.add(op.result.name)
        elif op.kind == "LIST_FILL_NEW":
            ctx.json_ops.append(
                {
                    "kind": "list_fill_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "RANGE_NEW":
            ctx.json_ops.append(
                {
                    "kind": "range_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_FROM_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "list_from_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TUPLE_NEW":
            _tuple_op: dict[str, Any] = {
                "kind": "tuple_new",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
                "type_hint": "tuple",
            }
            ctx.json_ops.append(self._serialization_carry_bound_local(op, _tuple_op))
        elif op.kind == "LIST_APPEND":
            ctx.json_ops.append(
                {
                    "kind": "list_append",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                    "type_hint": "list",
                }
            )
        elif op.kind == "LIST_POP":
            ctx.json_ops.append(
                {
                    "kind": "list_pop",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_EXTEND":
            ctx.json_ops.append(
                {
                    "kind": "list_extend",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_INSERT":
            ctx.json_ops.append(
                {
                    "kind": "list_insert",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_REMOVE":
            ctx.json_ops.append(
                {
                    "kind": "list_remove",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_CLEAR":
            ctx.json_ops.append(
                {
                    "kind": "list_clear",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_COPY":
            ctx.json_ops.append(
                {
                    "kind": "list_copy",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
            if (
                op.args
                and isinstance(op.args[0], MoltValue)
                and op.args[0].name in ctx.json_list_int_containers
            ):
                ctx.json_list_int_containers.add(op.result.name)
        elif op.kind == "LIST_REVERSE":
            ctx.json_ops.append(
                {
                    "kind": "list_reverse",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_COUNT":
            ctx.json_ops.append(
                {
                    "kind": "list_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_INDEX":
            ctx.json_ops.append(
                {
                    "kind": "list_index",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LIST_INDEX_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "list_index_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TUPLE_FROM_LIST":
            ctx.json_ops.append(
                {
                    "kind": "tuple_from_list",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "bytes_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTES_FROM_STR":
            ctx.json_ops.append(
                {
                    "kind": "bytes_from_str",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_FROM_STR":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_from_str",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BYTEARRAY_FILL_RANGE":
            ctx.json_ops.append(
                {
                    "kind": "bytearray_fill_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "INTARRAY_FROM_SEQ":
            ctx.json_ops.append(
                {
                    "kind": "intarray_from_seq",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FLOAT_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "float_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "INT_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "int_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "INT_FROM_STR_OF_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "int_from_str_of_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "COMPLEX_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "complex_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MEMORYVIEW_NEW":
            ctx.json_ops.append(
                {
                    "kind": "memoryview_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "MEMORYVIEW_TOBYTES":
            ctx.json_ops.append(
                {
                    "kind": "memoryview_tobytes",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_NEW":
            ctx.json_ops.append(
                self._serialization_carry_bound_local(
                    op,
                    {
                        "kind": "dict_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                        "type_hint": "dict",
                    },
                )
            )
        elif op.kind == "DICT_FROM_OBJ":
            ctx.json_ops.append(
                {
                    "kind": "dict_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_NEW":
            ctx.json_ops.append(
                self._serialization_carry_bound_local(
                    op,
                    {
                        "kind": "set_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                        "type_hint": "set",
                    },
                )
            )
        elif op.kind == "FROZENSET_NEW":
            ctx.json_ops.append(
                self._serialization_carry_bound_local(
                    op,
                    {
                        "kind": "frozenset_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                        "type_hint": "frozenset",
                    },
                )
            )
        elif op.kind == "DICT_GET":
            ctx.json_ops.append(
                {
                    "kind": "dict_get",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_INC":
            ctx.json_ops.append(
                {
                    "kind": "dict_inc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_STR_INT_INC":
            ctx.json_ops.append(
                {
                    "kind": "dict_str_int_inc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_SPLIT_WS_DICT_INC":
            ctx.json_ops.append(
                {
                    "kind": "string_split_ws_dict_inc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "STRING_SPLIT_SEP_DICT_INC":
            ctx.json_ops.append(
                {
                    "kind": "string_split_sep_dict_inc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TAQ_INGEST_LINE":
            ctx.json_ops.append(
                {
                    "kind": "taq_ingest_line",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_POP":
            ctx.json_ops.append(
                {
                    "kind": "dict_pop",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_SET":
            ds_entry: dict[str, Any] = {
                "kind": "dict_set",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # fast_int: when the key is a known int (list subscript store),
            # use molt_list_setitem_int_fast in the backend.
            if (
                len(op.args) >= 2
                and self._hints_enabled()
                and isinstance(op.args[1], MoltValue)
                and op.args[1].type_hint in {"int", "bool"}
            ):
                ds_entry["fast_int"] = True
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
                and op.args[0].type_hint == "list"
            ):
                ds_entry["container_type"] = "list"
            ctx.json_ops.append(ds_entry)
        elif op.kind == "DICT_SETDEFAULT":
            ctx.json_ops.append(
                {
                    "kind": "dict_setdefault",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_SETDEFAULT_EMPTY_LIST":
            ctx.json_ops.append(
                {
                    "kind": "dict_setdefault_empty_list",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_UPDATE":
            ctx.json_ops.append(
                {
                    "kind": "dict_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_UPDATE_MISSING":
            ctx.json_ops.append(
                {
                    "kind": "dict_update_missing",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_UPDATE_KWSTAR":
            ctx.json_ops.append(
                {
                    "kind": "dict_update_kwstar",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_CLEAR":
            ctx.json_ops.append(
                {
                    "kind": "dict_clear",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_COPY":
            ctx.json_ops.append(
                {
                    "kind": "dict_copy",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_POPITEM":
            ctx.json_ops.append(
                {
                    "kind": "dict_popitem",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_ADD":
            ctx.json_ops.append(
                {
                    "kind": "set_add",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_ADD_PROBE":
            ctx.json_ops.append(
                {
                    "kind": "set_add_probe",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "FROZENSET_ADD":
            ctx.json_ops.append(
                {
                    "kind": "frozenset_add",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_DISCARD":
            ctx.json_ops.append(
                {
                    "kind": "set_discard",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_REMOVE":
            ctx.json_ops.append(
                {
                    "kind": "set_remove",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_POP":
            ctx.json_ops.append(
                {
                    "kind": "set_pop",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_UPDATE":
            ctx.json_ops.append(
                {
                    "kind": "set_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_INTERSECTION_UPDATE":
            ctx.json_ops.append(
                {
                    "kind": "set_intersection_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_DIFFERENCE_UPDATE":
            ctx.json_ops.append(
                {
                    "kind": "set_difference_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "SET_SYMDIFF_UPDATE":
            ctx.json_ops.append(
                {
                    "kind": "set_symdiff_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_KEYS":
            ctx.json_ops.append(
                {
                    "kind": "dict_keys",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_VALUES":
            ctx.json_ops.append(
                {
                    "kind": "dict_values",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "DICT_ITEMS":
            ctx.json_ops.append(
                {
                    "kind": "dict_items",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TUPLE_COUNT":
            ctx.json_ops.append(
                {
                    "kind": "tuple_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TUPLE_INDEX":
            ctx.json_ops.append(
                {
                    "kind": "tuple_index",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ITER_NEW":
            ctx.json_ops.append(
                {
                    "kind": "iter",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                    "type_hint": "iter",
                }
            )
        elif op.kind == "ENUMERATE":
            ctx.json_ops.append(
                {
                    "kind": "enumerate",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "AITER":
            ctx.json_ops.append(
                {
                    "kind": "aiter",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ITER_NEXT":
            ctx.json_ops.append(
                {
                    "kind": "iter_next",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ANEXT":
            ctx.json_ops.append(
                {
                    "kind": "anext",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "INDEX":
            index_entry: dict[str, Any] = {
                "kind": "index",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # fast_int: when the index argument is a known int,
            # the backend can use molt_list_getitem_int_fast which
            # skips full type dispatch and bounds-checks directly.
            if (
                len(op.args) == 2
                and self._hints_enabled()
                and isinstance(op.args[1], MoltValue)
                and op.args[1].type_hint in {"int", "bool"}
            ):
                index_entry["fast_int"] = True
            list_int_container = (
                len(op.args) >= 1
                and isinstance(op.args[0], MoltValue)
                and op.args[0].name in ctx.json_list_int_containers
            )
            if (
                not list_int_container
                and len(op.args) >= 1
                and isinstance(op.args[0], MoltValue)
                and op.args[0].type_hint == "dict"
            ):
                index_entry["container_type"] = "dict"
            elif (
                not list_int_container
                and len(op.args) >= 1
                and isinstance(op.args[0], MoltValue)
                and op.args[0].type_hint == "list"
            ):
                index_entry["container_type"] = "list"
            elif (
                not list_int_container
                and len(op.args) >= 1
                and isinstance(op.args[0], MoltValue)
                and op.args[0].type_hint == "tuple"
            ):
                index_entry["container_type"] = "tuple"
            ctx.json_ops.append(index_entry)
        elif op.kind == "UNPACK_SEQUENCE":
            # args[0] is the sequence, args[1:] are output variable names
            metadata = op.metadata or {}
            ctx.json_ops.append(
                {
                    "kind": "unpack_sequence",
                    "args": [arg.name for arg in op.args],
                    "value": metadata["expected_count"],
                }
            )
        else:
            return False
        return True

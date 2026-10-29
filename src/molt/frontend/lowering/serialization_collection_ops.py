"""SerializationCollectionOpsMixin: JSON serialization for container construction, dict/set/list, iter, index, and unpack ops."""

from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
    FrontendOpKind,
    SIMPLEIR_FIRST_TRAILING_RESULT_ARG,
    WireKinds,
)


from molt.frontend._types import (
    MoltOp,
    MoltValue,
    loaded_method_self_name,
)
from molt.frontend.lowering.serialization_context import (
    SerializationContext,
    SimpleIROp,
)
from molt.frontend._mixin_base import GeneratorMixinBase


_SERIALIZE_COLLECTION_OP_KINDS_1: frozenset[FrontendOpKind] = frozenset(
    {"DICT_UPDATE", "DICT_UNPACK"}
)


class SerializationCollectionOpsMixin(GeneratorMixinBase):
    def _serialize_collection_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == FrontendKinds.LEN:
            len_entry: SimpleIROp = {
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
        elif op.kind == FrontendKinds.ID:
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
        elif op.kind == FrontendKinds.ORD:
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
        elif op.kind == FrontendKinds.ORD_AT:
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
        elif op.kind == FrontendKinds.CHR:
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
        elif op.kind == FrontendKinds.CALLARGS_NEW:
            callargs_op: SimpleIROp = {
                "kind": "callargs_new",
                "out": op.result.name,
            }
            # A CALL_FUNCTION_EX call site's builder is its tuple and mapping;
            # the runtime cannot recover the form from values. Source calls
            # record their form. Compiler-synthesized calls (decorators, class
            # creation, t-strings) record none and bind as CALL.
            form = (op.metadata or {}).get("call_form", "stack")
            if form == "expanded":
                callargs_op["s_value"] = "expanded"
            elif form != "stack":
                raise AssertionError(f"unknown call form {form!r}")
            ctx.json_ops.append(callargs_op)
        elif op.kind == FrontendKinds.CALLARGS_PUSH_METHOD_RECEIVER:
            ctx.json_ops.append(
                {
                    "kind": "callargs_push_method_receiver",
                    "args": [op.args[0].name, loaded_method_self_name(op)],
                }
            )
        elif op.kind == FrontendKinds.CALLARGS_PUSH_POS:
            ctx.json_ops.append(
                {
                    "kind": "callargs_push_pos",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CALLARGS_PUSH_KW:
            ctx.json_ops.append(
                {
                    "kind": "callargs_push_kw",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CALLARGS_EXPAND_STAR:
            ctx.json_ops.append(
                {
                    "kind": "callargs_expand_star",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CALLARGS_EXPAND_KWSTAR:
            ctx.json_ops.append(
                {
                    "kind": "callargs_expand_kwstar",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_NEW:
            _list_op: SimpleIROp = {
                "kind": "list_new",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
                "type_hint": "list",
            }
            ctx.json_ops.append(_list_op)
        elif op.kind == FrontendKinds.LIST_INT_NEW:
            # Specialized flat i64 list: args are [count, fill_value]
            ctx.json_ops.append(
                {
                    "kind": "list_int_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
            ctx.json_list_int_containers.add(op.result.name)
        elif op.kind == FrontendKinds.LIST_FILL_NEW:
            ctx.json_ops.append(
                {
                    "kind": "list_fill_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.RANGE_NEW:
            ctx.json_ops.append(
                {
                    "kind": "range_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_FROM_RANGE:
            ctx.json_ops.append(
                {
                    "kind": "list_from_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.TUPLE_NEW:
            _tuple_op: SimpleIROp = {
                "kind": "tuple_new",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
                "type_hint": "tuple",
            }
            ctx.json_ops.append(_tuple_op)
        elif op.kind == FrontendKinds.LIST_APPEND:
            ctx.json_ops.append(
                {
                    "kind": "list_append",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                    "type_hint": "list",
                }
            )
        elif op.kind == FrontendKinds.LIST_POP:
            ctx.json_ops.append(
                {
                    "kind": "list_pop",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_EXTEND:
            ctx.json_ops.append(
                {
                    "kind": "list_extend",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_INSERT:
            ctx.json_ops.append(
                {
                    "kind": "list_insert",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_REMOVE:
            ctx.json_ops.append(
                {
                    "kind": "list_remove",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_CLEAR:
            ctx.json_ops.append(
                {
                    "kind": "list_clear",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_COPY:
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
        elif op.kind == FrontendKinds.LIST_REVERSE:
            ctx.json_ops.append(
                {
                    "kind": "list_reverse",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_COUNT:
            ctx.json_ops.append(
                {
                    "kind": "list_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_INDEX:
            ctx.json_ops.append(
                {
                    "kind": "list_index",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LIST_INDEX_RANGE:
            ctx.json_ops.append(
                {
                    "kind": "list_index_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.TUPLE_FROM_LIST:
            ctx.json_ops.append(
                {
                    "kind": "tuple_from_list",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "bytes_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTES_FROM_STR:
            ctx.json_ops.append(
                {
                    "kind": "bytes_from_str",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_FROM_STR:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_from_str",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BYTEARRAY_FILL_RANGE:
            ctx.json_ops.append(
                {
                    "kind": "bytearray_fill_range",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.FLOAT_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "float_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.INT_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "int_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.INT_FROM_STR_OF_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "int_from_str_of_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.COMPLEX_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "complex_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_NEW:
            ctx.json_ops.append(
                {
                    "kind": "dict_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                    "type_hint": "dict",
                }
            )
        elif op.kind == FrontendKinds.DICT_FROM_OBJ:
            ctx.json_ops.append(
                {
                    "kind": "dict_from_obj",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_NEW:
            ctx.json_ops.append(
                {
                    "kind": "set_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                    "type_hint": "set",
                }
            )
        elif op.kind == FrontendKinds.FROZENSET_NEW:
            ctx.json_ops.append(
                {
                    "kind": "frozenset_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                    "type_hint": "frozenset",
                }
            )
        elif op.kind == FrontendKinds.DICT_GET:
            ctx.json_ops.append(
                {
                    "kind": "dict_get",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_STR_INT_INC:
            ctx.json_ops.append(
                {
                    "kind": "dict_str_int_inc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_SPLIT_WS_DICT_INC:
            ctx.json_ops.append(
                {
                    "kind": "string_split_ws_dict_inc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.STRING_SPLIT_SEP_DICT_INC:
            ctx.json_ops.append(
                {
                    "kind": "string_split_sep_dict_inc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_POP:
            ctx.json_ops.append(
                {
                    "kind": "dict_pop",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_SET:
            ds_entry: SimpleIROp = {
                "kind": "dict_set",
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
                and op.args[0].type_hint == "list"
            ):
                ds_entry["container_type"] = "list"
            ctx.json_ops.append(ds_entry)
        elif op.kind == FrontendKinds.DICT_SETDEFAULT:
            ctx.json_ops.append(
                {
                    "kind": "dict_setdefault",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_SETDEFAULT_EMPTY_LIST:
            ctx.json_ops.append(
                {
                    "kind": "dict_setdefault_empty_list",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind in _SERIALIZE_COLLECTION_OP_KINDS_1:
            ctx.json_ops.append(
                {
                    "kind": "dict_unpack"
                    if op.kind == FrontendKinds.DICT_UNPACK
                    else "dict_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_CLEAR:
            ctx.json_ops.append(
                {
                    "kind": "dict_clear",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_COPY:
            ctx.json_ops.append(
                {
                    "kind": "dict_copy",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_POPITEM:
            ctx.json_ops.append(
                {
                    "kind": "dict_popitem",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_ADD:
            ctx.json_ops.append(
                {
                    "kind": "set_add",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_ADD_PROBE:
            ctx.json_ops.append(
                {
                    "kind": "set_add_probe",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.FROZENSET_ADD:
            ctx.json_ops.append(
                {
                    "kind": "frozenset_add",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_DISCARD:
            ctx.json_ops.append(
                {
                    "kind": "set_discard",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_REMOVE:
            ctx.json_ops.append(
                {
                    "kind": "set_remove",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_POP:
            ctx.json_ops.append(
                {
                    "kind": "set_pop",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_UPDATE:
            ctx.json_ops.append(
                {
                    "kind": "set_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_INTERSECTION_UPDATE:
            ctx.json_ops.append(
                {
                    "kind": "set_intersection_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_DIFFERENCE_UPDATE:
            ctx.json_ops.append(
                {
                    "kind": "set_difference_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.SET_SYMDIFF_UPDATE:
            ctx.json_ops.append(
                {
                    "kind": "set_symdiff_update",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_KEYS:
            ctx.json_ops.append(
                {
                    "kind": "dict_keys",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_VALUES:
            ctx.json_ops.append(
                {
                    "kind": "dict_values",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.DICT_ITEMS:
            ctx.json_ops.append(
                {
                    "kind": "dict_items",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.TUPLE_COUNT:
            ctx.json_ops.append(
                {
                    "kind": "tuple_count",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.TUPLE_INDEX:
            ctx.json_ops.append(
                {
                    "kind": "tuple_index",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.ITER_NEW:
            ctx.json_ops.append(
                {
                    "kind": "iter",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                    "type_hint": "iter",
                }
            )
        elif op.kind == FrontendKinds.AITER:
            ctx.json_ops.append(
                {
                    "kind": "aiter",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.ITER_NEXT:
            ctx.json_ops.append(
                {
                    "kind": "iter_next",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.ANEXT:
            ctx.json_ops.append(
                {
                    "kind": "anext",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.INDEX:
            index_entry: SimpleIROp = {
                "kind": "index",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
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
        elif op.kind == FrontendKinds.UNPACK_SEQUENCE:
            first_result_arg = SIMPLEIR_FIRST_TRAILING_RESULT_ARG["unpack_sequence"]
            if first_result_arg != 1:
                raise AssertionError(
                    "generated unpack_sequence field authority must preserve one source operand"
                )
            # Generated field authority owns args[0] as the source and args[1:]
            # as output variable names.
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

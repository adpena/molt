"""SerializationBasicOpsMixin: JSON serialization for scalar constants, arithmetic, comparisons, simple control, tracing, and direct calls."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
    _INLINE_INT_MAX,
    _INLINE_INT_MIN,
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


class SerializationBasicOpsMixin(_MixinBase):
    def _serialize_basic_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == "CONST":
            value = op.args[0]
            if isinstance(value, bool):
                value = 1 if value else 0
            # Integers outside 47-bit signed inline range -> const_bigint
            if (
                isinstance(value, int)
                and not isinstance(value, bool)
                and not (_INLINE_INT_MIN <= value <= _INLINE_INT_MAX)
            ):
                ctx.json_ops.append(
                    {
                        "kind": "const_bigint",
                        "s_value": str(value),
                        "out": op.result.name,
                    }
                )
            else:
                ctx.json_ops.append(
                    {"kind": "const", "value": value, "out": op.result.name}
                )
        elif op.kind == "CONST_BIGINT":
            ctx.json_ops.append(
                {
                    "kind": "const_bigint",
                    "s_value": op.args[0],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONST_BOOL":
            value = 1 if op.args[0] else 0
            ctx.json_ops.append(
                {"kind": "const_bool", "value": value, "out": op.result.name}
            )
        elif op.kind == "CONST_FLOAT":
            _fval = op.args[0]
            # Encode non-finite floats as strings for JSON compliance;
            # bare Infinity/NaN tokens are not valid JSON.
            if isinstance(_fval, float) and (
                _fval != _fval  # NaN
                or _fval == float("inf")
                or _fval == float("-inf")
            ):
                if _fval != _fval:
                    _fval = "NaN"
                elif _fval > 0:
                    _fval = "Infinity"
                else:
                    _fval = "-Infinity"
            ctx.json_ops.append(
                {
                    "kind": "const_float",
                    "f_value": _fval,
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONST_STR":
            value = op.args[0]
            if isinstance(value, str) and any(
                0xD800 <= ord(ch) <= 0xDFFF for ch in value
            ):
                raw = value.encode("utf-8", "surrogatepass")
                ctx.json_ops.append(
                    {"kind": "const_str", "bytes": list(raw), "out": op.result.name}
                )
            else:
                ctx.json_ops.append(
                    {"kind": "const_str", "s_value": value, "out": op.result.name}
                )
        elif op.kind == "CONST_BYTES":
            ctx.json_ops.append(
                {
                    "kind": "const_bytes",
                    "bytes": list(op.args[0]),
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONST_NONE":
            ctx.json_ops.append({"kind": "const_none", "out": op.result.name})
        elif op.kind == "CONST_NOT_IMPLEMENTED":
            ctx.json_ops.append(
                {"kind": "const_not_implemented", "out": op.result.name}
            )
        elif op.kind == "CONST_ELLIPSIS":
            ctx.json_ops.append({"kind": "const_ellipsis", "out": op.result.name})
        elif op.kind in ("ADD", "SUB", "MUL"):
            # Compile-time constant fold: when both operands are known
            # integer constants and the result overflows the 47-bit signed
            # inline range, emit const_bigint instead of the arithmetic op.
            # This prevents Cranelift 0.130 from miscompiling the overflow
            # check during its constant-folding pass.
            _arith_folded = False
            if len(op.args) == 2 and self._should_fast_int(op):
                lhs_arg, rhs_arg = op.args
                if isinstance(lhs_arg, MoltValue) and isinstance(rhs_arg, MoltValue):
                    lhs_c = self.const_ints.get(lhs_arg.name)
                    rhs_c = self.const_ints.get(rhs_arg.name)
                    if lhs_c is not None and rhs_c is not None:
                        if op.kind == "ADD":
                            result_val = lhs_c + rhs_c
                        elif op.kind == "SUB":
                            result_val = lhs_c - rhs_c
                        else:
                            result_val = lhs_c * rhs_c
                        if not (_INLINE_INT_MIN <= result_val <= _INLINE_INT_MAX):
                            ctx.json_ops.append(
                                {
                                    "kind": "const_bigint",
                                    "s_value": str(result_val),
                                    "out": op.result.name,
                                }
                            )
                            _arith_folded = True
            if not _arith_folded:
                kind_lower = op.kind.lower()
                entry: dict[str, Any] = {
                    "kind": kind_lower,
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    entry["fast_int"] = True
                elif self._should_fast_float(op):
                    entry["fast_float"] = True
                ctx.json_ops.append(entry)
        elif op.kind in ("INPLACE_ADD", "INPLACE_SUB", "INPLACE_MUL"):
            kind_lower = op.kind.lower()
            entry: dict[str, Any] = {
                "kind": kind_lower,
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                entry["fast_int"] = True
            elif self._should_fast_float(op):
                entry["fast_float"] = True
            ctx.json_ops.append(entry)
        elif op.kind in ("DIV", "INPLACE_DIV"):
            # `/` and `/=`. The inplace variant carries the same fast
            # int/float lanes (builtin numerics have no __itruediv__) but its
            # boxed runtime symbol is molt_inplace_div, which tries
            # __itruediv__ before the binary __truediv__/__rtruediv__ chain.
            div_entry: dict[str, Any] = {
                "kind": "div" if op.kind == "DIV" else "inplace_div",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                div_entry["fast_int"] = True
            elif self._should_fast_float(op):
                div_entry["fast_float"] = True
            ctx.json_ops.append(div_entry)
        elif op.kind in ("FLOORDIV", "INPLACE_FLOORDIV"):
            floordiv_entry: dict[str, Any] = {
                "kind": "floordiv" if op.kind == "FLOORDIV" else "inplace_floordiv",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                floordiv_entry["fast_int"] = True
            elif self._should_fast_float(op):
                floordiv_entry["fast_float"] = True
            ctx.json_ops.append(floordiv_entry)
        elif op.kind in ("MOD", "INPLACE_MOD"):
            mod_entry: dict[str, Any] = {
                "kind": "mod" if op.kind == "MOD" else "inplace_mod",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                mod_entry["fast_int"] = True
            elif self._should_fast_float(op):
                mod_entry["fast_float"] = True
            ctx.json_ops.append(mod_entry)
        elif op.kind in ("POW", "INPLACE_POW"):
            ctx.json_ops.append(
                {
                    "kind": "pow" if op.kind == "POW" else "inplace_pow",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BIT_OR":
            bit_or_entry: dict[str, Any] = {
                "kind": "bit_or",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                bit_or_entry["fast_int"] = True
            ctx.json_ops.append(bit_or_entry)
        elif op.kind == "INPLACE_BIT_OR":
            bit_or_entry: dict[str, Any] = {
                "kind": "inplace_bit_or",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                bit_or_entry["fast_int"] = True
            ctx.json_ops.append(bit_or_entry)
        elif op.kind == "BIT_AND":
            bit_and_entry: dict[str, Any] = {
                "kind": "bit_and",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                bit_and_entry["fast_int"] = True
            ctx.json_ops.append(bit_and_entry)
        elif op.kind == "INPLACE_BIT_AND":
            bit_and_entry: dict[str, Any] = {
                "kind": "inplace_bit_and",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                bit_and_entry["fast_int"] = True
            ctx.json_ops.append(bit_and_entry)
        elif op.kind == "BIT_XOR":
            bit_xor_entry: dict[str, Any] = {
                "kind": "bit_xor",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                bit_xor_entry["fast_int"] = True
            ctx.json_ops.append(bit_xor_entry)
        elif op.kind == "INPLACE_BIT_XOR":
            bit_xor_entry: dict[str, Any] = {
                "kind": "inplace_bit_xor",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                bit_xor_entry["fast_int"] = True
            ctx.json_ops.append(bit_xor_entry)
        elif op.kind in ("LSHIFT", "INPLACE_LSHIFT"):
            lshift_entry: dict[str, Any] = {
                "kind": "lshift" if op.kind == "LSHIFT" else "inplace_lshift",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                lshift_entry["fast_int"] = True
            ctx.json_ops.append(lshift_entry)
        elif op.kind in ("RSHIFT", "INPLACE_RSHIFT"):
            rshift_entry: dict[str, Any] = {
                "kind": "rshift" if op.kind == "RSHIFT" else "inplace_rshift",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                rshift_entry["fast_int"] = True
            ctx.json_ops.append(rshift_entry)
        elif op.kind in ("MATMUL", "INPLACE_MATMUL"):
            ctx.json_ops.append(
                {
                    "kind": "matmul" if op.kind == "MATMUL" else "inplace_matmul",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "POW_MOD":
            ctx.json_ops.append(
                {
                    "kind": "pow_mod",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ROUND":
            ctx.json_ops.append(
                {
                    "kind": "round",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "TRUNC":
            ctx.json_ops.append(
                {
                    "kind": "trunc",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "LT":
            lt_entry: dict[str, Any] = {
                "kind": "lt",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                lt_entry["fast_int"] = True
            elif self._should_fast_float(op):
                lt_entry["fast_float"] = True
            ctx.json_ops.append(lt_entry)
        elif op.kind == "LE":
            le_entry: dict[str, Any] = {
                "kind": "le",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                le_entry["fast_int"] = True
            elif self._should_fast_float(op):
                le_entry["fast_float"] = True
            ctx.json_ops.append(le_entry)
        elif op.kind == "GT":
            gt_entry: dict[str, Any] = {
                "kind": "gt",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                gt_entry["fast_int"] = True
            elif self._should_fast_float(op):
                gt_entry["fast_float"] = True
            ctx.json_ops.append(gt_entry)
        elif op.kind == "GE":
            ge_entry: dict[str, Any] = {
                "kind": "ge",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                ge_entry["fast_int"] = True
            elif self._should_fast_float(op):
                ge_entry["fast_float"] = True
            ctx.json_ops.append(ge_entry)
        elif op.kind == "EQ":
            eq_entry: dict[str, Any] = {
                "kind": "eq",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                eq_entry["fast_int"] = True
            elif self._should_fast_float(op):
                eq_entry["fast_float"] = True
            ctx.json_ops.append(eq_entry)
        elif op.kind == "NE":
            ne_entry: dict[str, Any] = {
                "kind": "ne",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                ne_entry["fast_int"] = True
            elif self._should_fast_float(op):
                ne_entry["fast_float"] = True
            ctx.json_ops.append(ne_entry)
        elif op.kind == "STRING_EQ":
            ctx.json_ops.append(
                {
                    "kind": "string_eq",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "IS":
            # Re-materialise any CONST_NONE variable referenced by this
            # IS instruction so the definition and use share the same
            # Cranelift basic block.
            for a in op.args:
                if isinstance(a, MoltValue) and a.name in ctx.const_none_vars:
                    ctx.json_ops.append({"kind": "const_none", "out": a.name})
            ctx.json_ops.append(
                {
                    "kind": "is",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "INVERT":
            ctx.json_ops.append(
                {
                    "kind": "invert",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "NEG":
            entry: dict[str, Any] = {
                "kind": "neg",
                "args": [op.args[0].name],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                entry["fast_int"] = True
            elif self._should_fast_float(op):
                entry["fast_float"] = True
            ctx.json_ops.append(entry)
        elif op.kind == "POS":
            entry: dict[str, Any] = {
                "kind": "pos",
                "args": [op.args[0].name],
                "out": op.result.name,
            }
            if self._should_fast_int(op):
                entry["fast_int"] = True
            elif self._should_fast_float(op):
                entry["fast_float"] = True
            ctx.json_ops.append(entry)
        elif op.kind == "NOT":
            ctx.json_ops.append(
                {
                    "kind": "not",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "BOOL":
            ctx.json_ops.append(
                {
                    "kind": "bool",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "ABS":
            ctx.json_ops.append(
                {
                    "kind": "abs",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "AND":
            ctx.json_ops.append(
                {
                    "kind": "and",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "OR":
            ctx.json_ops.append(
                {
                    "kind": "or",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CONTAINS":
            entry: dict[str, object] = {
                "kind": "contains",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            container_arg = op.args[0]
            if isinstance(container_arg, MoltValue) and container_arg.type_hint in {
                "set",
                "frozenset",
                "dict",
                "list",
                "str",
            }:
                entry["container_type"] = container_arg.type_hint
            ctx.json_ops.append(entry)
        elif op.kind == "IF":
            _if_entry: dict[str, Any] = {"kind": "if", "args": [op.args[0].name]}
            _if_cond = op.args[0]
            if isinstance(_if_cond, MoltValue) and _if_cond.type_hint in {
                "int",
                "bool",
            }:
                _if_entry["type_hint"] = _if_cond.type_hint
            ctx.json_ops.append(_if_entry)
        elif op.kind == "ELSE":
            ctx.json_ops.append({"kind": "else"})
        elif op.kind == "END_IF":
            ctx.json_ops.append({"kind": "end_if"})
        elif op.kind == "LINE":
            line = int(op.args[0])
            d: dict[str, Any] = {
                "kind": "line",
                "value": line,
                "source_line": op.source_line or line,
            }
            if op.col_offset is not None:
                d["col_offset"] = op.col_offset
            if op.end_col_offset is not None:
                d["end_col_offset"] = op.end_col_offset
            ctx.json_ops.append(d)
        elif op.kind == "TRACE_ENTER_SLOT":
            ctx.json_ops.append({"kind": "trace_enter_slot", "value": int(op.args[0])})
        elif op.kind == "TRACE_EXIT":
            ctx.json_ops.append({"kind": "trace_exit"})
        elif op.kind == "FRAME_LOCALS_SET":
            ctx.json_ops.append(
                {
                    "kind": "frame_locals_set",
                    "args": [op.args[0].name],
                }
            )
        elif op.kind == "CALL":
            target = op.args[0]
            code_id = self.func_code_ids.get(target, 0)
            entry = {
                "kind": "call",
                "s_value": target,
                "args": [arg.name for arg in op.args[1:]],
                "value": code_id,
                "out": op.result.name,
            }
            # Same lane-classification fix as CALL_BIND/CALL_METHOD —
            # preserve a meaningful result type_hint so the backend's
            # preanalysis (function_compiler.rs:1618-1624) can route
            # the call result into int_like_vars / float_like_vars and
            # avoid coercing tight-loop accumulators to NaN-boxed
            # float.  Without this, `total += obj.method(i)` where
            # the Phase 1 fold rewrote the call to a direct CALL
            # op silently dropped the int return type.
            result_hint = op.result.type_hint
            if result_hint and result_hint != "Any":
                entry["type_hint"] = result_hint
            metadata = op.metadata or {}
            if metadata.get("defines_del") is True:
                entry["defines_del"] = True
            ctx.json_ops.append(entry)
        elif op.kind == "CALL_INTERNAL":
            target = op.args[0]
            code_id = self.func_code_ids.get(target, 0)
            ctx.json_ops.append(
                {
                    "kind": "call_internal",
                    "s_value": target,
                    "args": [arg.name for arg in op.args[1:]],
                    "value": code_id,
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALL_INDIRECT":
            ctx.json_ops.append(
                {
                    "kind": "call_indirect",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALL_GUARDED":
            target = op.metadata["target"] if op.metadata else ""
            ctx.json_ops.append(
                {
                    "kind": "call_guarded",
                    "s_value": target,
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CALL_FUNC":
            ctx.json_ops.append(
                {
                    "kind": "call_func",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "INVOKE_FFI":
            lane = ""
            if op.metadata is not None:
                raw_lane = op.metadata.get("ffi_lane")
                if isinstance(raw_lane, str):
                    lane = raw_lane
            invoke_op = {
                "kind": "invoke_ffi",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            if lane:
                invoke_op["s_value"] = lane
            ctx.json_ops.append(invoke_op)
        elif op.kind == "CALL_BIND":
            entry: dict[str, Any] = {
                "kind": "call_bind",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # Propagate the result's type_hint so the backend's
            # preanalysis can classify the call's lane (int/float/
            # bool/str).  Without this, `total += obj.method(i)`
            # in a tight loop falls through to the catch-all
            # NaN-boxed accumulator and silently coerces to float.
            # The frontend already populates `op.result.type_hint`
            # from the method's `return_hint`; this serialises it.
            result_hint = op.result.type_hint
            if result_hint and result_hint != "Any":
                entry["type_hint"] = result_hint
            metadata = op.metadata or {}
            if metadata.get("defines_del") is True:
                entry["defines_del"] = True
            if metadata.get("bound_local") is True:
                entry["bound_local"] = True
            ctx.json_ops.append(entry)
        elif op.kind == "DEL_BOUNDARY":
            entry = {
                "kind": "del_boundary",
                "args": [arg.name for arg in op.args],
            }
            boundary_var = (op.metadata or {}).get("var")
            if boundary_var:
                entry["s_value"] = boundary_var
            ctx.json_ops.append(entry)
        elif op.kind == "CALL_METHOD":
            entry = {
                "kind": "call_method",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # Propagate BoundMethod type info so the backend can
            # specialise known receiver+method pairs (e.g.
            # list.append, str.join, dict.get) into direct calls.
            callee_hint = getattr(op.args[0], "type_hint", None)
            if (
                callee_hint
                and isinstance(callee_hint, str)
                and callee_hint.startswith("BoundMethod:")
            ):
                entry["s_value"] = callee_hint
            # Same lane-classification fix as CALL_BIND above.
            result_hint = op.result.type_hint
            if result_hint and result_hint != "Any":
                entry["type_hint"] = result_hint
            ctx.json_ops.append(entry)
        elif op.kind == "BUILTIN_FUNC":
            func_name, arity = op.args
            ctx.json_ops.append(
                {
                    "kind": "builtin_func",
                    "s_value": func_name,
                    "value": arity,
                    "out": op.result.name,
                }
            )
        elif op.kind == "FUNC_NEW":
            func_name, arity = op.args
            ctx.json_ops.append(
                {
                    "kind": "func_new",
                    "s_value": func_name,
                    "value": arity,
                    "out": op.result.name,
                }
            )
        elif op.kind == "FUNC_NEW_CLOSURE":
            func_name, arity, closure = op.args
            ctx.json_ops.append(
                {
                    "kind": "func_new_closure",
                    "s_value": func_name,
                    "value": arity,
                    "args": [closure.name],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CODE_NEW":
            ctx.json_ops.append(
                {
                    "kind": "code_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == "CODE_SLOT_SET":
            code_id = 0
            if op.metadata and "code_id" in op.metadata:
                code_id = int(op.metadata["code_id"])
            ctx.json_ops.append(
                {
                    "kind": "code_slot_set",
                    "value": code_id,
                    "args": [op.args[0].name],
                }
            )
        elif op.kind == "FN_PTR_CODE_SET":
            func_name, code_val = op.args
            ctx.json_ops.append(
                {
                    "kind": "fn_ptr_code_set",
                    "s_value": func_name,
                    "args": [code_val.name],
                }
            )
        elif op.kind == "ASYNCGEN_LOCALS_REGISTER":
            func_name, names_tuple, offsets_tuple = op.args
            ctx.json_ops.append(
                {
                    "kind": "asyncgen_locals_register",
                    "s_value": func_name,
                    "args": [names_tuple.name, offsets_tuple.name],
                }
            )
        elif op.kind == "GEN_LOCALS_REGISTER":
            func_name, names_tuple, offsets_tuple = op.args
            ctx.json_ops.append(
                {
                    "kind": "gen_locals_register",
                    "s_value": func_name,
                    "args": [names_tuple.name, offsets_tuple.name],
                }
            )
        else:
            return False
        return True

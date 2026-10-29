"""SerializationBasicOpsMixin: JSON serialization for scalar constants, arithmetic, comparisons, simple control, tracing, and direct calls."""

from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import (
    FrontendKinds,
    FrontendOpKind,
    WireOpKind,
    frontend_wire_kind,
)


from molt.opcode_literal_payloads import integer_literal_bytes, integer_literal_payload


from molt.frontend._types import (
    _INLINE_INT_MAX,
    _INLINE_INT_MIN,
    BoundMethodHint,
    MoltOp,
    MoltValue,
    loaded_method_self_name,
)
from molt.frontend.lowering.serialization_context import (
    SerializationContext,
    SimpleIROp,
)
from molt.frontend._mixin_base import GeneratorMixinBase


_CALL_ARGUMENT_CUSTODY_KINDS_1: frozenset[WireOpKind] = frozenset(
    {"call_internal", "call"}
)
_SERIALIZE_BASIC_OP_KINDS_1: frozenset[FrontendOpKind] = frozenset(
    {"MUL", "SUB", "ADD"}
)
_SERIALIZE_BASIC_OP_KINDS_2: frozenset[FrontendOpKind] = frozenset(
    {"INPLACE_ADD", "INPLACE_MUL", "INPLACE_SUB"}
)
_SERIALIZE_BASIC_OP_KINDS_3: frozenset[FrontendOpKind] = frozenset(
    {"INPLACE_DIV", "DIV"}
)
_SERIALIZE_BASIC_OP_KINDS_4: frozenset[FrontendOpKind] = frozenset(
    {"FLOORDIV", "INPLACE_FLOORDIV"}
)
_SERIALIZE_BASIC_OP_KINDS_5: frozenset[FrontendOpKind] = frozenset(
    {"INPLACE_MOD", "MOD"}
)
_SERIALIZE_BASIC_OP_KINDS_6: frozenset[FrontendOpKind] = frozenset(
    {"INPLACE_POW", "POW"}
)
_SERIALIZE_BASIC_OP_KINDS_7: frozenset[FrontendOpKind] = frozenset(
    {"INPLACE_LSHIFT", "LSHIFT"}
)
_SERIALIZE_BASIC_OP_KINDS_8: frozenset[FrontendOpKind] = frozenset(
    {"INPLACE_RSHIFT", "RSHIFT"}
)
_SERIALIZE_BASIC_OP_KINDS_9: frozenset[FrontendOpKind] = frozenset(
    {"MATMUL", "INPLACE_MATMUL"}
)
_SERIALIZE_BASIC_OP_KINDS_10: frozenset[WireOpKind] = frozenset({"bool", "int"})
_SERIALIZE_BASIC_OP_KINDS_11: frozenset[FrontendOpKind] = frozenset(
    {"FRAME_HOME_STORE", "FRAME_HOME_CELL", "FRAME_HOME_PRIVATE_CELL"}
)
_SERIALIZE_BASIC_OP_KINDS_12: frozenset[FrontendOpKind] = frozenset(
    {"FRAME_HOME_TAKE", "FRAME_HOME_LOAD"}
)
_SERIALIZE_BASIC_OP_KINDS_13: frozenset[FrontendOpKind] = frozenset(
    {"FUNC_NEW_CLOSURE", "FUNC_NEW"}
)


class SerializationBasicOpsMixin(GeneratorMixinBase):
    def _call_argument_custody(
        self, op: MoltOp, kind: WireOpKind, target: str | None, operand_count: int
    ) -> list[str] | None:
        """Operand custody of a call instruction; None when it borrows.

        A raw direct call (`call`, `call_internal`) enters its target's machine
        entry, which owns exactly the parameters its declaration transfers, so
        it carries that custody whether the source or the compiler spelled it;
        the callee supplies a closure parameter itself. Every other call adopts
        only as the instruction that evaluates a source call expression
        (`_mark_source_call`), whose custody is an instruction fact recorded on
        the op; a call the compiler synthesizes borrows.
        """
        if kind in _CALL_ARGUMENT_CUSTODY_KINDS_1:
            if not operand_count or not target or not self._entry_adopts_symbol(target):
                return None
            return ["transferred"] * operand_count
        custody = (op.metadata or {}).get("argument_custody")
        if custody is None:
            return None
        if len(custody) != operand_count:
            raise AssertionError(
                f"{kind} custody names {len(custody)} of {operand_count} operands"
            )
        return list(custody)

    def _serialize_basic_op(self, op: MoltOp, ctx: SerializationContext) -> bool:
        if op.kind == FrontendKinds.CONST:
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
                        "bytes": list(integer_literal_bytes(value)),
                        "out": op.result.name,
                    }
                )
            else:
                ctx.json_ops.append(
                    {"kind": "const", "value": value, "out": op.result.name}
                )
        elif op.kind == FrontendKinds.CONST_BIGINT:
            integer_literal_payload(op.args[0])  # reject stale/malformed cached IR
            ctx.json_ops.append(
                {
                    "kind": "const_bigint",
                    "bytes": list(op.args[0]),
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CONST_BOOL:
            value = 1 if op.args[0] else 0
            ctx.json_ops.append(
                {"kind": "const_bool", "value": value, "out": op.result.name}
            )
        elif op.kind == FrontendKinds.CONST_FLOAT:
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
        elif op.kind == FrontendKinds.CONST_STR:
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
        elif op.kind == FrontendKinds.CONST_BYTES:
            ctx.json_ops.append(
                {
                    "kind": "const_bytes",
                    "bytes": list(op.args[0]),
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CONST_NONE:
            ctx.json_ops.append({"kind": "const_none", "out": op.result.name})
        elif op.kind == FrontendKinds.CONST_NOT_IMPLEMENTED:
            ctx.json_ops.append(
                {"kind": "const_not_implemented", "out": op.result.name}
            )
        elif op.kind == FrontendKinds.CONST_ELLIPSIS:
            ctx.json_ops.append({"kind": "const_ellipsis", "out": op.result.name})
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_1:
            # Compile-time constant fold: when both operands are known
            # integer constants and the result overflows the 47-bit signed
            # inline range, emit const_bigint instead of the arithmetic op.
            # This prevents Cranelift 0.130 from miscompiling the overflow
            # check during its constant-folding pass.
            _arith_folded = False
            if len(op.args) == 2:
                lhs_arg, rhs_arg = op.args
                if isinstance(lhs_arg, MoltValue) and isinstance(rhs_arg, MoltValue):
                    lhs_c = self.const_ints.get(lhs_arg.name)
                    rhs_c = self.const_ints.get(rhs_arg.name)
                    if lhs_c is not None and rhs_c is not None:
                        if op.kind == FrontendKinds.ADD:
                            result_val = lhs_c + rhs_c
                        elif op.kind == FrontendKinds.SUB:
                            result_val = lhs_c - rhs_c
                        else:
                            result_val = lhs_c * rhs_c
                        if not (_INLINE_INT_MIN <= result_val <= _INLINE_INT_MAX):
                            ctx.json_ops.append(
                                {
                                    "kind": "const_bigint",
                                    "bytes": list(integer_literal_bytes(result_val)),
                                    "out": op.result.name,
                                }
                            )
                            _arith_folded = True
            if not _arith_folded:
                kind_lower = frontend_wire_kind(op.kind)
                entry: SimpleIROp = {
                    "kind": kind_lower,
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                ctx.json_ops.append(entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_2:
            kind_lower = frontend_wire_kind(op.kind)
            entry: SimpleIROp = {
                "kind": kind_lower,
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_3:
            # `/` and `/=`. The inplace variant carries the same fast
            # int/float lanes (builtin numerics have no __itruediv__) but its
            # boxed runtime symbol is molt_inplace_div, which tries
            # __itruediv__ before the binary __truediv__/__rtruediv__ chain.
            div_entry: SimpleIROp = {
                "kind": "div" if op.kind == FrontendKinds.DIV else "inplace_div",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(div_entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_4:
            floordiv_entry: SimpleIROp = {
                "kind": "floordiv"
                if op.kind == FrontendKinds.FLOORDIV
                else "inplace_floordiv",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(floordiv_entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_5:
            mod_entry: SimpleIROp = {
                "kind": "mod" if op.kind == FrontendKinds.MOD else "inplace_mod",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(mod_entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_6:
            ctx.json_ops.append(
                {
                    "kind": "pow" if op.kind == FrontendKinds.POW else "inplace_pow",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BIT_OR:
            bit_or_entry: SimpleIROp = {
                "kind": "bit_or",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(bit_or_entry)
        elif op.kind == FrontendKinds.INPLACE_BIT_OR:
            bit_or_entry: SimpleIROp = {
                "kind": "inplace_bit_or",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(bit_or_entry)
        elif op.kind == FrontendKinds.BIT_AND:
            bit_and_entry: SimpleIROp = {
                "kind": "bit_and",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(bit_and_entry)
        elif op.kind == FrontendKinds.INPLACE_BIT_AND:
            bit_and_entry: SimpleIROp = {
                "kind": "inplace_bit_and",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(bit_and_entry)
        elif op.kind == FrontendKinds.BIT_XOR:
            bit_xor_entry: SimpleIROp = {
                "kind": "bit_xor",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(bit_xor_entry)
        elif op.kind == FrontendKinds.INPLACE_BIT_XOR:
            bit_xor_entry: SimpleIROp = {
                "kind": "inplace_bit_xor",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(bit_xor_entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_7:
            lshift_entry: SimpleIROp = {
                "kind": "lshift"
                if op.kind == FrontendKinds.LSHIFT
                else "inplace_lshift",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(lshift_entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_8:
            rshift_entry: SimpleIROp = {
                "kind": "rshift"
                if op.kind == FrontendKinds.RSHIFT
                else "inplace_rshift",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(rshift_entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_9:
            ctx.json_ops.append(
                {
                    "kind": "matmul"
                    if op.kind == FrontendKinds.MATMUL
                    else "inplace_matmul",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.LT:
            lt_entry: SimpleIROp = {
                "kind": "lt",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(lt_entry)
        elif op.kind == FrontendKinds.LE:
            le_entry: SimpleIROp = {
                "kind": "le",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(le_entry)
        elif op.kind == FrontendKinds.GT:
            gt_entry: SimpleIROp = {
                "kind": "gt",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(gt_entry)
        elif op.kind == FrontendKinds.GE:
            ge_entry: SimpleIROp = {
                "kind": "ge",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(ge_entry)
        elif op.kind == FrontendKinds.EQ:
            eq_entry: SimpleIROp = {
                "kind": "eq",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(eq_entry)
        elif op.kind == FrontendKinds.NE:
            ne_entry: SimpleIROp = {
                "kind": "ne",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            ctx.json_ops.append(ne_entry)
        elif op.kind == FrontendKinds.IS:
            ctx.json_ops.append(
                {
                    "kind": "is",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.INVERT:
            ctx.json_ops.append(
                {
                    "kind": "invert",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.NEG:
            entry: SimpleIROp = {
                "kind": "neg",
                "args": [op.args[0].name],
                "out": op.result.name,
            }
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.POS:
            entry: SimpleIROp = {
                "kind": "pos",
                "args": [op.args[0].name],
                "out": op.result.name,
            }
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.NOT:
            ctx.json_ops.append(
                {
                    "kind": "not",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.BOOL:
            ctx.json_ops.append(
                {
                    "kind": "bool",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.ABS:
            ctx.json_ops.append(
                {
                    "kind": "abs",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.AND:
            ctx.json_ops.append(
                {
                    "kind": "and",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.OR:
            ctx.json_ops.append(
                {
                    "kind": "or",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CONTAINS:
            entry: SimpleIROp = {
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
        elif op.kind == FrontendKinds.IF:
            _if_entry: SimpleIROp = {"kind": "if", "args": [op.args[0].name]}
            _if_cond = op.args[0]
            if (
                isinstance(_if_cond, MoltValue)
                and _if_cond.type_hint in _SERIALIZE_BASIC_OP_KINDS_10
            ):
                _if_entry["type_hint"] = _if_cond.type_hint
            ctx.json_ops.append(_if_entry)
        elif op.kind == FrontendKinds.ELSE:
            ctx.json_ops.append({"kind": "else"})
        elif op.kind == FrontendKinds.END_IF:
            ctx.json_ops.append({"kind": "end_if"})
        elif op.kind == FrontendKinds.LINE:
            line = int(op.args[0])
            d: SimpleIROp = {
                "kind": "line",
                "value": line,
                "source_line": op.source_line or line,
            }
            if op.col_offset is not None:
                d["col_offset"] = op.col_offset
            if op.end_col_offset is not None:
                d["end_col_offset"] = op.end_col_offset
            ctx.json_ops.append(d)
        elif op.kind == FrontendKinds.TRACE_ENTER_SLOT:
            ctx.json_ops.append(
                {
                    "kind": "trace_enter_slot",
                    "s_value": ctx.code_owner_symbol(op.args[0]),
                }
            )
        elif op.kind == FrontendKinds.TRACE_EXIT:
            ctx.json_ops.append({"kind": "trace_exit"})
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_11:
            # The code slot is the op's value; the result is the binding view.
            ctx.json_ops.append(
                {
                    "kind": frontend_wire_kind(op.kind),
                    "value": int((op.metadata or {})["slot"]),
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
            )
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_12:
            ctx.json_ops.append(
                {
                    "kind": frontend_wire_kind(op.kind),
                    "value": int((op.metadata or {})["slot"]),
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.FRAME_HOME_CLEAR:
            ctx.json_ops.append(
                {
                    "kind": "frame_home_clear",
                    "value": int((op.metadata or {})["slot"]),
                }
            )
        elif op.kind == FrontendKinds.FRAME_LOCALS:
            # value 1: the activation's one shared locals() dict (before PEP 667).
            ctx.json_ops.append(
                {
                    "kind": "frame_locals",
                    "args": [arg.name for arg in op.args],
                    "value": 1 if (op.metadata or {}).get("shared") else 0,
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.FRAME_LOCALS_SET:
            ctx.json_ops.append(
                {
                    "kind": "frame_locals_set",
                    "args": [op.args[0].name],
                }
            )
        elif op.kind == FrontendKinds.FRAME_CONTEXT_SET:
            ctx.json_ops.append(
                {
                    "kind": "frame_context_set",
                    "args": [arg.name for arg in op.args],
                }
            )
        elif op.kind == FrontendKinds.CALL:
            target = op.args[0]
            entry: SimpleIROp = {
                "kind": ("call_internal" if target in self.funcs_map else "call"),
                "s_value": target,
                "args": [arg.name for arg in op.args[1:]],
                "out": op.result.name,
            }
            custody = self._call_argument_custody(op, "call", target, len(op.args) - 1)
            if custody is not None:
                entry["argument_custody"] = custody
            # Same lane-classification fix as CALL_BIND/CALL_METHOD —
            # preserve a meaningful result type_hint so the backend's
            # preanalysis (function_compiler.rs:1618-1624) can route
            # the call result into int_like_vars / float_like_vars and
            # avoid coercing tight-loop accumulators to NaN-boxed
            # float.  Without this, `total += obj.method(i)` where
            # the Phase 1 fold rewrote the call to a direct CALL
            # op silently dropped the int return type.
            result_hint = self._wire_type_hint(op.result.type_hint)
            if result_hint and result_hint != "Any":
                entry["type_hint"] = result_hint
            metadata = op.metadata or {}
            if metadata.get("defines_del") is True:
                entry["defines_del"] = True
            if target in self.module_chunk_symbols:
                entry["passes_execution_context"] = True
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.CALL_INDIRECT:
            indirect_entry: SimpleIROp = {
                "kind": "call_indirect",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            custody = self._call_argument_custody(
                op, "call_indirect", None, len(op.args)
            )
            if custody is not None:
                indirect_entry["argument_custody"] = custody
            ctx.json_ops.append(indirect_entry)
        elif op.kind == FrontendKinds.CALL_GUARDED:
            target = op.metadata["target"] if op.metadata else ""
            guarded_entry: SimpleIROp = {
                "kind": "call_guarded",
                "s_value": target,
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            custody = self._call_argument_custody(
                op, "call_guarded", target, len(op.args)
            )
            if custody is not None:
                guarded_entry["argument_custody"] = custody
            ctx.json_ops.append(guarded_entry)
        elif op.kind == FrontendKinds.CALL_FUNC:
            entry: SimpleIROp = {
                "kind": "call_func",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            custody = self._call_argument_custody(op, "call_func", None, len(op.args))
            if custody is not None:
                entry["argument_custody"] = custody
            if op.metadata is not None:
                runtime_requirement_bits = op.metadata.get("runtime_requirement_bits")
                if (
                    isinstance(runtime_requirement_bits, int)
                    and runtime_requirement_bits
                ):
                    entry["runtime_requirement_bits"] = runtime_requirement_bits
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.CALL_LOADED_METHOD:
            # CPython's CALL after a method load. The frontend value names the
            # loaded pair; the wire call takes both of its results.
            pair, *arguments = op.args
            entry: SimpleIROp = {
                "kind": "call_loaded_method",
                "args": [
                    pair.name,
                    loaded_method_self_name(op),
                    *(argument.name for argument in arguments),
                ],
                "out": op.result.name,
            }
            custody = self._call_argument_custody(
                op, "call_loaded_method", None, len(op.args) + 1
            )
            if custody is None:
                raise AssertionError("CALL_LOADED_METHOD is always a source call")
            entry["argument_custody"] = custody
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.CALL_BIND:
            entry: SimpleIROp = {
                "kind": "call_bind",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # Propagate the result's type_hint so the backend's
            # preanalysis can classify the call's lane (int/float/
            # bool/str).  Without this, `total += obj.method(i)`
            # in a tight loop falls through to the catch-all
            # NaN-boxed accumulator and silently coerces to float.
            # Only producer-proven result hints may select this lane; Python
            # return annotations are not result proofs.
            result_hint = self._wire_type_hint(op.result.type_hint)
            if result_hint and result_hint != "Any":
                entry["type_hint"] = result_hint
            metadata = op.metadata or {}
            if metadata.get("defines_del") is True:
                entry["defines_del"] = True
            custody = self._call_argument_custody(op, "call_bind", None, len(op.args))
            if custody is not None:
                entry["argument_custody"] = custody
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.DEL_BOUNDARY:
            entry: SimpleIROp = {
                "kind": "del_boundary",
                "args": [arg.name for arg in op.args],
            }
            boundary_var = (op.metadata or {}).get("var")
            if boundary_var:
                entry["s_value"] = boundary_var
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.CALL_METHOD:
            entry: SimpleIROp = {
                "kind": "call_method",
                "args": [arg.name for arg in op.args],
                "out": op.result.name,
            }
            # Propagate BoundMethod type info so the backend can
            # specialise known receiver+method pairs (e.g.
            # list.append, str.join, dict.get) into direct calls.
            callee_hint = getattr(op.args[0], "type_hint", None)
            if isinstance(callee_hint, BoundMethodHint):
                bound_method = self._bound_method_wire(callee_hint)
                if bound_method is not None:
                    entry["s_value"] = bound_method
            # Same lane-classification fix as CALL_BIND above.
            result_hint = self._wire_type_hint(op.result.type_hint)
            if result_hint and result_hint != "Any":
                entry["type_hint"] = result_hint
            custody = self._call_argument_custody(op, "call_method", None, len(op.args))
            if custody is not None:
                entry["argument_custody"] = custody
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.BUILTIN_FUNC:
            func_name, arity, *name_args = op.args
            entry: SimpleIROp = {
                "kind": "builtin_func",
                "s_value": func_name,
                "value": arity,
                "out": op.result.name,
            }
            if name_args:
                entry["args"] = [name_args[0].name]
            builtin_name = (op.metadata or {}).get("builtin_name")
            if isinstance(builtin_name, str):
                entry["builtin_name"] = builtin_name
            runtime_requirement_bits = (op.metadata or {}).get(
                "runtime_requirement_bits"
            )
            if isinstance(runtime_requirement_bits, int) and runtime_requirement_bits:
                entry["runtime_requirement_bits"] = runtime_requirement_bits
            ctx.json_ops.append(entry)
        elif op.kind in _SERIALIZE_BASIC_OP_KINDS_13:
            func_name, arity = op.args[:2]
            entry: SimpleIROp = {
                "kind": frontend_wire_kind(op.kind),
                "s_value": func_name,
                "value": arity,
                "out": op.result.name,
            }
            if op.kind == FrontendKinds.FUNC_NEW_CLOSURE:
                entry["args"] = [op.args[2].name]
            metadata = op.metadata or {}
            if "task_kind" in metadata or "task_closure_size" in metadata:
                task_kind = metadata.get("task_kind")
                task_closure_size = metadata.get("task_closure_size")
                if not isinstance(task_kind, str) or task_kind not in {
                    "generator",
                    "coroutine",
                    "async_generator",
                }:
                    raise ValueError(
                        "function task definition requires a valid task_kind"
                    )
                if (
                    not isinstance(task_closure_size, int)
                    or isinstance(task_closure_size, bool)
                    or task_closure_size < 0
                ):
                    raise ValueError(
                        "function task definition requires non-negative task_closure_size bytes"
                    )
                entry["task_kind"] = task_kind
                entry["task_closure_size"] = task_closure_size
            ctx.json_ops.append(entry)
        elif op.kind == FrontendKinds.CODE_NEW:
            ctx.json_ops.append(
                {
                    "kind": "code_new",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
            )
        elif op.kind == FrontendKinds.CODE_SLOT_SET:
            code_symbol = (op.metadata or {})["code_symbol"]
            code_val, globals_dict = op.args
            ctx.json_ops.append(
                {
                    "kind": "code_slot_set",
                    "s_value": ctx.code_owner_symbol(code_symbol),
                    "args": [code_val.name, globals_dict.name],
                }
            )
        elif op.kind == FrontendKinds.STATEFUL_LOCALS_REGISTER:
            func_name, names_tuple, layout_tuple = op.args
            ctx.json_ops.append(
                {
                    "kind": "stateful_locals_register",
                    "s_value": func_name,
                    "args": [names_tuple.name, layout_tuple.name],
                }
            )
        else:
            return False
        return True

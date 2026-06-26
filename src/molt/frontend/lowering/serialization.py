"""SerializationMixin: the IR -> JSON op emitter (map_ops_to_json) and its
string-split scalarization/fusion helpers.

Move-only extraction from frontend/__init__.py (F1 phase 1). This is the
largest single seam in the generator: map_ops_to_json walks the emitted MoltOp
stream and produces the JSON IR consumed by the backend. self.<method>/<attr>
references resolve through the SimpleTIRGenerator MRO at runtime; the
_GeneratorProtocol annotation gives them static checking.
"""

from __future__ import annotations

import os

from typing import (
    TYPE_CHECKING,
    Any,
    Literal,
    cast,
)

from molt.frontend._types import (
    MoltOp,
    MoltValue,
    _INLINE_INT_MAX,
    _INLINE_INT_MIN,
    _next_ic_index,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class SerializationMixin(_MixinBase):
    @staticmethod
    def _require_async_poll_target(kind: str, target: Any) -> str:
        if not isinstance(target, str) or not target.endswith("_poll"):
            raise ValueError(
                f"{kind} requires a table-addressable poll target ending in _poll; "
                f"got {target!r}"
            )
        return target

    @staticmethod
    def _scalarize_string_split_fields_json(
        json_ops: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        const_ints: dict[str, int] = {}
        const_nones: set[str] = set()
        const_strings: dict[str, str] = {}

        class SplitCandidate:
            def __init__(self, op_index: int, hay: str, sep: str, region: int) -> None:
                self.op_index: int = op_index
                self.hay: str = hay
                self.sep: str = sep
                self.region: int = region
                self.alias_op_indexes: set[int] = set()
                self.index_values_by_op_index: dict[int, int] = {}
                self.unsafe: bool = False

        candidates: dict[str, SplitCandidate] = {}
        alias_to_split: dict[str, tuple[str, int]] = {}
        local_to_split: dict[str, tuple[str, int]] = {}
        split_locals_crossing_control: dict[str, str] = {}
        local_to_const_string: dict[str, tuple[str, int]] = {}
        current_region = 0
        control_depth = 0
        arg_user_kinds: dict[str, set[str]] = {}
        for op in json_ops:
            kind = op.get("kind")
            args = op.get("args")
            if not isinstance(kind, str) or not isinstance(args, list):
                continue
            for arg in args:
                if isinstance(arg, str):
                    arg_user_kinds.setdefault(arg, set()).add(kind)
        cleanup_load_outputs = {
            out
            for op in json_ops
            if op.get("kind") == "load_var"
            and isinstance((out := op.get("out")), str)
            and arg_user_kinds.get(out, set()).issubset({"del_boundary"})
        }
        control_boundary_kinds = {
            "if",
            "else",
            "end_if",
            "jump",
            "label",
            "loop_start",
            "loop_index_start",
            "loop_index_next",
            "loop_break_if_true",
            "loop_break_if_false",
            "loop_break_if_exception",
            "loop_break",
            "loop_continue",
            "loop_end",
        }

        def split_root(name: Any) -> str | None:
            if not isinstance(name, str):
                return None
            if name in candidates:
                candidate = candidates[name]
                return name if candidate.region == current_region else None
            alias = alias_to_split.get(name)
            if alias is None:
                return None
            root, region = alias
            return root if region == current_region else None

        def invalidate_branch_locals(depth: int) -> None:
            for var, (root, assigned_depth) in list(local_to_split.items()):
                if assigned_depth >= depth:
                    split_locals_crossing_control[var] = root
                    local_to_split.pop(var, None)
            for var, (_value, assigned_depth) in list(local_to_const_string.items()):
                if assigned_depth >= depth:
                    local_to_const_string.pop(var, None)

        def invalidate_loop_crossing_locals() -> None:
            for var, (root, _assigned_depth) in local_to_split.items():
                split_locals_crossing_control[var] = root
            local_to_split.clear()
            local_to_const_string.clear()

        for op_index, op in enumerate(json_ops):
            kind = op.get("kind")
            if kind in control_boundary_kinds:
                current_region += 1
                alias_to_split.clear()
                if kind == "if":
                    control_depth += 1
                elif kind == "else":
                    invalidate_branch_locals(control_depth)
                elif kind == "end_if":
                    invalidate_branch_locals(control_depth)
                    control_depth = max(0, control_depth - 1)
                else:
                    invalidate_loop_crossing_locals()
            out = op.get("out")
            if kind == "const" and isinstance(out, str):
                value = op.get("value")
                if isinstance(value, int) and not isinstance(value, bool):
                    const_ints[out] = value
                if isinstance(value, str):
                    const_strings[out] = value
                args = op.get("args")
                if (
                    value is None
                    and isinstance(args, list)
                    and args
                    and isinstance(args[0], int)
                    and not isinstance(args[0], bool)
                ):
                    const_ints[out] = args[0]
                continue
            if kind == "const_str" and isinstance(out, str):
                s_value = op.get("s_value")
                if isinstance(s_value, str):
                    const_strings[out] = s_value
                continue
            if kind == "const_none" and isinstance(out, str):
                const_nones.add(out)
                continue
            if kind == "string_split" and isinstance(out, str):
                args = op.get("args")
                if (
                    isinstance(args, list)
                    and len(args) == 2
                    and isinstance(args[1], str)
                    and args[1] in const_strings
                    and args[1] not in const_nones
                ):
                    candidates[out] = SplitCandidate(
                        op_index, args[0], args[1], current_region
                    )
                continue
            if kind == "store_var":
                args = op.get("args")
                var = op.get("var")
                root = (
                    split_root(args[0])
                    if isinstance(args, list) and len(args) == 1
                    else None
                )
                if root is not None and isinstance(var, str):
                    local_to_split[var] = (root, control_depth)
                    split_locals_crossing_control.pop(var, None)
                    candidates[root].alias_op_indexes.add(op_index)
                elif isinstance(var, str):
                    local_to_split.pop(var, None)
                    split_locals_crossing_control.pop(var, None)
                if (
                    isinstance(var, str)
                    and isinstance(args, list)
                    and len(args) == 1
                    and isinstance(args[0], str)
                    and args[0] in const_strings
                ):
                    local_to_const_string[var] = (
                        const_strings[args[0]],
                        control_depth,
                    )
                elif isinstance(var, str):
                    local_to_const_string.pop(var, None)
                continue
            if kind == "load_var":
                var = op.get("var")
                if isinstance(var, str) and isinstance(out, str):
                    if out in cleanup_load_outputs:
                        continue
                    escaped_root = split_locals_crossing_control.get(var)
                    if escaped_root is not None and escaped_root in candidates:
                        candidates[escaped_root].unsafe = True
                    split_alias = local_to_split.get(var)
                    if split_alias is not None:
                        root, _assigned_depth = split_alias
                        alias_to_split[out] = (root, current_region)
                        candidates[root].alias_op_indexes.add(op_index)
                    const_string = local_to_const_string.get(var)
                    if const_string is not None:
                        const_strings[out] = const_string[0]
                continue

            args = op.get("args")
            if not isinstance(args, list):
                continue
            arg_roots = [split_root(arg) for arg in args]
            used_roots = {root for root in arg_roots if root is not None}
            if not used_roots:
                continue
            if kind == "index" and len(args) >= 2 and arg_roots[0] is not None:
                root = arg_roots[0]
                index_value = const_ints.get(args[1])
                if index_value is not None and index_value >= 0:
                    candidates[root].index_values_by_op_index[op_index] = index_value
                    for other_root in used_roots - {root}:
                        candidates[other_root].unsafe = True
                else:
                    candidates[root].unsafe = True
                    for other_root in used_roots - {root}:
                        candidates[other_root].unsafe = True
            elif kind == "guard_tag" and len(args) >= 1 and arg_roots[0] is not None:
                root = arg_roots[0]
                candidates[root].alias_op_indexes.add(op_index)
                for other_root in used_roots - {root}:
                    candidates[other_root].unsafe = True
            else:
                for root in used_roots:
                    candidates[root].unsafe = True

        replace_indexes: dict[int, SplitCandidate] = {}
        remove_indexes: set[int] = set()
        for root, candidate in candidates.items():
            if candidate.unsafe or not candidate.index_values_by_op_index:
                continue
            remove_indexes.add(candidate.op_index)
            remove_indexes.update(candidate.alias_op_indexes)
            for op_index in candidate.index_values_by_op_index:
                replace_indexes[op_index] = candidate

        if not remove_indexes and not replace_indexes:
            return json_ops

        rewritten: list[dict[str, Any]] = []
        materialized_fields: dict[tuple[int, int], str] = {}
        for op_index, op in enumerate(json_ops):
            candidate = replace_indexes.get(op_index)
            if candidate is not None:
                args = op.get("args", [])
                output = op.get("out")
                field_key = (
                    candidate.op_index,
                    candidate.index_values_by_op_index[op_index],
                )
                existing_output = materialized_fields.get(field_key)
                if existing_output is None:
                    rewritten_op = {
                        "kind": "string_split_field",
                        "args": [candidate.hay, candidate.sep, args[1]],
                        "out": output,
                    }
                    if isinstance(output, str):
                        materialized_fields[field_key] = output
                else:
                    rewritten_op = {
                        "kind": "copy_var",
                        "args": [existing_output],
                        "out": output,
                    }
                if "source_line" in op:
                    rewritten_op["source_line"] = op["source_line"]
                if "col_offset" in op:
                    rewritten_op["col_offset"] = op["col_offset"]
                if "end_col_offset" in op:
                    rewritten_op["end_col_offset"] = op["end_col_offset"]
                rewritten.append(rewritten_op)
                continue
            if op_index in remove_indexes:
                if op.get("kind") == "string_split":
                    validate_op = {
                        "kind": "string_split_validate",
                        "args": op.get("args", []),
                        "out": op.get("out"),
                    }
                    if "source_line" in op:
                        validate_op["source_line"] = op["source_line"]
                    if "col_offset" in op:
                        validate_op["col_offset"] = op["col_offset"]
                    if "end_col_offset" in op:
                        validate_op["end_col_offset"] = op["end_col_offset"]
                    rewritten.append(validate_op)
                continue
            rewritten.append(op)
        return rewritten

    @staticmethod
    def _fuse_string_split_field_consumers_json(
        json_ops: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        const_strings: dict[str, str] = {}
        # Values provably equal to the integer 0 / boolean False at lowering time
        # (the `has_base` flag of a no-base `int(field)` is a CONST_BOOL False).
        const_falsey: set[str] = set()
        use_counts: dict[str, int] = {}
        split_fields_by_out: dict[str, tuple[int, list[str]]] = {}

        for op_index, op in enumerate(json_ops):
            out = op.get("out")
            kind = op.get("kind")
            if kind == "const_str" and isinstance(out, str):
                s_value = op.get("s_value")
                if isinstance(s_value, str):
                    const_strings[out] = s_value
            if kind in ("const", "const_bool") and isinstance(out, str):
                value = op.get("value")
                if value in (0, False):
                    const_falsey.add(out)
            args = op.get("args")
            if isinstance(args, list):
                for arg in args:
                    if isinstance(arg, str):
                        use_counts[arg] = use_counts.get(arg, 0) + 1
            if kind == "string_split_field" and isinstance(out, str):
                if isinstance(args, list) and len(args) == 3:
                    str_args = [arg for arg in args if isinstance(arg, str)]
                    if len(str_args) == 3:
                        split_fields_by_out[out] = (op_index, str_args)

        remove_indexes: set[int] = set()
        replace_ops: dict[int, dict[str, Any]] = {}

        def copy_source_site(src: dict[str, Any], dst: dict[str, Any]) -> None:
            if "source_line" in src:
                dst["source_line"] = src["source_line"]
            if "col_offset" in src:
                dst["col_offset"] = src["col_offset"]
            if "end_col_offset" in src:
                dst["end_col_offset"] = src["end_col_offset"]

        for op_index, op in enumerate(json_ops):
            args = op.get("args")
            if not isinstance(args, list):
                continue
            kind = op.get("kind")
            if (
                kind == "len"
                and len(args) == 1
                and isinstance(args[0], str)
                and use_counts.get(args[0], 0) == 1
            ):
                field = split_fields_by_out.get(args[0])
                if field is not None:
                    field_op_index, field_args = field
                    rewritten = {
                        "kind": "string_split_field_len",
                        "args": field_args,
                        "out": op.get("out"),
                    }
                    copy_source_site(op, rewritten)
                    replace_ops[op_index] = rewritten
                    remove_indexes.add(field_op_index)
                continue
            if kind == "eq" and len(args) == 2:
                left, right = args
                field_var: str | None = None
                expected_var: str | None = None
                if (
                    isinstance(left, str)
                    and isinstance(right, str)
                    and right in const_strings
                    and use_counts.get(left, 0) == 1
                    and left in split_fields_by_out
                ):
                    field_var = left
                    expected_var = right
                elif (
                    isinstance(left, str)
                    and isinstance(right, str)
                    and left in const_strings
                    and use_counts.get(right, 0) == 1
                    and right in split_fields_by_out
                ):
                    field_var = right
                    expected_var = left
                if field_var is not None and expected_var is not None:
                    field_op_index, field_args = split_fields_by_out[field_var]
                    rewritten = {
                        "kind": "string_split_field_eq",
                        "args": [*field_args, expected_var],
                        "out": op.get("out"),
                    }
                    copy_source_site(op, rewritten)
                    replace_ops[op_index] = rewritten
                    remove_indexes.add(field_op_index)
                continue
            # int(s.split(sep)[k]) — parse the int directly from the field bytes,
            # eliminating the per-field `alloc_string`. `int_from_obj` args are
            # [value, base, has_base]; fire only when the field is read once here
            # and `has_base` is provably False (no explicit base ⇒ base 10, which
            # `string_split_field_to_int` matches byte-for-byte). The explicit-base
            # form keeps the materializing path (correct, just not specialized).
            if (
                kind == "int_from_obj"
                and len(args) == 3
                and isinstance(args[0], str)
                and isinstance(args[2], str)
                and args[2] in const_falsey
                and use_counts.get(args[0], 0) == 1
                and args[0] in split_fields_by_out
            ):
                field_op_index, field_args = split_fields_by_out[args[0]]
                rewritten = {
                    "kind": "string_split_field_to_int",
                    "args": field_args,
                    "out": op.get("out"),
                }
                copy_source_site(op, rewritten)
                replace_ops[op_index] = rewritten
                remove_indexes.add(field_op_index)

        if not remove_indexes and not replace_ops:
            return json_ops

        rewritten_ops: list[dict[str, Any]] = []
        for op_index, op in enumerate(json_ops):
            if op_index in remove_indexes:
                continue
            replacement = replace_ops.get(op_index)
            rewritten_ops.append(replacement if replacement is not None else op)
        return rewritten_ops

    def map_ops_to_json(
        self,
        ops: list[MoltOp],
        *,
        function_name: str | None = None,
        run_midend: bool = True,
    ) -> list[dict[str, Any]]:
        if function_name is not None:
            self._active_midend_function_name = function_name
        else:
            self._active_midend_function_name = "<direct>"
        if run_midend:
            ops = self._run_ir_midend_passes(ops)
        ops, fused_dict_guard_prunes = (
            self._eliminate_redundant_fused_dict_increment_guards(ops)
        )
        if fused_dict_guard_prunes:
            self.midend_stats["fused_dict_guard_prunes"] = (
                self.midend_stats.get("fused_dict_guard_prunes", 0)
                + fused_dict_guard_prunes
            )
            func_stats = self._midend_function_stats()
            func_stats["fused_dict_guard_prunes"] += fused_dict_guard_prunes
        json_ops: list[dict[str, Any]] = []
        json_list_int_containers = set(getattr(self, "_list_int_containers", set()))
        emit_function_frame = self._function_needs_frame_trace(function_name)

        def field_offset(expected_class: str, attr: str) -> int | None:
            class_info = self.classes.get(expected_class)
            if not class_info:
                return None
            return class_info.get("fields", {}).get(attr)

        def control_value(op: MoltOp) -> int:
            raw = op.args[0]
            if isinstance(raw, bool):
                return int(raw)
            if isinstance(raw, int):
                return raw
            if isinstance(raw, str):
                text = raw.strip()
                if text.startswith(("+", "-")):
                    sign = text[0]
                    digits = text[1:]
                    if digits.isdigit():
                        return int(f"{sign}{digits}")
                elif text.isdigit():
                    return int(text)
            raise RuntimeError(
                f"Control-flow op {op.kind} requires int label, got {raw!r} ({type(raw).__name__})"
            )

        def carry_bound_local(op: MoltOp, entry: dict[str, Any]) -> dict[str, Any]:
            if (op.metadata or {}).get("bound_local"):
                entry["bound_local"] = True
            return entry

        # The midend LICM pass hoists CONST_NONE ops out of loops, and the
        # CSE then merges them with earlier CONST_NONE ops in the pre-loop
        # block.  This creates an alias (e.g. v187 -> v182) but the alias
        # only applies within the pre-loop block.  The IS instruction that
        # originally used v187 remains inside the loop, now referencing an
        # undefined variable.  The Cranelift backend defaults undefined i64
        # variables to 0, and since box_none() != 0, the IS(exc, none) check
        # always returns False, causing raise_if_pending to fire spuriously
        # with a TypeError.
        #
        # Fix: collect all variable names produced by CONST_NONE ops after
        # the midend.  In the lowering loop, re-emit const_none immediately
        # before every IS instruction that references one of these variables
        # to guarantee the definition and use share the same Cranelift block.
        const_none_vars: set[str] = {
            o.result.name
            for o in ops
            if o.kind == "CONST_NONE" and o.result.name != "none"
        }
        # Also collect variables that WERE produced by CONST_NONE but whose
        # definition was eliminated by DCE/CSE.  These are variables
        # referenced in IS args whose names are NOT defined by any op.
        defined_vars: set[str] = {o.result.name for o in ops if o.result.name != "none"}
        for o in ops:
            if o.kind == "IS":
                for a in o.args:
                    if (
                        isinstance(a, MoltValue)
                        and a.type_hint == "None"
                        and a.name not in defined_vars
                    ):
                        const_none_vars.add(a.name)

        # Track json_ops start index for each MoltOp so we can inject
        # expression-level col_offset after the main serialization loop.
        _col_inject: list[tuple[int, MoltOp]] = []
        for op in ops:
            _col_inject.append((len(json_ops), op))
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
                    json_ops.append(
                        {
                            "kind": "const_bigint",
                            "s_value": str(value),
                            "out": op.result.name,
                        }
                    )
                else:
                    json_ops.append(
                        {"kind": "const", "value": value, "out": op.result.name}
                    )
            elif op.kind == "CONST_BIGINT":
                json_ops.append(
                    {
                        "kind": "const_bigint",
                        "s_value": op.args[0],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONST_BOOL":
                value = 1 if op.args[0] else 0
                json_ops.append(
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
                json_ops.append(
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
                    json_ops.append(
                        {"kind": "const_str", "bytes": list(raw), "out": op.result.name}
                    )
                else:
                    json_ops.append(
                        {"kind": "const_str", "s_value": value, "out": op.result.name}
                    )
            elif op.kind == "CONST_BYTES":
                json_ops.append(
                    {
                        "kind": "const_bytes",
                        "bytes": list(op.args[0]),
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONST_NONE":
                json_ops.append({"kind": "const_none", "out": op.result.name})
            elif op.kind == "CONST_NOT_IMPLEMENTED":
                json_ops.append(
                    {"kind": "const_not_implemented", "out": op.result.name}
                )
            elif op.kind == "CONST_ELLIPSIS":
                json_ops.append({"kind": "const_ellipsis", "out": op.result.name})
            elif op.kind in ("ADD", "SUB", "MUL"):
                # Compile-time constant fold: when both operands are known
                # integer constants and the result overflows the 47-bit signed
                # inline range, emit const_bigint instead of the arithmetic op.
                # This prevents Cranelift 0.130 from miscompiling the overflow
                # check during its constant-folding pass.
                _arith_folded = False
                if len(op.args) == 2 and self._should_fast_int(op):
                    lhs_arg, rhs_arg = op.args
                    if isinstance(lhs_arg, MoltValue) and isinstance(
                        rhs_arg, MoltValue
                    ):
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
                                json_ops.append(
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
                    json_ops.append(entry)
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
                json_ops.append(entry)
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
                json_ops.append(div_entry)
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
                json_ops.append(floordiv_entry)
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
                json_ops.append(mod_entry)
            elif op.kind in ("POW", "INPLACE_POW"):
                json_ops.append(
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
                json_ops.append(bit_or_entry)
            elif op.kind == "INPLACE_BIT_OR":
                bit_or_entry: dict[str, Any] = {
                    "kind": "inplace_bit_or",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    bit_or_entry["fast_int"] = True
                json_ops.append(bit_or_entry)
            elif op.kind == "BIT_AND":
                bit_and_entry: dict[str, Any] = {
                    "kind": "bit_and",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    bit_and_entry["fast_int"] = True
                json_ops.append(bit_and_entry)
            elif op.kind == "INPLACE_BIT_AND":
                bit_and_entry: dict[str, Any] = {
                    "kind": "inplace_bit_and",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    bit_and_entry["fast_int"] = True
                json_ops.append(bit_and_entry)
            elif op.kind == "BIT_XOR":
                bit_xor_entry: dict[str, Any] = {
                    "kind": "bit_xor",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    bit_xor_entry["fast_int"] = True
                json_ops.append(bit_xor_entry)
            elif op.kind == "INPLACE_BIT_XOR":
                bit_xor_entry: dict[str, Any] = {
                    "kind": "inplace_bit_xor",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    bit_xor_entry["fast_int"] = True
                json_ops.append(bit_xor_entry)
            elif op.kind in ("LSHIFT", "INPLACE_LSHIFT"):
                lshift_entry: dict[str, Any] = {
                    "kind": "lshift" if op.kind == "LSHIFT" else "inplace_lshift",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    lshift_entry["fast_int"] = True
                json_ops.append(lshift_entry)
            elif op.kind in ("RSHIFT", "INPLACE_RSHIFT"):
                rshift_entry: dict[str, Any] = {
                    "kind": "rshift" if op.kind == "RSHIFT" else "inplace_rshift",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    rshift_entry["fast_int"] = True
                json_ops.append(rshift_entry)
            elif op.kind in ("MATMUL", "INPLACE_MATMUL"):
                json_ops.append(
                    {
                        "kind": "matmul" if op.kind == "MATMUL" else "inplace_matmul",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "POW_MOD":
                json_ops.append(
                    {
                        "kind": "pow_mod",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ROUND":
                json_ops.append(
                    {
                        "kind": "round",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TRUNC":
                json_ops.append(
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
                json_ops.append(lt_entry)
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
                json_ops.append(le_entry)
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
                json_ops.append(gt_entry)
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
                json_ops.append(ge_entry)
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
                json_ops.append(eq_entry)
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
                json_ops.append(ne_entry)
            elif op.kind == "STRING_EQ":
                json_ops.append(
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
                    if isinstance(a, MoltValue) and a.name in const_none_vars:
                        json_ops.append({"kind": "const_none", "out": a.name})
                json_ops.append(
                    {
                        "kind": "is",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INVERT":
                json_ops.append(
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
                json_ops.append(entry)
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
                json_ops.append(entry)
            elif op.kind == "NOT":
                json_ops.append(
                    {
                        "kind": "not",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BOOL":
                json_ops.append(
                    {
                        "kind": "bool",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ABS":
                json_ops.append(
                    {
                        "kind": "abs",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "AND":
                json_ops.append(
                    {
                        "kind": "and",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "OR":
                json_ops.append(
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
                json_ops.append(entry)
            elif op.kind == "IF":
                _if_entry: dict[str, Any] = {"kind": "if", "args": [op.args[0].name]}
                _if_cond = op.args[0]
                if isinstance(_if_cond, MoltValue) and _if_cond.type_hint in {
                    "int",
                    "bool",
                }:
                    _if_entry["type_hint"] = _if_cond.type_hint
                json_ops.append(_if_entry)
            elif op.kind == "ELSE":
                json_ops.append({"kind": "else"})
            elif op.kind == "END_IF":
                json_ops.append({"kind": "end_if"})
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
                json_ops.append(d)
            elif op.kind == "TRACE_ENTER_SLOT":
                json_ops.append({"kind": "trace_enter_slot", "value": int(op.args[0])})
            elif op.kind == "TRACE_EXIT":
                json_ops.append({"kind": "trace_exit"})
            elif op.kind == "FRAME_LOCALS_SET":
                json_ops.append(
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
                json_ops.append(entry)
            elif op.kind == "CALL_INTERNAL":
                target = op.args[0]
                code_id = self.func_code_ids.get(target, 0)
                json_ops.append(
                    {
                        "kind": "call_internal",
                        "s_value": target,
                        "args": [arg.name for arg in op.args[1:]],
                        "value": code_id,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_INDIRECT":
                json_ops.append(
                    {
                        "kind": "call_indirect",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_GUARDED":
                target = op.metadata["target"] if op.metadata else ""
                json_ops.append(
                    {
                        "kind": "call_guarded",
                        "s_value": target,
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_FUNC":
                json_ops.append(
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
                json_ops.append(invoke_op)
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
                json_ops.append(entry)
            elif op.kind == "DEL_BOUNDARY":
                entry = {
                    "kind": "del_boundary",
                    "args": [arg.name for arg in op.args],
                }
                boundary_var = (op.metadata or {}).get("var")
                if boundary_var:
                    entry["s_value"] = boundary_var
                json_ops.append(entry)
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
                json_ops.append(entry)
            elif op.kind == "BUILTIN_FUNC":
                func_name, arity = op.args
                json_ops.append(
                    {
                        "kind": "builtin_func",
                        "s_value": func_name,
                        "value": arity,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUNC_NEW":
                func_name, arity = op.args
                json_ops.append(
                    {
                        "kind": "func_new",
                        "s_value": func_name,
                        "value": arity,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUNC_NEW_CLOSURE":
                func_name, arity, closure = op.args
                json_ops.append(
                    {
                        "kind": "func_new_closure",
                        "s_value": func_name,
                        "value": arity,
                        "args": [closure.name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CODE_NEW":
                json_ops.append(
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
                json_ops.append(
                    {
                        "kind": "code_slot_set",
                        "value": code_id,
                        "args": [op.args[0].name],
                    }
                )
            elif op.kind == "FN_PTR_CODE_SET":
                func_name, code_val = op.args
                json_ops.append(
                    {
                        "kind": "fn_ptr_code_set",
                        "s_value": func_name,
                        "args": [code_val.name],
                    }
                )
            elif op.kind == "ASYNCGEN_LOCALS_REGISTER":
                func_name, names_tuple, offsets_tuple = op.args
                json_ops.append(
                    {
                        "kind": "asyncgen_locals_register",
                        "s_value": func_name,
                        "args": [names_tuple.name, offsets_tuple.name],
                    }
                )
            elif op.kind == "GEN_LOCALS_REGISTER":
                func_name, names_tuple, offsets_tuple = op.args
                json_ops.append(
                    {
                        "kind": "gen_locals_register",
                        "s_value": func_name,
                        "args": [names_tuple.name, offsets_tuple.name],
                    }
                )
            elif op.kind == "CODE_SLOTS_INIT":
                json_ops.append(
                    {
                        "kind": "code_slots_init",
                        "value": int(op.args[0]),
                    }
                )
            elif op.kind == "CLASS_NEW":
                json_ops.append(
                    {
                        "kind": "class_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_SET_BASE":
                json_ops.append(
                    {
                        "kind": "class_set_base",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_APPLY_SET_NAME":
                json_ops.append(
                    {
                        "kind": "class_apply_set_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_DEF":
                json_ops.append(
                    {
                        "kind": "class_def",
                        "args": [
                            arg.name if isinstance(arg, MoltValue) else str(arg)
                            for arg in op.args
                        ],
                        "s_value": op.metadata["s_value"] if op.metadata else "",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SUPER_NEW":
                json_ops.append(
                    {
                        "kind": "super_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MISSING":
                json_ops.append(
                    {
                        "kind": "missing",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "COPY":
                json_ops.append(
                    {
                        "kind": "copy",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUNCTION_CLOSURE_BITS":
                json_ops.append(
                    {
                        "kind": "function_closure_bits",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUILTIN_TYPE":
                json_ops.append(
                    {
                        "kind": "builtin_type",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TYPE_OF":
                json_ops.append(
                    {
                        "kind": "type_of",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_VERSION":
                json_ops.append(
                    {
                        "kind": "class_layout_version",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_SET_LAYOUT_VERSION":
                json_ops.append(
                    {
                        "kind": "class_set_layout_version",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_MERGE_LAYOUT":
                json_ops.append(
                    {
                        "kind": "class_merge_layout",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GUARD_LAYOUT":
                json_ops.append(
                    {
                        "kind": "guard_layout",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ISINSTANCE":
                json_ops.append(
                    {
                        "kind": "isinstance",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_MATCH_BUILTIN":
                metadata = op.metadata or {}
                json_ops.append(
                    {
                        "kind": "exception_match_builtin",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                        "s_value": metadata.get("exception_name", "Exception"),
                        "value": int(metadata.get("exception_tag", 2)),
                    }
                )
            elif op.kind == "ISSUBCLASS":
                json_ops.append(
                    {
                        "kind": "issubclass",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "OBJECT_NEW":
                json_ops.append(
                    {"kind": "object_new", "args": [], "out": op.result.name}
                )
            elif op.kind == "OBJECT_NEW_BOUND":
                # Phase-1-sibling class-instantiation fast path:
                # `Point(args)` for a known non-dynamic class lowers to
                # `OBJECT_NEW_BOUND(class_ref)` (allocates instance with
                # the right type tag) followed by a direct `CALL` to
                # `__init__`'s symbol — bypassing
                # `type.__call__` → bound-method-init → CALL_BIND.
                # See `_try_emit_class_static_call` for the predicates.
                # The optional `value` field carries the static
                # class-instance payload size in bytes (header NOT
                # included), which the escape-analysis-rewritten
                # `object_new_bound_stack` lowering uses to size the
                # Cranelift StackSlot.  Heap arm ignores it.
                _onb_op: dict[str, Any] = {
                    "kind": "object_new_bound",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                _onb_class = op.result.type_hint
                if _onb_class and _onb_class != "Any":
                    _onb_op["type_hint"] = _onb_class
                _onb_md = op.metadata or {}
                _onb_size = _onb_md.get("class_size_bytes")
                if isinstance(_onb_size, int) and _onb_size > 0:
                    _onb_op["value"] = _onb_size
                # A class that defines `__del__` (directly or anywhere in its MRO
                # except `object`) has a finalizer that CPython runs at the last
                # reference drop.  The backend escape pass would otherwise treat a
                # non-escaping instance as `NoEscape` and (a) strip its IncRef/
                # DecRef and/or (b) rewrite it to a stack-allocated immortal
                # object — either of which makes the refcount-zero transition
                # never occur, so `__del__` would silently never run.  Carry the
                # finalizer fact so escape analysis keeps such instances
                # heap-allocated with a live refcount (the finalizer-aware
                # `dec_ref_ptr` then dispatches `__del__` at the last drop).  The
                # heap `object_new_bound` fast path (and inlined `__init__`) is
                # preserved — only the unsound stack/RC-strip is suppressed.
                if _onb_class and _onb_class != "Any":
                    _del_info, _del_owner = self._resolve_method_info(
                        _onb_class, "__del__"
                    )
                    if _del_info is not None and _del_owner != "object":
                        _onb_op["defines_del"] = True
                if (op.metadata or {}).get("bound_local"):
                    _onb_op["bound_local"] = True
                json_ops.append(_onb_op)
            elif op.kind == "CLASSMETHOD_NEW":
                json_ops.append(
                    {
                        "kind": "classmethod_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATICMETHOD_NEW":
                json_ops.append(
                    {
                        "kind": "staticmethod_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROPERTY_NEW":
                json_ops.append(
                    {
                        "kind": "property_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BOUND_METHOD_NEW":
                json_ops.append(
                    {
                        "kind": "bound_method_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_NEW":
                json_ops.append(
                    {
                        "kind": "module_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_CACHE_GET":
                entry = {
                    "kind": "module_cache_get",
                    "args": [op.args[0].name],
                    "out": op.result.name,
                }
                if op.metadata is not None:
                    effect_proof = op.metadata.get("effect_proof")
                    if isinstance(effect_proof, str) and effect_proof:
                        entry["effect_proof"] = effect_proof
                json_ops.append(entry)
            elif op.kind == "MODULE_IMPORT":
                json_ops.append(
                    {
                        "kind": "module_import",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_CACHE_SET":
                json_ops.append(
                    {
                        "kind": "module_cache_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_CACHE_DEL":
                json_ops.append(
                    {
                        "kind": "module_cache_del",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_GET_ATTR":
                entry = {
                    "kind": "module_get_attr",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if op.metadata is not None:
                    effect_proof = op.metadata.get("effect_proof")
                    if isinstance(effect_proof, str) and effect_proof:
                        entry["effect_proof"] = effect_proof
                json_ops.append(entry)
            elif op.kind == "MODULE_IMPORT_FROM":
                json_ops.append(
                    {
                        "kind": "module_import_from",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_GET_GLOBAL":
                json_ops.append(
                    {
                        "kind": "module_get_global",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_DEL_GLOBAL":
                json_ops.append(
                    {
                        "kind": "module_del_global",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_DEL_GLOBAL_IF_PRESENT":
                json_ops.append(
                    {
                        "kind": "module_del_global_if_present",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_SET_ATTR":
                json_ops.append(
                    {
                        "kind": "module_set_attr",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_IMPORT_STAR":
                json_ops.append(
                    {
                        "kind": "module_import_star",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_NULL":
                json_ops.append(
                    {
                        "kind": "context_null",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_ENTER":
                json_ops.append(
                    {
                        "kind": "context_enter",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_EXIT":
                json_ops.append(
                    {
                        "kind": "context_exit",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_UNWIND":
                json_ops.append(
                    {
                        "kind": "context_unwind",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_DEPTH":
                json_ops.append({"kind": "context_depth", "out": op.result.name})
            elif op.kind == "CONTEXT_UNWIND_TO":
                json_ops.append(
                    {
                        "kind": "context_unwind_to",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_CLOSING":
                json_ops.append(
                    {
                        "kind": "context_closing",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_PUSH":
                json_ops.append({"kind": "exception_push", "out": op.result.name})
            elif op.kind == "EXCEPTION_POP":
                json_ops.append({"kind": "exception_pop", "out": op.result.name})
            elif op.kind == "EXCEPTION_STACK_CLEAR":
                json_ops.append(
                    {"kind": "exception_stack_clear", "out": op.result.name}
                )
            elif op.kind == "EXCEPTION_STACK_ENTER":
                json_ops.append(
                    {"kind": "exception_stack_enter", "out": op.result.name}
                )
            elif op.kind == "EXCEPTION_STACK_EXIT":
                json_ops.append(
                    {
                        "kind": "exception_stack_exit",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_STACK_DEPTH":
                json_ops.append(
                    {"kind": "exception_stack_depth", "out": op.result.name}
                )
            elif op.kind == "EXCEPTION_STACK_SET_DEPTH":
                json_ops.append(
                    {
                        "kind": "exception_stack_set_depth",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_LAST":
                json_ops.append({"kind": "exception_last", "out": op.result.name})
            elif op.kind == "EXCEPTION_LAST_PENDING":
                json_ops.append(
                    {"kind": "exception_last_pending", "out": op.result.name}
                )
            elif op.kind == "EXCEPTION_FINALLY_PENDING_OBSERVER":
                json_ops.append(
                    {
                        "kind": "exception_finally_pending_observer",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_NEW":
                json_ops.append(
                    {
                        "kind": "exception_new",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_NEW_BUILTIN":
                metadata = op.metadata or {}
                json_ops.append(
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
                json_ops.append(
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
                json_ops.append(
                    {
                        "kind": "exception_new_builtin_one",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                        "s_value": metadata.get("exception_name", "Exception"),
                        "value": int(metadata.get("exception_tag", 2)),
                    }
                )
            elif op.kind == "EXCEPTION_NEW_FROM_CLASS":
                json_ops.append(
                    {
                        "kind": "exception_new_from_class",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTIONGROUP_MATCH":
                json_ops.append(
                    {
                        "kind": "exceptiongroup_match",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTIONGROUP_COMBINE":
                json_ops.append(
                    {
                        "kind": "exceptiongroup_combine",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_SET_CAUSE":
                json_ops.append(
                    {
                        "kind": "exception_set_cause",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_SET_LAST":
                json_ops.append(
                    {
                        "kind": "exception_set_last",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_CONTEXT_SET":
                json_ops.append(
                    {
                        "kind": "exception_context_set",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_CLEAR":
                json_ops.append({"kind": "exception_clear", "out": op.result.name})
            elif op.kind == "EXCEPTION_KIND":
                json_ops.append(
                    {
                        "kind": "exception_kind",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_CLASS":
                json_ops.append(
                    {
                        "kind": "exception_class",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_MESSAGE":
                json_ops.append(
                    {
                        "kind": "exception_message",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "RAISE":
                json_ops.append(
                    {
                        "kind": "raise",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TRY_START":
                payload: dict[str, Any] = {"kind": "try_start"}
                if op.args:
                    payload["value"] = control_value(op)
                json_ops.append(payload)
            elif op.kind == "TRY_END":
                payload: dict[str, Any] = {"kind": "try_end"}
                if op.args:
                    payload["value"] = control_value(op)
                json_ops.append(payload)
            elif op.kind == "LABEL":
                json_ops.append({"kind": "label", "value": control_value(op)})
            elif op.kind == "STATE_LABEL":
                json_ops.append({"kind": "state_label", "value": control_value(op)})
            elif op.kind == "JUMP":
                json_ops.append({"kind": "jump", "value": control_value(op)})
            elif op.kind == "PHI":
                json_ops.append(
                    {
                        "kind": "phi",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHECK_EXCEPTION":
                json_ops.append({"kind": "check_exception", "value": control_value(op)})
            elif op.kind == "FILE_OPEN":
                json_ops.append(
                    {
                        "kind": "file_open",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FILE_READ":
                json_ops.append(
                    {
                        "kind": "file_read",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FILE_WRITE":
                json_ops.append(
                    {
                        "kind": "file_write",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FILE_CLOSE":
                json_ops.append(
                    {
                        "kind": "file_close",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FILE_FLUSH":
                json_ops.append(
                    {
                        "kind": "file_flush",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ENV_GET":
                json_ops.append(
                    {
                        "kind": "env_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PRINT":
                json_ops.append(
                    {
                        "kind": "print",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                    }
                )
            elif op.kind == "PRINT_NEWLINE":
                json_ops.append({"kind": "print_newline"})
            elif op.kind == "WARN_STDERR":
                if os.environ.get("MOLT_DEBUG_WARN"):
                    import sys as _sys

                    print(
                        f"[WARN_SERIALIZE] warn_stderr arg={op.args[0].name}",
                        file=_sys.stderr,
                    )
                json_ops.append(
                    {
                        "kind": "warn_stderr",
                        "args": [op.args[0].name],
                    }
                )
            elif op.kind == "ALLOC":
                json_ops.append(
                    {
                        "kind": "alloc",
                        "value": self.classes[op.args[0]]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ALLOC_CLASS":
                class_ref, class_id = op.args
                json_ops.append(
                    {
                        "kind": "alloc_class",
                        "args": [class_ref.name],
                        "value": self.classes[class_id]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ALLOC_CLASS_TRUSTED":
                class_ref, class_id = op.args
                json_ops.append(
                    {
                        "kind": "alloc_class_trusted",
                        "args": [class_ref.name],
                        "value": self.classes[class_id]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ALLOC_CLASS_STATIC":
                class_ref, class_id = op.args
                json_ops.append(
                    {
                        "kind": "alloc_class_static",
                        "args": [class_ref.name],
                        "value": self.classes[class_id]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "OBJECT_SET_CLASS":
                json_ops.append(
                    {
                        "kind": "object_set_class",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_NEW":
                json_ops.append(
                    {
                        "kind": "dataclass_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_NEW_VALUES":
                json_ops.append(
                    {
                        "kind": "dataclass_new_values",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SETATTR":
                obj, attr, val, *rest = op.args
                if rest:
                    expected_class = rest[0]
                else:
                    expected_class = list(self.classes.keys())[-1]
                offset = field_offset(expected_class, attr)
                # Metaclass __init__ receives `cls` which is a TYPE object,
                # not an INSTANCE. Field offsets apply to instances, not to
                # the class itself. When the expected class IS the metaclass
                # (a subclass of `type`), emit generic setattr so the
                # attribute goes into the class __dict__.
                _class_info_meta = self.classes.get(expected_class)
                _is_type_subclass = (
                    _class_info_meta is not None
                    and "type" in _class_info_meta.get("bases", [])
                )
                if offset is None or _is_type_subclass:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "store",
                            "args": [obj.name, val.name],
                            "value": offset,
                            # The concrete class whose fixed layout authored this
                            # `offset` (the frontend emits the raw-offset form only
                            # when the object's class is statically proven here).
                            # Carried through TIR so the alias oracle can assign a
                            # class+offset `TypedField` region (S5-1.5).
                            "class": expected_class,
                        }
                    )
            elif op.kind == "SETATTR_INIT":
                obj, attr, val, *rest = op.args
                if rest:
                    expected_class = rest[0]
                else:
                    expected_class = list(self.classes.keys())[-1]
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "store_init",
                            "args": [obj.name, val.name],
                            "value": offset,
                            "class": expected_class,
                        }
                    )
            elif op.kind == "GUARDED_SETATTR":
                obj, class_ref, expected_version, attr, val, expected_class = op.args
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "guarded_field_set",
                            "args": [
                                obj.name,
                                class_ref.name,
                                expected_version.name,
                                val.name,
                            ],
                            "s_value": attr,
                            "value": offset,
                            "out": op.result.name,
                            # The class the runtime version-guard proves at this
                            # op; authority for `offset`. Carried through TIR for
                            # the class+offset `TypedField` alias region (S5-1.5).
                            "class": expected_class,
                        }
                    )
            elif op.kind == "GUARDED_SETATTR_INIT":
                obj, class_ref, expected_version, attr, val, expected_class = op.args
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "guarded_field_init",
                            "args": [
                                obj.name,
                                class_ref.name,
                                expected_version.name,
                                val.name,
                            ],
                            "s_value": attr,
                            "value": offset,
                            "out": op.result.name,
                            "class": expected_class,
                        }
                    )
            elif op.kind == "SETATTR_GENERIC_PTR":
                json_ops.append(
                    {
                        "kind": "set_attr_generic_ptr",
                        "args": [op.args[0].name, op.args[2].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SETATTR_GENERIC_OBJ":
                json_ops.append(
                    {
                        "kind": "set_attr_generic_obj",
                        "args": [op.args[0].name, op.args[2].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DELATTR_GENERIC_PTR":
                json_ops.append(
                    {
                        "kind": "del_attr_generic_ptr",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DELATTR_GENERIC_OBJ":
                json_ops.append(
                    {
                        "kind": "del_attr_generic_obj",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_GET":
                json_ops.append(
                    {
                        "kind": "dataclass_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_SET":
                json_ops.append(
                    {
                        "kind": "dataclass_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_SET_CLASS":
                json_ops.append(
                    {
                        "kind": "dataclass_set_class",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR":
                obj, attr, *rest = op.args
                if rest:
                    expected_class = rest[0]
                else:
                    expected_class = list(self.classes.keys())[-1]
                offset = field_offset(expected_class, attr)
                # Metaclass methods operate on TYPE objects, not instances.
                # Field offsets don't apply — use generic getattr.
                _ga_class_info = self.classes.get(expected_class)
                _ga_is_type_sub = (
                    _ga_class_info is not None
                    and "type" in _ga_class_info.get("bases", [])
                )
                if offset is None or _ga_is_type_sub:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_obj",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        _ic = (
                            op.metadata["ic_index"]
                            if op.metadata and "ic_index" in op.metadata
                            else _next_ic_index()
                        )
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_ptr",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                                "metadata": {"ic_index": _ic},
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "load",
                            "args": [obj.name],
                            "value": offset,
                            "out": op.result.name,
                            # The statically-proven class authoring `offset`.
                            # Carried through TIR for the class+offset
                            # `TypedField` alias region (S5-1.5).
                            "class": expected_class,
                        }
                    )
            elif op.kind == "GUARDED_GETATTR":
                obj, class_ref, expected_version, attr, expected_class = op.args
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_obj",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        _ic = (
                            op.metadata["ic_index"]
                            if op.metadata and "ic_index" in op.metadata
                            else _next_ic_index()
                        )
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_ptr",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                                "metadata": {"ic_index": _ic},
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "guarded_field_get",
                            "args": [obj.name, class_ref.name, expected_version.name],
                            "s_value": attr,
                            "value": offset,
                            "out": op.result.name,
                            "metadata": {"expected_type_id": 100},
                            # The class the runtime version-guard proves at this
                            # op; authority for `offset`. Carried through TIR for
                            # the class+offset `TypedField` alias region (S5-1.5).
                            "class": expected_class,
                        }
                    )
            elif op.kind == "GETATTR_GENERIC_PTR":
                ptr_entry: dict[str, Any] = {
                    "kind": "get_attr_generic_ptr",
                    "args": [op.args[0].name],
                    "s_value": op.args[1],
                    "out": op.result.name,
                }
                if op.metadata and "ic_index" in op.metadata:
                    ptr_entry["metadata"] = {"ic_index": op.metadata["ic_index"]}
                json_ops.append(ptr_entry)
            elif op.kind == "GETATTR_GENERIC_OBJ":
                json_ops.append(
                    {
                        "kind": "get_attr_generic_obj",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUNCTION_DEFAULTS_VERSION":
                # Read a function object's __defaults__/__kwdefaults__ mutation
                # version stamp (one MoltValue operand: the function object).
                # Non-foldable; consumed by the defaults-devirt deopt guard.
                json_ops.append(
                    {
                        "kind": "function_defaults_version",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR_SPECIAL_OBJ":
                json_ops.append(
                    {
                        "kind": "get_attr_special_obj",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR_NAME":
                json_ops.append(
                    {
                        "kind": "get_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR_NAME_DEFAULT":
                json_ops.append(
                    {
                        "kind": "get_attr_name_default",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "HASATTR_NAME":
                json_ops.append(
                    {
                        "kind": "has_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IS_NATIVE_AWAITABLE":
                json_ops.append(
                    {
                        "kind": "is_native_awaitable",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SETATTR_NAME":
                json_ops.append(
                    {
                        "kind": "set_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DELATTR_NAME":
                json_ops.append(
                    {
                        "kind": "del_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GUARD_TYPE":
                json_ops.append(
                    {
                        "kind": "guard_type",
                        "args": [arg.name for arg in op.args],
                    }
                )
            elif op.kind == "GUARD_TAG":
                json_ops.append(
                    {
                        "kind": "guard_tag",
                        "args": [arg.name for arg in op.args],
                    }
                )
            elif op.kind == "GUARD_DICT_SHAPE":
                json_ops.append(
                    {
                        "kind": "guard_dict_shape",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INC_REF":
                json_ops.append(
                    {
                        "kind": "inc_ref",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DEC_REF":
                json_ops.append(
                    {
                        "kind": "dec_ref",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BORROW":
                json_ops.append(
                    {
                        "kind": "inc_ref",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "RELEASE":
                json_ops.append(
                    {
                        "kind": "dec_ref",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind in {
                "BOX",
                "UNBOX",
                "CAST",
                "WIDEN",
            }:
                if (
                    op.args
                    and isinstance(op.args[0], MoltValue)
                    and op.result.name != "none"
                ):
                    lowered_kind = {
                        "BOX": "box",
                        "UNBOX": "unbox",
                        "CAST": "cast",
                        "WIDEN": "widen",
                    }[op.kind]
                    json_ops.append(
                        {
                            "kind": lowered_kind,
                            "args": [op.args[0].name],
                            "out": op.result.name,
                        }
                    )
            elif op.kind == "IDENTITY_ALIAS":
                if (
                    op.args
                    and isinstance(op.args[0], MoltValue)
                    and op.result.name != "none"
                ):
                    json_ops.append(
                        {
                            "kind": "identity_alias",
                            "args": [op.args[0].name],
                            "out": op.result.name,
                        }
                    )
            elif op.kind == "BINDING_ALIAS":
                if (
                    op.args
                    and isinstance(op.args[0], MoltValue)
                    and op.result.name != "none"
                ):
                    json_ops.append(
                        {
                            "kind": "binding_alias",
                            "args": [op.args[0].name],
                            "out": op.result.name,
                        }
                    )
            elif op.kind == "JSON_PARSE":
                json_ops.append(
                    {
                        "kind": "json_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MSGPACK_PARSE":
                json_ops.append(
                    {
                        "kind": "msgpack_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CBOR_PARSE":
                json_ops.append(
                    {
                        "kind": "cbor_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LEN":
                len_entry: dict[str, object] = {
                    "kind": "len",
                    "args": [
                        arg.name if hasattr(arg, "name") else str(arg)
                        for arg in op.args
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
                json_ops.append(len_entry)
            elif op.kind == "ID":
                json_ops.append(
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
                json_ops.append(
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
                json_ops.append(
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
                json_ops.append(
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
                json_ops.append(
                    {
                        "kind": "callargs_new",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_PUSH_POS":
                json_ops.append(
                    {
                        "kind": "callargs_push_pos",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_PUSH_KW":
                json_ops.append(
                    {
                        "kind": "callargs_push_kw",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_EXPAND_STAR":
                json_ops.append(
                    {
                        "kind": "callargs_expand_star",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_EXPAND_KWSTAR":
                json_ops.append(
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
                json_ops.append(carry_bound_local(op, _list_op))
            elif op.kind == "LIST_INT_NEW":
                # Specialized flat i64 list: args are [count, fill_value]
                json_ops.append(
                    {
                        "kind": "list_int_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
                json_list_int_containers.add(op.result.name)
            elif op.kind == "LIST_FILL_NEW":
                json_ops.append(
                    {
                        "kind": "list_fill_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "RANGE_NEW":
                json_ops.append(
                    {
                        "kind": "range_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_FROM_RANGE":
                json_ops.append(
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
                json_ops.append(carry_bound_local(op, _tuple_op))
            elif op.kind == "LIST_APPEND":
                json_ops.append(
                    {
                        "kind": "list_append",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                        "type_hint": "list",
                    }
                )
            elif op.kind == "LIST_POP":
                json_ops.append(
                    {
                        "kind": "list_pop",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_EXTEND":
                json_ops.append(
                    {
                        "kind": "list_extend",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_INSERT":
                json_ops.append(
                    {
                        "kind": "list_insert",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_REMOVE":
                json_ops.append(
                    {
                        "kind": "list_remove",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_CLEAR":
                json_ops.append(
                    {
                        "kind": "list_clear",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_COPY":
                json_ops.append(
                    {
                        "kind": "list_copy",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
                if (
                    op.args
                    and isinstance(op.args[0], MoltValue)
                    and op.args[0].name in json_list_int_containers
                ):
                    json_list_int_containers.add(op.result.name)
            elif op.kind == "LIST_REVERSE":
                json_ops.append(
                    {
                        "kind": "list_reverse",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_COUNT":
                json_ops.append(
                    {
                        "kind": "list_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_INDEX":
                json_ops.append(
                    {
                        "kind": "list_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_INDEX_RANGE":
                json_ops.append(
                    {
                        "kind": "list_index_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_FROM_LIST":
                json_ops.append(
                    {
                        "kind": "tuple_from_list",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "bytes_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_FROM_STR":
                json_ops.append(
                    {
                        "kind": "bytes_from_str",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "bytearray_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FROM_STR":
                json_ops.append(
                    {
                        "kind": "bytearray_from_str",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FILL_RANGE":
                json_ops.append(
                    {
                        "kind": "bytearray_fill_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INTARRAY_FROM_SEQ":
                json_ops.append(
                    {
                        "kind": "intarray_from_seq",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FLOAT_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "float_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INT_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "int_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INT_FROM_STR_OF_OBJ":
                json_ops.append(
                    {
                        "kind": "int_from_str_of_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "COMPLEX_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "complex_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MEMORYVIEW_NEW":
                json_ops.append(
                    {
                        "kind": "memoryview_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MEMORYVIEW_TOBYTES":
                json_ops.append(
                    {
                        "kind": "memoryview_tobytes",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_NEW":
                json_ops.append(
                    carry_bound_local(
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
                json_ops.append(
                    {
                        "kind": "dict_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_NEW":
                json_ops.append(
                    carry_bound_local(
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
                json_ops.append(
                    carry_bound_local(
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
                json_ops.append(
                    {
                        "kind": "dict_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_INC":
                json_ops.append(
                    {
                        "kind": "dict_inc",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_STR_INT_INC":
                json_ops.append(
                    {
                        "kind": "dict_str_int_inc",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_SPLIT_WS_DICT_INC":
                json_ops.append(
                    {
                        "kind": "string_split_ws_dict_inc",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_SPLIT_SEP_DICT_INC":
                json_ops.append(
                    {
                        "kind": "string_split_sep_dict_inc",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TAQ_INGEST_LINE":
                json_ops.append(
                    {
                        "kind": "taq_ingest_line",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_POP":
                json_ops.append(
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
                    and op.args[0].name in json_list_int_containers
                ):
                    value_hint = (
                        op.args[2].type_hint
                        if len(op.args) >= 3 and isinstance(op.args[2], MoltValue)
                        else None
                    )
                    if value_hint == "int":
                        if op.result.name != "none":
                            json_list_int_containers.add(op.result.name)
                    else:
                        json_list_int_containers.discard(op.args[0].name)
                        if op.result.name != "none":
                            json_list_int_containers.discard(op.result.name)
                elif (
                    len(op.args) >= 1
                    and isinstance(op.args[0], MoltValue)
                    and op.args[0].type_hint == "list"
                ):
                    ds_entry["container_type"] = "list"
                json_ops.append(ds_entry)
            elif op.kind == "DICT_SETDEFAULT":
                json_ops.append(
                    {
                        "kind": "dict_setdefault",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_SETDEFAULT_EMPTY_LIST":
                json_ops.append(
                    {
                        "kind": "dict_setdefault_empty_list",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_UPDATE":
                json_ops.append(
                    {
                        "kind": "dict_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_UPDATE_MISSING":
                json_ops.append(
                    {
                        "kind": "dict_update_missing",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_UPDATE_KWSTAR":
                json_ops.append(
                    {
                        "kind": "dict_update_kwstar",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_CLEAR":
                json_ops.append(
                    {
                        "kind": "dict_clear",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_COPY":
                json_ops.append(
                    {
                        "kind": "dict_copy",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_POPITEM":
                json_ops.append(
                    {
                        "kind": "dict_popitem",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_ADD":
                json_ops.append(
                    {
                        "kind": "set_add",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_ADD_PROBE":
                json_ops.append(
                    {
                        "kind": "set_add_probe",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FROZENSET_ADD":
                json_ops.append(
                    {
                        "kind": "frozenset_add",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_DISCARD":
                json_ops.append(
                    {
                        "kind": "set_discard",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_REMOVE":
                json_ops.append(
                    {
                        "kind": "set_remove",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_POP":
                json_ops.append(
                    {
                        "kind": "set_pop",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_INTERSECTION_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_intersection_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_DIFFERENCE_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_difference_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_SYMDIFF_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_symdiff_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_KEYS":
                json_ops.append(
                    {
                        "kind": "dict_keys",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_VALUES":
                json_ops.append(
                    {
                        "kind": "dict_values",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_ITEMS":
                json_ops.append(
                    {
                        "kind": "dict_items",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_COUNT":
                json_ops.append(
                    {
                        "kind": "tuple_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_INDEX":
                json_ops.append(
                    {
                        "kind": "tuple_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ITER_NEW":
                json_ops.append(
                    {
                        "kind": "iter",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                        "type_hint": "iter",
                    }
                )
            elif op.kind == "ENUMERATE":
                json_ops.append(
                    {
                        "kind": "enumerate",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "AITER":
                json_ops.append(
                    {
                        "kind": "aiter",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ITER_NEXT":
                json_ops.append(
                    {
                        "kind": "iter_next",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ANEXT":
                json_ops.append(
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
                    and op.args[0].name in json_list_int_containers
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
                json_ops.append(index_entry)
            elif op.kind == "UNPACK_SEQUENCE":
                # args[0] is the sequence, args[1:] are output variable names
                metadata = op.metadata or {}
                json_ops.append(
                    {
                        "kind": "unpack_sequence",
                        "args": [arg.name for arg in op.args],
                        "value": metadata["expected_count"],
                    }
                )
            elif op.kind == "STORE_INDEX":
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
                    and op.args[0].name in json_list_int_containers
                ):
                    value_hint = (
                        op.args[2].type_hint
                        if len(op.args) >= 3 and isinstance(op.args[2], MoltValue)
                        else None
                    )
                    if value_hint == "int":
                        if op.result.name != "none":
                            json_list_int_containers.add(op.result.name)
                    else:
                        json_list_int_containers.discard(op.args[0].name)
                        if op.result.name != "none":
                            json_list_int_containers.discard(op.result.name)
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
                json_ops.append(si_entry)
            elif op.kind == "DEL_INDEX":
                json_ops.append(
                    {
                        "kind": "del_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_START":
                json_ops.append({"kind": "loop_start"})
            elif op.kind == "LOOP_INDEX_START":
                json_ops.append(
                    {
                        "kind": "loop_index_start",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_INDEX_NEXT":
                json_ops.append(
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
                json_ops.append(_lbit_entry)
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
                json_ops.append(_lbif_entry)
            elif op.kind == "LOOP_BREAK_IF_EXCEPTION":
                # Control op (no value arg) that breaks the loop when a runtime
                # exception is pending.  Lowers to the sacrosanct
                # `molt_exception_pending_fast` flag read in every backend, so
                # it can never be folded/copy-propagated away like a value op.
                json_ops.append({"kind": "loop_break_if_exception"})
            elif op.kind == "LOOP_BREAK":
                json_ops.append({"kind": "loop_break"})
            elif op.kind == "LOOP_CONTINUE":
                json_ops.append({"kind": "loop_continue"})
            elif op.kind == "LOOP_END":
                json_ops.append({"kind": "loop_end"})
            elif op.kind == "VEC_SUM_INT":
                json_ops.append(
                    {
                        "kind": "vec_sum_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_RANGE_ITER":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_range_iter",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_RANGE_ITER_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_range_iter_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_FLOAT":
                json_ops.append(
                    {
                        "kind": "vec_sum_float",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_FLOAT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_float_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_FLOAT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_sum_float_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_FLOAT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_float_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_FLOAT_RANGE_ITER":
                json_ops.append(
                    {
                        "kind": "vec_sum_float_range_iter",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_FLOAT_RANGE_ITER_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_float_range_iter_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT":
                json_ops.append(
                    {
                        "kind": "vec_prod_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_prod_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_prod_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_prod_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT":
                json_ops.append(
                    {
                        "kind": "vec_min_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_min_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_min_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_min_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT":
                json_ops.append(
                    {
                        "kind": "vec_max_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_max_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_max_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_max_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SLICE":
                json_ops.append(
                    {
                        "kind": "slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SLICE_NEW":
                json_ops.append(
                    {
                        "kind": "slice_new",
                        "args": [
                            arg.name if arg is not None else "_molt_none"
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_FIND":
                json_ops.append(
                    {
                        "kind": "bytes_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_FIND_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_find_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FIND":
                json_ops.append(
                    {
                        "kind": "bytearray_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FIND_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_find_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_STARTSWITH":
                json_ops.append(
                    {
                        "kind": "bytes_startswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_STARTSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_startswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_STARTSWITH":
                json_ops.append(
                    {
                        "kind": "bytearray_startswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_STARTSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_startswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_ENDSWITH":
                json_ops.append(
                    {
                        "kind": "bytes_endswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_ENDSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_endswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_ENDSWITH":
                json_ops.append(
                    {
                        "kind": "bytearray_endswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_ENDSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_endswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_COUNT":
                json_ops.append(
                    {
                        "kind": "bytes_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_COUNT":
                json_ops.append(
                    {
                        "kind": "bytearray_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_COUNT_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_count_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_COUNT_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_count_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STR_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "str_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "REPR_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "repr_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASCII_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "ascii_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_FIND":
                json_ops.append(
                    {
                        "kind": "string_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_FIND_SLICE":
                json_ops.append(
                    {
                        "kind": "string_find_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_FORMAT":
                json_ops.append(
                    {
                        "kind": "string_format",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_NEW":
                json_ops.append(
                    {
                        "kind": "buffer2d_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_GET":
                json_ops.append(
                    {
                        "kind": "buffer2d_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_SET":
                json_ops.append(
                    {
                        "kind": "buffer2d_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_MATMUL":
                json_ops.append(
                    {
                        "kind": "buffer2d_matmul",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_STARTSWITH":
                json_ops.append(
                    {
                        "kind": "string_startswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_STARTSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "string_startswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_ENDSWITH":
                json_ops.append(
                    {
                        "kind": "string_endswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_ENDSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "string_endswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_COUNT":
                json_ops.append(
                    {
                        "kind": "string_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_COUNT_SLICE":
                json_ops.append(
                    {
                        "kind": "string_count_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_JOIN":
                json_ops.append(
                    {
                        "kind": "string_join",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_SPLIT":
                json_ops.append(
                    {
                        "kind": "string_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_SPLIT_MAX":
                json_ops.append(
                    {
                        "kind": "string_split_max",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_LOWER":
                json_ops.append(
                    {
                        "kind": "string_lower",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_UPPER":
                json_ops.append(
                    {
                        "kind": "string_upper",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_CAPITALIZE":
                json_ops.append(
                    {
                        "kind": "string_capitalize",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_STRIP":
                json_ops.append(
                    {
                        "kind": "string_strip",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_LSTRIP":
                json_ops.append(
                    {
                        "kind": "string_lstrip",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_RSTRIP":
                json_ops.append(
                    {
                        "kind": "string_rstrip",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_REPLACE":
                json_ops.append(
                    {
                        "kind": "string_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_SPLIT":
                json_ops.append(
                    {
                        "kind": "bytes_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_SPLIT_MAX":
                json_ops.append(
                    {
                        "kind": "bytes_split_max",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_SPLIT":
                json_ops.append(
                    {
                        "kind": "bytearray_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_SPLIT_MAX":
                json_ops.append(
                    {
                        "kind": "bytearray_split_max",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATISTICS_MEAN_SLICE":
                json_ops.append(
                    {
                        "kind": "statistics_mean_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATISTICS_STDEV_SLICE":
                json_ops.append(
                    {
                        "kind": "statistics_stdev_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_REPLACE":
                json_ops.append(
                    {
                        "kind": "bytes_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_REPLACE":
                json_ops.append(
                    {
                        "kind": "bytearray_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASYNC_BLOCK_ON":
                json_ops.append(
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
                json_ops.append({"kind": "const", "value": 0, "out": op.result.name})
            elif op.kind == "BRIDGE_UNAVAILABLE":
                json_ops.append(
                    {
                        "kind": "bridge_unavailable",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STORE_VAR":
                var_name = op.metadata["var"] if op.metadata else ""
                json_ops.append(
                    {
                        "kind": "store_var",
                        "var": var_name,
                        "args": [op.args[0].name],
                    }
                )
            elif op.kind == "DELETE_VAR":
                var_name = op.metadata["var"] if op.metadata else ""
                json_ops.append(
                    {
                        "kind": "delete_var",
                        "var": var_name,
                        "args": [arg.name for arg in op.args],
                    }
                )
            elif op.kind == "LOAD_VAR":
                var_name = op.metadata["var"] if op.metadata else ""
                json_ops.append(
                    {
                        "kind": "load_var",
                        "var": var_name,
                        "out": op.result.name,
                    }
                )
                # Propagate list_int container hint through load_var:
                # if the variable being loaded is a list_int container,
                # the output name is also a list_int container.
                if var_name in json_list_int_containers:
                    json_list_int_containers.add(op.result.name)
            elif op.kind == "ret":
                if emit_function_frame:
                    json_ops.append({"kind": "trace_exit"})
                json_ops.append({"kind": "ret", "var": op.args[0].name})
            elif op.kind == "ret_void":
                if emit_function_frame:
                    json_ops.append({"kind": "trace_exit"})
                json_ops.append({"kind": "ret_void"})
            elif op.kind == "ALLOC_TASK":
                poll_func = self._require_async_poll_target("ALLOC_TASK", op.args[0])
                size = op.args[1]
                args = op.args[2:]
                task_kind = op.metadata.get("task_kind") if op.metadata else None
                if task_kind not in {"future", "generator", "coroutine"}:
                    raise ValueError(
                        f"ALLOC_TASK missing task_kind metadata: {task_kind!r}"
                    )
                json_ops.append(
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
                json_ops.append(
                    {
                        "kind": "asyncgen_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASYNCGEN_SHUTDOWN":
                json_ops.append(
                    {
                        "kind": "asyncgen_shutdown",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATE_SWITCH":
                json_ops.append({"kind": "state_switch"})
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
                json_ops.append(
                    {
                        "kind": "state_transition",
                        "args": args,
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATE_YIELD":
                pair, next_state = op.args
                json_ops.append(
                    {
                        "kind": "state_yield",
                        "args": [pair.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SPAWN":
                json_ops.append({"kind": "spawn", "args": [op.args[0].name]})
            elif op.kind == "CANCEL_TOKEN_NEW":
                json_ops.append(
                    {
                        "kind": "cancel_token_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_CLONE":
                json_ops.append(
                    {
                        "kind": "cancel_token_clone",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_DROP":
                json_ops.append(
                    {
                        "kind": "cancel_token_drop",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_CANCEL":
                json_ops.append(
                    {
                        "kind": "cancel_token_cancel",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUTURE_CANCEL":
                json_ops.append(
                    {
                        "kind": "future_cancel",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUTURE_CANCEL_MSG":
                json_ops.append(
                    {
                        "kind": "future_cancel_msg",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUTURE_CANCEL_CLEAR":
                json_ops.append(
                    {
                        "kind": "future_cancel_clear",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROMISE_NEW":
                json_ops.append(
                    {
                        "kind": "promise_new",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROMISE_SET_RESULT":
                json_ops.append(
                    {
                        "kind": "promise_set_result",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROMISE_SET_EXCEPTION":
                json_ops.append(
                    {
                        "kind": "promise_set_exception",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "THREAD_SUBMIT":
                json_ops.append(
                    {
                        "kind": "thread_submit",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TASK_REGISTER_TOKEN_OWNED":
                json_ops.append(
                    {
                        "kind": "task_register_token_owned",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_IS_CANCELLED":
                json_ops.append(
                    {
                        "kind": "cancel_token_is_cancelled",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_SET_CURRENT":
                json_ops.append(
                    {
                        "kind": "cancel_token_set_current",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_GET_CURRENT":
                json_ops.append(
                    {
                        "kind": "cancel_token_get_current",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCELLED":
                json_ops.append(
                    {
                        "kind": "cancelled",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_CURRENT":
                json_ops.append(
                    {
                        "kind": "cancel_current",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHAN_NEW":
                json_ops.append(
                    {
                        "kind": "chan_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHAN_SEND_YIELD":
                chan, val, pending_state, next_state = op.args
                json_ops.append(
                    {
                        "kind": "chan_send_yield",
                        "args": [chan.name, val.name, pending_state.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHAN_RECV_YIELD":
                chan, pending_state, next_state = op.args
                json_ops.append(
                    {
                        "kind": "chan_recv_yield",
                        "args": [chan.name, pending_state.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHAN_DROP":
                json_ops.append(
                    {
                        "kind": "chan_drop",
                        "args": [op.args[0].name],
                    }
                )
            elif op.kind == "CALL_ASYNC":
                poll_name = self._require_async_poll_target("CALL_ASYNC", op.args[0])
                payload_args = op.args[1:] if len(op.args) > 1 else []
                json_ops.append(
                    {
                        "kind": "call_async",
                        "s_value": poll_name,
                        "args": [arg.name for arg in payload_args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GEN_SEND":
                gen, val = op.args
                json_ops.append(
                    {
                        "kind": "gen_send",
                        "args": [gen.name, val.name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GEN_THROW":
                gen, val = op.args
                json_ops.append(
                    {
                        "kind": "gen_throw",
                        "args": [gen.name, val.name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GEN_CLOSE":
                json_ops.append(
                    {
                        "kind": "gen_close",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IS_GENERATOR":
                json_ops.append(
                    {
                        "kind": "is_generator",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IS_BOUND_METHOD":
                json_ops.append(
                    {
                        "kind": "is_bound_method",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IS_CALLABLE":
                json_ops.append(
                    {
                        "kind": "is_callable",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOAD_CLOSURE":
                self_ptr, offset = op.args
                json_ops.append(
                    {
                        "kind": "closure_load",
                        "args": [self_ptr],
                        "value": offset,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STORE_CLOSURE":
                self_ptr, offset, val = op.args
                json_ops.append(
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
                json_ops.append({"kind": op.kind, "out": op.result.name})

        if ops and ops[-1].kind not in {"ret", "ret_void"}:
            if emit_function_frame:
                json_ops.append({"kind": "trace_exit"})
            json_ops.append({"kind": "ret_void"})

        if emit_function_frame:
            code_id = self.func_code_ids.get(function_name or "")
            if code_id is None:
                code_id = self._register_code_symbol(function_name or "")
            json_ops.insert(
                0, {"kind": "trace_enter_slot", "value": int(cast(int, code_id))}
            )

        # Post-pass: inject expression-level col_offset/end_col_offset into
        # JSON dicts emitted by raising ops.  This is done after serialization
        # so we don't need to modify every json_ops.append site.
        for ci_idx, (start_idx, mop) in enumerate(_col_inject):
            if mop.col_offset is None:
                continue
            end_idx = (
                _col_inject[ci_idx + 1][0]
                if ci_idx + 1 < len(_col_inject)
                else len(json_ops)
            )
            if end_idx > start_idx:
                last_entry = json_ops[end_idx - 1]
                if isinstance(last_entry, dict) and "col_offset" not in last_entry:
                    last_entry["col_offset"] = mop.col_offset
                    if mop.end_col_offset is not None:
                        last_entry["end_col_offset"] = mop.end_col_offset

        active_source_line: int | None = None
        for entry in json_ops:
            if not isinstance(entry, dict):
                continue
            kind = entry.get("kind")
            if kind == "line":
                value = entry.get("value")
                if isinstance(value, int) and value > 0:
                    active_source_line = value
                    entry.setdefault("source_line", value)
                continue
            if active_source_line is not None:
                entry.setdefault("source_line", active_source_line)

        json_ops = self._scalarize_string_split_fields_json(json_ops)
        json_ops = self._fuse_string_split_field_consumers_json(json_ops)
        return json_ops

    def _finalize_code_ids(self) -> None:
        for data in self.funcs_map.values():
            for op in data["ops"]:
                if op.kind in {"CALL", "CALL_INTERNAL"} and op.args:
                    target = op.args[0]
                    if isinstance(target, str):
                        self._register_code_symbol(target)

    def _ensure_code_slots_init(self) -> None:
        if self.code_slots_emitted:
            return
        self.code_slots_emitted = True
        max_code_id = max(self.func_code_ids.values(), default=-1)
        for data in self.funcs_map.values():
            for op in data["ops"]:
                if op.kind == "CODE_SLOT_SET" and op.metadata:
                    code_id = op.metadata.get("code_id")
                    if code_id is not None:
                        max_code_id = max(max_code_id, int(code_id))
        count = max_code_id + 1
        init_op = MoltOp(
            kind="CODE_SLOTS_INIT",
            args=[count],
            result=MoltValue("none"),
        )
        ops = self.funcs_map.get("molt_main", {}).get("ops")
        if ops is not None:
            ops.insert(0, init_op)

    def to_json(
        self, *, midend_stage: Literal["pre-midend", "post-midend"] = "post-midend"
    ) -> dict[str, Any]:
        if midend_stage not in {"pre-midend", "post-midend"}:
            raise ValueError(f"unsupported IR serialization stage: {midend_stage}")
        self._finalize_code_ids()
        self._ensure_code_slots_init()
        funcs_json: list[dict[str, Any]] = []
        # DETERMINISM: sort to ensure stable output regardless of dict insertion order
        for name, data in sorted(self.funcs_map.items()):
            json_ops = self.map_ops_to_json(
                data["ops"],
                function_name=name,
                run_midend=midend_stage == "post-midend",
            )
            func_entry: dict[str, Any] = {
                "name": name,
                "params": data["params"],
                "ops": json_ops,
            }
            # Always emit param_types so the backend creates Cranelift block
            # params for function arguments. Without this, parameters are
            # uninitialized (read as 0x0 = float +0.0 in NaN-boxing).
            explicit_types = list(data.get("param_types") or [])
            if data["params"]:
                if len(explicit_types) < len(data["params"]):
                    explicit_types.extend(
                        ["i64"] * (len(data["params"]) - len(explicit_types))
                    )
                func_entry["param_types"] = explicit_types
            if self.source_path:
                func_entry["source_file"] = self.source_path
            # Perceus-style borrowing analysis: identify parameters that can
            # be treated as borrowed (no inc_ref on entry, no dec_ref on exit).
            if data["params"]:
                borrowed = self._analyze_borrowing(data["params"], json_ops)
                # Methods: `self` is always borrowed — the caller (class
                # dispatch / bound method) owns the reference. Without this,
                # the compiled __init__ dec-refs self on return, freeing the
                # instance before the caller can use it.
                if data["params"][0] == "self" and 0 not in borrowed:
                    borrowed.append(0)
                    borrowed.sort()
                if borrowed:
                    func_entry["borrowed_params"] = borrowed
            funcs_json.append(func_entry)
        max_code_id = -1
        for func in funcs_json:
            for op in func["ops"]:
                kind = op.get("kind")
                if kind in {"code_slot_set", "call"}:
                    max_code_id = max(max_code_id, int(op.get("value", -1)))
        if max_code_id >= 0:
            for func in funcs_json:
                if func["name"] != "molt_main":
                    continue
                for op in func["ops"]:
                    if op.get("kind") == "code_slots_init":
                        op["value"] = max_code_id + 1
                        break
                else:
                    func["ops"].insert(
                        0, {"kind": "code_slots_init", "value": max_code_id + 1}
                    )
                break
        self._maybe_report_midend_stats()
        return {"functions": funcs_json}

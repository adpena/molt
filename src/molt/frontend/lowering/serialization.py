"""SerializationMixin: the IR -> JSON op emitter (map_ops_to_json) and its
string-split scalarization/fusion helpers.

Move-only extraction from frontend/__init__.py (F1 phase 1). This is the
largest single seam in the generator: map_ops_to_json walks the emitted MoltOp
stream and produces the JSON IR consumed by the backend. self.<method>/<attr>
references resolve through the SimpleTIRGenerator MRO at runtime; the
_GeneratorProtocol annotation gives them static checking.
"""

from __future__ import annotations

from typing import (
    TYPE_CHECKING,
    Any,
    Literal,
)

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.lowering.serialization_basic_ops import SerializationBasicOpsMixin
from molt.frontend.lowering.serialization_collection_ops import (
    SerializationCollectionOpsMixin,
)
from molt.frontend.lowering.serialization_context import SerializationContext
from molt.frontend.lowering.serialization_exception_ops import (
    SerializationExceptionOpsMixin,
)
from molt.frontend.lowering.serialization_function_ops import (
    SerializationFunctionOpsMixin,
)
from molt.frontend.lowering.serialization_loop_string_async_ops import (
    SerializationLoopStringAsyncOpsMixin,
)
from molt.frontend.lowering.serialization_object_attr_ops import (
    SerializationObjectAttrOpsMixin,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class SerializationMixin(
    SerializationBasicOpsMixin,
    SerializationFunctionOpsMixin,
    SerializationExceptionOpsMixin,
    SerializationObjectAttrOpsMixin,
    SerializationCollectionOpsMixin,
    SerializationLoopStringAsyncOpsMixin,
):
    @staticmethod
    def _require_async_poll_target(kind: str, target: Any) -> str:
        if not isinstance(target, str) or not target.endswith("_poll"):
            raise ValueError(
                f"{kind} requires a table-addressable poll target ending in _poll; "
                f"got {target!r}"
            )
        return target

    def _serialization_field_offset(self, expected_class: str, attr: str) -> int | None:
        class_info = self.classes.get(expected_class)
        if not class_info:
            return None
        return class_info.get("fields", {}).get(attr)

    @staticmethod
    def _serialization_control_value(op: MoltOp) -> int:
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

    @staticmethod
    def _serialization_carry_bound_local(
        op: MoltOp, entry: dict[str, Any]
    ) -> dict[str, Any]:
        if (op.metadata or {}).get("bound_local"):
            entry["bound_local"] = True
        return entry

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
            ctx = SerializationContext(
                json_ops=json_ops,
                const_none_vars=const_none_vars,
                json_list_int_containers=json_list_int_containers,
                emit_function_frame=emit_function_frame,
                function_name=function_name,
            )
            if self._serialize_basic_op(op, ctx):
                continue
            if self._serialize_function_op(op, ctx):
                continue
            if self._serialize_exception_op(op, ctx):
                continue
            if self._serialize_object_attr_op(op, ctx):
                continue
            if self._serialize_collection_op(op, ctx):
                continue
            self._serialize_loop_string_async_op(op, ctx)

        if ops and ops[-1].kind not in {"ret", "ret_void"}:
            if emit_function_frame:
                json_ops.append({"kind": "trace_exit"})
            json_ops.append({"kind": "ret_void"})

        if emit_function_frame:
            code_id = self.func_code_ids.get(function_name or "")
            if code_id is None:
                code_id = self._register_code_symbol(function_name or "")
            json_ops.insert(0, {"kind": "trace_enter_slot", "value": int(code_id)})

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

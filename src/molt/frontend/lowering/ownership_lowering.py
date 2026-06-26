"""OwnershipLoweringMixin: RC op emission and borrowing analysis.

Move-only extraction from frontend/__init__.py. This lowering authority owns
reference-counting ownership op construction and the conservative Perceus-style
serialized-op borrowing analysis consumed by IR serialization.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt.frontend._types import MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class OwnershipLoweringMixin(_MixinBase):
    def _emit_inc_ref(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="INC_REF", args=[value], result=res))
        return res

    def _emit_dec_ref(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="DEC_REF", args=[value], result=res))
        return res

    def _emit_drop_owned_value(self, value: MoltValue | None) -> None:
        if value is None or value.name == "none":
            return
        self.emit(MoltOp(kind="DEC_REF", args=[value], result=MoltValue("none")))

    def _emit_borrow(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="BORROW", args=[value], result=res))
        return res

    def _emit_release(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="RELEASE", args=[value], result=res))
        return res

    def _emit_box(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="BOX", args=[value], result=res))
        return res

    def _emit_unbox(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="UNBOX", args=[value], result=res))
        return res

    def _emit_cast(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="CAST", args=[value], result=res))
        return res

    def _emit_widen(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="WIDEN", args=[value], result=res))
        return res

    @staticmethod
    def _analyze_borrowing(params: list[str], ops: list[dict[str, Any]]) -> list[int]:
        """Perceus-style borrowing analysis for function parameters.

        Returns the list of parameter indices that are provably *borrowed* --
        i.e., the callee never stores, returns, yields, or otherwise causes
        the parameter value to escape the function scope.

        The analysis is conservative: if there is any doubt, the parameter is
        treated as escaping (owned), which preserves the status-quo RC behavior.
        False negatives (marking a borrowable param as owned) are safe; false
        positives (marking an escaping param as borrowed) would cause
        use-after-free bugs.

        The analysis operates on the serialized JSON ops (post-midend).  It
        performs a forward data-flow walk that tracks which SSA variable names
        are *tainted* -- meaning they carry (or may carry) the identity of a
        parameter value.  A tainted variable that appears in an escaping
        position causes the originating parameter to be marked as escaping.

        Escaping positions:
          - ``ret`` / ``ret_tuple`` operand (value returned to caller)
          - Stored into a container: ``list_new`` args, ``list_append`` value,
            ``tuple_new`` args, ``dict_set`` value, ``set_add`` value,
            ``store_index`` value, ``dict_setdefault`` value
          - Stored as an object attribute: ``set_attr_*`` value arg
          - Stored into a global/module: ``module_set_attr`` value arg,
            ``global_set`` value arg
          - Passed to any function call (conservative -- we lack interprocedural
            info): ``call``, ``call_func``, ``call_bind``, ``call_method``,
            ``call_internal``, ``call_indirect``, ``call_guarded``,
            ``call_async``, ``callargs_push_pos``, ``callargs_push_kw``,
            ``callargs_expand_star``, ``callargs_expand_kwstar``
          - Yielded via generator state ops: ``state_yield``
          - Closure capture: ``closure_store``
          - Exception creation: ``exception_new`` args and single-arg
            ``exception_new_builtin_one`` payloads

        Non-escaping (safe) uses:
          - Binary/unary arithmetic and comparison ops produce a *new* value;
            the operands do not escape.  ``add``, ``sub``, ``mul``, ``div``,
            ``mod``, ``pow``, ``floor_div``, ``lshift``, ``rshift``,
            ``bit_and``, ``bit_or``, ``bit_xor``, ``matmul``,
            ``compare_*``, ``is``, ``is_not``, ``contains``,
            ``not``, ``neg``, ``pos``, ``invert``, ``bool_cast``,
            ``int_cast``, ``float_cast``, ``str_cast``, ``repr_cast``,
            ``len``, ``hash``, ``type_check``, ``isinstance``,
            ``issubclass``, ``hasattr``, ``getattr``
          - Control flow / metadata: ``if``, ``else``, ``end_if``, ``loop_*``,
            ``label``, ``jump``, ``line``, ``check_exception``,
            ``exception_stack_*``, ``frame_locals_set``, ``trace_*``,
            ``nop``, ``ret_void``, ``code_slots_init``, ``code_slot_set``,
            ``phi``, ``phi_select``
          - Read-only indexing: ``index`` (reads from a container, does not
            store the operand)
          - ``print`` (consumes value for display, does not store)
          - ``get_iter``, ``iter_next``, ``iter_next_checked``
          - ``format``, ``format_spec``, ``str_concat``, ``str_join``,
            ``str_format``
        """
        if not params:
            return []
        # taint_map: variable_name -> set of param names whose identity it
        # may carry.  Params start tainted with themselves.
        taint_map: dict[str, set[str]] = {p: {p} for p in params}

        # escaped: param names that have been proven to escape.
        escaped: set[str] = set()

        # ---- Op classification tables ----

        # Ops that are purely safe for all their operands (operands do not
        # escape and the result is a fresh value).
        _SAFE_OPS: set[str] = {
            # Arithmetic / bitwise
            "add",
            "sub",
            "mul",
            "div",
            "mod",
            "pow",
            "floor_div",
            "lshift",
            "rshift",
            "bit_and",
            "bit_or",
            "bit_xor",
            "matmul",
            "iadd",
            "isub",
            "imul",
            "idiv",
            "imod",
            "ipow",
            "ifloor_div",
            "ilshift",
            "irshift",
            "ibit_and",
            "ibit_or",
            "ibit_xor",
            # Comparison
            "compare_eq",
            "compare_ne",
            "compare_lt",
            "compare_le",
            "compare_gt",
            "compare_ge",
            "lt",
            "le",
            "gt",
            "ge",
            "eq",
            "ne",
            "is",
            "is_not",
            "contains",
            "not_contains",
            # Unary
            "not",
            "neg",
            "pos",
            "invert",
            # Casts / introspection
            "bool_cast",
            "int_cast",
            "float_cast",
            "str_cast",
            "repr_cast",
            "len",
            "hash",
            "type_check",
            "isinstance",
            "issubclass",
            "hasattr",
            "id",
            # Read-only container access (reads, does not store the operand)
            "index",
            "get_iter",
            "iter_next",
            "iter_next_checked",
            # String ops (produce new strings)
            "format",
            "format_spec",
            "str_concat",
            "str_join",
            "str_format",
            "str_replace",
            "str_split",
            "str_strip",
            "str_lstrip",
            "str_rstrip",
            "str_lower",
            "str_upper",
            "str_startswith",
            "str_endswith",
            "str_find",
            "str_rfind",
            "str_count",
            "str_encode",
            "str_decode",
            # Print (consumes for display only)
            "print",
            # Attribute read (does not store the operand -- reads from it)
            "get_attr_name",
            "get_attr_name_default",
            "get_attr_generic_obj",
            "get_attr_generic_ptr",
            "module_get_attr",
            # Constants / metadata
            "const",
            "const_bool",
            "const_float",
            "const_str",
            "const_bytes",
            "const_none",
            "const_bigint",
            "missing",
            # Control flow / structural
            "if",
            "else",
            "end_if",
            "loop_start",
            "loop_end",
            "loop_continue",
            "loop_break",
            "loop_break_if_true",
            "loop_break_if_false",
            "label",
            "jump",
            "line",
            "nop",
            "ret_void",
            "check_exception",
            "exception_stack_enter",
            "exception_stack_exit",
            "exception_stack_depth",
            "exception_stack_set_depth",
            "exception_stack_clear",
            "exception_last",
            "exception_last_pending",
            "frame_locals_set",
            "trace_enter_slot",
            "trace_exit",
            "code_slots_init",
            "code_slot_set",
            "code_new",
            "phi",
            "phi_select",
            "func_new",
            "builtin_func",
            "class_new",
            "class_def",
            # Unpack operations (produce new values from a container)
            "unpack_sequence",
            "unpack_ex",
            # Slice
            "slice_new",
            "get_slice",
            # Variable ops (SSA-level, no heap escape)
            "store_var",
            "load_var",
        }

        # Ops where certain arg positions store the value into a container
        # (the value escapes into the heap).
        # Format: op_kind -> set of arg indices that are "value" positions
        # (0-based into the 'args' list).
        _CONTAINER_STORE_OPS: dict[str, set[int]] = {
            "list_append": {1},  # list_append(list, value)
            "dict_set": {2},  # dict_set(dict, key, value)
            "dict_setdefault": {2},  # dict_setdefault(dict, key, value)
            "dict_setdefault_empty_list": {2},
            "set_add": {1},  # set_add(set, value)
            "store_index": {2},  # store_index(container, index, value)
            "store_slice": {2},  # store_slice(container, slice, value)
        }

        # Ops that store a value as an object attribute.
        # The value arg is at index 1 in 'args': set_attr_*(obj, value)
        _ATTR_STORE_OPS: set[str] = {
            "set_attr_generic_obj",
            "set_attr_generic_ptr",
            "set_attr_name",
            "module_set_attr",
            "global_set",
        }

        # Ops where all args escape (function calls -- conservative).
        _CALL_OPS: set[str] = {
            "call",
            "call_func",
            "call_bind",
            "call_method",
            "call_internal",
            "call_indirect",
            "call_guarded",
            "call_async",
            "class_merge_layout",
        }

        # CallArgs ops where the value arg escapes into the callargs builder.
        _CALLARGS_ESCAPE_OPS: set[str] = {
            "callargs_push_pos",  # callargs_push_pos(builder, value)
            "callargs_push_kw",  # callargs_push_kw(builder, key, value)
            "callargs_expand_star",  # callargs_expand_star(builder, iterable)
            "callargs_expand_kwstar",
        }

        # Ops that create a new container with initial elements.
        # All args are stored into the container (escape).
        _CONTAINER_BUILD_OPS: set[str] = {
            "list_new",  # list_new(elem1, elem2, ...)
            "tuple_new",  # tuple_new(elem1, elem2, ...)
            "dict_new",  # dict_new(key1, val1, key2, val2, ...) -- safe if empty
            "set_new",  # set_new(elem1, elem2, ...)
        }

        # Other escaping ops
        _YIELD_OPS: set[str] = {
            "state_yield",
            "yield_value",
        }
        _CLOSURE_STORE_OPS: set[str] = {
            "closure_store",
            "closure_set",
        }

        def _get_op_args(op: dict[str, Any]) -> list[str]:
            """Extract string variable names from an op's args list."""
            args = op.get("args", [])
            return [a for a in args if isinstance(a, str)]

        def _mark_escaped(var_names: list[str]) -> None:
            """Mark all params tainted by these variables as escaped."""
            for v in var_names:
                taints = taint_map.get(v)
                if taints:
                    escaped.update(taints)

        def _propagate_taint(src_vars: list[str], dest: str | None) -> None:
            """Propagate taint from source variables to a destination variable.

            Used for ops like ``copy`` or aliasing where the output may carry
            the identity of an input.
            """
            if dest is None:
                return
            combined: set[str] = set()
            for v in src_vars:
                taints = taint_map.get(v)
                if taints:
                    combined.update(taints)
            if combined:
                existing = taint_map.get(dest)
                if existing:
                    existing.update(combined)
                else:
                    taint_map[dest] = set(combined)

        for op in ops:
            kind = op.get("kind", "")
            out = op.get("out")

            # Early exit: if all params already escaped, no point continuing.
            if len(escaped) >= len(params):
                break

            # --- Return: the returned value escapes ---
            if kind == "ret":
                var = op.get("var", "")
                if isinstance(var, str):
                    _mark_escaped([var])
                continue

            if kind == "ret_tuple":
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Container builds: all args escape into the new container ---
            if kind in _CONTAINER_BUILD_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                # The output is a new container; it doesn't carry param identity.
                continue

            # --- Container stores: value position escapes ---
            if kind in _CONTAINER_STORE_OPS:
                escape_indices = _CONTAINER_STORE_OPS[kind]
                args = op.get("args", [])
                for idx in escape_indices:
                    if idx < len(args) and isinstance(args[idx], str):
                        _mark_escaped([args[idx]])
                continue

            # --- Attribute stores: value escapes ---
            if kind in _ATTR_STORE_OPS:
                args = op.get("args", [])
                # For set_attr ops, the value is at index 1: set_attr(obj, value)
                if len(args) >= 2 and isinstance(args[1], str):
                    _mark_escaped([args[1]])
                continue

            # --- Call ops: all args escape (conservative) ---
            if kind in _CALL_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- CallArgs escape ops ---
            if kind in _CALLARGS_ESCAPE_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Yield / closure store ---
            if kind in _YIELD_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue
            if kind in _CLOSURE_STORE_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Exception creation: args escape ---
            if kind in {
                "exception_new",
                "exception_new_builtin",
                "exception_new_builtin_one",
            }:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Raise: the value escapes ---
            if kind in {"raise", "raise_cause", "reraise"}:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Safe ops: operands do not escape. ---
            if kind in _SAFE_OPS:
                # The output of safe ops is a *new* value, not tainted by
                # inputs (e.g., x + y produces a new int, not x or y).
                continue

            # --- Copy / alias ops: propagate taint ---
            if kind in {"copy", "alias", "move"}:
                args = _get_op_args(op)
                _propagate_taint(args, out)
                continue

            # --- Callargs builder creation: safe (no values yet) ---
            if kind == "callargs_new":
                continue

            # --- Dict update / merge ops: value args escape ---
            if kind in {"dict_update", "dict_update_missing", "dict_merge"}:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Bridge / intrinsic calls: treat as escaping (conservative) ---
            if kind.startswith("bridge_") or kind.startswith("intrinsic_"):
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Unknown op: be conservative. Mark all args as escaping. ---
            # This ensures safety for any op kind we haven't explicitly
            # classified.  New op kinds added to the compiler will default
            # to the safe (conservative) behavior.
            args = _get_op_args(op)
            _mark_escaped(args)

        # Build result: param indices that are NOT in the escaped set.
        borrowed_indices: list[int] = []
        for i, p in enumerate(params):
            if p not in escaped:
                borrowed_indices.append(i)
        return borrowed_indices

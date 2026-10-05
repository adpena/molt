use super::*;

/// Handler-owned authority for LLVM preserved SimpleIR ops lowered directly by
/// this module. The dispatcher in `preserved_ops.rs` routes through this slice;
/// audit tooling compares it with the adjacent `match kind` arms so routing and
/// lowering cannot drift independently. A kind whose runtime entry is exactly
/// `molt_<kind>` with a generated positional boxed-ABI row has no arm here: it
/// takes the shared admitted boxed-call path.
pub(super) const HANDLED_KINDS: &[&str] = &[
    "call_async",
    "alloc_class",
    "cast",
    "widen",
    "store_var",
    "copy_var",
    "gen_send",
    "super_new",
    "class_def",
    "exception_new_builtin",
    "exception_new_builtin_empty",
    "exception_new_builtin_one",
    "exception_push",
    "exception_stack_enter",
    "exception_stack_depth",
    "exception_pop",
    "exception_stack_clear",
    "exception_last",
    "exception_last_pending",
    "exception_finally_pending_observer",
    "exception_active",
    "exception_current",
    "exception_clear",
    "builtin_type",
    "class_layout_version",
    "class_set_layout_version",
    "object_set_class",
    "string_format",
    "object_new",
    "exception_match_builtin",
    "type_of",
    "missing",
    "get_attr_name_default",
    "context_depth",
    "dataclass_new",
    "dataclass_new_values",
    "abs",
    "const_ellipsis",
    "const_not_implemented",
    "gen_throw",
    "gen_close",
    "get_attr_special_obj",
    "borrow",
    "identity_alias",
    "binding_alias",
    "release",
    "stateful_locals_register",
    "guard_type",
    "guard_tag",
    "guard_layout",
    "guard_dict_shape",
    "json_parse",
    "msgpack_parse",
    "cbor_parse",
    "floordiv",
    "invert",
    "contains",
    "inplace_bit_and",
    "inplace_bit_or",
    "inplace_bit_xor",
    "inplace_div",
    "inplace_floordiv",
    "inplace_mod",
    "inplace_pow",
    "inplace_lshift",
    "inplace_rshift",
];

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    fn runtime_guard_profile_flag(&mut self) -> inkwell::values::IntValue<'ctx> {
        if let Some(flag) = self.guard_profile_flag {
            return flag;
        }
        let function = self.ensure_runtime_i64_fn("molt_profile_enabled", 0);
        let builder = self.backend.context.create_builder();
        let entry = self.llvm_fn.get_first_basic_block().expect("LLVM entry");
        // The runtime profile flag is initialized once per runtime epoch. One
        // activation observes it at entry, before any guard or callback runs.
        if let Some(first) = entry.get_first_instruction() {
            builder.position_before(&first);
        } else {
            builder.position_at_end(entry);
        }
        let flag = builder
            .build_call(function, &[], "runtime_profile_enabled")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        self.guard_profile_flag = Some(flag);
        flag
    }

    pub(super) fn lower_preserved_direct_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
            "call_async" => self.lower_preserved_call_async_op(op),
            // Repr-identity SimpleIR ops. Native and WASM lower these as
            // operand-0 passthroughs over the same NaN-boxed value format; LLVM
            // must claim the exact same identity fact explicitly so the terminal
            // preserved-op guard remains a true fail-loud backstop rather than a
            // backend skew. No runtime call, ownership transfer, or new value is
            // introduced here.
            "cast" | "widen" | "store_var" | "copy_var" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_ty = self
                    .value_types
                    .get(&src_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                for &result_id in &op.results {
                    self.values.insert(result_id, src_val);
                    self.value_types.insert(result_id, src_ty.clone());
                }
                true
            }
            // Dedicated arms below own a mixed ABI or a symbol other than
            // `molt_<kind>`. Boxed operands are borrowed through the
            // operation's custody; results are owned unless noted.
            "gen_send" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_generator_send", 2);
                self.emit_positional_runtime_call(
                    op,
                    func,
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "gen_send",
                );
                true
            }
            "super_new" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_super_new", 2);
                self.emit_positional_runtime_call(
                    op,
                    func,
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "super_new",
                );
                true
            }
            "class_def" => {
                let Some(meta) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let mut parts = meta.split(',');
                let Some(nbases) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                    return false;
                };
                let Some(nattrs) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                    return false;
                };
                let Some(layout_size) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                let Some(layout_version) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                let Some(flags) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                if op.operands.is_empty() || op.operands.len() != 1 + nbases + nattrs * 2 {
                    return false;
                }
                self.emit_class_definition(op, nbases, nattrs, layout_size, layout_version, flags);
                true
            }
            // The builtin-exception tag is a raw ABI word; the argument (an
            // args tuple, or the single argument) is borrowed.
            "exception_new_builtin" | "exception_new_builtin_one" => {
                let Some(&arg_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let symbol = if kind == "exception_new_builtin" {
                    "molt_exception_new_builtin"
                } else {
                    "molt_exception_new_builtin_one"
                };
                let new_fn = self.ensure_runtime_i64_fn(symbol, 2);
                let tag_val = i64_ty.const_int(*tag as u64, false);
                self.emit_borrowed_runtime_call(
                    op,
                    new_fn,
                    &[
                        RuntimeArg::Word(tag_val.into()),
                        RuntimeArg::Operand(arg_id),
                    ],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    kind,
                );
                true
            }
            "exception_new_builtin_empty" => {
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new_builtin_empty", 1);
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let result = self
                    .backend
                    .builder
                    .build_call(new_fn, &[tag_val.into()], "exception_new_builtin_empty")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_push" => {
                let push_fn = self.ensure_runtime_i64_fn("molt_exception_push", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(push_fn, &[], "exception_push")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_enter" => {
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_stack_enter", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[], "exception_stack_enter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_depth" => {
                let depth_fn = self.ensure_runtime_i64_fn("molt_exception_stack_depth", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(depth_fn, &[], "exception_stack_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_pop" => {
                let pop_fn = self.ensure_runtime_i64_fn("molt_exception_pop", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(pop_fn, &[], "exception_pop")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_clear" => {
                let clear_fn = self.ensure_runtime_i64_fn("molt_exception_stack_clear", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(clear_fn, &[], "exception_stack_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_last" => {
                let last_fn = self.ensure_runtime_i64_fn("molt_exception_last", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(last_fn, &[], "exception_last")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_last_pending" | "exception_finally_pending_observer" => {
                let symbol =
                    crate::exception_observer_abi::pending_exception_observer_runtime_symbol(
                        kind,
                        op.is_async_work_poll(),
                    )
                    .expect("pending-exception observer kind must have a runtime projection");
                let last_fn = self.ensure_runtime_i64_fn(symbol, 0);
                let result = self
                    .backend
                    .builder
                    .build_call(last_fn, &[], "exception_last_pending")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_active" => {
                let active_fn = self.ensure_runtime_i64_fn("molt_exception_active", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(active_fn, &[], "exception_active")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_current" => {
                let current_fn = self.ensure_runtime_i64_fn("molt_exception_current", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(current_fn, &[], "exception_current")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_clear" => {
                let clear_fn = self.ensure_runtime_i64_fn("molt_exception_clear", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(clear_fn, &[], "exception_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // The tag is a boxed int; the builtin type is returned retained.
            "builtin_type" => {
                let Some(&tag_id) = op.operands.first() else {
                    return false;
                };
                let builtin_type_fn = self.ensure_runtime_i64_fn("molt_builtin_type", 1);
                self.emit_borrowed_runtime_call(
                    op,
                    builtin_type_fn,
                    &[RuntimeArg::Operand(tag_id)],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "builtin_type",
                );
                true
            }
            "class_layout_version" => {
                let Some(&class_id) = op.operands.first() else {
                    return false;
                };
                let version_fn = self.ensure_runtime_i64_fn("molt_class_layout_version", 1);
                self.emit_borrowed_runtime_call(
                    op,
                    version_fn,
                    &[RuntimeArg::Operand(class_id)],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "class_layout_version",
                );
                true
            }
            "class_set_layout_version" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_class_set_layout_version", 2);
                self.emit_positional_runtime_call(
                    op,
                    set_fn,
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "class_set_layout_version",
                );
                true
            }
            // The receiver is an object address, not a boxed operand; the
            // class is borrowed.
            "object_set_class" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let obj_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                let set_fn = self.ensure_runtime_i64_fn("molt_object_set_class", 2);
                self.emit_borrowed_runtime_call(
                    op,
                    set_fn,
                    &[
                        RuntimeArg::Word(obj_ptr_bits.into()),
                        RuntimeArg::Operand(op.operands[1]),
                    ],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "object_set_class",
                );
                true
            }
            // -- obj[start:end] (the slice subscript): a fresh owned object, NOT
            //    operand 0. `molt_slice(obj, start, end)`. THIS is the exact
            //    adversarial-review P0 #1 double-free vector — `s[-5:]` fell
            //    through to the passthrough, returned `s`, and was double-freed. --
            // -- format(val, spec) (f-string field / format()): fresh owned str. --
            // `molt_format_builtin(val, spec)`.
            "string_format" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let fmt_fn = self.ensure_runtime_i64_fn("molt_format_builtin", 2);
                self.emit_positional_runtime_call(
                    op,
                    fmt_fn,
                    Self::canonical_boxed_return("molt_format_builtin", 2),
                    kind,
                    "string_format",
                );
                true
            }
            // NOTE: `contains` (the `x in y` membership test) is ALSO a fresh-value
            // `Copy` kind (classified `OwnedValue` in `alias_analysis`), but it is
            // already lowered explicitly further down via `emit_containment`
            // (`molt_contains` + `NotIn` negation). It therefore never reaches the
            // `Copy` passthrough fatal gate, and adding a second `"contains" =>` arm
            // here would be an unreachable duplicate. Left to its established arm.
            // -- slice(start, stop, step): a fresh owned slice object. --
            // -- dict.keys()/values()/items(): fresh owned view objects. --
            // -- enumerate(iterable[, start]): a fresh owned enumerate object. --
            // -- dict(x): a fresh owned dict. --
            // -- object(): a fresh owned bare object. No operands. --
            "object_new" => {
                let object_new_fn = self.ensure_runtime_i64_fn("molt_object_new", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(object_new_fn, &[], "object_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // The builtin-exception tag is a raw ABI word; the match result is
            // a boolean.
            "exception_match_builtin" => {
                let Some(&exc_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let match_fn = self.ensure_runtime_i64_fn("molt_exception_match_builtin", 2);
                let tag_val = i64_ty.const_int(*tag as u64, false);
                self.emit_borrowed_runtime_call(
                    op,
                    match_fn,
                    &[
                        RuntimeArg::Operand(exc_id),
                        RuntimeArg::Word(tag_val.into()),
                    ],
                    RuntimeResultCustody::Unowned,
                    kind,
                    "exception_match_builtin",
                );
                true
            }
            // The type is returned retained.
            "type_of" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let type_of_fn = self.ensure_runtime_i64_fn("molt_type_of", 1);
                self.emit_borrowed_runtime_call(
                    op,
                    type_of_fn,
                    &[RuntimeArg::Operand(obj_id)],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "type_of",
                );
                true
            }
            "missing" => {
                let missing_fn = self.ensure_runtime_i64_fn("molt_missing", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(missing_fn, &[], "missing")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // One owned result on both branches: a miss may preserve the
            // default's identity, but the runtime retains it for the result.
            "get_attr_name_default" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let get_fn = self.ensure_runtime_i64_fn("molt_get_attr_name_default", 3);
                self.emit_positional_runtime_call(
                    op,
                    get_fn,
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "get_attr_name_default",
                );
                true
            }
            "context_depth" => {
                let depth_fn = self.ensure_runtime_i64_fn("molt_context_depth", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(depth_fn, &[], "context_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dataclass_new" => self.emit_dataclass_new(op),
            "dataclass_new_values" => self.emit_dataclass_from_values(op),

            // -- Preserved value-producing / side-effecting ops whose runtime
            //    symbol name DIFFERS from `molt_<kind>` (so the generic
            //    `try_lower_preserved_runtime_call` fallback declines them), or
            //    which are RESULT-LESS side effects the runtime-call fallback
            //    refuses on principle. Each is the byte-for-byte LLVM analogue
            //    of the native (`function_compiler{,/fc/*}.rs`) handler, with the
            //    SAME runtime symbol and operand convention. Before these arms
            //    landed, every one of these kinds fell to the `Copy`
            //    passthrough: 0-operand singletons (`...`, `NotImplemented`)
            //    became `None`; `abs(x)` returned `x`; generator `throw`/`close`,
            //    the `__cause__` chain link, special-attr loads, the RC alias
            //    ops, and the type/layout guards were all silently DROPPED. --

            // `abs(x)` — boxed builtin (the native int-lane branchless fast path
            // does not apply on the TIR/LLVM lane, which has no raw-int primary
            // vars; the boxed path is correct and overflow-safe for BigInt).
            // Symbol is `molt_abs_builtin`, NOT `molt_abs`.
            "abs" => {
                let Some(&x_id) = op.operands.first() else {
                    return false;
                };
                let abs_fn = self.ensure_runtime_i64_fn("molt_abs_builtin", 1);
                self.emit_borrowed_runtime_call(
                    op,
                    abs_fn,
                    &[RuntimeArg::Operand(x_id)],
                    Self::canonical_boxed_return("molt_abs_builtin", 1),
                    kind,
                    "abs",
                );
                true
            }
            // `...` literal ? the Ellipsis singleton. Symbol `molt_ellipsis`,
            // NOT `molt_const_ellipsis`. 0 operands.
            "const_ellipsis" => {
                let ell_fn = self.ensure_runtime_i64_fn("molt_ellipsis", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(ell_fn, &[], "const_ellipsis")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `NotImplemented` singleton (e.g. a `__eq__` returning it). Symbol
            // `molt_not_implemented`, NOT `molt_const_not_implemented`.
            "const_not_implemented" => {
                let ni_fn = self.ensure_runtime_i64_fn("molt_not_implemented", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(ni_fn, &[], "const_not_implemented")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `gen.throw(exc)` ? `molt_generator_throw(gen, val)` (operands
            // [gen, val]). Symbol differs from `molt_gen_throw`.
            "gen_throw" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let throw_fn = self.ensure_runtime_i64_fn("molt_generator_throw", 2);
                self.emit_positional_runtime_call(
                    op,
                    throw_fn,
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "gen_throw",
                );
                true
            }
            // `gen.close()` -> `molt_generator_close(gen)` (operand [gen]).
            // Symbol differs from `molt_gen_close`.
            "gen_close" => {
                let Some(&gen_id) = op.operands.first() else {
                    return false;
                };
                let close_fn = self.ensure_runtime_i64_fn("molt_generator_close", 1);
                self.emit_borrowed_runtime_call(
                    op,
                    close_fn,
                    &[RuntimeArg::Operand(gen_id)],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "gen_close",
                );
                true
            }
            // Special-attribute load (`__class__`, `__name__`, ...) ->
            // `molt_get_attr_special(obj, name_ptr, name_len)`. The attribute
            // name is a compile-time string carried in `s_value`, materialized as
            // a private constant (the label-carrying convention, identical to the
            // native handler and the `call_method_ic` arm above).
            //
            // OWNERSHIP: `molt_get_attr_special` returns one owned reference on
            // every successful path, matching the other canonical getattr
            // entry points.
            "get_attr_special_obj" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let Some(attr_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let i64_ty = self.backend.context.i64_type();
                let ptr_ty = self
                    .backend
                    .context
                    .ptr_type(inkwell::AddressSpace::default());
                let (name_ptr, name_len_bits) = self.raw_string_const_ptr_and_len(&attr_name);
                let fn_ty = i64_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into()], false);
                let get_fn = declare_fixed_runtime_function(
                    self.backend.context,
                    &self.backend.module,
                    "molt_get_attr_special",
                )
                .unwrap_or_else(|| {
                    panic!("molt_get_attr_special must be a fixed LLVM runtime import")
                });
                let get_fn = require_llvm_function_type("molt_get_attr_special", get_fn, fn_ty);
                self.emit_borrowed_runtime_call(
                    op,
                    get_fn,
                    &[
                        RuntimeArg::Operand(obj_id),
                        RuntimeArg::Word(name_ptr.into()),
                        RuntimeArg::Word(name_len_bits.into()),
                    ],
                    RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
                    kind,
                    "get_attr_special_obj",
                );
                true
            }
            // RC-alias ops: `borrow` and the generated owned-alias kind mint a
            // +1, while generated transparent aliases only forward the bits.
            // `release` is the dual and is handled in its own arm below because
            // its result convention differs (it never aliases the source).
            "borrow" | "identity_alias" | "binding_alias" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let ty = self
                    .value_types
                    .get(&src_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                if kind == "borrow"
                    || crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table(kind)
                {
                    // Only a heap-capable carrier holds a reference, as the
                    // Cranelift and WASM lowerings retain: a raw scalar's alias
                    // owns nothing, and its payload bits must never reach the
                    // object reference-count ABI.
                    if Self::tir_type_is_dynbox_like(&ty) {
                        let src_bits = self.ensure_i64(src_val);
                        let inc_fn = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
                        self.backend
                            .builder
                            .build_call(inc_fn, &[src_bits.into()], "")
                            .unwrap();
                    }
                } else {
                    debug_assert!(
                        crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(
                            kind
                        ),
                        "LLVM alias '{kind}' lacks generated ownership classification"
                    );
                }
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, src_val);
                    self.value_types.insert(result_id, ty);
                }
                true
            }
            // `release` == `dec_ref` the source. CRITICAL: unlike `borrow`, the
            // result must NOT alias the source — after `molt_dec_ref_obj` the
            // source may be freed, so aliasing+using it is a use-after-free. The
            // native handler (`function_compiler.rs` `dec_ref|release`) dec_refs
            // the source and, when the op carries an out var, binds it to NONE
            // (`def_var_named(out, box_none())`), never to the released source.
            // We mirror that: emit the dec_ref, then bind any result to None.
            "release" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_bits = self.ensure_i64(src_val);
                let dec_fn = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
                self.backend
                    .builder
                    .build_call(dec_fn, &[src_bits.into()], "")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = i64_ty
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "alloc_class" => {
                if op.operands.len() != 1 || op.results.len() > 1 {
                    return false;
                }
                let Some(AttrValue::Int(size)) = op.attrs.get("value") else {
                    return false;
                };
                let size = i64_ty.const_int(*size as u64, false);
                let mut custody = self.begin_borrowed_operands(&op.operands, kind);
                let class_bits = self.borrowed_operand(&mut custody, op.operands[0]);
                let alloc = self.ensure_runtime_i64_fn("molt_alloc_class", 2);
                let unpublished = self
                    .backend
                    .builder
                    .build_call(alloc, &[size.into(), class_bits.into()], "alloc_class")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                // Allocation returns an unpublished instance. Publish its
                // initialized object edges before exposing or retiring it.
                let publish = self.ensure_runtime_i64_fn("molt_object_publish_initialized", 1);
                let initialized = self
                    .backend
                    .builder
                    .build_call(publish, &[unpublished.into()], "class_initialized")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let result = self.finish_borrowed_operands(
                    custody,
                    initialized,
                    "alloc_class_result",
                    |_| {},
                );
                self.bind_owned_runtime_result(op, result.into());
                true
            }

            // Every stateful family registers a raw function address and its
            // two boxed layout tuples. This adapter owns that mixed ABI; the
            // operation may not enter the positional boxed-call fallback.
            "stateful_locals_register" => {
                if op.operands.len() != 2 || op.results.len() > 1 {
                    return false;
                }
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let func = self.ensure_function_symbol(&func_name, arity, false);
                let func_addr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "stateful_locals_func_ptr",
                    )
                    .unwrap();
                let reg_fn = self.ensure_runtime_i64_fn("molt_stateful_locals_register", 3);
                self.emit_borrowed_runtime_call(
                    op,
                    reg_fn,
                    &[
                        RuntimeArg::Word(func_addr.into()),
                        RuntimeArg::Operand(op.operands[0]),
                        RuntimeArg::Operand(op.operands[1]),
                    ],
                    RuntimeResultCustody::SideEffect,
                    kind,
                    kind,
                );
                true
            }

            // Runtime mismatch only profiles and preserves the source. A tag
            // rejected by to_i64 can raise; unary TypeGuard is distinct.
            "guard_type" | "guard_tag" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let profile_join = if self.guard_facts.is_profile_only(op) {
                    let enabled = self.runtime_guard_profile_flag();
                    let enabled = self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            enabled,
                            i64_ty.const_zero(),
                            "guard_profile_enabled",
                        )
                        .unwrap();
                    let origin = self.backend.builder.get_insert_block().unwrap();
                    let check = self
                        .backend
                        .context
                        .append_basic_block(self.llvm_fn, "guard_profile");
                    let join = self
                        .backend
                        .context
                        .append_basic_block(self.llvm_fn, "guard_done");
                    self.all_llvm_blocks.extend([check, join]);
                    self.backend
                        .builder
                        .build_conditional_branch(enabled, check, join)
                        .unwrap();
                    self.record_llvm_edge(origin, check);
                    self.record_llvm_edge(origin, join);
                    self.backend.builder.position_at_end(check);
                    Some(join)
                } else {
                    None
                };
                let guard_fn = self.ensure_runtime_i64_fn("molt_guard_type", 2);
                self.emit_positional_runtime_call(
                    op,
                    guard_fn,
                    RuntimeResultCustody::SideEffect,
                    kind,
                    "guard_type",
                );
                if let Some(join) = profile_join {
                    let checked = self.backend.builder.get_insert_block().unwrap();
                    self.backend
                        .builder
                        .build_unconditional_branch(join)
                        .unwrap();
                    self.record_llvm_edge(checked, join);
                    self.backend.builder.position_at_end(join);
                }
                let source = op.operands[0];
                let value = self.resolve(source);
                let ty = self
                    .value_types
                    .get(&source)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                for &result in &op.results {
                    self.values.insert(result, value);
                    self.value_types.insert(result, ty.clone());
                }
                true
            }

            // Both layout guards share runtime receiver admission. A class
            // hint is not a pointer proof: keep the receiver tagged so scalar
            // mismatches return false without dereferencing their payload.
            // operands = [obj, class, expected_version]; the result is a boolean.
            "guard_layout" | "guard_dict_shape" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let guard_fn = self.ensure_runtime_i64_fn("molt_guard_layout", 3);
                self.emit_positional_runtime_call(
                    op,
                    guard_fn,
                    RuntimeResultCustody::Unowned,
                    kind,
                    "guard_layout",
                );
                true
            }

            // Structured-data scalar parse (`json.loads`/`msgpack`/`cbor` on a
            // single scalar): `molt_<fmt>_parse_scalar_obj(value)`. The native
            // handler (`fc::parse_ops::handle_parse_op`) has a raw-pointer FAST
            // path (it reads `{arg}_ptr`/`{arg}_len` companion vars into a stack
            // out-param via `molt_<fmt>_parse_scalar`) AND this boxed SLOW path
            // for the general case. The fast path is a pure perf optimization
            // keyed on a native-only raw-string-pointer var convention the
            // TIR/LLVM lane does not carry; the slow `*_scalar_obj(value)` call is
            // the SEMANTICALLY COMPLETE lowering native falls back to whenever the
            // companion vars are absent (its `else` branch), so the LLVM lane uses
            // it unconditionally — same result, no fast-path reboxing avoidance.
            // The generic `molt_<kind>` fallback cannot claim these: the symbol is
            // `molt_<fmt>_parse_scalar_obj`, not `molt_<fmt>_parse`. operands =
            // [value]. A `Copy` passthrough would return the unparsed input.
            "json_parse" | "msgpack_parse" | "cbor_parse" => {
                let Some(&val_id) = op.operands.first() else {
                    return false;
                };
                let symbol = match kind {
                    "json_parse" => "molt_json_parse_scalar_obj",
                    "msgpack_parse" => "molt_msgpack_parse_scalar_obj",
                    "cbor_parse" => "molt_cbor_parse_scalar_obj",
                    _ => unreachable!("outer match restricts kind to the three parse ops"),
                };
                let parse_fn = self.ensure_runtime_i64_fn(symbol, 1);
                self.emit_borrowed_runtime_call(
                    op,
                    parse_fn,
                    &[RuntimeArg::Operand(val_id)],
                    Self::canonical_boxed_return(symbol, 1),
                    kind,
                    "parse_scalar",
                );
                true
            }

            // -- Arithmetic / comparison / bitwise carried as preserved `Copy`
            //    ops --
            //
            // The SimpleIR?TIR lift (`kind_to_opcode`) maps some operator kinds
            // the frontend emits — `floordiv`, `invert`, `contains`, the
            // `inplace_bit_*` family, `matmul`, `pow_mod` — to `OpCode::Copy`
            // with `_original_kind` preserved, rather than to their dedicated
            // opcodes. The native/Cranelift and WASM lanes consume SimpleIR
            // directly (where these are real op kinds) and are unaffected, but
            // the LLVM lane lowers the TIR, where these arrive as `Copy`. Without
            // this arm the generic `Copy` handler falls through to "pass through
            // operand 0", silently replacing e.g. `a // b` with `a` (and dropping
            // any exception the operator would raise) — a silent miscompile of
            // every such operator on the LLVM lane.
            //
            // Each kind is lowered with the SAME emit helper its dedicated opcode
            // uses (the helpers take the operator name as a parameter and do not
            // read `op.opcode`; `emit_containment` checks only for `NotIn`, so the
            // `Copy`-carried `in`/`contains` correctly take the non-negated path).
            // `matmul`/`pow_mod` have no dedicated opcode or arith-specialized
            // path, so they lower to their boxed runtime calls (mirroring WASM).
            "floordiv" => {
                self.emit_binary_arith(op, "floordiv");
                true
            }
            "invert" => {
                self.emit_unary(op, "invert");
                true
            }
            "contains" => {
                // `x in y`. `emit_containment` negates only for `OpCode::NotIn`;
                // a `Copy`-carried `contains` is the affirmative membership test.
                self.emit_containment(op);
                true
            }
            "inplace_bit_and" => {
                self.emit_bitwise(op, "bit_and");
                true
            }
            "inplace_bit_or" => {
                self.emit_bitwise(op, "bit_or");
                true
            }
            "inplace_bit_xor" => {
                self.emit_bitwise(op, "bit_xor");
                true
            }
            // In-place augmented arithmetic for `//=`, `%=`, `**=`, `<<=`, `>>=`.
            // These ride `Copy{_original_kind}` (no first-class opcode, mirroring
            // `floordiv`/`inplace_bit_*`). We lower them with the SAME fast int/
            // float lane emitter as their binary opcode (the static int/float path
            // is byte-identical — builtin numerics have no in-place dunder), and
            // `emit_binary_arith`/`emit_bitwise` detect the `inplace_` prefix on
            // `_original_kind` to route the BOXED slow path to
            // `molt_inplace_<op>` (which tries `__i<op>__` first). `@=`/
            // `inplace_matmul` has no arith-specialized path and falls through to
            // the generic runtime-call fallback below, which emits
            // `molt_inplace_matmul`.
            "inplace_div" => {
                self.emit_binary_arith(op, "div");
                true
            }
            "inplace_floordiv" => {
                self.emit_binary_arith(op, "floordiv");
                true
            }
            "inplace_mod" => {
                self.emit_binary_arith(op, "mod");
                true
            }
            "inplace_pow" => {
                self.emit_binary_arith(op, "pow");
                true
            }
            "inplace_lshift" => {
                self.emit_bitwise(op, "lshift");
                true
            }
            "inplace_rshift" => {
                self.emit_bitwise(op, "rshift");
                true
            }
            _ => unreachable!("llvm preserved direct-op family routed unsupported kind `{kind}`"),
        }
    }
}

use super::*;

pub(super) const HANDLED_KINDS: &[&str] = &[
    "list_from_range",
    "iter_next_unboxed",
    "len",
    "list_new",
    "list_fill_new",
    "list_append",
    "list_extend",
    "dict_new",
    "tuple_new",
    "tuple_from_list",
    "set_new",
    "set_add",
    "set_add_probe",
    "frozenset_new",
    "frozenset_add",
    "dict_set",
    "dict_setdefault",
    "dict_setdefault_empty_list",
    "dict_get",
    "iter",
    "unpack_sequence",
    "dict_update",
    "dict_update_missing",
    "dict_update_kwstar",
    "dict_clear",
    "dict_copy",
    "dict_popitem",
    "slice",
    "slice_new",
    "dict_keys",
    "dict_values",
    "dict_items",
    "enumerate",
    "dict_from_obj",
];

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_preserved_container_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
            "list_from_range" => {
                // `list(range(start, stop, step))` materialized eagerly by the
                // frontend (`LIST_FROM_RANGE`). Like `range_new`, it has no
                // dedicated TIR `OpCode` and survives as a `Copy` carrying
                // `_original_kind`; without this arm the LLVM Copy passthrough
                // would return operand 0 (the `start` bound) instead of the
                // built list â€” a silent wrong-result miscompile. Lower to the
                // dedicated runtime constructor `molt_list_from_range(start,
                // stop, step)`, mirroring the native/WASM backends.
                debug_assert_eq!(
                    op.operands.len(),
                    3,
                    "list_from_range must carry exactly [start, stop, step]"
                );
                if op.operands.len() != 3 {
                    return false;
                }
                let list_from_range_fn = self.ensure_runtime_i64_fn("molt_list_from_range", 3);
                let start = self.materialize_dynbox_operand(op.operands[0]).into();
                let stop = self.materialize_dynbox_operand(op.operands[1]).into();
                let step = self.materialize_dynbox_operand(op.operands[2]).into();
                let result = self
                    .backend
                    .builder
                    .build_call(list_from_range_fn, &[start, stop, step], "list_from_range")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "iter_next_unboxed" => self.emit_iter_next_unboxed_results(op),

            "len" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let fn_name = self.container_len_fn(op.operands[0]);
                let len_fn = self.ensure_runtime_i64_fn(fn_name, 1);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(len_fn, &[obj_bits.into()], "len")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "list_new" => {
                let list_new_fn = self.ensure_runtime_i64_fn("molt_list_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        list_new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "list_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self.ensure_runtime_void_fn("molt_list_builder_append", 2);
                for &item_id in &op.operands {
                    let item_bits = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_bits.into()], "list_append")
                        .unwrap();
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_list_builder_finish", 1);
                let list = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "list_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, list);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "list_fill_new" => {
                let list_fill_fn = self.ensure_runtime_i64_fn("molt_list_fill_new", 2);
                let count = self.materialize_dynbox_operand(op.operands[0]);
                let fill = self.materialize_dynbox_operand(op.operands[1]);
                let list = self
                    .backend
                    .builder
                    .build_call(list_fill_fn, &[count.into(), fill.into()], "list_fill_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, list);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "list_append" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_list_append", 2);
                let list_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[list_bits.into(), item_bits.into()], "list_append")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "list_extend" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_list_extend", 2);
                let list_bits = self.materialize_dynbox_operand(op.operands[0]);
                let other_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[list_bits.into(), other_bits.into()], "list_extend")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_new" => {
                let dict_new_fn = self.ensure_runtime_i64_fn("molt_dict_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        dict_new_fn,
                        &[i64_ty
                            .const_int((op.operands.len() / 2) as u64, false)
                            .into()],
                        "dict_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let set_fn = self.ensure_runtime_void_fn("molt_dict_builder_append", 3);
                let mut idx = 0;
                while idx + 1 < op.operands.len() {
                    let key_bits = self.materialize_dynbox_operand(op.operands[idx]);
                    let val_bits = self.materialize_dynbox_operand(op.operands[idx + 1]);
                    self.backend
                        .builder
                        .build_call(
                            set_fn,
                            &[builder.into(), key_bits.into(), val_bits.into()],
                            "dict_append",
                        )
                        .unwrap();
                    idx += 2;
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_dict_builder_finish", 1);
                let dict = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "dict_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, dict);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "tuple_new" => {
                let tuple_new_fn = self.ensure_runtime_i64_fn("molt_list_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        tuple_new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "tuple_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let append_fn = self.ensure_runtime_void_fn("molt_list_builder_append", 2);
                for &item_id in &op.operands {
                    let item_bits = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(
                            append_fn,
                            &[builder.into(), item_bits.into()],
                            "tuple_append",
                        )
                        .unwrap();
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_tuple_builder_finish", 1);
                let tuple_bits = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "tuple_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, tuple_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "tuple_from_list" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_tuple_from_list", 1);
                let list_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[list_bits.into()], "tuple_from_list")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "set_new" => {
                let set_new_fn = self.ensure_runtime_i64_fn("molt_set_builder_new", 1);
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        set_new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "set_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let append_fn = self.ensure_runtime_void_fn("molt_set_builder_append", 2);
                for &item_id in &op.operands {
                    let item_bits = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(append_fn, &[builder.into(), item_bits.into()], "set_append")
                        .unwrap();
                }
                let finish_fn = self.ensure_runtime_i64_fn("molt_set_builder_finish", 1);
                let set_bits = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "set_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, set_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "set_add" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_set_add", 2);
                let set_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[set_bits.into(), item_bits.into()], "set_add")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "set_add_probe" => {
                // Probe-only realization: bare unhashable context on every version.
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_set_add_probe", 2);
                let set_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[set_bits.into(), item_bits.into()], "set_add_probe")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "frozenset_new" => {
                // `frozenset([...])` constructor. Like `set_new`/`list_from_range`
                // it has no dedicated TIR `OpCode` â€” the SSA lifter folds it into a
                // `Copy` carrying `_original_kind = "frozenset_new"`. Without this
                // arm the LLVM Copy passthrough returned operand 0 (or, for the
                // common zero-operand `frozenset_new` + separate `frozenset_add`
                // shape, the None sentinel because there is no operand 0) â€” so
                // `frozenset([1,2,3])` evaluated to `None` entirely (#61). The
                // native/WASM/Luau backends all carry an explicit arm; this closes
                // the LLVM-only coverage gap, mirroring `fc::set_ops::handle_set_op`
                // exactly: `molt_frozenset_new(capacity)` then a `molt_frozenset_add`
                // per element (the frozenset is mutated in place during
                // construction). Any bundled elements are added inline; the
                // zero-operand shape relies on the sibling `frozenset_add` arm.
                let new_fn = self.ensure_runtime_i64_fn("molt_frozenset_new", 1);
                let set_bits = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[i64_ty.const_int(op.operands.len() as u64, false).into()],
                        "frozenset_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.operands.is_empty() {
                    let add_fn = self.ensure_runtime_i64_fn("molt_frozenset_add", 2);
                    for &item_id in &op.operands {
                        let item_bits = self.materialize_dynbox_operand(item_id);
                        self.backend
                            .builder
                            .build_call(
                                add_fn,
                                &[self.ensure_i64(set_bits).into(), item_bits.into()],
                                "frozenset_add",
                            )
                            .unwrap();
                    }
                }
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, set_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "frozenset_add" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_frozenset_add", 2);
                let set_bits = self.materialize_dynbox_operand(op.operands[0]);
                let item_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[set_bits.into(), item_bits.into()], "frozenset_add")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_set" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_set", 3);
                let dict_bits = self.materialize_dynbox_operand(op.operands[0]);
                let key_bits = self.materialize_dynbox_operand(op.operands[1]);
                let value_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into(), value_bits.into()],
                        "dict_set",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_setdefault" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_setdefault", 3);
                let dict_bits = self.materialize_dynbox_operand(op.operands[0]);
                let key_bits = self.materialize_dynbox_operand(op.operands[1]);
                let default_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into(), default_bits.into()],
                        "dict_setdefault",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_setdefault_empty_list" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_setdefault_empty_list", 2);
                let dict_bits = self.materialize_dynbox_operand(op.operands[0]);
                let key_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into()],
                        "dict_setdefault_empty_list",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_get" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let dict_get_fn = self.ensure_runtime_i64_fn("molt_dict_get", 3);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let key_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let default_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        dict_get_fn,
                        &[dict_bits.into(), key_bits.into(), default_bits.into()],
                        "dict_get",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "iter" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let iter_fn = self.ensure_runtime_i64_fn("molt_iter_checked", 1);
                let obj_bits = self.ensure_i64(self.resolve(obj_id));
                let result = self
                    .backend
                    .builder
                    .build_call(iter_fn, &[obj_bits.into()], "iter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "unpack_sequence" => {
                let Some(&seq_id) = op.operands.first() else {
                    return false;
                };
                let expected = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(op.results.len());
                let out_alloca = self
                    .backend
                    .builder
                    .build_array_alloca(
                        i64_ty,
                        i64_ty.const_int(expected.max(1) as u64, false),
                        "unpack_out",
                    )
                    .unwrap();
                let out_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(out_alloca, i64_ty, "unpack_out_ptr")
                    .unwrap();
                let unpack_fn = self.ensure_runtime_i64_fn("molt_unpack_sequence", 3);
                let seq_bits = self.ensure_i64(self.resolve(seq_id));
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        unpack_fn,
                        &[
                            seq_bits.into(),
                            i64_ty.const_int(expected as u64, false).into(),
                            out_ptr_bits.into(),
                        ],
                        "unpack_sequence",
                    )
                    .unwrap();
                for (idx, &result_id) in op.results.iter().enumerate() {
                    let elem_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                out_alloca,
                                &[i64_ty.const_int(idx as u64, false)],
                                "unpack_elem_ptr",
                            )
                            .unwrap()
                    };
                    let elem = self
                        .backend
                        .builder
                        .build_load(i64_ty, elem_ptr, "unpack_elem")
                        .unwrap();
                    self.values.insert(result_id, elem);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_update" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_update", 2);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let other_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into(), other_bits.into()], "dict_update")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_update_missing" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_update_missing", 3);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let key_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let val_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), key_bits.into(), val_bits.into()],
                        "dict_update_missing",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_update_kwstar" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_update_kwstar", 2);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let mapping_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        func,
                        &[dict_bits.into(), mapping_bits.into()],
                        "dict_update_kwstar",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_clear" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_clear", 1);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into()], "dict_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_copy" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_copy", 1);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into()], "dict_copy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_popitem" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_dict_popitem", 1);
                let dict_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[dict_bits.into()], "dict_popitem")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "slice" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let slice_fn = self.ensure_runtime_i64_fn("molt_slice", 3);
                let obj = self.materialize_dynbox_operand(op.operands[0]);
                let start = self.materialize_dynbox_operand(op.operands[1]);
                let end = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(slice_fn, &[obj.into(), start.into(), end.into()], "slice")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "slice_new" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let slice_new_fn = self.ensure_runtime_i64_fn("molt_slice_new", 3);
                let start = self.materialize_dynbox_operand(op.operands[0]);
                let stop = self.materialize_dynbox_operand(op.operands[1]);
                let step = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        slice_new_fn,
                        &[start.into(), stop.into(), step.into()],
                        "slice_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_keys" | "dict_values" | "dict_items" => {
                let Some(&dict_id) = op.operands.first() else {
                    return false;
                };
                let symbol = match kind {
                    "dict_keys" => "molt_dict_keys",
                    "dict_values" => "molt_dict_values",
                    _ => "molt_dict_items",
                };
                let view_fn = self.ensure_runtime_i64_fn(symbol, 1);
                let dict_bits = self.materialize_dynbox_operand(dict_id);
                let result = self
                    .backend
                    .builder
                    .build_call(view_fn, &[dict_bits.into()], kind)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "enumerate" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let enum_fn = self.ensure_runtime_i64_fn("molt_enumerate", 3);
                let iterable = self.materialize_dynbox_operand(op.operands[0]);
                let start = self.materialize_dynbox_operand(op.operands[1]);
                let has_start = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        enum_fn,
                        &[iterable.into(), start.into(), has_start.into()],
                        "enumerate",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            "dict_from_obj" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let dict_fn = self.ensure_runtime_i64_fn("molt_dict_from_obj", 1);
                let obj_bits = self.materialize_dynbox_operand(obj_id);
                let result = self
                    .backend
                    .builder
                    .build_call(dict_fn, &[obj_bits.into()], "dict_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            _ => false,
        }
    }
}

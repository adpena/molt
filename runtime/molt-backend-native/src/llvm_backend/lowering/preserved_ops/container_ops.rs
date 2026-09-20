use super::*;

// Only container operations with custom lowering belong here. Exact
// `molt_<kind>` positional boxed calls deliberately fall through to the
// generated runtime ABI dispatcher so argument boxing, availability, and
// owned/borrowed result custody have one authority.
pub(super) const HANDLED_KINDS: &[&str] = &[
    "iter_next_unboxed",
    "len",
    "list_new",
    "dict_new",
    "tuple_new",
    "set_new",
    "frozenset_new",
    "iter",
    "unpack_sequence",
];

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(in crate::llvm_backend::lowering) fn lower_preserved_container_op(
        &mut self,
        op: &TirOp,
        kind: &str,
    ) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
            "iter_next_unboxed" => self.emit_iter_next_unboxed_results(op),

            "len" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let fn_name = self.container_len_fn(op.operands[0]);
                let abi = runtime_boxed_abi(fn_name, 1)
                    .unwrap_or_else(|| panic!("{fn_name} must have a generated boxed ABI"));
                self.emit_boxed_runtime_call(op, abi);
                true
            }

            "list_new" => {
                self.emit_build_list(op);
                true
            }

            "dict_new" => {
                self.emit_build_dict(op);
                true
            }

            "tuple_new" => {
                self.emit_build_tuple(op);
                true
            }

            "set_new" => {
                self.emit_build_set(op);
                true
            }

            "frozenset_new" => {
                // Frozenset has no dedicated TIR opcode, but its raw-capacity
                // constructor and in-place construction mutator share the same
                // failure/ownership protocol as dict and set literals.
                self.emit_build_frozenset(op);
                true
            }

            "iter" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let abi = runtime_boxed_abi("molt_iter_checked", 1)
                    .expect("molt_iter_checked must have a generated boxed ABI");
                self.emit_boxed_runtime_call(op, abi);
                true
            }

            "unpack_sequence" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let seq_id = op.operands[0];
                let Some(expected) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => usize::try_from(*v).ok(),
                    _ => None,
                }) else {
                    return false;
                };
                if expected != op.results.len() {
                    return false;
                }
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
                let (seq_bits, owns_seq_temporary) =
                    self.materialize_dynbox_operand_with_temporary_owner(seq_id);
                let unpack_result = self
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
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
                if owns_seq_temporary {
                    self.backend
                        .builder
                        .build_call(release, &[seq_bits.into()], "unpack_input_release")
                        .unwrap();
                }
                self.backend
                    .builder
                    .build_call(release, &[unpack_result.into()], "unpack_result_release")
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

            _ => false,
        }
    }
}

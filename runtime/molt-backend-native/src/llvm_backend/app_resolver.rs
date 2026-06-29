#[cfg(feature = "llvm")]
use super::LlvmBackend;
#[cfg(feature = "llvm")]
use crate::app_resolver_abi::{
    APP_RESOLVER_NAMES_SYMBOL, APP_RESOLVER_RECORD_BYTES, APP_RESOLVER_SYMBOL,
    APP_RESOLVER_TABLE_SYMBOL, dump_intrinsic_manifest,
};
#[cfg(feature = "llvm")]
use inkwell::{
    AddressSpace, IntPredicate,
    builder::Builder,
    context::Context,
    module::Linkage,
    values::{FunctionValue, IntValue, StructValue},
};
#[cfg(feature = "llvm")]
use std::collections::BTreeSet;

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    /// Emit the per-app intrinsic resolver `molt_app_resolve_intrinsic` into the
    /// LLVM module, structurally identical to the Cranelift backend's
    /// `SimpleBackend::emit_app_resolver_function`.
    ///
    /// The main stub (emitted by the CLI) references this symbol and registers
    /// it with the runtime via `molt_set_app_intrinsic_resolver` before
    /// `molt_runtime_init`. Without it the LLVM-compiled application object
    /// leaves `_molt_app_resolve_intrinsic` undefined at link, and every
    /// name-based intrinsic resolution at runtime would fail because
    /// `resolve_symbol`/`resolve_core_symbol` are intentionally left
    /// native-unreachable for dead-stripping.
    ///
    /// Layout (mirrors the Cranelift emitter so the two backends are byte-for-
    /// byte interchangeable at the ABI level):
    ///
    /// * `molt_app_intrinsic_names`: the manifest names, sorted by unsigned-byte
    ///   lexicographic order (the `BTreeSet` iteration order) and concatenated.
    /// * `molt_app_intrinsic_table`: N fixed-size 16-byte records, one per name,
    ///   `{ i32 name_off, i32 name_len, ptr func_ptr }`, sorted to match the
    ///   names blob. `func_ptr` is the address of the intrinsic — LLVM emits the
    ///   pointer relocation that the linker resolves against the runtime
    ///   staticlib. The intrinsics are declared `External`, so `-dead_strip` /
    ///   `--gc-sections` still removes every intrinsic whose name appears in no
    ///   manifest record.
    /// * `molt_app_resolve_intrinsic`: an `i64 (ptr, i64)` binary-search lookup
    ///   over the sorted table, returning the intrinsic function pointer as a
    ///   `u64`, or 0 when the name is not in the manifest.
    #[cfg(feature = "llvm")]
    pub fn emit_app_resolver_function(&self, manifest_names: &BTreeSet<String>) {
        dump_intrinsic_manifest(manifest_names);

        let ctx = self.context;
        let module = &self.module;
        let builder = self.context.create_builder();
        let i8_ty = ctx.i8_type();
        let i32_ty = ctx.i32_type();
        let i64_ty = ctx.i64_type();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());

        debug_assert_eq!(APP_RESOLVER_RECORD_BYTES, 16);
        // 16-byte record: { i32 name_off, i32 name_len, ptr func_ptr }.
        let record_ty = ctx.struct_type(&[i32_ty.into(), i32_ty.into(), ptr_ty.into()], false);

        // Build the concatenated names blob and the record initializers. The
        // manifest is a `BTreeSet`, so iteration is already unsigned-byte
        // lexicographic — exactly the order the binary search requires.
        let mut names_blob: Vec<u8> = Vec::new();
        let mut record_values: Vec<StructValue> = Vec::with_capacity(manifest_names.len());
        for name in manifest_names {
            let off = names_blob.len();
            let bytes = name.as_bytes();
            assert!(
                off <= u32::MAX as usize && bytes.len() <= u32::MAX as usize,
                "app resolver: intrinsic name table exceeds u32 addressing"
            );
            names_blob.extend_from_slice(bytes);

            // Address-take the intrinsic. Declare it `External` if a prior
            // runtime-import declaration did not already create it; with opaque
            // pointers the declared signature is irrelevant for address use.
            let intrinsic_fn = module.get_function(name).unwrap_or_else(|| {
                let placeholder_ty = i64_ty.fn_type(&[i64_ty.into()], false);
                module.add_function(name, placeholder_ty, Some(Linkage::External))
            });
            let func_addr = intrinsic_fn.as_global_value().as_pointer_value();

            let record = record_ty.const_named_struct(&[
                i32_ty.const_int(off as u64, false).into(),
                i32_ty.const_int(bytes.len() as u64, false).into(),
                func_addr.into(),
            ]);
            record_values.push(record);
        }
        let count = record_values.len();

        // Names blob global (immutable, private — dead-strippable when the
        // resolver itself is unreferenced).
        let names_array_ty = i8_ty.array_type(names_blob.len() as u32);
        let names_global = module.add_global(names_array_ty, None, APP_RESOLVER_NAMES_SYMBOL);
        names_global.set_linkage(Linkage::Private);
        names_global.set_constant(true);
        names_global.set_unnamed_addr(true);
        names_global.set_initializer(&ctx.const_string(&names_blob, false));

        // Record table global. The `ptr` field initializers carry the pointer
        // relocations LLVM lowers for the linker.
        let table_array_ty = record_ty.array_type(count as u32);
        let table_global = module.add_global(table_array_ty, None, APP_RESOLVER_TABLE_SYMBOL);
        table_global.set_linkage(Linkage::Private);
        table_global.set_constant(true);
        let table_init = record_ty.const_array(&record_values);
        table_global.set_initializer(&table_init);

        // Resolver function: i64 molt_app_resolve_intrinsic(ptr name, i64 len).
        // Matches the C stub declaration `unsigned long long(const char*,
        // unsigned long long)` and the Cranelift emitter's ABI.
        let resolver_ty = i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
        let resolver =
            module.add_function(APP_RESOLVER_SYMBOL, resolver_ty, Some(Linkage::External));

        let entry_bb = ctx.append_basic_block(resolver, "entry");
        let not_found_bb = ctx.append_basic_block(resolver, "not_found");
        builder.position_at_end(entry_bb);

        let name_ptr = resolver.get_nth_param(0).unwrap().into_pointer_value();
        let name_addr = builder
            .build_ptr_to_int(name_ptr, i64_ty, "name_addr")
            .unwrap();
        let name_len = resolver.get_nth_param(1).unwrap().into_int_value();

        if count == 0 {
            builder.build_unconditional_branch(not_found_bb).unwrap();
        } else {
            let names_base = builder
                .build_ptr_to_int(names_global.as_pointer_value(), i64_ty, "names_base")
                .unwrap();

            // Binary search over [lo, hi). loop_head carries (lo, hi).
            let loop_head_bb = ctx.append_basic_block(resolver, "loop_head");
            let probe_bb = ctx.append_basic_block(resolver, "probe");
            let hit_bb = ctx.append_basic_block(resolver, "hit");
            let go_lr_bb = ctx.append_basic_block(resolver, "go_left_or_right");
            let left_bb = ctx.append_basic_block(resolver, "left");
            let right_bb = ctx.append_basic_block(resolver, "right");

            let zero64 = i64_ty.const_zero();
            let count_val = i64_ty.const_int(count as u64, false);
            builder.build_unconditional_branch(loop_head_bb).unwrap();

            // loop_head: phi(lo, hi). if lo < hi -> probe else not_found.
            builder.position_at_end(loop_head_bb);
            let lo_phi = builder.build_phi(i64_ty, "lo").unwrap();
            let hi_phi = builder.build_phi(i64_ty, "hi").unwrap();
            lo_phi.add_incoming(&[(&zero64, entry_bb)]);
            hi_phi.add_incoming(&[(&count_val, entry_bb)]);
            let lo = lo_phi.as_basic_value().into_int_value();
            let hi = hi_phi.as_basic_value().into_int_value();
            let range_nonempty = builder
                .build_int_compare(IntPredicate::SLT, lo, hi, "range_nonempty")
                .unwrap();
            builder
                .build_conditional_branch(range_nonempty, probe_bb, not_found_bb)
                .unwrap();

            // probe: mid = lo + (hi - lo) / 2; load record(mid); compare.
            builder.position_at_end(probe_bb);
            let span = builder.build_int_sub(hi, lo, "span").unwrap();
            let half = builder
                .build_right_shift(span, i64_ty.const_int(1, false), false, "half")
                .unwrap();
            let mid = builder.build_int_add(lo, half, "mid").unwrap();
            // record(mid): GEP into the table, then load off/len and func ptr.
            let rec_ptr = unsafe {
                builder
                    .build_in_bounds_gep(
                        table_array_ty,
                        table_global.as_pointer_value(),
                        &[zero64, mid],
                        "rec_ptr",
                    )
                    .unwrap()
            };
            let off_ptr = builder
                .build_struct_gep(record_ty, rec_ptr, 0, "off_ptr")
                .unwrap();
            let cand_off32 = builder
                .build_load(i32_ty, off_ptr, "cand_off32")
                .unwrap()
                .into_int_value();
            let len_ptr = builder
                .build_struct_gep(record_ty, rec_ptr, 1, "len_ptr")
                .unwrap();
            let cand_len32 = builder
                .build_load(i32_ty, len_ptr, "cand_len32")
                .unwrap()
                .into_int_value();
            let cand_off = builder
                .build_int_z_extend(cand_off32, i64_ty, "cand_off")
                .unwrap();
            let cand_len = builder
                .build_int_z_extend(cand_len32, i64_ty, "cand_len")
                .unwrap();
            let cand_addr = builder
                .build_int_add(names_base, cand_off, "cand_addr")
                .unwrap();

            // cmp = lexicographic_compare(query, candidate) in {-1, 0, 1}.
            let cmp = Self::emit_llvm_lexicographic_compare(
                ctx, &builder, resolver, name_addr, name_len, cand_addr, cand_len,
            );

            let is_eq = builder
                .build_int_compare(IntPredicate::EQ, cmp, i64_ty.const_zero(), "is_eq")
                .unwrap();
            builder
                .build_conditional_branch(is_eq, hit_bb, go_lr_bb)
                .unwrap();

            // hit: load and return func_ptr at record(mid).field(2).
            builder.position_at_end(hit_bb);
            let fp_ptr = builder
                .build_struct_gep(record_ty, rec_ptr, 2, "fp_ptr")
                .unwrap();
            let func_ptr_val = builder
                .build_load(ptr_ty, fp_ptr, "func_ptr")
                .unwrap()
                .into_pointer_value();
            let func_ptr_int = builder
                .build_ptr_to_int(func_ptr_val, i64_ty, "func_ptr_int")
                .unwrap();
            builder.build_return(Some(&func_ptr_int)).unwrap();

            // go_left_or_right: cmp < 0 -> left [lo, mid); else right [mid+1, hi).
            builder.position_at_end(go_lr_bb);
            let cmp_lt = builder
                .build_int_compare(IntPredicate::SLT, cmp, i64_ty.const_zero(), "cmp_lt")
                .unwrap();
            builder
                .build_conditional_branch(cmp_lt, left_bb, right_bb)
                .unwrap();

            builder.position_at_end(left_bb);
            lo_phi.add_incoming(&[(&lo, left_bb)]);
            hi_phi.add_incoming(&[(&mid, left_bb)]);
            builder.build_unconditional_branch(loop_head_bb).unwrap();

            builder.position_at_end(right_bb);
            let mid_plus_1 = builder
                .build_int_add(mid, i64_ty.const_int(1, false), "mid_plus_1")
                .unwrap();
            lo_phi.add_incoming(&[(&mid_plus_1, right_bb)]);
            hi_phi.add_incoming(&[(&hi, right_bb)]);
            builder.build_unconditional_branch(loop_head_bb).unwrap();
        }

        // not_found: return 0.
        builder.position_at_end(not_found_bb);
        builder.build_return(Some(&i64_ty.const_zero())).unwrap();
    }

    /// Emit an unsigned byte-wise lexicographic comparison of two runtime byte
    /// ranges `(a_addr, a_len)` and `(b_addr, b_len)` (both integer addresses),
    /// returning an `i64` in `{-1, 0, 1}` — the same ordering `BTreeSet<String>`
    /// uses to sort the table, so binary search is consistent. Mirrors the
    /// Cranelift backend's `emit_lexicographic_compare`.
    #[cfg(feature = "llvm")]
    fn emit_llvm_lexicographic_compare<'a>(
        ctx: &'a Context,
        builder: &Builder<'a>,
        func: FunctionValue<'a>,
        a_addr: IntValue<'a>,
        a_len: IntValue<'a>,
        b_addr: IntValue<'a>,
        b_len: IntValue<'a>,
    ) -> IntValue<'a> {
        use inkwell::AddressSpace;
        use inkwell::IntPredicate;

        let i8_ty = ctx.i8_type();
        let i64_ty = ctx.i64_type();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());
        let neg_one = i64_ty.const_int((-1i64) as u64, true);
        let zero = i64_ty.const_zero();
        let one = i64_ty.const_int(1, false);

        // min_len = min(a_len, b_len).
        let a_lt_b_len = builder
            .build_int_compare(IntPredicate::ULT, a_len, b_len, "a_lt_b_len")
            .unwrap();
        let min_len = builder
            .build_select(a_lt_b_len, a_len, b_len, "min_len")
            .unwrap()
            .into_int_value();

        let head_bb = ctx.append_basic_block(func, "cmp_head");
        let body_bb = ctx.append_basic_block(func, "cmp_body");
        let advance_bb = ctx.append_basic_block(func, "cmp_advance");
        let diff_bb = ctx.append_basic_block(func, "cmp_diff");
        let tail_bb = ctx.append_basic_block(func, "cmp_tail");
        let ret_bb = ctx.append_basic_block(func, "cmp_ret");

        let pre_bb = builder.get_insert_block().unwrap();
        builder.build_unconditional_branch(head_bb).unwrap();

        // head: phi(i). if i < min_len -> body else tail.
        builder.position_at_end(head_bb);
        let i_phi = builder.build_phi(i64_ty, "i").unwrap();
        i_phi.add_incoming(&[(&zero, pre_bb)]);
        let i = i_phi.as_basic_value().into_int_value();
        let in_range = builder
            .build_int_compare(IntPredicate::ULT, i, min_len, "in_range")
            .unwrap();
        builder
            .build_conditional_branch(in_range, body_bb, tail_bb)
            .unwrap();

        // body: load byte a[i], b[i]; equal -> advance, else -> diff.
        builder.position_at_end(body_bb);
        let a_byte_addr = builder.build_int_add(a_addr, i, "a_byte_addr").unwrap();
        let b_byte_addr = builder.build_int_add(b_addr, i, "b_byte_addr").unwrap();
        let a_byte_ptr = builder
            .build_int_to_ptr(a_byte_addr, ptr_ty, "a_byte_ptr")
            .unwrap();
        let b_byte_ptr = builder
            .build_int_to_ptr(b_byte_addr, ptr_ty, "b_byte_ptr")
            .unwrap();
        let a_byte8 = builder
            .build_load(i8_ty, a_byte_ptr, "a_byte8")
            .unwrap()
            .into_int_value();
        let b_byte8 = builder
            .build_load(i8_ty, b_byte_ptr, "b_byte8")
            .unwrap()
            .into_int_value();
        let a_byte = builder
            .build_int_z_extend(a_byte8, i64_ty, "a_byte")
            .unwrap();
        let b_byte = builder
            .build_int_z_extend(b_byte8, i64_ty, "b_byte")
            .unwrap();
        let bytes_eq = builder
            .build_int_compare(IntPredicate::EQ, a_byte, b_byte, "bytes_eq")
            .unwrap();
        builder
            .build_conditional_branch(bytes_eq, advance_bb, diff_bb)
            .unwrap();

        // advance: i += 1, continue.
        builder.position_at_end(advance_bb);
        let next_i = builder.build_int_add(i, one, "next_i").unwrap();
        i_phi.add_incoming(&[(&next_i, advance_bb)]);
        builder.build_unconditional_branch(head_bb).unwrap();

        // diff: sign of (a_byte - b_byte).
        builder.position_at_end(diff_bb);
        let a_lt_b = builder
            .build_int_compare(IntPredicate::ULT, a_byte, b_byte, "a_lt_b")
            .unwrap();
        let diff_sign = builder
            .build_select(a_lt_b, neg_one, one, "diff_sign")
            .unwrap()
            .into_int_value();
        builder.build_unconditional_branch(ret_bb).unwrap();

        // tail: common prefix equal — order by length.
        builder.position_at_end(tail_bb);
        let len_lt = builder
            .build_int_compare(IntPredicate::ULT, a_len, b_len, "len_lt")
            .unwrap();
        let len_gt = builder
            .build_int_compare(IntPredicate::UGT, a_len, b_len, "len_gt")
            .unwrap();
        let lt_or_zero = builder
            .build_select(len_lt, neg_one, zero, "lt_or_zero")
            .unwrap()
            .into_int_value();
        let tail_result = builder
            .build_select(len_gt, one, lt_or_zero, "tail_result")
            .unwrap()
            .into_int_value();
        builder.build_unconditional_branch(ret_bb).unwrap();

        // ret: phi(result).
        builder.position_at_end(ret_bb);
        let result_phi = builder.build_phi(i64_ty, "cmp_result").unwrap();
        result_phi.add_incoming(&[(&diff_sign, diff_bb), (&tail_result, tail_bb)]);
        result_phi.as_basic_value().into_int_value()
    }
}

#[cfg(test)]
#[cfg(feature = "llvm")]
mod tests {
    use super::*;
    use inkwell::context::Context;

    #[test]
    fn app_resolver_emits_symbol_and_data_tables() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "app_resolver_smoke");
        let manifest = BTreeSet::from(["molt_len".to_string(), "molt_print_obj".to_string()]);

        backend.emit_app_resolver_function(&manifest);
        let ir = backend.dump_ir();

        assert!(
            ir.contains(APP_RESOLVER_SYMBOL),
            "missing resolver symbol: {ir}"
        );
        assert!(
            ir.contains(APP_RESOLVER_NAMES_SYMBOL),
            "missing resolver names table: {ir}"
        );
        assert!(
            ir.contains(APP_RESOLVER_TABLE_SYMBOL),
            "missing resolver record table: {ir}"
        );
        assert!(
            ir.contains("@molt_len"),
            "missing molt_len declaration: {ir}"
        );
        assert!(
            ir.contains("@molt_print_obj"),
            "missing molt_print_obj declaration: {ir}"
        );
    }
}

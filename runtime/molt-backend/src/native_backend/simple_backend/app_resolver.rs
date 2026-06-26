use super::*;

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Emit the per-app intrinsic resolver `molt_app_resolve_intrinsic` into the
    /// user object as a compact, relocated **data table** plus a small O(log n)
    /// binary-search lookup, rather than a giant O(n) linear-scan function.
    ///
    /// Layout (all `Local`, so the linker dead-strips them when the resolver
    /// itself is unreferenced  e.g. WASM builds  and keeps only this object's
    /// table otherwise):
    ///
    /// * `molt_app_intrinsic_names`: the manifest names, sorted by unsigned-byte
    ///   lexicographic order and concatenated (no separators).
    /// * `molt_app_intrinsic_table`: N fixed-size 16-byte records, sorted to
    ///   match the names blob: `[name_off: u32][name_len: u32][func_ptr: u64]`.
    ///   Each `func_ptr` slot carries a single pointer relocation
    ///   (`ARM64_RELOC_UNSIGNED` / `R_X86_64_64` / `R_AARCH64_ABS64` /
    ///   `IMAGE_REL_AMD64_ADDR64`) to the intrinsic, emitted via
    ///   `DataDescription::write_function_addr`. This is the portable, scalable
    ///   relocation form  the linker applies thousands of these without the
    ///   21-bit ADRP / branch-range pressure of thousands of `func_addr`
    ///   instructions packed into one oversized function (the failure mode that
    ///   corrupted the Mach-O header).
    ///
    /// The intrinsic `FuncId`s are declared `Import` (reusing any declaration a
    /// direct call already created), so the linker resolves the pointer relocs
    /// against the runtime staticlib. Only manifest intrinsics are referenced, so
    /// `-dead_strip` / `--gc-sections` still removes every unused intrinsic once
    /// `resolve_symbol` / `resolve_core_symbol` are native-unreachable.
    ///
    /// ABI: `extern "C" fn(name_ptr: i64, name_len: i64) -> i64`. Returns the
    /// intrinsic function pointer as a `u64`, or 0 when the name is not in the
    /// manifest.
    pub(in crate::native_backend::simple_backend) fn emit_app_resolver_function(
        &mut self,
        manifest_names: &BTreeSet<String>,
    ) {
        const RESOLVER_NAME: &str = "molt_app_resolve_intrinsic";
        const RECORD_BYTES: usize = 16; // u32 name_off + u32 name_len + u64 func_ptr

        // Diagnostic-only (default off): emit the exact per-app intrinsic manifest
        // so size-reduction work can verify, deterministically and at the manifest
        // level (not just the final binary size), exactly which intrinsics the
        // reachability gate keeps. Mirrors the `MOLT_DUMP_*` diagnostic family.
        if std::env::var("MOLT_DUMP_INTRINSIC_MANIFEST").as_deref() == Ok("1") {
            eprintln!("MOLT_INTRINSIC_MANIFEST: count={}", manifest_names.len());
            for name in manifest_names {
                eprintln!("MOLT_INTRINSIC_MANIFEST: {name}");
            }
        }

        // Declare the exported resolver: (i64 name_ptr, i64 name_len) -> i64.
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let resolver_id = self
            .module
            .declare_function(RESOLVER_NAME, Linkage::Export, &sig)
            .unwrap_or_else(|e| panic!("failed to declare {RESOLVER_NAME}: {e:?}"));

        // Pre-resolve a FuncId for every manifest intrinsic, reusing any
        // declaration created by a direct call (so we never re-declare with a
        // conflicting signature). The signature only matters when the name was
        // not already declared; the address is taken via a pointer relocation and
        // is signature-independent. `manifest_names` is a `BTreeSet`, so the
        // iteration order is already unsigned-byte lexicographic  exactly the
        // order the binary search requires.
        let mut canonical_sig = self.module.make_signature();
        canonical_sig.params.push(AbiParam::new(types::I64));
        canonical_sig.returns.push(AbiParam::new(types::I64));
        let mut entries: Vec<(&str, cranelift_module::FuncId)> =
            Vec::with_capacity(manifest_names.len());
        for name in manifest_names {
            let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                self.module.get_name(name)
            {
                id
            } else {
                self.module
                    .declare_function(name, Linkage::Import, &canonical_sig)
                    .unwrap_or_else(|e| {
                        panic!("app resolver: failed to declare intrinsic '{name}': {e:?}")
                    })
            };
            entries.push((name.as_str(), func_id));
        }
        let count = entries.len();

        // Build the names blob and the record table. The record table's
        // `func_ptr` slots are filled by relocations, not literal bytes; we
        // pre-size the blob with zeros and attach a `write_function_addr` reloc at
        // each slot offset.
        let mut names_blob: Vec<u8> = Vec::new();
        let mut table_blob: Vec<u8> = vec![0u8; count * RECORD_BYTES];
        let mut name_spans: Vec<(u32, u32)> = Vec::with_capacity(count);
        for (idx, (name, _)) in entries.iter().enumerate() {
            let off = names_blob.len();
            let bytes = name.as_bytes();
            assert!(
                off <= u32::MAX as usize && bytes.len() <= u32::MAX as usize,
                "app resolver: intrinsic name table exceeds u32 addressing"
            );
            names_blob.extend_from_slice(bytes);
            name_spans.push((off as u32, bytes.len() as u32));
            let rec = idx * RECORD_BYTES;
            table_blob[rec..rec + 4].copy_from_slice(&(off as u32).to_le_bytes());
            table_blob[rec + 4..rec + 8].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            // bytes [rec+8 .. rec+16] (the func_ptr) stay zero; the relocation
            // supplies the address at link time.
        }

        // Declare and define the names blob (immutable, no relocations).
        let names_data_id = self
            .module
            .declare_data("molt_app_intrinsic_names", Linkage::Local, false, false)
            .unwrap_or_else(|e| panic!("app resolver: failed to declare names blob: {e:?}"));
        let mut names_desc = DataDescription::new();
        names_desc.define(names_blob.into_boxed_slice());
        self.module
            .define_data(names_data_id, &names_desc)
            .unwrap_or_else(|e| panic!("app resolver: failed to define names blob: {e:?}"));

        // Declare and define the record table with one pointer relocation per
        // func_ptr slot. `import_function` + `write_function_addr` emit a native
        // absolute-pointer relocation (8 bytes)  portable across Mach-O, ELF and
        // COFF  that the linker resolves to the intrinsic in the staticlib.
        let table_data_id = self
            .module
            .declare_data("molt_app_intrinsic_table", Linkage::Local, false, false)
            .unwrap_or_else(|e| panic!("app resolver: failed to declare table: {e:?}"));
        let mut table_desc = DataDescription::new();
        table_desc.set_align(8);
        table_desc.define(table_blob.into_boxed_slice());
        for (idx, (_, func_id)) in entries.iter().enumerate() {
            let func_ref = self.module.declare_func_in_data(*func_id, &mut table_desc);
            let slot = (idx * RECORD_BYTES + 8) as u32;
            table_desc.write_function_addr(slot, func_ref);
        }
        self.module
            .define_data(table_data_id, &table_desc)
            .unwrap_or_else(|e| panic!("app resolver: failed to define table: {e:?}"));

        // Build the lookup function body: binary search over the sorted record
        // table, comparing the query name against each candidate via an unsigned
        // byte-wise lexicographic compare.
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        let name_ptr = builder.block_params(entry_block)[0];
        let name_len = builder.block_params(entry_block)[1];

        let not_found_block = builder.create_block();

        if count == 0 {
            // Empty manifest: the resolver always reports "not found". The table
            // and names blobs are still emitted (size 0) for layout uniformity.
            builder.ins().jump(not_found_block, &[]);
        } else {
            // Materialize the base addresses of the two data segments.
            let names_gv = self
                .module
                .declare_data_in_func(names_data_id, builder.func);
            let names_base = builder.ins().symbol_value(types::I64, names_gv);
            let table_gv = self
                .module
                .declare_data_in_func(table_data_id, builder.func);
            let table_base = builder.ins().symbol_value(types::I64, table_gv);

            // Binary-search loop: maintain a half-open range [lo, hi).
            //   loop_head(lo, hi): if lo >= hi -> not_found; else probe mid.
            let loop_head = builder.create_block();
            builder.append_block_param(loop_head, types::I64); // lo
            builder.append_block_param(loop_head, types::I64); // hi
            let zero = builder.ins().iconst(types::I64, 0);
            let count_val = builder.ins().iconst(types::I64, count as i64);
            jump_block(&mut builder, loop_head, &[zero, count_val]);

            builder.switch_to_block(loop_head);
            let lo = builder.block_params(loop_head)[0];
            let hi = builder.block_params(loop_head)[1];
            let probe_block = builder.create_block();
            let range_nonempty = builder.ins().icmp(IntCC::SignedLessThan, lo, hi);
            builder
                .ins()
                .brif(range_nonempty, probe_block, &[], not_found_block, &[]);

            // probe: mid = lo + (hi - lo) / 2; load record(mid); compare.
            builder.switch_to_block(probe_block);
            let span = builder.ins().isub(hi, lo);
            let half = builder.ins().ushr_imm(span, 1);
            let mid = builder.ins().iadd(lo, half);
            let rec_stride = builder.ins().iconst(types::I64, RECORD_BYTES as i64);
            let rec_off = builder.ins().imul(mid, rec_stride);
            let rec_ptr = builder.ins().iadd(table_base, rec_off);
            let flags = MemFlagsData::new();
            let cand_off32 = builder.ins().load(types::I32, flags, rec_ptr, 0);
            let cand_len32 = builder.ins().load(types::I32, flags, rec_ptr, 4);
            let cand_off = builder.ins().uextend(types::I64, cand_off32);
            let cand_len = builder.ins().uextend(types::I64, cand_len32);
            let cand_ptr = builder.ins().iadd(names_base, cand_off);

            // cmp = lexicographic_compare(query, candidate) in {-1, 0, 1}.
            let cmp = Self::emit_lexicographic_compare(
                &mut builder,
                name_ptr,
                name_len,
                cand_ptr,
                cand_len,
            );

            // cmp == 0 -> hit: load and return func_ptr at rec_ptr+8.
            let hit_block = builder.create_block();
            let go_left_or_right = builder.create_block();
            let is_eq = builder.ins().icmp_imm(IntCC::Equal, cmp, 0);
            builder
                .ins()
                .brif(is_eq, hit_block, &[], go_left_or_right, &[]);

            builder.switch_to_block(hit_block);
            builder.seal_block(hit_block);
            let func_ptr = builder.ins().load(types::I64, flags, rec_ptr, 8);
            builder.ins().return_(&[func_ptr]);

            // cmp < 0 -> search left half [lo, mid); else right half [mid+1, hi).
            builder.switch_to_block(go_left_or_right);
            builder.seal_block(go_left_or_right);
            let left_block = builder.create_block();
            let right_block = builder.create_block();
            let cmp_lt = builder.ins().icmp_imm(IntCC::SignedLessThan, cmp, 0);
            builder
                .ins()
                .brif(cmp_lt, left_block, &[], right_block, &[]);

            builder.switch_to_block(left_block);
            builder.seal_block(left_block);
            jump_block(&mut builder, loop_head, &[lo, mid]);

            builder.switch_to_block(right_block);
            builder.seal_block(right_block);
            let one = builder.ins().iconst(types::I64, 1);
            let mid_plus_1 = builder.ins().iadd(mid, one);
            jump_block(&mut builder, loop_head, &[mid_plus_1, hi]);

            builder.seal_block(probe_block);
            builder.seal_block(loop_head);
        }

        builder.switch_to_block(not_found_block);
        builder.seal_block(not_found_block);
        let zero_ret = builder.ins().iconst(types::I64, 0);
        builder.ins().return_(&[zero_ret]);

        builder.finalize();

        self.module
            .define_function(resolver_id, &mut ctx)
            .unwrap_or_else(|e| panic!("failed to define {RESOLVER_NAME}: {e:?}"));
        self.defined_func_names.insert(RESOLVER_NAME.to_string());
    }

    /// Emit an unsigned byte-wise lexicographic comparison of two runtime byte
    /// ranges `(a_ptr, a_len)` and `(b_ptr, b_len)`, returning an `I64` in
    /// `{-1, 0, 1}` (a<b, a==b, a>b)  the same ordering `BTreeSet<String>` uses
    /// to sort the table, so binary search is consistent.
    ///
    /// The compare loop walks `min(a_len, b_len)` bytes; on the first differing
    /// byte it returns the sign of the unsigned difference, and on a common
    /// prefix it returns the sign of `a_len - b_len`. All loads stay within their
    /// respective `[0, len)` ranges.
    fn emit_lexicographic_compare(
        builder: &mut FunctionBuilder,
        a_ptr: Value,
        a_len: Value,
        b_ptr: Value,
        b_len: Value,
    ) -> Value {
        let flags = MemFlagsData::new();
        let neg_one = builder.ins().iconst(types::I64, -1);
        let zero = builder.ins().iconst(types::I64, 0);
        let one = builder.ins().iconst(types::I64, 1);

        // min_len = min(a_len, b_len)
        let a_lt_b_len = builder.ins().icmp(IntCC::UnsignedLessThan, a_len, b_len);
        let min_len = builder.ins().select(a_lt_b_len, a_len, b_len);

        // Loop over i in [0, min_len). loop_head(i): if i>=min_len break to tail.
        let loop_head = builder.create_block();
        builder.append_block_param(loop_head, types::I64); // i
        let body_block = builder.create_block();
        let tail_block = builder.create_block();
        let ret_block = builder.create_block();
        builder.append_block_param(ret_block, types::I64); // result
        jump_block(builder, loop_head, &[zero]);

        builder.switch_to_block(loop_head);
        let i = builder.block_params(loop_head)[0];
        let in_range = builder.ins().icmp(IntCC::UnsignedLessThan, i, min_len);
        builder
            .ins()
            .brif(in_range, body_block, &[], tail_block, &[]);

        // body: compare bytes at offset i.
        builder.switch_to_block(body_block);
        builder.seal_block(body_block);
        let a_addr = builder.ins().iadd(a_ptr, i);
        let b_addr = builder.ins().iadd(b_ptr, i);
        let a_byte = builder.ins().uload8(types::I64, flags, a_addr, 0);
        let b_byte = builder.ins().uload8(types::I64, flags, b_addr, 0);
        let bytes_eq = builder.ins().icmp(IntCC::Equal, a_byte, b_byte);
        let advance_block = builder.create_block();
        let diff_block = builder.create_block();
        builder
            .ins()
            .brif(bytes_eq, advance_block, &[], diff_block, &[]);

        // advance: i += 1, continue.
        builder.switch_to_block(advance_block);
        builder.seal_block(advance_block);
        let next_i = builder.ins().iadd(i, one);
        jump_block(builder, loop_head, &[next_i]);

        // diff: bytes differ  sign of (a_byte - b_byte).
        builder.switch_to_block(diff_block);
        builder.seal_block(diff_block);
        let a_lt_b = builder.ins().icmp(IntCC::UnsignedLessThan, a_byte, b_byte);
        let diff_sign = builder.ins().select(a_lt_b, neg_one, one);
        jump_block(builder, ret_block, &[diff_sign]);

        // tail: common prefix equal  order by length.
        builder.switch_to_block(tail_block);
        builder.seal_block(tail_block);
        let len_lt = builder.ins().icmp(IntCC::UnsignedLessThan, a_len, b_len);
        let len_gt = builder.ins().icmp(IntCC::UnsignedGreaterThan, a_len, b_len);
        let lt_or_zero = builder.ins().select(len_lt, neg_one, zero);
        let tail_result = builder.ins().select(len_gt, one, lt_or_zero);
        jump_block(builder, ret_block, &[tail_result]);

        builder.seal_block(loop_head);

        builder.switch_to_block(ret_block);
        builder.seal_block(ret_block);
        builder.block_params(ret_block)[0]
    }
}

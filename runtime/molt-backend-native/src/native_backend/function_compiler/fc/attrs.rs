use super::super::*;

/// Single-source kind authority for [`handle_attr_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    // Canonical attribute-op kinds: the spelling `tir::lower_to_simple::lower_op`
    // emits for a LoadAttr/StoreAttr/DelAttr that carries no specialized
    // `_original_kind` — its documented default, the same no-`_original_kind`
    // fallback every other op family already claims (`index`/`store_index`/
    // `del_index`/`call`/`call_builtin`). A TIR pass that yields a generic
    // by-name attribute op produces exactly these (e.g. the cold fallback the
    // release-fast guard-splitting passes leave when they specialize the
    // `guarded_field_get`s in `__future__._Feature.__repr__`). rust/luau/llvm
    // all handle the canonical forms; the native backend must too, or the op
    // hits the dispatch's loud no-codegen catch-all at user `molt build` time.
    // Each routes to its `*_generic_obj` arm below (the bits-validating,
    // tagged-safe, generic-by-name path).
    "get_attr",
    "get_attr_generic_ptr",
    "get_attr_generic_obj",
    "get_attr_special_obj",
    "get_attr_name",
    "get_attr_name_default",
    "has_attr_name",
    "set_attr",
    "set_attr_name",
    "set_attr_generic_ptr",
    "set_attr_generic_obj",
    "del_attr",
    "del_attr_generic_ptr",
    "del_attr_generic_obj",
    "del_attr_name",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for object attribute ops: get (`get_attr` (canonical)/`get_attr_generic_ptr`/`_obj`/`_special_obj`/`_name`/`_name_default`), has (`has_attr_name`), set (`set_attr` (canonical)/`set_attr_name`/`_generic_ptr`/`_generic_obj`), and del (`del_attr` (canonical)/`del_attr_generic_ptr`/`_obj`/`_name`). The canonical `get_attr`/`set_attr`/`del_attr` — `tir::lower_to_simple`'s no-`_original_kind` default — route to the matching `*_generic_obj` arm.
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed params,
/// outer-loop `continue`/`break` -> `OpFlow` returns).
/// The op-local closure `var_get_boxed_overflow_safe` is reconstructed with the
/// same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_attr_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    local_inc_ref_obj: FuncRef,
) -> OpFlow {
    // Reconstruct the original op-local closure (captures representation_plan +
    // nbc; all other state threads through explicit params) so the moved arm
    // bodies call it exactly as they did inline.
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    match op.kind.as_str() {
        "get_attr_generic_ptr" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj, nbc);
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);

            let res = if let Some(ic_idx) = op.ic_index {
                // Split-phase IC: fast GIL-free probe, then slow path on miss.
                //
                // Phase 1: molt_ic_probe_fast(obj_ptr, ic_index) → hit or 0
                // Phase 2 (miss only): molt_getattr_ic_slow(obj_ptr, attr, len, ic_index)
                //
                // The raw ic_index is passed as a plain i64 — NOT NaN-boxed —
                // because the runtime treats it as a direct table index.
                let ic_raw = builder.ins().iconst(types::I64, ic_idx);

                // --- Declare molt_ic_probe_fast(obj_ptr, ic_index) -> i64 ---
                let probe_callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_ic_probe_fast",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let probe_local = module.declare_func_in_func(probe_callee, builder.func);

                // --- Declare molt_getattr_ic_slow(obj_ptr, attr, len, ic_index) -> i64 ---
                let slow_callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_getattr_ic_slow",
                    &[types::I64, types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let slow_local = module.declare_func_in_func(slow_callee, builder.func);

                // --- Emit: probe_result = molt_ic_probe_fast(obj_ptr, ic_raw) ---
                let probe_call = builder.ins().call(probe_local, &[obj_ptr, ic_raw]);
                let probe_result = builder.inst_results(probe_call)[0];

                // --- Branch: hit (probe_result != 0) vs miss ---
                let hit_block = builder.create_block();
                let miss_block = builder.create_block();
                builder.set_cold_block(miss_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);

                let zero = builder.ins().iconst(types::I64, 0);
                let is_hit = builder.ins().icmp(IntCC::NotEqual, probe_result, zero);
                builder.ins().brif(is_hit, hit_block, &[], miss_block, &[]);

                // --- Hit block: probe returned an owned reference ---
                switch_to_block_materialized(&mut *builder, hit_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, hit_block);
                jump_block(&mut *builder, merge_block, &[probe_result]);

                // --- Miss block: full resolution via slow path ---
                switch_to_block_materialized(&mut *builder, miss_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, miss_block);
                let slow_call = builder
                    .ins()
                    .call(slow_local, &[obj_ptr, attr_ptr, attr_len, ic_raw]);
                let slow_result = builder.inst_results(slow_call)[0];
                // Slow path returns a borrowed reference; inc_ref to own it.
                emit_maybe_ref_adjust_v2(&mut *builder, slow_result, local_inc_ref_obj, nbc);
                jump_block(&mut *builder, merge_block, &[slow_result]);

                // --- Merge ---
                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            } else {
                // Legacy path: no IC index available.
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_get_attr_ptr",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder
                    .ins()
                    .call(local_callee, &[obj_ptr, attr_ptr, attr_len]);
                let slow_res = builder.inst_results(call)[0];
                // Attribute lookup may return borrowed values from object/class internals.
                // Normalize to an owned reference so last-use decref remains safe.
                emit_maybe_ref_adjust_v2(&mut *builder, slow_res, local_inc_ref_obj, nbc);
                slow_res
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "get_attr" | "get_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_get_attr_object_ic",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let source_op_idx = required_source_op_idx(op, op_idx, "get_attr_generic_obj");
            let site_bits = builder.ins().iconst(
                types::I64,
                box_int(stable_ic_site_id(
                    func_name,
                    source_op_idx,
                    "get_attr_generic_obj",
                )),
            );
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len, site_bits]);
            let res = builder.inst_results(call)[0];
            // `molt_get_attr_object_ic` delegates to `molt_get_attr_name`, which can
            // hand back borrowed values on fast paths. Own the result here.
            emit_maybe_ref_adjust_v2(&mut *builder, res, local_inc_ref_obj, nbc);
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "get_attr_special_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_get_attr_special",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len]);
            let res = builder.inst_results(call)[0];
            // Keep attribute result ownership consistent across all get-attr ops.
            emit_maybe_ref_adjust_v2(&mut *builder, res, local_inc_ref_obj, nbc);
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "get_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_get_attr_name",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name]);
            let res = builder.inst_results(call)[0];
            // Attribute lookup returns a borrowed reference from object internals/dicts in
            // some fast paths. Convert it to an owned reference so lifetime tracking can
            // safely decref at last use without corrupting dict-owned values.
            emit_maybe_ref_adjust_v2(&mut *builder, res, local_inc_ref_obj, nbc);
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "get_attr_name_default" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let default = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Attr default not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_get_attr_name_default",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name, *default]);
            let res = builder.inst_results(call)[0];
            // See `get_attr_name` above: ensure the returned value is owned.
            emit_maybe_ref_adjust_v2(&mut *builder, res, local_inc_ref_obj, nbc);
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "has_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_has_attr_name",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "set_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Attr value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_set_attr_name",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "set_attr_generic_ptr" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr value not found");
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            // Pass the NaN-boxed receiver (NOT a pre-unboxed pointer) and call the
            // bits-validating `molt_set_attr_object`. SETATTR's `_generic_ptr`
            // SimpleIR form is emitted whenever the attr is not a statically-known
            // field offset — which includes polymorphic receivers whose runtime
            // value can be a TAGGED non-pointer (a tagged int/bool/None/float,
            // e.g. `typing.final(42)` → `f.__final__ = True`). `unbox_ptr_value`
            // on a tagged value yields a garbage address (the tag bits, e.g.
            // 0x12), and the old `molt_set_attr_ptr` then dereferenced the object
            // header at `addr-16` → SIGSEGV. `molt_set_attr_object` resolves the
            // pointer via `maybe_ptr_from_bits`, raising a clean catchable
            // AttributeError/TypeError for a tagged receiver and taking the exact
            // same `molt_set_attr_generic` path for a real heap object — so there
            // is no behavior change for writable receivers, only the missing
            // tagged-receiver guard the `_ptr` variant unsoundly skipped.
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_set_attr_object",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "set_attr" | "set_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr value not found");
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_set_attr_object",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len, *val]);
            if let Some(out_name) = op.out.as_ref() {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name, res);
            }
        }
        "del_attr_generic_ptr" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            // Pass the NaN-boxed receiver and call the bits-validating
            // `molt_del_attr_object` (mirrors the `set_attr_generic_ptr` fix
            // above): the `_generic_ptr` DELATTR form can target a tagged
            // non-pointer receiver, and `unbox_ptr_value` of a tagged value
            // followed by `molt_del_attr_ptr`'s header deref would SIGSEGV.
            // `molt_del_attr_object` resolves via `maybe_ptr_from_bits` and raises
            // a clean AttributeError/TypeError for a tagged receiver, with no
            // behavior change for real heap objects.
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_del_attr_object",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "del_attr" | "del_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_del_attr_object",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "del_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_del_attr_name",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
    OpFlow::Proceed
}

fn required_source_op_idx(op: &OpIR, op_idx: usize, kind: &str) -> usize {
    match op.source_op_idx {
        Some(value) => usize::try_from(value)
            .unwrap_or_else(|_| panic!("{kind} has invalid negative source_op_idx {value}")),
        None => panic!("{kind} at stream op {op_idx} requires transported source_op_idx"),
    }
}

use super::super::*;

/// Single-source kind authority for [`handle_object_construct_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "bound_method_new",
    "object_new",
    "object_new_bound",
    "object_new_bound_stack",
    "super_new",
    "classmethod_new",
    "staticmethod_new",
    "property_new",
];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for object/descriptor construction: `object_new`/`object_new_bound`/`object_new_bound_stack`/`super_new`/`classmethod_new`/`staticmethod_new`/`property_new`/`bound_method_new`.
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed params).
/// The op-local closure `var_get_boxed_overflow_safe` is reconstructed with the
/// same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_object_construct_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
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
        "bound_method_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let func_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Func not found");
            let self_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Self not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bound_method_new",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*func_bits, *self_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "object_new" => {
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_object_new",
                &[],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "object_new_bound" => {
            // Lower the frontend's class-instantiation fast path:
            // `Point(args)` where Point is a known non-dynamic class
            // becomes `OBJECT_NEW_BOUND(Point_class_ref)` + direct
            // CALL to `__init__`.  This bypasses
            // `type.__call__` → `__new__` → bound-method-init →
            // CALL_BIND IC dispatch — all of which the frontend
            // proves unnecessary for the known-class case.
            //
            // When the frontend carries the static instance
            // payload size on `op.value` (set from
            // `class_info["size"]` in the class-instantiation
            // fold), we route through the sized entry point
            // `molt_object_new_bound_sized` which skips the
            // runtime `class_layout_size` lookup entirely
            // (~5 dict probes + name interning + MRO walks
            // saved per allocation).  The frontend always
            // emits the size for the typed-class fold; the
            // unsized path remains for any hand-built
            // SimpleIR that lacks the hint.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let cls_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Class ref not found for object_new_bound");
            let payload_size = op.value.unwrap_or(0);
            let res = if payload_size > 0 {
                let payload_size_val = builder.ins().iconst(types::I64, payload_size);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_object_new_bound_sized",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder
                    .ins()
                    .call(local_callee, &[*cls_bits, payload_size_val]);
                builder.inst_results(call)[0]
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_object_new_bound",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*cls_bits]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "object_new_bound_stack" => {
            // Phase 5 step 3: stack-allocate the instance.
            //
            // The escape analysis pass has proven that this
            // `OBJECT_NEW_BOUND` site's result is consumed
            // entirely within the function (no return, no
            // store-into-escaping-container, no stash through
            // an escaping op).  We can therefore allocate the
            // MoltHeader + payload on the Cranelift StackSlot
            // and call `molt_object_init_stack` to stamp the
            // header with `HEADER_FLAG_IMMORTAL` (so dec_ref
            // is a no-op), `HEADER_FLAG_SKIP_CLASS_DECREF`
            // (the stack object borrows the module-owned
            // class), and a per-class shared cold idx (so
            // `object_class_bits()` works without per-instance
            // mutex contention on `alloc_cold_header`).
            //
            // The payload size in bytes lives on `op.value`
            // (set by the frontend from `class_info["size"]`,
            // round-tripped through the SSA `value` attr).
            // The verifier rejects ObjectNewBoundStack without
            // that size; codegen treats violations as compiler
            // bugs rather than silently changing allocation mode.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let cls_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Class ref not found for object_new_bound_stack");
            let payload_i64 = op
                .value
                .expect("object_new_bound_stack requires payload byte size");
            assert!(
                payload_i64 > 0,
                "object_new_bound_stack requires positive payload byte size"
            );
            let payload_bytes = u32::try_from(payload_i64)
                .expect("object_new_bound_stack payload size exceeds u32");
            // MoltHeader is 24 bytes, header + payload is
            // 8-byte aligned (align_pow_of_2 = 3).
            const MOLT_HEADER_SIZE: u32 = 24;
            let total = MOLT_HEADER_SIZE
                .checked_add(payload_bytes)
                .expect("object_new_bound_stack total size overflow");
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                total,
                3,
            ));
            // Inline the body of `molt_object_init_stack`,
            // eliminating the C-call frame + argument
            // marshaling (~30 ns saved per stack alloc on
            // bench_struct's tight loop).
            //
            // Step 1: zero-fill the slot in 8-byte chunks.
            // `total` is known at compile time (24 + payload),
            // typically 40-56 bytes ⇒ 5-7 stores.  Cranelift
            // stack slots are guaranteed to be at least
            // `total` bytes; the trailing
            // `(8 - total % 8) % 8` bytes lie within the
            // slot allocation (Cranelift rounds slot size
            // up to `align_pow_of_2 = 3` ⇒ 8 byte alignment),
            // so writing the final whole-i64 chunk is sound
            // regardless of payload byte count.
            let zero64 = builder.ins().iconst(types::I64, 0);
            let n_chunks = (total as usize).div_ceil(8);
            for chunk in 0..n_chunks {
                builder.ins().stack_store(zero64, slot, (chunk * 8) as i32);
            }
            // Step 2: stamp MoltHeader fields in #[repr(C)]
            // layout (24 bytes total).  The earlier zero-fill
            // covers size_class@12 (i16) + 2 bytes padding
            // and reserved@20 (i32) — only the fields below
            // need explicit writes.
            //
            //   offset  0: type_id    (i32) = TYPE_ID_OBJECT (100)
            //   offset  4: ref_count  (i32) = 1
            //              (AtomicU32 raw value — `MoltRefCount`
            //              is `#[repr(transparent)]` over it)
            //   offset  8: flags      (i32) =
            //              HEADER_FLAG_IMMORTAL (0x8000)
            //              | HEADER_FLAG_SKIP_CLASS_DECREF (0x0002)
            //              = 0x8002.  **Both flags are
            //              load-bearing**: IMMORTAL prevents
            //              `dec_ref_ptr` from freeing the
            //              stack pointer (heap corruption
            //              would result); SKIP_CLASS_DECREF
            //              ensures the class refcount is
            //              not decremented (the stack object
            //              borrows the module-owned class).
            //              Match the existing init_stack
            //              runtime body at `object/mod.rs:932`
            //              exactly — using only IMMORTAL
            //              would leave class-refcount
            //              corruption as a defense-in-depth
            //              hazard.
            //   offset 16: cold_idx   (u32) = result of
            //              `molt_ensure_shared_cold_idx(cls_bits)`.
            //              This is the only call we cannot
            //              inline — it does an atomic
            //              compare-exchange on the class's
            //              `MoltHeader::reserved` field plus
            //              a slab alloc on cache miss.
            let type_id_val = builder.ins().iconst(types::I32, 100); // TYPE_ID_OBJECT
            builder.ins().stack_store(type_id_val, slot, 0);
            let ref_count_val = builder.ins().iconst(types::I32, 1);
            builder.ins().stack_store(ref_count_val, slot, 4);
            let flags_val = builder.ins().iconst(types::I32, 0x8002);
            builder.ins().stack_store(flags_val, slot, 8);
            // cold_idx: one runtime call (atomic CAS + slab).
            let cold_idx_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_ensure_shared_cold_idx",
                &[types::I64],
                &[types::I32],
            );
            let cold_idx_local = module.declare_func_in_func(cold_idx_callee, builder.func);
            let cold_idx_call = builder.ins().call(cold_idx_local, &[*cls_bits]);
            let cold_idx_val = builder.inst_results(cold_idx_call)[0];
            builder.ins().stack_store(cold_idx_val, slot, 16);
            // Step 3: compute data_ptr = header_ptr + 24 and
            // NaN-box it as a TAG_PTR value, matching what
            // `MoltObject::from_ptr(data_ptr).bits()` does
            // inside the runtime `molt_object_init_stack`.
            let data_ptr = builder
                .ins()
                .stack_addr(types::I64, slot, MOLT_HEADER_SIZE as i32);
            let res = box_ptr_value(&mut *builder, data_ptr, nbc);
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "super_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let type_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Type not found");
            let obj_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Object not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_super_new",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*type_bits, *obj_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "classmethod_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let func_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Func not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_classmethod_new",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*func_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "staticmethod_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let func_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Func not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_staticmethod_new",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*func_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "property_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let getter = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Getter not found");
            let setter = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Setter not found");
            let deleter = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Deleter not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_property_new",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*getter, *setter, *deleter]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}

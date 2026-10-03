#[cfg(feature = "native-backend")]
use super::*;

/// Publish the exact generated import return contract before operand cleanup.
/// A borrowed result can alias a temporary argument; a bound result must own an
/// independent credit before that argument's owner is released.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn bind_runtime_import_result(
    op: &OpIR,
    result: Value,
    symbol: &str,
    arity: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    bind_runtime_import_result_name(
        crate::tir::simple_def_use::simple_ir_out_result(op),
        result,
        symbol,
        arity,
        module,
        import_ids,
        builder,
        vars,
    );
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn bind_runtime_import_result_name(
    out: Option<&str>,
    result: Value,
    symbol: &str,
    arity: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    use molt_ir::runtime_boxed_abi_generated::{RuntimeBoxedReturn, runtime_boxed_abi};
    let contract = runtime_boxed_abi(symbol, arity)
        .unwrap_or_else(|| panic!("runtime result has no canonical boxed ABI: {symbol}/{arity}"))
        .result;
    match contract {
        RuntimeBoxedReturn::OwnedValue | RuntimeBoxedReturn::PollValue => {
            bind_owned_runtime_result_name(out, result, module, import_ids, builder, vars);
        }
        RuntimeBoxedReturn::BorrowedValue => {
            if let Some(out) = out {
                let retain = SimpleBackend::import_func_id_split(
                    module,
                    import_ids,
                    "molt_inc_ref_obj",
                    &[types::I64],
                    &[],
                );
                let retain = module.declare_func_in_func(retain, builder.func);
                builder.ins().call(retain, &[result]);
                def_var_named(builder, vars, out, result);
            }
        }
        RuntimeBoxedReturn::Void => assert!(
            out.is_none(),
            "void runtime import {symbol} cannot bind a result"
        ),
    }
}

/// Fixed aggregate ABI and operand partition. Values are borrowed; only physical
/// boxes minted while transporting raw integers belong to this operation.
#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
pub(in crate::native_backend::function_compiler) enum FixedAggregateConstructor {
    List,
    Tuple,
    Dataclass,
    DataclassTuple,
    ClassNew,
    ClassDefinition {
        nbases: usize,
        nattrs: usize,
        layout_size: i64,
        layout_version: i64,
        flags: i64,
    },
    /// `molt_slice_new(start, stop, step)` takes its bounds as direct words; an
    /// omitted trailing bound is None.
    Slice,
}

/// Store a borrowed word range in fixed frame storage. Empty ranges pass a
/// null pointer; they never allocate a dummy slot. The runtime snapshots and
/// retains the words before any user callback may reuse its scratch storage.
#[cfg(feature = "native-backend")]
fn store_borrowed_word_range(builder: &mut FunctionBuilder<'_>, words: &[Value]) -> (Value, Value) {
    let count = builder.ins().iconst(types::I64, words.len() as i64);
    if words.is_empty() {
        return (builder.ins().iconst(types::I64, 0), count);
    }
    let bytes = words
        .len()
        .checked_mul(8)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n <= i32::MAX as u32)
        .expect("fixed aggregate word range exceeds the native stack address ABI");
    let slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3));
    for (index, &value) in words.iter().enumerate() {
        builder.ins().stack_store(value, slot, (index * 8) as i32);
    }
    (builder.ins().stack_addr(types::I64, slot, 0), count)
}

/// One operation's borrowed boxed operands and the temporary owners they need.
///
/// Each distinct SSA source is materialized at most once, lazily in consumer
/// order, by the shared escape-boxing rule. Only boxes minted from full-width
/// raw integers are owned; their slots hold None from `begin`, before any
/// failure edge, so no execution observes an earlier owner. A failed box skips
/// all later materialization and the consumer. `finish` joins success with the
/// failure edge (None, with the original exception pending), releases every
/// initialized owner on both paths and carries internal CFG cleanup tracking.
/// A borrowed result that escapes is retained before `finish`; owned results
/// enter the owned-result sink afterward. A reused box must dominate its later
/// uses, so a conditional arm that needs a box opens its own transaction.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct NativeOperandTransaction<'op> {
    origin: Option<Block>,
    none: Value,
    owners: BTreeMap<&'op str, Variable>,
    materialized: BTreeMap<&'op str, Value>,
    failed: Option<Block>,
    adopted_objects: Vec<Value>,
    adopted_callable: Option<Value>,
}

#[cfg(feature = "native-backend")]
impl<'op> NativeOperandTransaction<'op> {
    /// Open before the operation emits code. `operands` are the sources this
    /// transaction may materialize.
    pub(in crate::native_backend::function_compiler) fn begin(
        builder: &mut FunctionBuilder<'_>,
        representation_plan: &ScalarRepresentationPlan,
        operands: impl IntoIterator<Item = &'op str>,
    ) -> Self {
        let origin = builder.current_block();
        let none = builder.ins().iconst(types::I64, box_none());
        let mut owners = BTreeMap::new();
        for name in operands {
            if representation_plan.is_full_deopt_int_name(name) {
                owners.entry(name).or_insert_with(|| {
                    let owner = builder.declare_var(types::I64);
                    builder.def_var(owner, none);
                    owner
                });
            }
        }
        Self {
            origin,
            none,
            owners,
            materialized: BTreeMap::new(),
            failed: None,
            adopted_objects: Vec::new(),
            adopted_callable: None,
        }
    }

    /// Snapshot the references already supplied by DropInsertion before the
    /// first box can fail. Raw carriers own nothing until materialization.
    pub(in crate::native_backend::function_compiler) fn adopt_call_inputs(
        &mut self,
        op: &OpIR,
        builder: &mut FunctionBuilder<'_>,
        vars: &BTreeMap<String, Variable>,
        representation_plan: &ScalarRepresentationPlan,
    ) {
        let callable = crate::tir::op_kinds_generated::kind_source_call_callable_operand(&op.kind);
        for (index, name) in op.args.iter().flatten().enumerate() {
            if !Self::call_takes_position(op, index)
                || representation_plan.is_raw_int_carrier_name(name)
                || representation_plan.is_float_unboxed(name)
                || representation_plan.is_bool_unboxed(name)
            {
                continue;
            }
            let word = *var_get(builder, vars, name).expect("adopted call operand missing");
            if callable == Some(index) {
                self.adopted_callable = Some(word);
            } else {
                // Occurrences are intentional: upstream supplies one reference
                // per transferred position, including repeated object words.
                self.adopted_objects.push(word);
            }
        }
    }

    fn call_takes_position(op: &OpIR, index: usize) -> bool {
        op.argument_custody.as_deref().is_some_and(|custody| {
            custody.get(index) == Some(&molt_ir::ParameterCustody::Transferred)
        }) || crate::tir::op_kinds_generated::kind_consumed_operand_table(
            &op.kind,
            op.args.as_ref().map_or(0, Vec::len),
        ) == Some(index)
    }

    /// Hand each taking position a separate credit to its shared fresh box.
    /// The first transfer takes the original credit unless a borrowed position
    /// needs it through return; repeated transfers each receive another credit.
    /// Existing boxed objects were funded upstream and receive no extra retain.
    pub(in crate::native_backend::function_compiler) fn commit_call_inputs(
        &self,
        op: &OpIR,
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut BTreeMap<&'static str, FuncRef>,
    ) {
        let arguments = op.args.as_deref().unwrap_or(&[]);
        for (&name, &owner) in &self.owners {
            let count = arguments
                .iter()
                .enumerate()
                .filter(|(index, argument)| {
                    argument.as_str() == name && Self::call_takes_position(op, *index)
                })
                .count();
            if count == 0 {
                continue;
            }
            let borrowed = arguments
                .iter()
                .enumerate()
                .any(|(index, argument)| argument == name && !Self::call_takes_position(op, index));
            let retains = count - usize::from(!borrowed);
            if retains != 0 {
                let retain = import_func_ref(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    "molt_inc_ref_obj",
                    &[types::I64],
                    &[],
                );
                let word = self.materialized[name];
                for _ in 0..retains {
                    builder.ins().call(retain, &[word]);
                }
            }
            if !borrowed {
                builder.def_var(owner, self.none);
            }
        }
    }

    /// Consumers read the one word materialized before call admission. No
    /// consumer can silently mint a second identity or introduce a late failure.
    pub(in crate::native_backend::function_compiler) fn word(
        &self,
        name: &str,
    ) -> Option<crate::VarValue> {
        self.materialized.get(name).copied().map(crate::VarValue)
    }

    /// Preparation can split the original native block. Both continuations
    /// inherit its candidate inventory; executable owner tokens stay in SSA.
    /// The call handler must see the success inventory before draining dead
    /// arguments, including when its failure arm returns from the activation.
    pub(in crate::native_backend::function_compiler) fn enter_consumer(
        &mut self,
        builder: &FunctionBuilder<'_>,
        block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
        block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    ) {
        let current = builder
            .current_block()
            .expect("operand consumer needs a block");
        if let Some(origin) = self.origin.filter(|origin| *origin != current) {
            for tracked in [block_tracked_obj, block_tracked_ptr] {
                let live = tracked.remove(&origin).unwrap_or_default();
                if let Some(failed) = self.failed {
                    extend_unique_tracked(tracked.entry(failed).or_default(), live.clone());
                }
                extend_unique_tracked(tracked.entry(current).or_default(), live);
            }
        }
        self.origin = Some(current);
    }

    /// Used on normal completion and on dispatch branches that return directly
    /// from the activation. Only temporary credits belong to this transaction;
    /// the dispatch branch separately settles its committed call inputs.
    pub(in crate::native_backend::function_compiler) fn release_temporaries(
        &self,
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut BTreeMap<&'static str, FuncRef>,
    ) {
        if self.owners.is_empty() {
            return;
        }
        let release = import_func_ref(
            module,
            import_ids,
            builder,
            import_refs,
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        for &owner in self.owners.values().rev() {
            let word = builder.use_var(owner);
            builder.ins().call(release, &[word]);
        }
    }

    /// Close a call whose handler has already published its normal result.
    /// Mint failure skips the handler, releases every preexisting adopted input
    /// in the runtime's version-specific order, and publishes None. The owner
    /// slots were initialized before every failure edge, including loop entry.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::native_backend::function_compiler) fn finish_operation(
        self,
        op: &OpIR,
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut BTreeMap<&'static str, FuncRef>,
        sealed_blocks: &mut BTreeSet<Block>,
        vars: &BTreeMap<String, Variable>,
        representation_plan: &ScalarRepresentationPlan,
        nbc: &crate::NanBoxConsts,
        block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
        block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    ) {
        if let Some(current) = builder.current_block() {
            carry_internal_cfg_tracking(self.origin, current, block_tracked_obj, block_tracked_ptr);
        }
        self.release_temporaries(module, import_ids, builder, import_refs);
        if let Some(failed) = self.failed {
            let join = builder.create_block();
            carry_internal_cfg_tracking(
                builder.current_block(),
                join,
                block_tracked_obj,
                block_tracked_ptr,
            );
            jump_block(builder, join, &[]);
            switch_to_block_materialized(builder, failed);
            seal_block_once(builder, sealed_blocks, failed);
            if !self.adopted_objects.is_empty() || self.adopted_callable.is_some() {
                let (arguments, count) = store_borrowed_word_range(builder, &self.adopted_objects);
                let callable = self
                    .adopted_callable
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
                let release = import_func_ref(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    "molt_call_inputs_release",
                    &[types::I64; 3],
                    &[],
                );
                builder.ins().call(release, &[callable, arguments, count]);
            }
            self.release_temporaries(module, import_ids, builder, import_refs);
            if let Some(out) = crate::tir::simple_def_use::simple_ir_out_result(op) {
                def_var_from_boxed_transport(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    out,
                    self.none,
                );
            }
            carry_internal_cfg_tracking(Some(failed), join, block_tracked_obj, block_tracked_ptr);
            jump_block(builder, join, &[]);
            switch_to_block_materialized(builder, join);
            seal_block_once(builder, sealed_blocks, join);
        }
        if let Some(current) = builder.current_block() {
            carry_internal_cfg_tracking(self.origin, current, block_tracked_obj, block_tracked_ptr);
        }
    }

    /// The boxed None materialized when the transaction opened.
    pub(in crate::native_backend::function_compiler) fn none(&self) -> Value {
        self.none
    }

    /// Whether some source may mint an owned box.
    pub(in crate::native_backend::function_compiler) fn owns_temporaries(&self) -> bool {
        !self.owners.is_empty()
    }

    /// The borrowed boxed word for `name`, materialized on first use.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::native_backend::function_compiler) fn operand(
        &mut self,
        name: &'op str,
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut BTreeMap<&'static str, FuncRef>,
        sealed_blocks: &mut BTreeSet<Block>,
        vars: &BTreeMap<String, Variable>,
        representation_plan: &ScalarRepresentationPlan,
        nbc: &crate::NanBoxConsts,
    ) -> Value {
        if let Some(&value) = self.materialized.get(name) {
            return value;
        }
        assert!(
            name == "none" || vars.contains_key(name),
            "operation operand not found: {name}"
        );
        let owner = self.owners.get(name).copied();
        assert!(
            owner.is_some() || !representation_plan.is_full_deopt_int_name(name),
            "operand transaction has no owner slot for {name}"
        );
        let value = ensure_boxed_primitive_safe(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            nbc,
            representation_plan,
            name,
        );
        self.materialized.insert(name, value);
        if let Some(owner) = owner {
            builder.def_var(owner, value);
            self.continue_unless_pending(module, import_ids, builder, import_refs, sealed_blocks);
        }
        value
    }

    /// Skip dependent work when the preceding call left an exception pending.
    pub(in crate::native_backend::function_compiler) fn continue_unless_pending(
        &mut self,
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut BTreeMap<&'static str, FuncRef>,
        sealed_blocks: &mut BTreeSet<Block>,
    ) {
        let pending = import_func_ref(
            module,
            import_ids,
            builder,
            import_refs,
            "molt_exception_pending_fast",
            &[],
            &[types::I64],
        );
        let failed = emit_exception_pending_condition(builder, pending, None);
        self.fail_if(builder, sealed_blocks, failed);
    }

    /// Branch to the transaction's failure edge when `failed` holds.
    pub(in crate::native_backend::function_compiler) fn fail_if(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        sealed_blocks: &mut BTreeSet<Block>,
        failed: Value,
    ) {
        let abort = *self.failed.get_or_insert_with(|| {
            let block = builder.create_block();
            builder.set_cold_block(block);
            block
        });
        let next = builder.create_block();
        builder.ins().brif(failed, abort, &[], next, &[]);
        switch_to_block_materialized(builder, next);
        seal_block_once(builder, sealed_blocks, next);
    }

    /// Join `result` with the failure edge and release every initialized owner
    /// on both paths. `abandon` is a partial result owned only by the failure
    /// path, such as a hash container under construction; it must dominate
    /// every failure edge. Returns the joined result, None on failure.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::native_backend::function_compiler) fn finish(
        self,
        result: Value,
        abandon: Option<Value>,
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut BTreeMap<&'static str, FuncRef>,
        sealed_blocks: &mut BTreeSet<Block>,
        block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
        block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    ) -> Value {
        let result = match self.failed {
            Some(failed) => {
                let join = builder.create_block();
                builder.append_block_param(join, types::I64);
                jump_block(builder, join, &[result]);
                switch_to_block_materialized(builder, failed);
                seal_block_once(builder, sealed_blocks, failed);
                if let Some(partial) = abandon {
                    let release = import_func_ref(
                        module,
                        import_ids,
                        builder,
                        import_refs,
                        "molt_dec_ref_obj",
                        &[types::I64],
                        &[],
                    );
                    builder.ins().call(release, &[partial]);
                }
                jump_block(builder, join, &[self.none]);
                switch_to_block_materialized(builder, join);
                seal_block_once(builder, sealed_blocks, join);
                builder.block_params(join)[0]
            }
            None => result,
        };
        self.release_temporaries(module, import_ids, builder, import_refs);
        if let Some(current) = builder.current_block() {
            carry_internal_cfg_tracking(self.origin, current, block_tracked_obj, block_tracked_ptr);
        }
        result
    }
}

/// Call `symbol` with exactly the operation's `args`, borrowed through one
/// operand transaction, and publish its return under the generated boxed ABI.
/// A bound borrowed return may alias a minted operand, so it acquires its own
/// credit before the transaction releases temporaries; owned and secured
/// returns then enter the owned-result sink, and an unbound borrowed return
/// acquires nothing.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_operand_transaction_call(
    op: &OpIR,
    args: &[String],
    symbol: &'static str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    use molt_ir::runtime_boxed_abi_generated::{RuntimeBoxedReturn, runtime_boxed_abi};
    let contract = runtime_boxed_abi(symbol, args.len())
        .unwrap_or_else(|| {
            panic!(
                "runtime call has no canonical boxed ABI: {symbol}/{}",
                args.len()
            )
        })
        .result;
    assert!(
        contract != RuntimeBoxedReturn::Void,
        "operand transaction call {symbol} must return a value"
    );
    let out = crate::tir::simple_def_use::simple_ir_out_result(op);
    let mut transaction = NativeOperandTransaction::begin(
        builder,
        representation_plan,
        args.iter().map(String::as_str),
    );
    let mut words = Vec::with_capacity(args.len());
    for name in args {
        words.push(transaction.operand(
            name,
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            representation_plan,
            nbc,
        ));
    }
    let callee = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        symbol,
        &vec![types::I64; args.len()],
        &[types::I64],
    );
    let call = builder.ins().call(callee, &words);
    let result = builder.inst_results(call)[0];
    let borrowed = contract == RuntimeBoxedReturn::BorrowedValue;
    if borrowed && out.is_some() {
        let retain = import_func_ref(
            module,
            import_ids,
            builder,
            import_refs,
            "molt_inc_ref_obj",
            &[types::I64],
            &[],
        );
        builder.ins().call(retain, &[result]);
    }
    let result = transaction.finish(
        result,
        None,
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        block_tracked_obj,
        block_tracked_ptr,
    );
    if let Some(out) = out {
        def_var_from_boxed_transport(
            module,
            import_ids,
            builder,
            import_refs,
            vars,
            representation_plan,
            nbc,
            out,
            result,
        );
    } else if !borrowed {
        bind_owned_runtime_result_name(None, result, module, import_ids, builder, vars);
    }
}

/// Call the canonical borrowed constructor (word ranges, or a slice's direct
/// bound words) inside one operand transaction: each input is materialized
/// once, a failed box skips the constructor, and minted owners are released on
/// both paths after the constructor retained them. Every backend mini-CFG
/// rejoins ordinary IR exception handling with a None result on failure.
/// Returned element views contain no minted owners and may serve an admitted
/// scalarized tuple. The constructed result follows the owned-result sink.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_fixed_aggregate_constructor(
    op: &OpIR,
    constructor: FixedAggregateConstructor,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) -> Option<Vec<Value>> {
    let args = op.args.as_deref().unwrap_or(&[]);
    let (symbol, first_word, arity) = match constructor {
        FixedAggregateConstructor::List => ("molt_list_from_values", 0, 2),
        FixedAggregateConstructor::Tuple => ("molt_tuple_from_values", 0, 2),
        FixedAggregateConstructor::Dataclass => ("molt_dataclass_new_from_values", 3, 5),
        FixedAggregateConstructor::DataclassTuple => {
            assert_eq!(
                args.len(),
                4,
                "dataclass_new needs name, fields, values and flags"
            );
            ("molt_dataclass_new", 0, 4)
        }
        FixedAggregateConstructor::ClassNew => {
            assert_eq!(args.len(), 1, "class_new needs a name");
            ("molt_class_new", 0, 1)
        }
        FixedAggregateConstructor::ClassDefinition { nbases, nattrs, .. } => {
            let expected = nattrs
                .checked_mul(2)
                .and_then(|n| n.checked_add(nbases))
                .and_then(|n| n.checked_add(1))
                .expect("class_def operand count overflow");
            assert_eq!(
                args.len(),
                expected,
                "class_def operands disagree with metadata"
            );
            ("molt_guarded_class_def", 1, 8)
        }
        FixedAggregateConstructor::Slice => {
            assert!(
                args.len() <= 3,
                "slice construction takes at most three bounds"
            );
            ("molt_slice_new", 0, 3)
        }
    };
    assert!(
        args.len() >= first_word,
        "fixed aggregate header is incomplete"
    );
    let mut transaction = NativeOperandTransaction::begin(
        builder,
        representation_plan,
        args.iter().map(String::as_str),
    );
    let none = transaction.none();
    let mut operands = Vec::with_capacity(args.len());
    for name in args {
        operands.push(transaction.operand(
            name,
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            representation_plan,
            nbc,
        ));
    }
    let words = &operands[first_word..];
    let call_args = match constructor {
        FixedAggregateConstructor::List | FixedAggregateConstructor::Tuple => {
            let (address, count) = store_borrowed_word_range(builder, words);
            vec![address, count]
        }
        FixedAggregateConstructor::Dataclass => {
            let (address, count) = store_borrowed_word_range(builder, words);
            vec![operands[0], operands[1], address, count, operands[2]]
        }
        FixedAggregateConstructor::DataclassTuple | FixedAggregateConstructor::ClassNew => {
            operands.clone()
        }
        FixedAggregateConstructor::ClassDefinition {
            nbases,
            nattrs,
            layout_size,
            layout_version,
            flags,
        } => {
            let (bases, base_count) = store_borrowed_word_range(builder, &words[..nbases]);
            let (attrs, _) = store_borrowed_word_range(builder, &words[nbases..]);
            let attr_count = builder.ins().iconst(types::I64, nattrs as i64);
            let size = builder.ins().iconst(types::I64, layout_size);
            let version = builder.ins().iconst(types::I64, layout_version);
            let flags = builder.ins().iconst(types::I64, flags);
            vec![
                operands[0],
                bases,
                base_count,
                attrs,
                attr_count,
                size,
                version,
                flags,
            ]
        }
        // Slice bounds are the constructor's direct arguments, so no range
        // storage exists; an omitted trailing bound is None.
        FixedAggregateConstructor::Slice => (0..arity)
            .map(|position| words.get(position).copied().unwrap_or(none))
            .collect(),
    };
    let callee = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        symbol,
        &vec![types::I64; arity],
        &[types::I64],
    );
    let call = builder.ins().call(callee, &call_args);
    let result = builder.inst_results(call)[0];
    let owns_temporaries = transaction.owns_temporaries();
    let result = transaction.finish(
        result,
        None,
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        block_tracked_obj,
        block_tracked_ptr,
    );
    bind_owned_runtime_result(op, result, module, import_ids, builder, vars);
    // Borrowed scalar views are valid only while the successfully constructed
    // tuple owns the elements. Publish None for every view on failure so no
    // consumer can retain an element whose source owner has already died.
    if !owns_temporaries && matches!(constructor, FixedAggregateConstructor::Tuple) {
        let constructed = builder.ins().icmp(IntCC::NotEqual, result, none);
        Some(
            words
                .iter()
                .map(|&word| builder.ins().select(constructed, word, none))
                .collect(),
        )
    } else {
        None
    }
}

/// One failure-atomic construction protocol for dict, set, and frozenset.
/// The aggregate keeps its original owner; mutator returns follow generated
/// ABI facts. Entries materialize lazily in entry order through one operand
/// transaction: a source repeated in a later entry reuses its first box, and
/// minted boxes stay owned until construction ends, so no shared box is
/// released while a later entry borrows it. Failed allocation, boxing or
/// insertion skips all later boxing and entries, releases the partial
/// aggregate and every initialized temporary, and leaves the pending exception
/// for the ordinary IR exception edge.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_hash_container_constructor(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    let (new_symbol, insert_symbol, width) = match op.kind.as_str() {
        "dict_new" => ("molt_dict_new", "molt_dict_set", 2),
        "set_new" => ("molt_set_new", "molt_set_add", 1),
        "frozenset_new" => ("molt_frozenset_new", "molt_frozenset_add", 1),
        kind => panic!("not a hash container constructor: {kind}"),
    };
    let args = op.args.as_deref().unwrap_or(&[]);
    assert!(
        args.len().is_multiple_of(width),
        "incomplete {} entry",
        op.kind
    );
    // Owner slots for every distinct full-width source precede allocation,
    // the first failure edge.
    let mut transaction = NativeOperandTransaction::begin(
        builder,
        representation_plan,
        args.iter().map(String::as_str),
    );
    let create = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        new_symbol,
        &[types::I64],
        &[types::I64],
    );
    let insert_params = vec![types::I64; width + 1];
    let insert = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        insert_symbol,
        &insert_params,
        &[types::I64],
    );
    let capacity = builder
        .ins()
        .iconst(types::I64, (args.len() / width) as i64);
    let created = builder.ins().call(create, &[capacity]);
    let aggregate = builder.inst_results(created)[0];
    let unallocated = builder.ins().icmp_imm(IntCC::Equal, aggregate, box_none());
    transaction.fail_if(builder, sealed_blocks, unallocated);
    for entry in args.chunks(width) {
        let mut operands = vec![aggregate];
        for name in entry {
            operands.push(transaction.operand(
                name,
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                vars,
                representation_plan,
                nbc,
            ));
        }
        let inserted = builder.ins().call(insert, &operands);
        let result = builder.inst_results(inserted)[0];
        bind_runtime_import_result_name(
            None,
            result,
            insert_symbol,
            width + 1,
            module,
            import_ids,
            builder,
            vars,
        );
        transaction.continue_unless_pending(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
        );
    }
    let result = transaction.finish(
        aggregate,
        Some(aggregate),
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        block_tracked_obj,
        block_tracked_ptr,
    );
    bind_owned_runtime_result(op, result, module, import_ids, builder, vars);
}

/// Consume a transferred runtime owner even when there is no named SSA result.
/// Borrowed results must not enter this sink without first being retained.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn bind_owned_runtime_result(
    op: &OpIR,
    result: Value,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    bind_owned_runtime_result_name(
        crate::tir::simple_def_use::simple_ir_out_result(op),
        result,
        module,
        import_ids,
        builder,
        vars,
    );
}

/// Field-role consumers pass the selected result name, not a synthetic op.
/// This is the same owned-result sink for ordinary and positional results.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn bind_owned_runtime_result_name(
    out: Option<&str>,
    result: Value,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    if let Some(out) = out.filter(|name| *name != "none") {
        def_var_named(builder, vars, out, result);
    } else {
        let release = SimpleBackend::import_func_id_split(
            module,
            import_ids,
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        let release = module.declare_func_in_func(release, builder.func);
        builder.ins().call(release, &[result]);
    }
}

/// Carry per-block ownership cleanup roots across compiler-internal CFG.
///
/// These splits are transparent to TIR, so values owned by the origin block
/// remain owned after every internal edge rejoins. Leaving them keyed by the
/// now-closed origin makes later return/exception cleanup unable to find them.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn carry_internal_cfg_tracking(
    origin: Option<Block>,
    merge: Block,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    let Some(origin) = origin.filter(|origin| *origin != merge) else {
        return;
    };
    for tracked in [block_tracked_obj, block_tracked_ptr] {
        let live = tracked.remove(&origin).unwrap_or_default();
        if !live.is_empty() {
            extend_unique_tracked(tracked.entry(merge).or_default(), live);
        }
    }
}

/// Keep task initialization off the allocation-failure edge. The original boxed
/// result reaches the ordinary exception edge, which owns frame/RC unwinding.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn begin_task_initialization(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    task: Value,
) -> Block {
    let initialize = builder.create_block();
    let done = builder.create_block();
    let allocated = builder.ins().icmp_imm(IntCC::NotEqual, task, box_none());
    builder.ins().brif(allocated, initialize, &[], done, &[]);
    switch_to_block_materialized(builder, initialize);
    seal_block_once(builder, sealed_blocks, initialize);
    done
}

/// Enter the code-slot-backed frame owned by this compiled invocation.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_owned_execution_frame_enter(
    entered: Variable,
    code_id: i64,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
) {
    let code_id_val = builder.ins().iconst(types::I64, code_id);
    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        "molt_trace_enter_slot",
        &[types::I64],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let _ = builder.ins().call(local_callee, &[code_id_val]);
    let active = builder.ins().iconst(types::I8, 1);
    builder.def_var(entered, active);
}

/// Release only a frame entered by this invocation, including early-return paths.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_owned_execution_frame_exit(
    entered: Option<Variable>,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
) {
    let Some(entered) = entered else {
        return;
    };
    let active = builder.use_var(entered);
    let pop_block = builder.create_block();
    let done_block = builder.create_block();
    builder.ins().brif(active, pop_block, &[], done_block, &[]);
    switch_to_block_materialized(builder, pop_block);
    seal_block_once(builder, sealed_blocks, pop_block);
    // Retire ownership before releasing frame-owned values can call Python.
    let inactive = builder.ins().iconst(types::I8, 0);
    builder.def_var(entered, inactive);
    let exit = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_trace_exit",
        &[],
        &[types::I64],
    );
    builder.ins().call(exit, &[]);
    jump_block(builder, done_block, &[]);
    switch_to_block_materialized(builder, done_block);
    seal_block_once(builder, sealed_blocks, done_block);
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) static EMPTY_VEC_STRING: Vec<String> = Vec::new();

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn is_cold_module_chunk_function(
    name: &str,
) -> bool {
    name.contains("__molt_module_chunk_")
}

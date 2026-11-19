use super::*;
use molt_ir::runtime_fn_generated::RuntimeFn;

/// One object module's runtime imports, by dense `RuntimeFn` index. An entry
/// is declared on first use from its generated record; its signature is never
/// restated, so a second request cannot disagree with the first.
#[cfg(feature = "native-backend")]
pub(crate) struct RuntimeImportIds {
    ids: Box<[cranelift_codegen::packed_option::PackedOption<cranelift_module::FuncId>]>,
}

#[cfg(feature = "native-backend")]
impl Default for RuntimeImportIds {
    fn default() -> Self {
        Self {
            ids: vec![Default::default(); RuntimeFn::COUNT].into_boxed_slice(),
        }
    }
}

#[cfg(feature = "native-backend")]
impl RuntimeImportIds {
    /// The module's declaration of `function`, made on first use. Requires
    /// `module` to be the module this table belongs to.
    pub(crate) fn func_id(
        &mut self,
        module: &mut dyn Module,
        function: RuntimeFn,
    ) -> cranelift_module::FuncId {
        let slot = &mut self.ids[function.index()];
        if let Some(id) = slot.expand() {
            return id;
        }
        let id = declare_runtime_function(module, function);
        *slot = id.into();
        id
    }

    /// The module's declaration of `function`, if one was made.
    #[cfg(test)]
    pub(crate) fn get(&self, function: RuntimeFn) -> Option<cranelift_module::FuncId> {
        self.ids[function.index()].expand()
    }

    /// Every declared entry, in `RuntimeFn` order.
    #[cfg(test)]
    pub(crate) fn declared(
        &self,
    ) -> impl Iterator<Item = (RuntimeFn, cranelift_module::FuncId)> + '_ {
        RuntimeFn::ALL
            .iter()
            .filter_map(|&function| self.get(function).map(|id| (function, id)))
    }
}

/// The one Cranelift path from a runtime entry to a callable reference in
/// this body: the module declaration, then this function's reference, each
/// cached by dense `RuntimeFn` index.
#[cfg(feature = "native-backend")]
pub(crate) fn import_runtime_func_ref(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder,
    local_refs: &mut NativeFunctionImports,
    function: RuntimeFn,
) -> FuncRef {
    if let Some(func_ref) = local_refs.get(function) {
        return func_ref;
    }
    let func_id = import_ids.func_id(module, function);
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    local_refs.insert(function, func_ref);
    func_ref
}

/// Load an immutable dense index through its typed stable program identity.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_program_slot_id(
    kind: molt_ir::code_slots::ProgramSlotKind,
    owner: &str,
    module: &mut dyn Module,
    builder: &mut FunctionBuilder<'_>,
) -> Value {
    let symbol = molt_ir::code_slots::program_slot_symbol(kind, owner);
    let data = module
        .declare_data(&symbol, Linkage::Import, false, false)
        .unwrap_or_else(|error| panic!("failed to import program-slot binding {symbol}: {error}"));
    let reference = module.declare_data_in_func(data, builder.func);
    let pointer = builder
        .ins()
        .symbol_value(module.target_config().pointer_type(), reference);
    builder.ins().load(
        types::I64,
        MemFlagsData::trusted().with_readonly(),
        pointer,
        0,
    )
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Relocate a compiled poll and its provider's spill extent together.
    pub(crate) fn task_poll_operands(
        module: &mut dyn Module,
        builder: &mut FunctionBuilder<'_>,
        poll: molt_ir::code_slots::TaskPoll<(&str, Linkage)>,
        layout_bytes: i64,
    ) -> (Value, Value) {
        poll.operands(
            builder,
            layout_bytes,
            |builder, bits| builder.ins().iconst(types::I64, bits as i64),
            |builder, (name, linkage)| {
                let mut signature = module.make_signature();
                signature.params.push(AbiParam::new(types::I64));
                signature.returns.push(AbiParam::new(types::I64));
                let function = declare_compiled_execution_function(
                    module, name, linkage, &signature,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "task poll target `{name}` must have ABI (context, task) -> i64: {error}"
                    )
                });
                let local = module.declare_func_in_func(function, builder.func);
                let address = builder.ins().func_addr(types::I64, local);
                let bytes = crate::native_backend::load_stateful_frame_size(module, builder, name);
                (address, bytes)
            },
        )
    }

    pub(crate) fn operation_poll<'a>(
        op: &OpIR,
        table: &'a molt_ir::FunctionTable,
    ) -> molt_ir::code_slots::TaskPoll<(&'a str, Linkage)> {
        let compiled = op.callee.map(|id| {
            let linkage = if table.defines(id) {
                Linkage::Export
            } else {
                Linkage::Import
            };
            (table.symbol(id), linkage)
        });
        molt_ir::code_slots::TaskPoll::from_targets(compiled, op.runtime).unwrap_or_else(|| {
            panic!(
                "{} requires a compiled poll or a manifest poll key",
                op.kind
            )
        })
    }

    pub(crate) fn intern_data_segment(
        module: &mut dyn Module,
        data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
        next_data_id: &mut u64,
        bytes: &[u8],
    ) -> cranelift_module::DataId {
        if let Some(existing) = data_pool.get(bytes) {
            return *existing;
        }
        let name = format!("data_pool_{}", *next_data_id);
        *next_data_id += 1;
        let data_id = module
            .declare_data(&name, Linkage::Local, false, false)
            .unwrap();
        let mut data_ctx = DataDescription::new();
        data_ctx.define(bytes.to_vec().into_boxed_slice());
        module.define_data(data_id, &data_ctx).unwrap();
        data_pool.insert(bytes.to_vec(), data_id);
        data_id
    }

    /// The module's declaration of runtime entry `function`.
    pub(crate) fn import_runtime_func_id_split(
        module: &mut dyn Module,
        import_ids: &mut RuntimeImportIds,
        function: RuntimeFn,
    ) -> cranelift_module::FuncId {
        import_ids.func_id(module, function)
    }
}

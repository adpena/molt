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

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Runtime polls use manifest keys; compiled polls use raw code addresses.
    /// Resolve the domain here so task construction and dispatch need no scan.
    pub(crate) fn task_poll_identity(
        module: &mut dyn Module,
        builder: &mut FunctionBuilder<'_>,
        name: &str,
        linkage: Linkage,
    ) -> Value {
        if let Some(key) = molt_ir::runtime_callable_abi_generated::runtime_poll_native_key(name) {
            return builder.ins().iconst(types::I64, key as i64);
        }
        let mut signature = module.make_signature();
        signature.params.push(AbiParam::new(types::I64));
        signature.returns.push(AbiParam::new(types::I64));
        let function = declare_compiled_execution_function(module, name, linkage, &signature)
            .unwrap_or_else(|error| {
                panic!("task poll target `{name}` must have ABI (context, task) -> i64: {error}")
            });
        let local = module.declare_func_in_func(function, builder.func);
        builder.ins().func_addr(types::I64, local)
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

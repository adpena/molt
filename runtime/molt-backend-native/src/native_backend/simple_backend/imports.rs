use super::*;
use crate::runtime_import_abi::{RuntimeImportSignature, RuntimeReturnAbi};

#[cfg(feature = "native-backend")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImportSignatureShape {
    params: Vec<String>,
    returns: Vec<String>,
}

#[cfg(feature = "native-backend")]
impl ImportSignatureShape {
    pub(crate) fn from_types(params: &[types::Type], returns: &[types::Type]) -> Self {
        Self {
            params: params.iter().map(ToString::to_string).collect(),
            returns: returns.iter().map(ToString::to_string).collect(),
        }
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(crate) fn intern_data_segment(
        module: &mut ObjectModule,
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

    /// Walk backwards from `before_idx` to find a `"const"` op whose `out`
    /// matches `var_name` and return its integer value.  Used by the
    /// iter_next peephole to resolve constant index arguments.
    pub(crate) fn resolve_const_int(
        ops: &[OpIR],
        before_idx: usize,
        var_name: &str,
    ) -> Option<i64> {
        for i in (0..before_idx).rev() {
            let op = &ops[i];
            if op.kind == "const"
                && let Some(ref out) = op.out
                && out == var_name
            {
                return op.value;
            }
        }
        None
    }

    /// Cached version of `module.declare_function(name, Linkage::Import, &sig)`.
    /// Returns the `FuncId` for the given runtime import, reusing a previous
    /// declaration when the same name has already been declared.  The signature
    /// shape is validated on cache hits to guard against mismatches.
    ///
    /// Takes split borrows (`module` + `import_ids`) so callers can hold a
    /// concurrent `FunctionBuilder` borrow on `self.ctx.func`.
    pub(crate) fn import_func_id_split(
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        name: &'static str,
        params: &[types::Type],
        returns: &[types::Type],
    ) -> cranelift_module::FuncId {
        let shape = ImportSignatureShape::from_types(params, returns);
        if let Some((func_id, cached_shape)) = import_ids.get(name) {
            assert_eq!(
                cached_shape, &shape,
                "import signature mismatch for {name}: {:?} vs {:?}",
                cached_shape, shape
            );
            return *func_id;
        }

        let mut sig = module.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(*param));
        }
        for ret in returns {
            sig.returns.push(AbiParam::new(*ret));
        }
        let func_id = module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap_or_else(|err| {
                panic!("import declaration mismatch for `{name}`: expected {shape:?}: {err}")
            });
        import_ids.insert(name, (func_id, shape));
        func_id
    }

    pub(crate) fn import_runtime_func_id_split(
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        signature: RuntimeImportSignature,
    ) -> cranelift_module::FuncId {
        let params = vec![types::I64; signature.param_count];
        match signature.return_abi {
            RuntimeReturnAbi::I64 => Self::import_func_id_split(
                module,
                import_ids,
                signature.name,
                &params,
                &[types::I64],
            ),
            RuntimeReturnAbi::Void => {
                Self::import_func_id_split(module, import_ids, signature.name, &params, &[])
            }
        }
    }
}

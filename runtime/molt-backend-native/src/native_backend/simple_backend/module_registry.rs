//! Native emission of the per-build module registry blob (import bedrock,
//! `docs/design/foundation/import_bedrock_frozen_module_layer.md` §3).
//!
//! The Python generator (`molt.cli.module_registry`, the checked-in layout
//! authority) serializes the registry — header, rows, sorted name table —
//! into `ModuleRegistryIR::blob` with zeroed init-pointer slots and a
//! relocation list.  This emitter defines the bytes under the exported
//! `molt_module_registry_blob` symbol and attaches one native
//! function-address relocation per MODULE_INIT_TABLE entry
//! (`DataDescription::write_function_addr`, the same portable pointer-reloc
//! form the app callable resolver uses).  The C main stub registers the blob
//! with the runtime (`molt_module_registry_install`) before
//! `molt_runtime_init`, so module identity is resolved without this backend
//! ever knowing the blob layout.

use super::*;

pub(crate) const MODULE_REGISTRY_BLOB_SYMBOL: &str = "molt_module_registry_blob";

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Emit the registry blob into this object.  Must run after the main
    /// function flush so init functions defined in this object reuse their
    /// existing `FuncId` declarations; inits living in other batch objects
    /// are declared `Import` and resolve at link (their `Export` definitions
    /// live in the sibling objects).
    pub(in crate::native_backend::simple_backend) fn emit_module_registry_blob(
        &mut self,
        registry: &ModuleRegistryIR,
    ) {
        registry
            .validate()
            .unwrap_or_else(|err| panic!("module registry blob rejected: {err}"));

        let data_id = self
            .module
            .declare_data(MODULE_REGISTRY_BLOB_SYMBOL, Linkage::Export, false, false)
            .unwrap_or_else(|e| panic!("failed to declare {MODULE_REGISTRY_BLOB_SYMBOL}: {e:?}"));
        let mut desc = DataDescription::new();
        desc.set_align(8);
        desc.define(registry.blob.clone().into_boxed_slice());

        // Init bodies take no parameters; the address is taken via a pointer
        // relocation, so the signature only matters when the symbol was not
        // already declared by a direct call in this object.
        let mut canonical_sig = self.module.make_signature();
        canonical_sig.returns.push(AbiParam::new(types::I64));
        for (offset, symbol) in &registry.relocs {
            let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                self.module.get_name(symbol)
            {
                id
            } else {
                self.module
                    .declare_function(symbol, Linkage::Import, &canonical_sig)
                    .unwrap_or_else(|e| {
                        panic!("module registry: failed to declare init symbol '{symbol}': {e:?}")
                    })
            };
            let func_ref = self.module.declare_func_in_data(func_id, &mut desc);
            let slot = u32::try_from(*offset).unwrap_or_else(|_| {
                panic!("module registry reloc offset {offset} exceeds u32 addressing")
            });
            desc.write_function_addr(slot, func_ref);
        }
        self.module
            .define_data(data_id, &desc)
            .unwrap_or_else(|e| panic!("failed to define {MODULE_REGISTRY_BLOB_SYMBOL}: {e:?}"));
    }
}

//! LLVM emission of the per-build module registry blob (import bedrock,
//! design doc 69 §3) — the byte-for-byte ABI twin of the Cranelift emitter in
//! `native_backend/simple_backend/module_registry.rs`.
//!
//! The blob bytes come fully serialized from the Python layout authority
//! (`molt.cli.module_registry`); this emitter splits them at the relocation
//! offsets into a packed LLVM constant struct whose pointer fields carry the
//! init-function-address relocations.  Every relocation offset is 8-aligned
//! by the layout contract (header 48 bytes, rows 32 bytes, init pointer at
//! row offset 8), so the packed struct with global alignment 8 places each
//! pointer exactly at its blob offset.

#[cfg(feature = "llvm")]
use super::LlvmBackend;
#[cfg(feature = "llvm")]
use inkwell::{AddressSpace, module::Linkage, types::BasicTypeEnum, values::BasicValueEnum};
#[cfg(feature = "llvm")]
use molt_ir::ModuleRegistryIR;

#[cfg(feature = "llvm")]
pub(crate) const MODULE_REGISTRY_BLOB_SYMBOL: &str = "molt_module_registry_blob";

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub fn emit_module_registry_blob(&self, registry: &ModuleRegistryIR) {
        registry
            .validate()
            .unwrap_or_else(|err| panic!("module registry blob rejected: {err}"));

        let ctx = self.context;
        let module = &self.module;
        let i8_ty = ctx.i8_type();
        let i64_ty = ctx.i64_type();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());

        let mut relocs: Vec<(u64, &str)> = registry
            .relocs
            .iter()
            .map(|(offset, symbol)| (*offset, symbol.as_str()))
            .collect();
        relocs.sort_by_key(|(offset, _)| *offset);
        for pair in relocs.windows(2) {
            assert!(
                pair[0].0 + 8 <= pair[1].0,
                "module registry relocations overlap: {} and {}",
                pair[0].0,
                pair[1].0
            );
        }

        let blob = &registry.blob;
        let mut field_types: Vec<BasicTypeEnum> = Vec::new();
        let mut field_values: Vec<BasicValueEnum> = Vec::new();
        let mut cursor: usize = 0;
        let mut push_bytes =
            |bytes: &[u8],
             field_types: &mut Vec<BasicTypeEnum<'ctx>>,
             field_values: &mut Vec<BasicValueEnum<'ctx>>| {
                if bytes.is_empty() {
                    return;
                }
                field_types.push(i8_ty.array_type(bytes.len() as u32).into());
                field_values.push(ctx.const_string(bytes, false).into());
            };
        for (offset, symbol) in &relocs {
            let offset = *offset as usize;
            assert!(
                offset % 8 == 0,
                "module registry reloc offset {offset} is not 8-aligned"
            );
            push_bytes(&blob[cursor..offset], &mut field_types, &mut field_values);
            let init_fn = module.get_function(symbol).unwrap_or_else(|| {
                // Address-taken only: with opaque pointers the declared
                // signature is irrelevant; declare External so relocations
                // resolve against the sibling batch object or staticlib.
                let placeholder_ty = i64_ty.fn_type(&[], false);
                module.add_function(symbol, placeholder_ty, Some(Linkage::External))
            });
            field_types.push(ptr_ty.into());
            field_values.push(init_fn.as_global_value().as_pointer_value().into());
            cursor = offset + 8;
        }
        push_bytes(&blob[cursor..], &mut field_types, &mut field_values);

        // Packed struct: field offsets are exactly the byte offsets above.
        let struct_ty = ctx.struct_type(&field_types, true);
        let initializer = ctx.const_struct(&field_values, true);
        let global = module.add_global(struct_ty, None, MODULE_REGISTRY_BLOB_SYMBOL);
        global.set_linkage(Linkage::External);
        global.set_constant(true);
        global.set_alignment(8);
        global.set_initializer(&initializer);
    }
}

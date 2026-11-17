//! LLVM emission of the per-build module registry blob (import bedrock,
//! design doc 69 §3) — the byte-for-byte ABI twin of the Cranelift emitter in
//! `native_backend/simple_backend/module_registry.rs`.
//!
//! The blob bytes come fully serialized from the Python layout authority
//! (`molt.cli.module_registry`); this emitter splits them at the relocation
//! offsets into a packed LLVM constant struct whose pointer fields carry the
//! init-function-address relocations.  Every relocation offset is 8-aligned
//! by the layout authority, so the packed struct with global alignment 8 places each
//! pointer exactly at its blob offset.

#[cfg(feature = "llvm")]
use super::LlvmBackend;
#[cfg(feature = "llvm")]
use inkwell::{AddressSpace, module::Linkage, types::BasicTypeEnum, values::BasicValueEnum};
#[cfg(feature = "llvm")]
use molt_ir::ModuleRegistryIR;

#[cfg(feature = "llvm")]
pub(crate) use molt_ir::MODULE_REGISTRY_BLOB_SYMBOL;

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub fn emit_module_registry_blob(&self, registry: &ModuleRegistryIR) {
        registry
            .validate()
            .unwrap_or_else(|err| panic!("module registry blob rejected: {err}"));

        let ctx = self.context;
        let module = &self.module;
        let i8_ty = ctx.i8_type();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());
        let pointer_bytes = self.pointer_sized_int_type().get_bit_width() / 8;
        assert!(
            pointer_bytes == 4 || pointer_bytes == 8,
            "module registry slots hold a 4- or 8-byte pointer"
        );

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
        let push_bytes = |bytes: &[u8],
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
                offset.is_multiple_of(8),
                "module registry reloc offset {offset} is not 8-aligned"
            );
            push_bytes(&blob[cursor..offset], &mut field_types, &mut field_values);
            // The body may live in a sibling batch object or the shared stdlib
            // archive; its declaration carries the exact linkage row.
            let init_fn = crate::llvm_backend::lowering::linked_compiled_function(self, symbol);
            field_types.push(ptr_ty.into());
            field_values.push(init_fn.as_global_value().as_pointer_value().into());
            // Each slot is a little-endian u64 (the runtime reads a word). A
            // 32-bit target's pointer fills its low half; the high half is 0.
            let pad = 8 - pointer_bytes;
            if pad > 0 {
                field_types.push(i8_ty.array_type(pad).into());
                field_values.push(i8_ty.array_type(pad).const_zero().into());
            }
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

        // The ready block: writable zeroed words the runtime stores borrowed
        // module pointers into (IC2).
        let words = (registry.ready_block_bytes() / 8) as u32;
        let ready_ty = ctx.i64_type().array_type(words);
        let ready = module.add_global(ready_ty, None, molt_ir::MODULE_READY_SYMBOL);
        ready.set_linkage(Linkage::External);
        ready.set_constant(false);
        ready.set_alignment(8);
        ready.set_initializer(&ready_ty.const_zero());
    }
}

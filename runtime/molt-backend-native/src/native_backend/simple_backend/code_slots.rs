//! Application-owned dense IDs behind stable, immutable slot symbols.

use super::*;
use molt_ir::code_slots::{CodeSlotBindings, code_slot_symbol};

impl SimpleBackend {
    pub(in crate::native_backend::simple_backend) fn emit_code_slot_bindings(
        &mut self,
        bindings: &CodeSlotBindings,
    ) {
        for (owner, slot) in bindings.iter() {
            let symbol = code_slot_symbol(owner);
            let data = self
                .module
                .declare_data(&symbol, Linkage::Export, false, false)
                .unwrap_or_else(|error| {
                    panic!("failed to declare code-slot binding {symbol}: {error}")
                });
            let bytes = match self.module.isa().endianness() {
                cranelift_codegen::ir::Endianness::Little => slot.to_le_bytes(),
                cranelift_codegen::ir::Endianness::Big => slot.to_be_bytes(),
            };
            let mut description = DataDescription::new();
            description.set_align(8);
            description.define(Box::new(bytes));
            self.module
                .define_data(data, &description)
                .unwrap_or_else(|error| {
                    panic!("failed to define code-slot binding {symbol}: {error}")
                });
        }
    }
}

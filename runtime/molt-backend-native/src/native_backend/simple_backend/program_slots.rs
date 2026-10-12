//! Application-owned dense IDs behind stable, immutable slot symbols.

use super::*;
use molt_ir::code_slots::{CodeSlotBindings, ProgramSlotKind, program_slot_symbol};

impl SimpleBackend {
    pub(in crate::native_backend::simple_backend) fn emit_program_slot_bindings(
        &mut self,
        code_slots: Option<&CodeSlotBindings>,
        modules: Option<&molt_ir::ModuleRegistryIR>,
    ) {
        let codes = code_slots
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .map(|(owner, slot)| (ProgramSlotKind::Code, owner, slot));
        let modules = modules
            .into_iter()
            .flat_map(|registry| registry.module_slot_bindings())
            .map(|(owner, slot)| (ProgramSlotKind::Module, owner, slot));
        for (kind, owner, slot) in codes.chain(modules) {
            let symbol = program_slot_symbol(kind, owner);
            let data = self
                .module
                .declare_data(&symbol, Linkage::Export, false, false)
                .unwrap_or_else(|error| {
                    panic!("failed to declare program-slot binding {symbol}: {error}")
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
                    panic!("failed to define program-slot binding {symbol}: {error}")
                });
        }
    }
}

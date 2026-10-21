//! Object-format-aware test symbol inspection and Cranelift IR fixtures shared
//! by native test families.

use std::collections::BTreeSet;

/// The host ISA's frontend configuration, for tests that build a Cranelift
/// function without a module. Production code passes its module's
/// `target_config()` to `FunctionBuilder::finalize`.
#[cfg(feature = "native-backend")]
pub(super) fn host_frontend_config() -> cranelift_codegen::isa::TargetFrontendConfig {
    use cranelift_codegen::settings;
    cranelift_native::builder()
        .expect("host ISA is supported")
        .finish(settings::Flags::new(settings::builder()))
        .expect("host ISA accepts the default flags")
        .frontend_config()
}

/// A `store` through a `stack_addr`: the pair `InstBuilder::stack_store` emits.
#[cfg(feature = "native-backend")]
pub(super) fn is_stack_store(
    function: &cranelift_codegen::ir::Function,
    inst: cranelift_codegen::ir::Inst,
) -> bool {
    function.dfg.insts[inst].opcode() == cranelift_codegen::ir::Opcode::Store
        && is_stack_address(function, function.dfg.inst_args(inst)[1])
}

/// A `load` through a `stack_addr`: the pair `InstBuilder::stack_load` emits.
#[cfg(feature = "native-backend")]
pub(super) fn is_stack_load(
    function: &cranelift_codegen::ir::Function,
    inst: cranelift_codegen::ir::Inst,
) -> bool {
    function.dfg.insts[inst].opcode() == cranelift_codegen::ir::Opcode::Load
        && is_stack_address(function, function.dfg.inst_args(inst)[0])
}

#[cfg(feature = "native-backend")]
fn is_stack_address(
    function: &cranelift_codegen::ir::Function,
    address: cranelift_codegen::ir::Value,
) -> bool {
    function.dfg.value_def(address).inst().is_some_and(|def| {
        function.dfg.insts[def].opcode() == cranelift_codegen::ir::Opcode::StackAddr
    })
}

pub(super) struct NativeObjectSymbols {
    pub(super) defined: BTreeSet<String>,
    pub(super) undefined: BTreeSet<String>,
}

pub(super) fn native_object_symbols(bytes: &[u8]) -> NativeObjectSymbols {
    use object::{BinaryFormat, Object, ObjectSymbol};
    let object = object::File::parse(bytes).expect("parse native object");
    let mut symbols = NativeObjectSymbols {
        defined: BTreeSet::new(),
        undefined: BTreeSet::new(),
    };
    for symbol in object.symbols() {
        let name = symbol.name().expect("native symbol name");
        let name = if object.format() == BinaryFormat::MachO {
            name.strip_prefix('_').unwrap_or(name)
        } else {
            name
        };
        if symbol.is_undefined() {
            symbols.undefined.insert(name.to_owned());
        } else if symbol.is_definition() {
            symbols.defined.insert(name.to_owned());
        }
    }
    symbols
}

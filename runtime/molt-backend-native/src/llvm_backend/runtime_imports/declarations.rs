#[cfg(feature = "llvm")]
use super::attributes::{add_memory_read, add_nounwind, add_willreturn};
#[cfg(feature = "llvm")]
use inkwell::AddressSpace;
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType};
#[cfg(feature = "llvm")]
use inkwell::values::FunctionValue;
#[cfg(feature = "llvm")]
use molt_ir::runtime_callable_abi_generated::RuntimeExecutionContext;
#[cfg(feature = "llvm")]
use molt_ir::runtime_fn_generated::{RuntimeEffects, RuntimeFn, RuntimeMachineType};
#[cfg(feature = "llvm")]
use std::cell::Cell;

/// The LLVM type of one runtime machine type.
#[cfg(feature = "llvm")]
fn machine_type<'ctx>(ctx: &'ctx Context, value: RuntimeMachineType) -> BasicTypeEnum<'ctx> {
    match value {
        RuntimeMachineType::Word => ctx.i64_type().into(),
        RuntimeMachineType::Address => ctx.ptr_type(AddressSpace::default()).into(),
        RuntimeMachineType::I32 => ctx.i32_type().into(),
        RuntimeMachineType::F64 => ctx.f64_type().into(),
    }
}

/// The physical LLVM type of a runtime entry, from its generated record alone:
/// the execution context word first when the entry is attached, then one
/// parameter per machine type (I2).
#[cfg(feature = "llvm")]
pub(crate) fn runtime_function_type<'ctx>(
    ctx: &'ctx Context,
    function: RuntimeFn,
) -> FunctionType<'ctx> {
    let mut params: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::with_capacity(function.params() + 1);
    if function.context() == RuntimeExecutionContext::Attached {
        params.push(ctx.i64_type().into());
    }
    params.extend(
        function
            .param_types()
            .iter()
            .map(|&value| BasicMetadataTypeEnum::from(machine_type(ctx, value))),
    );
    match function
        .result_type()
        .map(|result| machine_type(ctx, result))
    {
        Some(BasicTypeEnum::IntType(result)) => result.fn_type(&params, false),
        Some(BasicTypeEnum::FloatType(result)) => result.fn_type(&params, false),
        Some(BasicTypeEnum::PointerType(result)) => result.fn_type(&params, false),
        Some(other) => unreachable!("runtime machine type {other:?} is not a scalar"),
        None => ctx.void_type().fn_type(&params, false),
    }
}

/// Every runtime entry is `nounwind`: panics become pending exceptions at the
/// export boundary. Stronger facts come only from the manifest's `effects`
/// (I7): an entry that may run Python code may not terminate, so it never
/// receives `willreturn`.
#[cfg(feature = "llvm")]
fn apply_runtime_attributes(ctx: &Context, func: FunctionValue<'_>, effects: RuntimeEffects) {
    add_nounwind(ctx, func);
    match effects {
        RuntimeEffects::Impure => {}
        RuntimeEffects::MayRaise | RuntimeEffects::NothrowWrite => add_willreturn(ctx, func),
        RuntimeEffects::NothrowRead => {
            add_willreturn(ctx, func);
            add_memory_read(ctx, func);
        }
    }
}

/// The module's declaration of `function`: the one LLVM path that declares a
/// runtime function. The linker symbol, the type and the attributes all come
/// from the generated record. An existing declaration (made earlier through
/// this same path) is returned as is, so a second request cannot duplicate it.
#[cfg(feature = "llvm")]
pub(crate) fn runtime_function<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    function: RuntimeFn,
) -> FunctionValue<'ctx> {
    if let Some(declared) = module.get_function(function.linker_symbol()) {
        return declared;
    }
    let declared = module.add_function(
        function.linker_symbol(),
        runtime_function_type(ctx, function),
        Some(inkwell::module::Linkage::External),
    );
    apply_runtime_attributes(ctx, declared, function.effects());
    declared
}

/// One LLVM module's runtime declarations, by dense `RuntimeFn` index, so a
/// lowering asks the module once per entry.
#[cfg(feature = "llvm")]
pub(crate) struct LlvmRuntimeImports<'ctx> {
    slots: Box<[Cell<Option<FunctionValue<'ctx>>>]>,
}

#[cfg(feature = "llvm")]
impl Default for LlvmRuntimeImports<'_> {
    fn default() -> Self {
        Self {
            slots: (0..RuntimeFn::COUNT).map(|_| Cell::new(None)).collect(),
        }
    }
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmRuntimeImports<'ctx> {
    /// The module's declaration of `function`. Requires `module` to be the
    /// module this table belongs to.
    pub(crate) fn get(
        &self,
        ctx: &'ctx Context,
        module: &Module<'ctx>,
        function: RuntimeFn,
    ) -> FunctionValue<'ctx> {
        let slot = &self.slots[function.index()];
        if let Some(declared) = slot.get() {
            return declared;
        }
        let declared = runtime_function(ctx, module, function);
        slot.set(Some(declared));
        declared
    }
}

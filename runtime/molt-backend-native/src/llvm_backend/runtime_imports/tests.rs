use super::attributes::MEMORY_READ;
use super::declarations::runtime_function_type;
use super::*;
use crate::runtime_import_abi::TRAMPOLINE_RUNTIME_IMPORTS;
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::context::Context;
use inkwell::types::{BasicMetadataTypeEnum, BasicTypeEnum};
use inkwell::values::FunctionValue;
use molt_ir::runtime_callable_abi_generated::RuntimeExecutionContext;
use molt_ir::runtime_fn_generated::{RuntimeEffects, RuntimeFn, RuntimeMachineType};

fn has_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
    let kind = Attribute::get_named_enum_kind_id(attr_name);
    func.get_enum_attribute(AttributeLoc::Function, kind)
        .is_some()
}

fn memory_attr(func: FunctionValue<'_>) -> Option<u64> {
    let kind = Attribute::get_named_enum_kind_id("memory");
    func.get_enum_attribute(AttributeLoc::Function, kind)
        .map(|attribute| attribute.get_enum_value())
}

/// The machine class an LLVM parameter or result type denotes, read back from
/// the module. The oracle for every declaration is the generated record.
fn machine_class(ty: BasicTypeEnum<'_>) -> RuntimeMachineType {
    match ty {
        BasicTypeEnum::IntType(int) if int.get_bit_width() == 64 => RuntimeMachineType::Word,
        BasicTypeEnum::IntType(int) if int.get_bit_width() == 32 => RuntimeMachineType::I32,
        BasicTypeEnum::FloatType(_) => RuntimeMachineType::F64,
        BasicTypeEnum::PointerType(_) => RuntimeMachineType::Address,
        other => panic!("runtime declaration uses non-machine type {other:?}"),
    }
}

fn param_class(ty: BasicMetadataTypeEnum<'_>) -> RuntimeMachineType {
    machine_class(BasicTypeEnum::try_from(ty).expect("runtime parameter is a basic type"))
}

/// Every runtime entry declares under its generated linker symbol with
/// exactly its record's machine signature, the context word first when it is
/// attached, and nothing else.
#[test]
fn every_runtime_entry_declares_its_record_signature() {
    let ctx = Context::create();
    let module = ctx.create_module("runtime_imports");
    let imports = LlvmRuntimeImports::default();
    for &function in RuntimeFn::ALL {
        let declared = imports.get(&ctx, &module, function);
        assert_eq!(
            declared.get_name().to_str().unwrap(),
            function.linker_symbol(),
            "{} declares under its linker symbol",
            function.symbol()
        );
        let ty = declared.get_type();
        let mut params = ty.get_param_types().into_iter();
        if function.context() == RuntimeExecutionContext::Attached {
            assert_eq!(
                params.next().map(param_class),
                Some(RuntimeMachineType::Word),
                "{} takes its execution context first",
                function.symbol()
            );
        }
        let logical: Vec<RuntimeMachineType> = params.map(param_class).collect();
        assert_eq!(
            logical,
            function.param_types(),
            "{} parameters",
            function.symbol()
        );
        assert_eq!(
            ty.get_return_type().map(machine_class),
            function.result_type(),
            "{} result",
            function.symbol()
        );
        assert_eq!(ty, runtime_function_type(&ctx, function));
    }
}

/// A second request returns the first declaration, and a declaration made
/// through the uncached path is the same function: the module never holds a
/// renamed duplicate (`name.1`) of a runtime entry.
#[test]
fn a_runtime_entry_is_declared_once_per_module() {
    let ctx = Context::create();
    let module = ctx.create_module("runtime_imports_once");
    let imports = LlvmRuntimeImports::default();
    let early = runtime_function(&ctx, &module, RuntimeFn::IntFromI64);
    let cached = imports.get(&ctx, &module, RuntimeFn::IntFromI64);
    let again = imports.get(&ctx, &module, RuntimeFn::IntFromI64);
    assert_eq!(early, cached);
    assert_eq!(cached, again);
    let count = module
        .get_functions()
        .filter(|function| {
            function
                .get_name()
                .to_str()
                .unwrap()
                .starts_with(RuntimeFn::IntFromI64.linker_symbol())
        })
        .count();
    assert_eq!(count, 1);
}

/// LLVM attributes follow the manifest's effects alone (I7): every entry is
/// nounwind; only an entry that never runs Python code may promise to return;
/// only a nothrow reader may promise not to write memory.
#[test]
fn llvm_attributes_follow_manifest_effects() {
    let ctx = Context::create();
    let module = ctx.create_module("runtime_attributes");
    let imports = LlvmRuntimeImports::default();
    let mut seen = [false; 4];
    for &function in RuntimeFn::ALL {
        let declared = imports.get(&ctx, &module, function);
        assert!(has_fn_attr(declared, "nounwind"), "{}", function.symbol());
        let (willreturn, memory_read, slot) = match function.effects() {
            RuntimeEffects::Impure => (false, false, 0),
            RuntimeEffects::MayRaise => (true, false, 1),
            RuntimeEffects::NothrowWrite => (true, false, 2),
            RuntimeEffects::NothrowRead => (true, true, 3),
        };
        seen[slot] = true;
        assert_eq!(
            has_fn_attr(declared, "willreturn"),
            willreturn,
            "{} willreturn",
            function.symbol()
        );
        assert_eq!(
            memory_attr(declared) == Some(MEMORY_READ),
            memory_read,
            "{} memory(read)",
            function.symbol()
        );
    }
    assert!(
        seen[0] && seen[2] && seen[3],
        "the manifest exercises each effect class"
    );
}

/// Entries that run Python code lost the unsound `willreturn` the hand-written
/// table gave them: `__next__`, `__str__`, `__del__` and `__bool__` may loop.
#[test]
fn entries_that_may_run_python_do_not_promise_to_return() {
    let ctx = Context::create();
    let module = ctx.create_module("runtime_python_entries");
    let imports = LlvmRuntimeImports::default();
    for function in [
        RuntimeFn::IterNext,
        RuntimeFn::PrintObj,
        RuntimeFn::DecRefObj,
        RuntimeFn::IsTruthyInt,
        RuntimeFn::IsTruthyBool,
    ] {
        let declared = imports.get(&ctx, &module, function);
        assert!(
            !has_fn_attr(declared, "willreturn"),
            "{}",
            function.symbol()
        );
        assert_eq!(memory_attr(declared), None, "{}", function.symbol());
    }
    let pending = imports.get(&ctx, &module, RuntimeFn::ExceptionPending);
    assert!(has_fn_attr(pending, "willreturn"));
    assert_eq!(memory_attr(pending), Some(MEMORY_READ));
}

/// Pointer, status and float signatures reach LLVM as `ptr`, `i32` and
/// `double`, so an address is never truncated and a status never widened.
#[test]
fn non_word_signatures_keep_their_machine_classes() {
    let ctx = Context::create();
    let module = ctx.create_module("runtime_machine_classes");
    let imports = LlvmRuntimeImports::default();
    let ptr = ctx.ptr_type(inkwell::AddressSpace::default());
    let string_from_bytes = imports.get(&ctx, &module, RuntimeFn::StringFromBytes);
    let ty = string_from_bytes.get_type();
    assert_eq!(ty.get_return_type(), Some(ctx.i32_type().into()));
    let params = ty.get_param_types();
    assert_eq!(params.last().copied(), Some(ptr.into()));
    let float = imports.get(&ctx, &module, RuntimeFn::FloatAsF64);
    assert_eq!(
        float.get_type().get_return_type(),
        Some(ctx.f64_type().into())
    );
    let resolve = imports.get(&ctx, &module, RuntimeFn::HandleResolve);
    assert_eq!(resolve.get_type().get_return_type(), Some(ptr.into()));
    let load_super_method = imports.get(&ctx, &module, RuntimeFn::LoadSuperMethod);
    assert!(
        load_super_method
            .get_type()
            .get_param_types()
            .contains(&ptr.into())
    );
}

#[test]
fn trampoline_runtime_imports_are_typed_entries() {
    let ctx = Context::create();
    let module = ctx.create_module("trampoline_imports");
    let imports = LlvmRuntimeImports::default();
    for &function in TRAMPOLINE_RUNTIME_IMPORTS {
        let declared = imports.get(&ctx, &module, function);
        assert_eq!(declared.get_type(), runtime_function_type(&ctx, function));
    }
}

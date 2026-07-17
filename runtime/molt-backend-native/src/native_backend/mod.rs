use super::*;

// Shared Cranelift / std imports for the native (Cranelift) backend module
// tree. These live here — the common ancestor of `simple_backend` and
// `function_compiler` — so both submodules pick them up unqualified through
// their `use super::*` glob (module-ancestry privacy), exactly as they did
// when `SimpleBackend` and its codegen impls lived at the crate root in
// `lib.rs`. This whole module is `#[cfg(feature = "native-backend")]` via its
// declaration in `lib.rs`, so the imports are native-only without per-line
// gating.
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, Block, BlockArg, FuncRef, Function, InstBuilder, MemFlagsData, StackSlotData,
    StackSlotKind, Value, types,
};
use cranelift_codegen::isa;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_native::builder_with_options as native_isa_builder_with_options;
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::sync::OnceLock;

const _: () = assert!(
    molt_codegen_abi::MOLT_FLAGS_ATOMIC == cfg!(not(target_arch = "wasm32"))
        && cfg!(feature = "free-threaded") == molt_codegen_abi::MOLT_REFCOUNT_ATOMIC,
    "molt-backend-native/free-threaded must exactly match molt-codegen-abi/free-threaded",
);

/// Attach a retained, zero-runtime-cost relocation to the runtime's exact
/// generated-object ABI. The full fact fingerprint and execution mode are
/// encoded in the imported symbol, so independently cached backend and runtime
/// artifacts fail at link admission instead of producing mixed contracts.
fn emit_generated_object_abi_anchor(module: &mut ObjectModule) {
    let witness = module
        .declare_data(
            molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
            Linkage::Import,
            false,
            false,
        )
        .expect("declare generated-object ABI witness import");
    let anchor = module
        .declare_data(
            GENERATED_OBJECT_ABI_ANCHOR_SYMBOL,
            Linkage::Local,
            false,
            false,
        )
        .expect("declare generated-object ABI anchor");
    let mut description = DataDescription::new();
    description.define_zeroinit(module.target_config().pointer_type().bytes() as usize);
    description.set_used(true);
    let witness_ref = module.declare_data_in_data(witness, &mut description);
    description.write_data_addr(0, witness_ref, 0);
    module
        .define_data(anchor, &description)
        .expect("define generated-object ABI anchor");
}

/// Load MoltHeader's shared flag word under the selected execution contract.
/// The two generated consumers use only `HAS_PTRS` and `IMMORTAL`, which are
/// metadata facts whose payload visibility is established elsewhere. An
/// aligned target load is the exact relaxed atomic access for the native ABI;
/// Cranelift's `atomic_load` is sequentially consistent and would emit an
/// unnecessary `ldar` on AArch64. State publication remains in runtime helpers.
#[inline]
pub(crate) fn emit_header_flags_load(
    builder: &mut FunctionBuilder<'_>,
    object_ptr: Value,
) -> Value {
    builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        object_ptr,
        HEADER_FLAGS_OFFSET,
    )
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct TrampolineKey {
    pub(crate) name: String,
    pub(crate) arity: usize,
    pub(crate) has_closure: bool,
    pub(crate) is_import: bool,
    pub(crate) kind: TrampolineKind,
    pub(crate) closure_size: i64,
    pub(crate) target_has_ret: bool,
}

pub(crate) mod vec_layout;
pub(crate) use vec_layout::vec_u64_layout;

mod simple_backend;
// The three externally-public backend types must flow through a `pub` path so
// `lib.rs` can re-export them publicly (`molt_backend::SimpleBackend`, etc.);
// the remaining crate-internal items stay `pub(crate)`.
pub(crate) use simple_backend::*;
pub use simple_backend::{CompileOutput, NativeBackendModuleContext, SimpleBackend};

mod function_compiler;

#[cfg(test)]
mod header_flags_tests {
    use super::*;
    use cranelift_object::object::{Object, ObjectSymbol};

    #[test]
    fn generated_metadata_flag_reads_are_relaxed_in_every_execution_mode() {
        let mut function = Function::new();
        function.signature.params.push(AbiParam::new(types::I64));
        function.signature.returns.push(AbiParam::new(types::I32));
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let object_ptr = builder.block_params(entry)[0];
            let flags = emit_header_flags_load(&mut builder, object_ptr);
            builder.ins().return_(&[flags]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        let ir = function.display().to_string();
        assert!(
            ir.contains("load.i32") && !ir.contains("atomic_load"),
            "metadata flags must use the relaxed generated load class:\n{ir}"
        );
        assert_eq!(
            molt_codegen_abi::MOLT_FLAGS_ATOMIC,
            cfg!(not(target_arch = "wasm32")),
            "native generated metadata flags must always be atomic",
        );
    }

    #[test]
    fn aarch64_metadata_flag_load_uses_ldr_not_ldar() {
        use cranelift_codegen::control::ControlPlane;

        let backend = SimpleBackend::new_with_target(Some("aarch64-unknown-linux-gnu"));
        let isa = backend.module.isa();
        let mut function = Function::new();
        function.signature.call_conv = isa.default_call_conv();
        function.signature.params.push(AbiParam::new(types::I64));
        function.signature.returns.push(AbiParam::new(types::I32));
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let object_ptr = builder.block_params(entry)[0];
            let flags = emit_header_flags_load(&mut builder, object_ptr);
            builder.ins().return_(&[flags]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        let mut context = Context::for_function(function);
        context
            .compile(isa, &mut ControlPlane::default())
            .expect("compile AArch64 flag load");
        let code = context
            .compiled_code()
            .expect("compiled AArch64 code")
            .buffer
            .data();
        let instructions: Vec<u32> = code
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("instruction")))
            .collect();
        assert!(
            instructions.iter().any(|instruction| {
                instruction & 0xffe0_0c00 == 0xb840_0000 || instruction & 0xffc0_0000 == 0xb940_0000
            }),
            "expected an AArch64 LDR/LDUR W metadata load: {instructions:08x?}"
        );
        assert!(
            instructions
                .iter()
                .all(|instruction| instruction & 0xffff_fc00 != 0x88df_fc00),
            "metadata load must not emit acquire LDAR W: {instructions:08x?}"
        );
    }

    #[test]
    fn generated_object_retains_exact_abi_import() {
        let backend = SimpleBackend::new();
        let mut module = backend.module;
        emit_generated_object_abi_anchor(&mut module);
        let bytes = module.finish().emit().expect("emit ABI witness object");
        let object =
            cranelift_object::object::File::parse(&*bytes).expect("parse ABI witness object");
        let undefined: BTreeSet<String> = object
            .symbols()
            .filter(|symbol| symbol.is_undefined())
            .filter_map(|symbol| symbol.name().ok().map(str::to_owned))
            .collect();
        assert!(undefined.contains(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL));
        let opposite = if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC {
            molt_codegen_abi::GENERATED_OBJECT_ABI_GIL_SYMBOL
        } else {
            molt_codegen_abi::GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL
        };
        assert!(!undefined.contains(opposite));
    }
}

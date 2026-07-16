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
    AbiParam, AtomicRmwOp, Block, BlockArg, FuncRef, Function, InstBuilder, MemFlagsData,
    StackSlotData, StackSlotKind, Value, types,
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

/// Load MoltHeader's shared flag word under the selected execution contract.
/// Default CPython/GIL mode emits a plain load; explicit free-threaded mode
/// emits Cranelift's atomic load. The build feature and storage representation
/// are propagated from one `molt-codegen-abi/free-threaded` authority.
#[inline]
pub(crate) fn emit_header_flags_load(
    builder: &mut FunctionBuilder<'_>,
    object_ptr: Value,
) -> Value {
    if molt_codegen_abi::MOLT_FLAGS_ATOMIC {
        let flags_addr = builder
            .ins()
            .iadd_imm(object_ptr, i64::from(HEADER_FLAGS_OFFSET));
        builder
            .ins()
            .atomic_load(types::I32, MemFlagsData::trusted(), flags_addr)
    } else {
        builder.ins().load(
            types::I32,
            MemFlagsData::trusted(),
            object_ptr,
            HEADER_FLAGS_OFFSET,
        )
    }
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

    #[test]
    fn generated_header_flag_reads_match_execution_mode() {
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
        if molt_codegen_abi::MOLT_FLAGS_ATOMIC {
            assert!(
                ir.contains("atomic_load.i32"),
                "atomic flag storage requires atomic generated loads:\n{ir}"
            );
        } else {
            assert!(
                ir.contains("load.i32") && !ir.contains("atomic_load"),
                "default GIL flags must avoid atomic lowering:\n{ir}"
            );
        }
        assert!(
            !cfg!(feature = "free-threaded") || molt_codegen_abi::MOLT_FLAGS_ATOMIC,
            "a backend free-threaded request must propagate to ABI storage"
        );
    }
}

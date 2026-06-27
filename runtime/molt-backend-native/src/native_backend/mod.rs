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

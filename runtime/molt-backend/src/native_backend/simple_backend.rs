//! Native (Cranelift) `SimpleBackend` code generation: NaN-box/box helpers,
//! variable/block helpers, RC emission, cleanup tracking, the `SimpleBackend`
//! struct and its codegen impls, plus the native-backend unit tests. Moved
//! verbatim from lib.rs as a pure structural split. This module is `cfg(native
//! -backend)` via its declaration in `native_backend/mod.rs`; the crate-root
//! glob (`use super::*`) makes every crate-root item (FunctionIR, OpIR, passes,
//! the shared `molt-codegen-abi` constants, etc.) visible exactly as in lib.rs.

use super::*;
// The shared Cranelift / std collection imports (and the `std::fmt::Write`
// trait used by the `writeln!` in `dump_ops_to_string`) live in
// `native_backend/mod.rs` and at the crate root, and reach this module
// unqualified via `use super::*`, matching how they reached this code when it
// lived at the crate root in `lib.rs`.

mod imports;
pub(crate) use imports::*;
mod value_encoding;
pub(crate) use value_encoding::*;
mod module_metadata;
pub use module_metadata::NativeBackendModuleContext;
pub(crate) use module_metadata::*;
mod block_builder;
pub(crate) use block_builder::*;
mod refcount;
pub(crate) use refcount::*;
mod variables;
pub(crate) use variables::*;
mod trace_ops;
pub(crate) use trace_ops::*;
mod cleanup;
pub(crate) use cleanup::*;
mod control_frames;
pub(crate) use control_frames::*;
mod config;
mod deferred_codegen;
#[cfg(test)]
pub(crate) use deferred_codegen::{
    DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT, DEFERRED_CODEGEN_FLUSH_OP_BUDGET,
};
pub(crate) use deferred_codegen::{DeferredDefine, should_flush_deferred_codegen};
mod app_resolver;
mod compile_driver;
#[cfg(test)]
pub(crate) use compile_driver::preprocess_backend_tir_input;
mod trampolines;

/// Output of a native compilation pass.
///
/// Separating bytes from diagnostics lets callers handle warnings
/// structurally instead of parsing stderr.  The design follows
/// Lattner's principle: compilation is a pure function from IR to
/// (artifact, diagnostics) - side effects are the caller's concern.
#[cfg(feature = "native-backend")]
pub struct CompileOutput {
    /// The compiled object file bytes.
    pub bytes: Vec<u8>,
}

#[cfg(feature = "native-backend")]
pub struct SimpleBackend {
    pub(crate) module: ObjectModule,
    pub(crate) ctx: Context,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    pub(crate) trampoline_ids: BTreeMap<TrampolineKey, cranelift_module::FuncId>,
    pub(crate) import_ids: BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    pub skip_ir_passes: bool,
    pub skip_shared_stdlib_partition: bool,
    /// Whether this object emits the per-app `molt_app_resolve_intrinsic` resolver.
    /// Exactly one object per final binary must emit it (the one main_stub.c
    /// registers): the main application object. Stdlib-cache batch objects and all
    /// but one program batch set this `false` to avoid a duplicate symbol.
    pub emit_app_intrinsic_resolver: bool,
    /// Pre-computed per-app intrinsic manifest (the intrinsics reached by the
    /// dynamic name-based resolver path). Set by the orchestrator when the full
    /// function set is split across objects (stdlib cache split / batching) so the
    /// resolver covers names whose defining functions live in another object. When
    /// `None`, `compile` derives it from this object's own `ir.functions` (the
    /// single-object, non-split case where `ir` already holds the full set).
    pub app_intrinsic_manifest: Option<std::collections::BTreeSet<String>>,
    /// Function names that exist in other batches  use Linkage::Import.
    pub external_function_names: std::collections::BTreeSet<String>,
    module_context: Option<NativeBackendModuleContext>,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    pub(crate) data_pool: BTreeMap<Vec<u8>, cranelift_module::DataId>,
    pub(crate) next_data_id: u64,
    // Track the arity each user-defined function was declared with so that
    // call sites that reference the same function (potentially with a
    // different number of actual arguments, e.g. kwargs expansion) can
    // construct a matching Cranelift signature for `declare_function`.
    pub(crate) declared_func_arities: BTreeMap<String, usize>,
    /// Track which functions have been given a body (defined), so we can fail
    /// closed if any exported declaration is left without codegen.
    pub(crate) defined_func_names: std::collections::BTreeSet<String>,
    /// Deferred Cranelift function definitions for parallel compilation.
    /// Instead of compiling each function immediately in `define_function`,
    /// we collect the finalized IR here and compile them all in parallel
    /// via `flush_deferred_defines()`.
    pub(crate) deferred_defines: Vec<DeferredDefine>,
}

#[cfg(all(test, feature = "native-backend"))]
mod tests;

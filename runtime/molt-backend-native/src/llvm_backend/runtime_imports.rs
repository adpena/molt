//! Declares runtime functions that compiled LLVM code calls into.
//!
//! These correspond to `extern "C"` functions in `molt-runtime/src/object/ops.rs`
//! and related modules. Fixed declarations below may use target pointers for
//! dedicated buffer helpers; the conservative classified-import table is an
//! all-`i64` ABI surface whose pointer addresses are passed as integer bits and
//! cast inside the runtime export.
//!
//! ## LLVM function attributes
//!
//! Every declared function is annotated with LLVM attributes that enable
//! interprocedural optimization:
//!
//! - **`nounwind`**: All molt runtime functions use explicit error return
//!   values (NaN-boxed sentinels) and `catch_unwind` at FFI boundaries.
//!   Panics never escape as C++ exceptions, so LLVM can omit landing pads
//!   and exception handling tables entirely.
//!
//! - **`willreturn`**: Applied to functions that always terminate (no
//!   infinite loops, no coroutine suspension). Enables more aggressive
//!   dead code elimination and code motion.
//!
//! - **`memory(read)`** (= `readonly`): Applied to functions that read
//!   memory but never write to it. Enables CSE and LICM of repeated
//!   calls with the same arguments.
//!
//! - **`memory(none)`** (= `readnone`): Applied to functions that
//!   neither read nor write memory — pure functions of their arguments.
//!   Enables full redundancy elimination.

#[cfg(feature = "llvm")]
mod abi_facts;
#[cfg(feature = "llvm")]
mod attributes;
#[cfg(feature = "llvm")]
mod declarations;
#[cfg(feature = "llvm")]
mod fixed;
#[cfg(all(test, feature = "llvm"))]
mod tests;

#[cfg(feature = "llvm")]
pub(crate) use abi_facts::{
    CONSERVATIVE_RUNTIME_IMPORTS, is_runtime_import_abi, runtime_import_return_abi,
};
#[cfg(feature = "llvm")]
pub(crate) use declarations::{
    declare_conservative_runtime_function, declare_fixed_runtime_function,
    declare_runtime_functions,
};

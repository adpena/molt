//! Declares the runtime functions that compiled LLVM code calls into.
//!
//! Every runtime entry is chosen by its generated `RuntimeFn` identity
//! (HF-821). `declarations::LlvmRuntimeImports` declares an entry on first use
//! from its generated record: the linker symbol, the machine signature (an
//! `Address` parameter is an LLVM `ptr`), the execution context word, and the
//! attributes its manifest `effects` imply.
//!
//! ## LLVM function attributes
//!
//! - **`nounwind`** on every entry: molt runtime functions report errors
//!   through pending exceptions and `catch_unwind` at FFI boundaries, so no
//!   unwind tables are needed.
//! - **`willreturn`** only for `MayRaise`, `NothrowRead` and `NothrowWrite`
//!   entries. An entry that may run Python code may not terminate.
//! - **`memory(read)`** only for `NothrowRead` entries.

#[cfg(feature = "llvm")]
mod attributes;
#[cfg(feature = "llvm")]
mod declarations;
#[cfg(all(test, feature = "llvm"))]
mod tests;

#[cfg(feature = "llvm")]
pub(crate) use declarations::{LlvmRuntimeImports, runtime_function};

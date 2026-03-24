//! Core API surface for molt-runtime subcrates.
//!
//! Extracted crates (molt-runtime-crypto, molt-runtime-net, etc.)
//! depend on this crate instead of the full molt-runtime.

pub use molt_obj_model::MoltObject;

/// Prelude for extracted stdlib crates.
pub mod prelude {
    pub use molt_obj_model::MoltObject;
}

//! Asyncio stdlib intrinsics extracted from `molt-runtime`.
//!
//! This crate owns the pure asyncio data-structure intrinsics for futures,
//! events, locks, semaphores, queues, and small helper operations. Runtime
//! object/lifecycle access flows through `bridge.rs`, while interpreter-scoped
//! asyncio state is stored in the runtime extension-state registry.

pub mod asyncio_core;
pub mod asyncio_helpers;
pub mod asyncio_queue;
mod bridge;

#[cfg(test)]
#[path = "../../molt-runtime-core/src/bridge_test_stubs.rs"]
mod bridge_test_stubs;

pub use asyncio_core::*;
pub use asyncio_helpers::*;
pub use asyncio_queue::*;
pub(crate) use bridge::{
    dec_ref_bits, inc_ref_bits, int_bits_from_i64, is_truthy, raise_exception, to_i64, type_name,
};
pub(crate) use molt_runtime_core::prelude::*;

macro_rules! with_gil_entry_nopanic {
    ($py:ident, $body:block) => {{ molt_runtime_core::with_core_gil!($py, $body) }};
}

pub(crate) use with_gil_entry_nopanic;

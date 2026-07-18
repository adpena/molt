//! molt-gpu: Tinygrad-conformant GPU primitive stack.
//!
//! Implements all of deep learning with 26 compute primitives,
//! a zero-copy ShapeTracker view system, lazy evaluation DAG,
//! kernel fusion, and multi-backend rendering/execution.

pub mod dce;
pub mod device;
pub mod dtype;
pub mod fuse;
pub mod lazy;
pub mod mlir;
pub mod ops;
pub mod primitives_ffi;
pub mod render;
#[cfg(feature = "runtime-integration")]
pub mod runtime;

#[cfg(all(test, feature = "runtime-integration"))]
#[path = "../../molt-runtime-core/src/bridge_test_stubs.rs"]
mod bridge_test_stubs;
pub mod runtime_backend;
pub mod schedule;
pub mod shapetracker;

#[cfg(test)]
mod test_perf_regression;

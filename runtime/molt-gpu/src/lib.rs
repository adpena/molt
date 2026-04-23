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
pub mod render;
pub mod schedule;
pub mod shapetracker;

#[cfg(test)]
mod test_perf_regression;

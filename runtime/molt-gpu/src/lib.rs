//! molt-gpu: Tinygrad-conformant GPU primitive stack.
//!
//! Implements all of deep learning with 26 compute primitives,
//! a zero-copy ShapeTracker view system, lazy evaluation DAG,
//! kernel fusion, and multi-backend rendering/execution.

pub mod ops;
pub mod dtype;
pub mod shapetracker;
pub mod lazy;
pub mod render;
pub mod device;
pub mod schedule;
pub mod fuse;
pub mod dce;
pub mod mlir;

#[cfg(test)]
mod test_perf_regression;

//! TIR optimization passes.
//! Each pass transforms a TirFunction in-place and returns statistics.

pub mod bce;
pub mod dce;
pub mod escape_analysis;
pub mod sccp;
pub mod strength_reduction;
pub mod unboxing;

/// Statistics returned by each optimization pass.
#[derive(Debug, Default, Clone)]
pub struct PassStats {
    pub name: &'static str,
    pub values_changed: usize,
    pub ops_removed: usize,
    pub ops_added: usize,
}

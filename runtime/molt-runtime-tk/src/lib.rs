//! molt-runtime-tk: tkinter/GUI module group
//!
//! Extracted from molt-runtime to allow tree-shaking the ~18.5K lines
//! of tkinter + GUI code when not needed (e.g. WASM edge deploys).

#[cfg(feature = "tk")]
#[allow(dead_code)]
pub(crate) mod bridge;
#[cfg(test)]
#[path = "../../molt-runtime-core/src/bridge_test_stubs.rs"]
mod bridge_test_stubs;

#[cfg(feature = "tk")]
pub mod intrinsics;
#[cfg(feature = "tk")]
pub mod intrinsics_generated;
#[cfg(feature = "tk")]
#[allow(dead_code)]
pub mod tk;
#[cfg(feature = "tk")]
pub mod tkinter_core;

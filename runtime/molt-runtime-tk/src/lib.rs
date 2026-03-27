//! molt-runtime-tk: tkinter/GUI module group
//!
//! Extracted from molt-runtime to allow tree-shaking the ~18.5K lines
//! of tkinter + GUI code when not needed (e.g. WASM edge deploys).

#[cfg(feature = "tk")]
#[allow(dead_code)]
pub mod tk;
#[cfg(feature = "tk")]
pub mod tkinter_core;

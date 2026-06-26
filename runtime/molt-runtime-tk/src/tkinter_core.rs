//! Tkinter core intrinsics — event parsing, Tcl list/dict parsing, option
//! normalization, color parsing, and value conversion.
//!
//! These intrinsics move hot-path tkinter helper logic from Python shims into
//! Rust, eliminating per-event allocation churn and string re-parsing overhead
//! in bind callbacks, widget configuration, and Tcl response decoding.
//!
//! All public functions follow the Molt intrinsic ABI:
//!   `#[unsafe(no_mangle)] pub extern "C" fn name(args: u64...) -> u64`
//! where all values are NaN-boxed u64 bits (MoltObject).
//!
//! WASM compatibility: All intrinsics in this module are **parsing-only** —
//! pure string/numeric operations with no I/O, no display server interaction,
//! and no platform-specific syscalls. They compile and run correctly on all
//! targets including wasm32-wasi and wasm32-unknown-unknown.
//!
//! Note: tkinter as a whole is NOT available on WASM (no display server / Tcl/Tk
//! runtime). These parsing intrinsics are safe on WASM, but the actual Tk widget
//! operations (which live in the Python shim layer and communicate with a Tcl
//! interpreter) are gated at the Python import level — `import tkinter` raises
//! `ImportError` on WASM targets. This module does not need `#[cfg]` gating
//! because it never touches the display server or Tcl interpreter directly.

mod color;
mod common;
mod conversion;
mod event;
mod options;
mod tcl_list;

pub use color::molt_tk_hex_to_rgb;
pub use conversion::{molt_tk_convert_stringval, molt_tk_normalize_delay_ms};
pub use event::{molt_tk_event_build_from_args, molt_tk_event_int, molt_tk_event_state_decode};
pub use options::{molt_tk_cnfmerge, molt_tk_flatten_args, molt_tk_normalize_option};
pub use tcl_list::molt_tk_splitdict;

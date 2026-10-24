//! Native dependency metadata for the admitted LLVM compiler SDK.
//!
//! This crate exports no runtime API. Bundling the exact static inputs in a
//! dependency rlib lets compiler binaries and backend-local tests consume the
//! same selected archive bytes; the SDK and Python planner own all selection.

include!(concat!(env!("OUT_DIR"), "/native_links.rs"));

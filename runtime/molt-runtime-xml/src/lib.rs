#![allow(dead_code, unused_imports)]
//! `molt-runtime-xml` — XML intrinsics for the Molt runtime.
//!
//! Isolates the xml.etree.ElementTree and xml.sax intrinsics into a
//! dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_xml` feature flag.  When the feature is disabled the linker
//! can strip all XML intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod xml_etree;
pub mod xml_sax;

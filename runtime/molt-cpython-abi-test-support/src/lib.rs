//! Native C overlay witness linked only into explicit ABI attestation tests.

/// Force Cargo to propagate this crate's native-link metadata into the test.
#[inline]
pub fn link() {}

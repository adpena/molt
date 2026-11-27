//! TIR function inliner entrypoint.
//!
//! The inliner is a module transform. This root module stays as the public
//! pass-family surface; the driver, eligibility policy, body cloning, call-site
//! collection, exception labels, and splice mechanics live in submodules.

mod activation;
mod call_sites;
mod checked_scalar;
mod clone_body;
mod driver;
mod eligibility;
mod packed_read;
mod packed_splice;
pub(crate) mod packed_wrapper;
mod packed_wrapper_prefix;
mod packed_wrapper_resume;
mod splice;
mod sync_frame;

#[cfg(test)]
mod tests;

pub use self::driver::{InlinerStats, run_inliner};
pub use self::eligibility::{classify_inline_eligibility, is_inlineable};

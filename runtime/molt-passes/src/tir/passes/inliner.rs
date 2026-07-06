//! TIR function inliner entrypoint.
//!
//! The inliner is a module transform. This root module stays as the public
//! pass-family surface; the driver, eligibility policy, body cloning, call-site
//! collection, exception labels, and splice mechanics live in submodules.

mod call_sites;
mod clone_body;
mod driver;
mod eligibility;
mod exception_labels;
mod splice;

#[cfg(test)]
mod tests;

pub use self::driver::{InlinerStats, run_inliner};
pub use self::eligibility::{classify_inline_eligibility, is_inlineable};

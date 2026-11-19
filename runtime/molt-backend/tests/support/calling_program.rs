use molt_ir::runtime_fn_generated::RuntimeFn;
use sha2::{Digest, Sha256};
use std::path::Path;

/// An out-of-line Python call can refuse recursion admission before invoking its callee.
pub(crate) fn write_symbols(path: &Path, additional: &[&str]) -> String {
    let mut exports = String::new();
    for symbol in additional
        .iter()
        .copied()
        .chain([RuntimeFn::RaiseRecursionError.symbol()])
    {
        exports.push_str(symbol);
        exports.push('\n');
    }
    std::fs::write(path, &exports).expect("write selected runtime exports");
    molt_ir::content_digest::bytes_to_lower_hex(&Sha256::digest(exports.as_bytes()))
}

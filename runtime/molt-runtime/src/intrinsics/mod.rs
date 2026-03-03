pub(crate) mod capabilities;
mod generated;
mod registry;

#[allow(unused_imports)]
pub(crate) use capabilities::*;
#[allow(unused_imports)]
pub(crate) use generated::{INTRINSICS, resolve_symbol};
pub(crate) use registry::install_into_builtins;

#[cfg(target_arch = "wasm32")]
pub(crate) fn resolve_symbol_name(fn_ptr: u64) -> Option<&'static str> {
    for spec in INTRINSICS {
        if resolve_symbol(spec.symbol) == Some(fn_ptr) {
            return Some(spec.symbol);
        }
    }
    None
}

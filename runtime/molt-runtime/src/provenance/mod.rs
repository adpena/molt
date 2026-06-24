pub(crate) mod pointer_registry;

pub(crate) mod handles;
#[cfg(any(
    not(feature = "stdlib_ipaddress"),
    not(feature = "stdlib_math"),
    not(feature = "stdlib_serial")
))]
pub(crate) use pointer_registry::opaque_handle_ptr_from_bits;
pub(crate) use pointer_registry::{
    opaque_handle_bits, release_ptr, reset_ptr_registry, resolve_ptr,
};

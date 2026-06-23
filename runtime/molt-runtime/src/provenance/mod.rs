pub(crate) mod pointer_registry;

pub(crate) mod handles;
pub(crate) use pointer_registry::{
    opaque_handle_bits, release_ptr, reset_ptr_registry, resolve_ptr,
};

pub(crate) mod pointer_registry;

pub(crate) mod handles;
pub(crate) use pointer_registry::{release_ptr, reset_ptr_registry, resolve_ptr};

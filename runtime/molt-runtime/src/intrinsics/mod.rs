pub(crate) mod capabilities;
#[allow(dead_code)]
mod generated;
pub(crate) mod public_objects;
#[allow(dead_code)]
pub(crate) mod public_objects_generated;
pub(crate) mod registry;

#[allow(unused_imports)]
pub(crate) use capabilities::*;
#[allow(unused_imports)]
pub(crate) use generated::{
    INTRINSIC_MANIFEST_FINGERPRINT, INTRINSICS, RUNTIME_ROOTS, RUNTIME_ROOTS_FINGERPRINT,
    resolve_symbol,
};
pub(crate) use registry::install_into_builtins;
// Per-app runtime-callable resolver entry point. Production WASM apps register
// a resolver emitted into the app object before runtime init. That resolver
// covers manifest-reachable intrinsic symbols and reachable builtin runtime
// callables, keeping monolithic generated resolvers native-unreachable so the
// linker dead-strips every unused callable. Unit tests keep `resolve_symbol`
// reachable because they validate the generated intrinsic registry directly.

pub(crate) mod capabilities;
mod generated;
mod registry;

pub(crate) use capabilities::*;
pub(crate) use generated::INTRINSICS;
pub(crate) use registry::install_into_builtins;

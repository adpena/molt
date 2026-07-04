// Binary pickle protocol 2-5 dump/load VM and multiprocessing codec authority.
//
// MOVE-ONLY split of the former monolithic binary.rs into cohesive submodules.
// Every item is relocated verbatim; only per-module `use` lines and this
// wiring changed. All submodule items stay private-to-crate and are surfaced
// here so intra-module paths (super::*) resolve identically to the old file.

use super::*;

mod consts;
mod dump;
mod entry;
mod load;
mod read;
mod state;

pub(crate) use consts::*;
pub(crate) use dump::*;
pub use entry::*;
pub(crate) use load::*;
pub(crate) use read::*;
pub(crate) use state::*;

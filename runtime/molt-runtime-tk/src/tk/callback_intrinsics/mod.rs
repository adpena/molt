//! Tk callback intrinsics — the `#[no_mangle]` C-ABI entry points that back
//! tkinter's callback surface. Split by callback family; each submodule owns a
//! cohesive group of extern functions with no shared local state.
//!
//!   * [`timers`]        — `after` / `after_idle` / `after_cancel` / `after_info`
//!   * [`traces`]        — variable trace add/remove/info/clear
//!   * [`tkwait`]        — `tkwait` variable/window/visibility
//!   * [`binds`]         — bind/tag_bind callback register/unregister families,
//!     `bind`/`unbind` command registration, and bind-script rewriting
//!   * [`filehandlers`]  — `createfilehandler` / `deletefilehandler`
//!   * [`event_subst`]   — event `%`-substitution field parsing

mod binds;
mod event_subst;
mod filehandlers;
mod timers;
mod tkwait;
mod traces;

pub use binds::*;
pub use event_subst::*;
pub use filehandlers::*;
pub use timers::*;
pub use tkwait::*;
pub use traces::*;

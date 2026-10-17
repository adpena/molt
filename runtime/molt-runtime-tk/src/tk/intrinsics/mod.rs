//! Core Tk intrinsics — the `#[no_mangle]` C-ABI entry points backing tkinter's
//! interpreter surface. Split by responsibility:
//!
//!   * [`lifecycle`] — availability probe, app creation/quit, `mainloop` /
//!     `dooneevent` pumping, `tk call`, widget destruction, `last_error`, the
//!     `getboolean` / `getdouble` / `splitlist` value converters, and
//!     `errorinfo` append.
//!   * [`dialogs`]   — `tk_dialog`, common/message/file dialogs, and the
//!     `simpledialog` query flow.

mod dialogs;
mod lifecycle;

#[allow(
    clippy::wildcard_imports,
    reason = "re-exports the C-ABI intrinsics into crate::intrinsics"
)]
pub use {dialogs::*, lifecycle::*};

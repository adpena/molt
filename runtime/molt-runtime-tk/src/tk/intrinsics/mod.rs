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

pub use dialogs::*;
pub use lifecycle::*;

use molt_runtime_core::prelude::*;

use crate::bridge::{
    alloc_string_result, alloc_tuple_result, call_callable_args, call_callable0, clear_exception,
    dec_ref_bits, decode_value_list, dict_order, exception_pending, format_obj_str, has_capability,
    inc_ref_bits, int_from_obj, is_callable_bits, is_truthy, object_type_id, raise_exception_u64,
    string_obj_to_owned, to_f64, to_i64,
};

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use libloading::Library;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    ptr,
    thread::{self, ThreadId},
};

mod app_commands;
mod args;
mod callback_intrinsics;
mod callbacks;
mod commands;
mod dialogs;
mod dispatch;
mod focus_commands;
mod geometry_commands;
mod grab_commands;
mod intrinsics;
mod native;
mod parsing;
mod resources;
mod selection_commands;
mod state;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
mod tcl;
mod tix_commands;
mod ttk;
mod ttk_treeview;
mod widget_create;
mod widgets;
mod winfo_commands;
mod wm_commands;

use args::*;
use callbacks::*;
use commands::*;
use dialogs::*;
use dispatch::*;
use native::*;
use parsing::*;
use state::*;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use tcl::*;
use ttk::*;
use ttk_treeview::*;
use widgets::*;

pub use callback_intrinsics::*;
pub use intrinsics::*;

#[cfg(test)]
mod tests;

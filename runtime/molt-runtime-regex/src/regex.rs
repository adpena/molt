//! Regex intrinsics for Molt stdlib.
//!
//! The `re` package compiles a pattern with `molt_re_compile`, runs it with
//! `molt_re_execute`, and reads results, substitutions and splits through the
//! other `molt_re_*` entry points re-exported below. `molt_re_strip_verbose`
//! pre-processes a VERBOSE/X-flag pattern by removing unescaped whitespace and
//! `#`-comments (it respects `[…]` classes and escape sequences).
//!
//! All functions follow the canonical Molt intrinsic ABI:
//!   `pub extern "C" fn molt_re_*(args: u64) -> u64`
//!   with `molt_runtime_core::with_core_gil!(_py, { … })` as the outer frame.

use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;

use crate::bridge::{
    alloc_bytes, alloc_dict_with_pairs, alloc_list, alloc_string, alloc_tuple,
    bytes_obj_to_borrowed, bytes_pattern_to_string, dec_ref_bits, dict_snapshot, object_type_id,
    pattern_handle_arc, pattern_handle_new, raise_exception, scanner_handle_arc,
    scanner_handle_new, seq_snapshot, string_obj_to_owned, subject_slice_to_object, to_i64,
};

#[path = "regex/common.rs"]
mod common;
#[path = "regex/compile_api.rs"]
mod compile_api;
#[path = "regex/execute.rs"]
mod execute;
#[path = "regex/execute_api.rs"]
mod execute_api;
#[path = "regex/functions_re.rs"]
mod functions_re;
#[path = "regex/ir.rs"]
mod ir;
#[path = "regex/matcher.rs"]
mod matcher;
#[path = "regex/parser.rs"]
mod parser;
#[path = "regex/program.rs"]
mod program;
#[path = "regex/registry.rs"]
mod registry;
#[cfg(test)]
#[path = "regex/state_machine_tests.rs"]
mod state_machine_tests;
#[path = "regex/substitution.rs"]
mod substitution;
#[cfg(test)]
#[path = "regex/tests.rs"]
mod tests;
#[path = "regex/verbose_backref.rs"]
mod verbose_backref;

pub use registry::{PatternHandle, ScannerHandle};

#[allow(unused_imports)]
use common::*;
#[allow(unused_imports)]
use compile_api::*;
#[allow(unused_imports)]
use execute::*;
#[allow(unused_imports)]
use execute_api::*;
#[allow(unused_imports)]
use functions_re::*;
#[allow(unused_imports)]
use ir::*;
#[allow(unused_imports)]
use matcher::*;
#[allow(unused_imports)]
use parser::*;
use program::*;
#[allow(unused_imports)]
use registry::*;
#[allow(unused_imports)]
use substitution::*;
#[allow(unused_imports)]
use verbose_backref::*;

pub use compile_api::{molt_re_compile, molt_re_pattern_info};
pub use execute_api::{molt_re_execute, molt_re_finditer_next, molt_re_scanner_new};
pub use functions_re::{molt_re_expand_replacement, molt_re_group_values};

pub use substitution::{molt_re_escape, molt_re_split, molt_re_sub};
pub use verbose_backref::molt_re_strip_verbose;

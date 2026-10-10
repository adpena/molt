#![allow(dead_code, unused_imports)]
// `while_let_loop`: the satellite consumes the bridge's `Option`-returning
// `molt_iter_next` in `loop { let Some(..) = next() else { break }; .. }` form to
// stay control-flow-identical to the in-tree copy (builtins/regex.rs), which
// consumes a raw-bits `molt_iter_next` and breaks via `as_ptr()`. Rewriting to
// `while let` would diverge the two copies under the satellite-parity guard; the
// shapes unify at the Move R.2 access-layer collapse. Suppress crate-module-wide
// since this Option-bridge iteration idiom recurs across the regex intrinsics.
#![allow(clippy::while_let_loop)]
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
    alloc_dict_with_pairs, alloc_list, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    dec_ref_bits, dict_get_in_place, dict_set_in_place, dict_snapshot, inc_ref_bits,
    object_type_id, raise_exception, seq_snapshot, string_obj_to_owned, to_i64,
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
#[path = "regex/match_api.rs"]
mod match_api;
#[path = "regex/matcher.rs"]
mod matcher;
#[path = "regex/parser.rs"]
mod parser;
#[path = "regex/registry.rs"]
mod registry;
#[path = "regex/substitution.rs"]
mod substitution;
#[cfg(test)]
#[path = "regex/tests.rs"]
mod tests;
#[path = "regex/verbose_backref.rs"]
mod verbose_backref;

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
use match_api::*;
#[allow(unused_imports)]
use matcher::*;
#[allow(unused_imports)]
use parser::*;
#[allow(unused_imports)]
use registry::*;
#[allow(unused_imports)]
use substitution::*;
#[allow(unused_imports)]
use verbose_backref::*;

pub use compile_api::{molt_re_compile, molt_re_pattern_info};
pub use execute_api::{molt_re_execute, molt_re_finditer_collect};
pub use functions_re::{molt_re_expand_replacement, molt_re_group_values};

pub use match_api::{molt_re_match_group, molt_re_match_groupdict, molt_re_match_groups};
pub use substitution::{molt_re_escape, molt_re_split, molt_re_sub};
pub use verbose_backref::molt_re_strip_verbose;

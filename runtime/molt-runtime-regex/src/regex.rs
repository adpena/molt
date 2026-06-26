#![allow(dead_code, unused_imports)]
// `while_let_loop`: the satellite consumes the bridge's `Option`-returning
// `molt_iter_next` in `loop { let Some(..) = next() else { break }; .. }` form to
// stay control-flow-identical to the in-tree copy (builtins/regex.rs), which
// consumes a raw-bits `molt_iter_next` and breaks via `as_ptr()`. Rewriting to
// `while let` would diverge the two copies under the satellite-parity guard; the
// shapes unify at the Move R.2 access-layer collapse. Suppress crate-module-wide
// since this Option-bridge iteration idiom recurs across the regex intrinsics.
#![allow(clippy::while_let_loop)]
//! Regex intrinsics for Molt stdlib — advanced pattern helpers.
//!
//! This module provides lookaround and parser fidelity intrinsics that the
//! Python-side
//! `re` module cannot implement efficiently with existing helpers:
//!
//! * `molt_re_positive_lookahead`  — check that a sub-pattern DOES match at
//!   the current position (same descriptor protocol as the negative variant).
//! * `molt_re_negative_lookahead`  — check that a sub-pattern does NOT match
//!   at the current position (literal and char-class fast paths; complex
//!   sub-patterns return the sentinel −2 so Python falls back).
//! * `molt_re_positive_lookbehind` — positive fixed-width look-behind.
//! * `molt_re_negative_lookbehind` — same, but for fixed-width look-behind.
//! * `molt_re_strip_verbose`       — pre-process a VERBOSE/X-flag pattern by
//!   removing unescaped whitespace and `#`-comments (respects `[…]` classes
//!   and escape sequences).
//! * `molt_re_fullmatch_check`     — verify that a match spans the entire
//!   search window (start == match_start, end == match_end).
//! * `molt_re_named_backref_advance` — advance past a named back-reference by
//!   looking up the group span from a name→index dict and delegating to the
//!   existing byte-comparison logic.
//!
//! All functions follow the canonical Molt intrinsic ABI:
//!   `pub extern "C" fn molt_re_*(args: u64) -> u64`
//!   with `molt_runtime_core::with_core_gil!(_py, { … })` as the outer frame.

use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;

use crate::bridge::{
    alloc_dict_with_pairs, alloc_list, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    call_callable1, dec_ref_bits, dict_get_in_place, dict_order_clone, dict_set_in_place,
    exception_pending, inc_ref_bits, is_truthy, molt_iter, molt_iter_next, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_i64,
};

#[path = "regex/common.rs"]
mod common;
#[path = "regex/compiler.rs"]
mod compiler;
#[path = "regex/engine.rs"]
mod engine;
#[path = "regex/lookaround.rs"]
mod lookaround;
#[path = "regex/match_api.rs"]
mod match_api;
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
use compiler::*;
#[allow(unused_imports)]
use engine::*;
#[allow(unused_imports)]
use lookaround::*;
#[allow(unused_imports)]
use match_api::*;
#[allow(unused_imports)]
use substitution::*;
#[allow(unused_imports)]
use verbose_backref::*;

pub use compiler::{molt_re_compile, molt_re_pattern_info};
pub use engine::{molt_re_execute, molt_re_finditer_collect};
pub use lookaround::{
    molt_re_negative_lookahead, molt_re_negative_lookbehind, molt_re_positive_lookahead,
    molt_re_positive_lookbehind,
};
pub use match_api::{molt_re_match_group, molt_re_match_groupdict, molt_re_match_groups};
pub use substitution::{molt_re_escape, molt_re_split, molt_re_sub, molt_re_sub_callable};
pub use verbose_backref::{
    molt_re_fullmatch_check, molt_re_named_backref_advance, molt_re_strip_verbose,
};

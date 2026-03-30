// Bisect stdlib implementation.
// Extracted from functions.rs for tree shaking.

use crate::*;
use molt_obj_model::MoltObject;
use super::functions::*;


pub(crate) fn bisect_normalize_bounds(
    _py: &crate::PyToken<'_>,


pub(crate) fn bisect_find_index(
    _py: &crate::PyToken<'_>,


pub(crate) fn bisect_insert_at(
    _py: &crate::PyToken<'_>,


#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_left(
    seq_bits: u64,


#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_right(
    seq_bits: u64,


#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_insort_left(
    seq_bits: u64,


#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_insort_right(
    seq_bits: u64,


//! One builder-parametric generator for a proven builtin member read
//! (call_anatomy.md 4.5a, "Builtin layouts"). The frontend proves a value's
//! builtin layout root and emits the member's typed id; the layout tables in
//! this crate fix the word slot, storage policy and missing-read rule, and the
//! shared walker spells only the word and control primitives, as it does for
//! `field_slot`.
//!
//! ```text
//!   if free-threaded: cold
//!   p = unbox_ptr(value)                          ; the fact proves a pointer
//!   w = load [p + 8 * slot]
//!   Object:  w == MISSING -> None, or cold when the member raises
//!            else retain w
//!   PySsize: w == -1 and the member raises -> cold
//!            fits inline -> box_int(w), else cold
//! ```
//!
//! Invariants:
//!
//! 1. The runtime allocates, reads and writes a builtin's members by the same
//!    slots (`exception_field_slot`), and its getter reads the slot without
//!    consulting a C-API view, so this load reads exactly what the getter reads.
//! 2. An absent object member holds `EXCEPTION_FIELD_MISSING_BITS`, never None.
//! 3. Under the GIL no other thread replaces the member between the load and
//!    the retain. A free-threaded build takes `cold`.
//! 4. `cold` is the runtime getter; it owns every raising and boxing edge.

use crate::boxed_word::{
    BoxTag, generate_box_inline_int, generate_has_tag, generate_inline_int_fits,
};
use crate::exception_layout::{
    EXCEPTION_FIELD_MISSING_BITS, EXCEPTION_PY_SSIZE_MISSING, ExceptionFieldStorage,
    ExceptionLayoutKind, ExceptionMissingRead, ExceptionTypedField, exception_field_slot,
};
use crate::frame_record::FrameBuilder;
use crate::list_item::ListLocking;
use crate::word::WordComparison;
use crate::{POINTER_MASK, QNAN, TAG_NONE};

/// How compiled code reads one builtin member.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinFieldRead {
    /// A boxed word, retained; the missing marker reads as None or raises.
    Object { slot: usize, missing_raises: bool },
    /// A raw `Py_ssize_t`, boxed; -1 raises when the member says so.
    PySsize { slot: usize, missing_raises: bool },
}

/// The layout family that declares `field` (typed fields are unambiguous
/// across families).
pub const fn exception_field_layout(field: ExceptionTypedField) -> ExceptionLayoutKind {
    let mut index = 0;
    while index < ExceptionLayoutKind::ALL.len() {
        let layout = ExceptionLayoutKind::ALL[index];
        if layout.field_policy(field).is_some() {
            return layout;
        }
        index += 1;
    }
    panic!("every typed exception field belongs to a layout family");
}

/// The read of exception member `field` from its layout table.
pub const fn exception_field_read(field: ExceptionTypedField) -> BuiltinFieldRead {
    let layout = exception_field_layout(field);
    let Some(policy) = layout.field_policy(field) else {
        panic!("the declaring layout has the field's policy");
    };
    let Some(slot) = exception_field_slot(layout, field) else {
        panic!("the declaring layout has the field's slot");
    };
    let missing_raises = policy.missing_read as u8 == ExceptionMissingRead::AttributeError as u8;
    match policy.storage {
        ExceptionFieldStorage::RuntimeMessage | ExceptionFieldStorage::Object => {
            BuiltinFieldRead::Object {
                slot,
                missing_raises,
            }
        }
        ExceptionFieldStorage::PySsize => BuiltinFieldRead::PySsize {
            slot,
            missing_raises,
        },
    }
}

fn slot_offset(slot: usize) -> i32 {
    i32::try_from(slot * 8).expect("a builtin member slot is a small word index")
}

/// The member `read` of the builtin object `value` as an owned word: the
/// slot's word (retained or boxed), None for an absent member that reads as
/// None, or what `cold(value)` returns, the runtime getter that owns raising
/// and boxing.
///
/// Requires: `value` is a borrowed boxed word whose builtin layout the frontend
/// proved. Ensures: the result is owned by the caller; the fast path runs no
/// Python code and changes only the member value's refcount.
pub fn generate_builtin_field_get<B: FrameBuilder>(
    b: &mut B,
    value: B::Word,
    read: BuiltinFieldRead,
    locking: ListLocking,
    cold: impl FnOnce(&mut B, B::Word) -> B::Word,
) -> B::Word {
    let result = b.local();
    if locking == ListLocking::FreeThreaded {
        let word = cold(b, value);
        b.set(result, word);
        return b.get(result);
    }
    let is_ptr = generate_has_tag(b, value, BoxTag::Ptr);
    b.attempt(
        |b| {
            // The fact proves a pointer; anything else is the runtime's.
            b.require(is_ptr);
            let pointer = b.and(value, POINTER_MASK);
            match read {
                BuiltinFieldRead::Object {
                    slot,
                    missing_raises,
                } => {
                    let word = b.load_word(pointer, slot_offset(slot));
                    let missing = b.constant(EXCEPTION_FIELD_MISSING_BITS);
                    if missing_raises {
                        let present = b.compare(WordComparison::NotEqual, word, missing);
                        b.require(present);
                        b.retain(word);
                        b.set(result, word);
                    } else {
                        let none = b.constant(QNAN | TAG_NONE);
                        b.set(result, none);
                        let present = b.compare(WordComparison::NotEqual, word, missing);
                        b.if_then(present, |b| {
                            b.retain(word);
                            b.set(result, word);
                        });
                    }
                }
                BuiltinFieldRead::PySsize {
                    slot,
                    missing_raises,
                } => {
                    let raw = b.load_word(pointer, slot_offset(slot));
                    if missing_raises {
                        let unset = b.constant(EXCEPTION_PY_SSIZE_MISSING);
                        let present = b.compare(WordComparison::NotEqual, raw, unset);
                        b.require(present);
                    }
                    let fits = generate_inline_int_fits(b, raw);
                    b.require(fits);
                    let boxed = generate_box_inline_int(b, raw);
                    b.set(result, boxed);
                }
            }
        },
        |b| {
            let word = cold(b, value);
            b.set(result, word);
        },
    );
    b.get(result)
}

#[cfg(test)]
mod tests;

//! Frame records for spliced activations (docs/design/foundation/
//! spliced_frames.md).
//!
//! A splice (an inlined callee, a fused generator poll) copies a callee body
//! into its caller and drops the callee's frame record from the hot path.
//! Compiled code builds a spliced activation's record only where an observer
//! runs: on an Origin exception edge, after an explicit raise's Origin, and
//! around an operation that may run Python code. It links one owned record
//! per spliced level (outermost first) over the executing frame
//! ([`molt_spliced_frame_link_attached_body`]), binds the level's locals in
//! its homes ([`molt_spliced_frame_bind_home_attached_body`]), and then either
//! captures the linked records into the pending traceback and pops them
//! ([`molt_spliced_frame_capture_origin_attached_body`], in `exceptions.rs`) or pops
//! them after the bracketed operation
//! ([`molt_spliced_frame_pop_attached_body`]). None of these allocates a
//! Python object or refuses a pending exception: they run on exception edges.
//!
//! A binding's key is a code slot (`CODE_SLOT`: a synchronous callee's home
//! index) or a payload offset of a generator or coroutine binding
//! (`STATEFUL_OFFSET`), mapped through the poll's registered locals layout. A
//! location below zero is unknown; the frame then reports its code's first
//! site. A `missing` value leaves its binding unbound.

use molt_codegen_abi::{FRAME_HOME_PLAIN, FRAME_HOME_WORDS};

use super::bindings::FramePlan;
use super::locations::LocationId;
use super::records::{self, FrameRecord};
use crate::object::layout::{function_builtins_bits, function_code_bits, function_globals_bits};
use crate::{PyToken, TYPE_ID_FUNCTION, inc_ref_bits, obj_from_bits, object_type_id};

/// The key of a code slot binding.
const CODE_SLOT: i64 = 0;
/// The key of a stateful activation's payload-offset binding.
const STATEFUL_OFFSET: i64 = 1;

fn int(bits: u64) -> Option<i64> {
    crate::to_i64(obj_from_bits(bits))
}

/// `molt_spliced_frame_link(function, location, identity_slot)`: link one owned
/// record for a spliced activation of `function` over the executing frame,
/// its homes unbound. Always links exactly one record, so the pops that
/// follow stay paired: a malformed function links a record without code,
/// which observers skip.
pub(crate) fn molt_spliced_frame_link_attached_body(
    _py: &PyToken<'_>,
    function_bits: u64,
    location_bits: u64,
    identity_slot: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let function = obj_from_bits(function_bits)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) } == TYPE_ID_FUNCTION);
        let context = function.map(|function| unsafe {
            (
                function_globals_bits(function),
                function_builtins_bits(function),
                function_code_bits(function),
            )
        });
        let plan = context.and_then(|(_, _, code)| FramePlan::for_code(py, code).ok());
        match (context, plan) {
            (Some((globals, builtins, code)), Some(plan)) => {
                // A record without room for its homes is still linked: the
                // frame keeps its code and line, as a `MemoryError`
                // traceback keeps it.
                let entered = super::frame_stack_enter(
                    py,
                    super::OwnedFrameNamespace::retain(py, [globals, builtins, code]),
                    plan,
                );
                let location = int(location_bits)
                    .and_then(|location| u64::try_from(location).ok())
                    .map_or(LocationId::from_word(0), LocationId::from_word);
                let record = records::top().expect("an entered frame is linked");
                record.bindings().set_location(location);
                if entered {
                    let slot = crate::provenance::abi::mut_ptr::<u64>(identity_slot)
                        .expect("a spliced frame borrows its activation's identity slot");
                    record.update_owned(|entry| entry.bindings.set_splice_slot(slot as usize));
                    let observed = unsafe { *slot };
                    if observed != 0 {
                        super::bindings::resume_splice(py, record, observed);
                    }
                }
            }
            _ => super::frame_stack_push_owned(py, 0, 0, 0),
        }
        0
    })
}

/// The compiler took this owned word from a completed splice's slot. All its
/// records are unlinked; an escaped frame now retains its final caller.
pub(crate) fn molt_spliced_frame_finish_attached_body(_py: &PyToken<'_>, payload_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        super::bindings::finish_splice(py, payload_bits);
        0
    })
}

/// The code slot `key` names in a frame of `code_bits`.
fn code_slot(py: &PyToken<'_>, key_space: i64, key: i64, code_bits: u64) -> Option<usize> {
    match key_space {
        CODE_SLOT => usize::try_from(key).ok(),
        STATEFUL_OFFSET => {
            let offset = usize::try_from(key).ok()?;
            super::activation::stateful_offset_code_slots(py, code_bits)?
                .into_iter()
                .find(|(slot_offset, _)| *slot_offset == offset)
                .and_then(|(_, slot)| slot)
        }
        _ => None,
    }
}

/// `molt_spliced_frame_bind_home(key_space, key, value, present)`: bind `value` in the home
/// `key` names of the record the last link made. A key the code does not name
/// and an absent or `missing` value bind nothing.
pub(crate) fn molt_spliced_frame_bind_home_attached_body(
    _py: &PyToken<'_>,
    key_space_bits: u64,
    key_bits: u64,
    value: u64,
    present: u64,
) {
    crate::with_attached_entry_nopanic!(_py, py, {
        if obj_from_bits(present).as_bool() == Some(true) {
            bind(py, key_space_bits, key_bits, value);
        }
    })
}

fn bind(py: &PyToken<'_>, key_space_bits: u64, key_bits: u64, value: u64) {
    let Some(record) = records::top() else {
        return;
    };
    let code = record.code_bits();
    let (Some(key_space), Some(key)) = (int(key_space_bits), int(key_bits)) else {
        return;
    };
    if crate::is_missing_bits(py, value) || obj_from_bits(code).as_ptr().is_none() {
        return;
    }
    let Ok(plan) = FramePlan::for_code(py, code) else {
        return;
    };
    let Some(slot) = code_slot(py, key_space, key, code).filter(|&slot| slot < plan.homes as usize)
    else {
        return;
    };
    let bindings = record.bindings();
    if !bindings.has_python_bindings() {
        return;
    }
    inc_ref_bits(py, value);
    unsafe {
        let home = (record.base() as *mut u64).add(slot * FRAME_HOME_WORDS);
        // A well-formed state binds each home once; release a repeated key's
        // earlier binding rather than leak it.
        let previous_kind = *home as i64;
        let previous = *home.add(1);
        home.write(FRAME_HOME_PLAIN as u64);
        home.add(1).write(value);
        if previous_kind == FRAME_HOME_PLAIN {
            crate::dec_ref_bits(py, previous);
        }
    }
}

/// Pop `count` records the links linked, innermost first.
fn unlink(py: &PyToken<'_>, count: usize) {
    for _ in 0..count {
        super::frame_stack_pop(py);
    }
}

/// `molt_spliced_frame_pop(count)`: pop the records the links of a
/// bracket linked.
pub(crate) fn molt_spliced_frame_pop_attached_body(_py: &PyToken<'_>, count_bits: u64) {
    crate::with_attached_entry_nopanic!(_py, py, {
        let count = int(count_bits)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0);
        unlink(py, count);
    })
}

/// The traceback `tail` with an entry for each of the `count` innermost
/// records (the spliced levels the links linked). When the head entry is the
/// record below them (the record-owning caller the backend's Origin capture
/// recorded), the entries go beneath it. Otherwise that capture found a
/// spliced record executing and recorded nothing (an asynchronous-work poll
/// inside the bracket), so the caller's entry goes in front with them.
/// Returns the new owned head, or `None` when nothing changed. The records
/// stay linked ([`pop_spliced_frames`] pops them).
pub(crate) fn trace_with_spliced_frames(
    py: &PyToken<'_>,
    tail_bits: u64,
    count: usize,
) -> Option<u64> {
    let mut chain: Vec<FrameRecord> = records::innermost_first().take(count).collect();
    if chain.is_empty() {
        return None;
    }
    let caller = records::innermost_first().nth(chain.len());
    let below_head = caller.is_some_and(|caller| super::traceback_head_is(tail_bits, caller));
    if !below_head {
        chain.extend(caller);
    }
    chain.reverse();
    let snapshot = super::FrameStackSnapshot::capture(py, &chain).ok()?;
    super::traceback_payload_insert(py, tail_bits, below_head, &snapshot.entries)
}

/// Pop the `count` records the links of a spliced frame state linked.
pub(crate) fn pop_spliced_frames(py: &PyToken<'_>, count: usize) {
    unlink(py, count);
}

include!("../../attached_exports_generated/builtins/frames/spliced.rs");

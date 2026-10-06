//! Binding homes and payloads: arena custody, home reads and moves, live and
//! retired observation, proxy writes, argument zero, the exit's release
//! orders and invalid states.

use molt_obj_model::MoltObject;

use super::*;
use crate::builtins::frames::{frame_stack_enter_homes, frame_stack_pop, frame_stack_push_owned};
use crate::object::builders::{alloc_code_obj, alloc_tuple};
use crate::state::runtime_state::PythonVersionInfo;
use crate::{alloc_string, dict_get_in_place};

fn string(py: &PyToken<'_>, text: &[u8]) -> u64 {
    MoltObject::from_ptr(alloc_string(py, text)).bits()
}

fn names(py: &PyToken<'_>, items: &[&[u8]]) -> u64 {
    let bits: Vec<u64> = items.iter().map(|name| string(py, name)).collect();
    let tuple = MoltObject::from_ptr(alloc_tuple(py, &bits)).bits();
    for name in bits {
        dec_ref_bits(py, name);
    }
    tuple
}

fn refs(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().unwrap();
    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

/// An owned code object whose varnames are `varnames` (no parameters), with
/// the given lexical cells, published the way function metadata publishes it.
fn slot_code(py: &PyToken<'_>, varnames: &[&[u8]], cellvars: &[&[u8]]) -> u64 {
    let none = MoltObject::none().bits();
    let filename = string(py, b"<frame-bindings>");
    let name = string(py, b"frame");
    let varnames_bits = names(py, varnames);
    let empty = names(py, &[]);
    let code = alloc_code_obj(py, filename, name, 1, none, varnames_bits, empty, 0, 0, 0);
    assert!(!code.is_null());
    let cellvars_bits = names(py, cellvars);
    assert!(unsafe { crate::code_publish_lexical_metadata(py, code, empty, cellvars_bits) });
    for bits in [filename, name, varnames_bits, empty, cellvars_bits] {
        dec_ref_bits(py, bits);
    }
    MoltObject::from_ptr(code).bits()
}

/// Run `body` with the runtime targeting Python 3.`minor`.
fn with_target(py: &PyToken<'_>, minor: i64, body: impl FnOnce()) {
    let state = crate::runtime_state(py);
    let saved = state.sys_version_info.lock().unwrap().clone();
    *state.sys_version_info.lock().unwrap() = Some(PythonVersionInfo {
        major: 3,
        minor,
        micro: 0,
        releaselevel: "final".to_string(),
        serial: 0,
    });
    body();
    *state.sys_version_info.lock().unwrap() = saved;
}

fn plan(homes: usize, policy: FramePolicy) -> FramePlan {
    FramePlan {
        optimized: true,
        homes: u32::try_from(homes).unwrap(),
        policy,
    }
}

/// Push a synchronous frame of `code` whose plan gives it `slots` homes, as
/// `molt_trace_enter_slot` does, and borrow them as its compiled entry does.
/// Returns the homes and the entry's stack index.
fn enter_frame(py: &PyToken<'_>, code: u64, slots: usize) -> (*mut u64, usize) {
    inc_ref_bits(py, code);
    frame_stack_push_owned(py, code, 0, 0, 0);
    assert!(frame_stack_enter_homes(plan(
        slots,
        FramePolicy::of_runtime(py)
    )));
    let homes = molt_frame_homes(slots as u64) as usize as *mut u64;
    assert!(!exception_pending(py));
    assert!(!homes.is_null() || slots == 0);
    let index = FRAME_STACK.with(|stack| stack.borrow().len() - 1);
    (homes, index)
}

unsafe fn set_home(homes: *mut u64, slot: usize, kind: i64, bits: u64) {
    unsafe {
        *homes.add(slot * FRAME_HOME_WORDS) = kind as u64;
        *homes.add(slot * FRAME_HOME_WORDS + 1) = bits;
    }
}

unsafe fn home_kind(homes: *mut u64, slot: usize) -> i64 {
    unsafe { *homes.add(slot * FRAME_HOME_WORDS) as i64 }
}

fn home(homes: *mut u64, slot: usize) -> u64 {
    homes.wrapping_add(slot * FRAME_HOME_WORDS) as usize as u64
}

/// Bind `value` in a plain home, which owns a reference of its own, as a
/// compiled store leaves it.
unsafe fn bind(py: &PyToken<'_>, homes: *mut u64, slot: usize, value: u64) {
    inc_ref_bits(py, value);
    unsafe { set_home(homes, slot, FRAME_HOME_PLAIN, value) };
}

fn bound(py: &PyToken<'_>, payload: u64) -> Vec<(String, u64)> {
    let items = frame_bindings_items(py, payload).expect("projection");
    items
        .bound()
        .map(|(name, value)| {
            (
                crate::string_obj_to_owned(obj_from_bits(name)).unwrap(),
                value,
            )
        })
        .collect()
}

fn take_pending(py: &PyToken<'_>) -> u64 {
    assert!(exception_pending(py));
    let pending = crate::builtins::exceptions::molt_exception_last_pending();
    crate::molt_exception_clear();
    pending
}

#[test]
fn homes_are_a_stack_of_zeroed_words() {
    let policy = FramePolicy::default();
    let first = FrameBindings::enter(plan(3, policy)).expect("homes");
    let second = FrameBindings::enter(plan(2, policy)).expect("homes");
    assert_eq!(second.homes, first.homes + 3 * FRAME_HOME_WORDS * WORD);
    unsafe { *(second.homes as *mut u64) = 7 };
    second.exit();
    let again = FrameBindings::enter(plan(2, policy)).expect("homes");
    assert_eq!(again.homes, second.homes, "the exit returned its homes");
    assert_eq!(
        unsafe { *(again.homes as *const u64) },
        0,
        "homes start unbound"
    );
    // A frame larger than a chunk gets its own chunk and returns it.
    let large = FrameBindings::enter(plan(HOME_CHUNK_WORDS, policy)).expect("large homes");
    large.exit();
    again.exit();
    first.exit();
    let reused = FrameBindings::enter(plan(3, policy)).expect("homes");
    assert_eq!(reused.homes, first.homes);
    reused.exit();
    // An optimized code object without slots, and a module body, take none.
    for empty in [plan(0, policy), FramePlan::default()] {
        let bindings = FrameBindings::enter(empty).expect("no homes");
        assert_eq!(bindings.homes, 0);
        bindings.exit();
    }
}

#[test]
fn compiled_home_reads_and_moves_follow_the_home_kind() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let code = slot_code(py, &[b"plain", b"raw", b"unbound", b"shared"], &[b"shared"]);
        let value = string(py, b"binding");
        let cell = MoltObject::from_ptr(crate::object::cells::alloc_cell(py, value)).bits();
        let (homes, _) = enter_frame(py, code, 4);
        unsafe {
            bind(py, homes, 0, value);
            set_home(homes, 1, FRAME_HOME_RAW_INT, (-7_i64) as u64);
            inc_ref_bits(py, cell);
            set_home(homes, 3, FRAME_HOME_CELL, cell);
        }
        // A read borrows the home's binding.
        assert_eq!(molt_frame_home_load(home(homes, 0)), value);
        assert_eq!(
            refs(value),
            3,
            "the test, the home and the cell; no read edge"
        );
        // An unbound home reads as the missing sentinel.
        assert!(is_missing_bits(py, molt_frame_home_load(home(homes, 2))));
        // A raw integer is boxed into its home, so the read still borrows.
        let boxed = molt_frame_home_load(home(homes, 1));
        assert_eq!(unsafe { home_kind(homes, 1) }, FRAME_HOME_PLAIN);
        assert_eq!(unsafe { *homes.add(FRAME_HOME_WORDS + 1) }, boxed);
        assert_eq!(crate::to_i64(obj_from_bits(boxed)), Some(-7));
        // A cell home holds no plain binding.
        assert!(is_missing_bits(py, molt_frame_home_load(home(homes, 3))));
        dec_ref_bits(py, take_pending(py));
        // A move leaves the home unbound and hands its reference over.
        let taken = molt_frame_home_take(home(homes, 0));
        assert_eq!(taken, value);
        assert_eq!(unsafe { home_kind(homes, 0) }, FRAME_HOME_UNBOUND);
        assert_eq!(refs(value), 3, "moved, neither retained nor released");
        dec_ref_bits(py, taken);
        let taken_cell = molt_frame_home_take(home(homes, 3));
        assert_eq!(taken_cell, cell);
        dec_ref_bits(py, taken_cell);
        assert!(is_missing_bits(py, molt_frame_home_take(home(homes, 2))));
        // A home that compiled code was never lent is malformed input.
        assert!(obj_from_bits(molt_frame_home_load(0)).is_none());
        dec_ref_bits(py, take_pending(py));
        // With an exception pending (a failed frame entry), it is kept.
        let _ = raise_exception::<u64>(py, "ValueError", "entry failed");
        let before = crate::builtins::exceptions::molt_exception_last_pending();
        let _ = molt_frame_home_take(0);
        let after = take_pending(py);
        assert_eq!(before, after);
        frame_stack_pop(py);
        for bits in [before, after, cell, value, code] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn argument_zero_is_read_from_the_first_home_when_asked() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let code = slot_code(py, &[b"receiver"], &[b"receiver"]);
        let first = string(py, b"receiver");
        let second = string(py, b"rebound receiver");
        let (homes, _) = enter_frame(py, code, 1);
        assert_eq!(
            frame_argument_zero(py),
            Ok(None),
            "an unbound slot: no argument"
        );
        unsafe { bind(py, homes, 0, first) };
        assert_eq!(frame_argument_zero(py), Ok(Some(first)));
        assert_eq!(refs(first), 3, "the caller receives its own reference");
        dec_ref_bits(py, first);
        // A rebinding (or a proxy write) is what the next call sees.
        unsafe {
            dec_ref_bits(py, first);
            bind(py, homes, 0, second);
        }
        assert_eq!(frame_argument_zero(py), Ok(Some(second)));
        dec_ref_bits(py, second);
        // A captured argument's home holds its cell: the contents are read.
        let cell = MoltObject::from_ptr(crate::object::cells::alloc_cell(py, first)).bits();
        unsafe {
            dec_ref_bits(py, second);
            set_home(homes, 0, FRAME_HOME_CELL, cell);
        }
        assert_eq!(frame_argument_zero(py), Ok(Some(first)));
        dec_ref_bits(py, first);
        frame_stack_pop(py);
        for bits in [first, second, code] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn raw_home_observers_share_the_persistent_binding_identity() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for minor in [12, 13, 14] {
            with_target(py, minor, || {
                // Both live homes and the pairs a retained frame takes over
                // must publish the first observation, whoever asks for it.
                for first_observer in 0..4 {
                    let code = slot_code(py, &[b"number"], &[]);
                    let key = string(py, b"number");
                    let (homes, index) = enter_frame(py, code, 1);
                    unsafe { set_home(homes, 0, FRAME_HOME_RAW_INT, 1 << 50) };
                    let payload = frame_bindings_observe(py, index).expect("payload");
                    inc_ref_bits(py, payload);
                    if first_observer == 3 {
                        frame_stack_pop(py);
                    }
                    let held = match first_observer {
                        0 => {
                            let bits = molt_frame_home_load(home(homes, 0));
                            inc_ref_bits(py, bits);
                            bits
                        }
                        1 => frame_argument_zero(py).unwrap().unwrap(),
                        _ => {
                            let items = frame_bindings_items(py, payload).unwrap();
                            let bits = items.slots[0].1.unwrap();
                            inc_ref_bits(py, bits);
                            bits
                        }
                    };
                    assert_eq!(crate::to_i64(obj_from_bits(held)), Some(1 << 50));
                    assert_eq!(
                        refs(held),
                        2,
                        "persistent home plus retained first observation"
                    );
                    for _ in 0..2 {
                        let items = frame_bindings_items(py, payload).unwrap();
                        assert_eq!(items.slots[0].1, Some(held));
                        assert_eq!(refs(held), 3, "items retain their own reference");
                    }
                    if first_observer != 3 {
                        assert_eq!(unsafe { home_kind(homes, 0) }, FRAME_HOME_PLAIN);
                        assert_eq!(molt_frame_home_load(home(homes, 0)), held);
                        let argument = frame_argument_zero(py).unwrap().unwrap();
                        assert_eq!(argument, held);
                        dec_ref_bits(py, argument);
                        let moved = molt_frame_home_take(home(homes, 0));
                        assert_eq!(moved, held, "take moves the same identity");
                        assert_eq!(unsafe { home_kind(homes, 0) }, FRAME_HOME_UNBOUND);
                        assert_eq!(
                            refs(held),
                            2,
                            "take moves, rather than retains, the home owner"
                        );
                        unsafe { set_home(homes, 0, FRAME_HOME_PLAIN, moved) };
                    }
                    let snapshot = frame_bindings_snapshot(py, payload).unwrap();
                    assert_eq!(
                        unsafe {
                            dict_get_in_place(py, obj_from_bits(snapshot).as_ptr().unwrap(), key)
                        },
                        Some(held)
                    );
                    dec_ref_bits(py, snapshot);
                    if minor == 12 {
                        let locals = frame_bindings_locals_dict(py, payload).unwrap();
                        assert_eq!(
                            unsafe {
                                dict_get_in_place(py, obj_from_bits(locals).as_ptr().unwrap(), key)
                            },
                            Some(held)
                        );
                        dec_ref_bits(py, locals);
                    } else {
                        let replacement = string(py, b"replacement");
                        frame_bindings_write(py, payload, key, Some(replacement)).unwrap();
                        assert_eq!(bound(py, payload)[0].1, replacement);
                        assert_eq!(
                            crate::to_i64(obj_from_bits(held)),
                            Some(1 << 50),
                            "rebinding preserves a retained observation"
                        );
                        dec_ref_bits(py, replacement);
                    }
                    if first_observer != 3 {
                        frame_stack_pop(py);
                    }
                    frame_bindings_clear(py, payload).unwrap();
                    assert!(bound(py, payload).is_empty());
                    assert_eq!(
                        refs(held),
                        1,
                        "clear retires every storage edge, not the observer's edge"
                    );
                    for bits in [held, payload, key, code] {
                        dec_ref_bits(py, bits);
                    }
                }
            });
        }
    });
}

#[test]
fn raw_home_publication_failure_preserves_storage_and_first_error() {
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    struct RestoreBudget;
    impl Drop for RestoreBudget {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let _ = missing_bits(py);
        for observer in 0..6 {
            let code = slot_code(py, &[b"number"], &[]);
            let (homes, index) = enter_frame(py, code, 1);
            unsafe { set_home(homes, 0, FRAME_HOME_RAW_INT, 1 << 50) };
            let payload = frame_bindings_observe(py, index).unwrap();
            inc_ref_bits(py, payload);
            if observer == 5 {
                frame_stack_pop(py);
            }
            let ptr = frame_bindings_ptr(py, payload).unwrap();
            let pair = unsafe { pair_words(ptr) }.unwrap();
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_allocations: Some(0),
                ..Default::default()
            })));
            let budget = RestoreBudget;
            match observer {
                0 => assert!(is_missing_bits(py, molt_frame_home_load(home(homes, 0)))),
                1 => assert!(is_missing_bits(py, molt_frame_home_take(home(homes, 0)))),
                2 => assert!(frame_argument_zero(py).is_err()),
                3 | 5 => assert!(frame_bindings_items(py, payload).is_err()),
                4 => assert!(frame_bindings_snapshot(py, payload).is_err()),
                _ => unreachable!(),
            }
            drop(budget);
            assert!(exception_pending(py));
            assert_eq!(
                unsafe { (*pair as i64, *pair.add(1)) },
                (FRAME_HOME_RAW_INT, 1 << 50)
            );
            let before = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(
                unsafe { plain_home_value(py, pair) }.is_err(),
                "an existing error must not mint an unowned box"
            );
            let after = take_pending(py);
            assert_eq!(before, after);
            let boxed = unsafe { plain_home_value(py, pair) }.unwrap();
            assert_eq!(
                unsafe { (*pair as i64, *pair.add(1)) },
                (FRAME_HOME_PLAIN, boxed)
            );
            assert_eq!(
                refs(boxed),
                1,
                "successful retry belongs solely to persistent storage"
            );
            assert_eq!(bound(py, payload)[0].1, boxed);
            if observer != 5 {
                frame_stack_pop(py);
            }
            for bits in [before, after, payload, code] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn an_observer_takes_over_the_bindings_of_an_exiting_frame() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        with_target(py, 12, || {
            let code = slot_code(py, &[b"first", b"second", b"unbound"], &[]);
            let first = string(py, b"first binding");
            let (homes, index) = enter_frame(py, code, 3);
            unsafe {
                bind(py, homes, 0, first);
                // A full-range raw integer is boxed only when observed.
                set_home(homes, 1, FRAME_HOME_RAW_INT, (1_i64 << 50) as u64);
            }
            let payload = frame_bindings_observe(py, index).expect("payload");
            assert_ne!(payload, 0);
            assert_eq!(
                frame_bindings_observe(py, index).unwrap(),
                payload,
                "once per activation"
            );
            // An observer (a traceback or frame object) shares the payload.
            inc_ref_bits(py, payload);
            let live = bound(py, payload);
            assert_eq!(live[0], ("first".to_string(), first));
            assert_eq!(live[1].0, "second");
            assert_eq!(live.len(), 2, "an unbound slot is omitted");
            assert_eq!(refs(first), 2, "the home, not the live payload, owns it");
            let depth = FRAME_STACK.with(|stack| stack.borrow().len());
            frame_stack_pop(py);
            assert!(!exception_pending(py));
            assert_eq!(FRAME_STACK.with(|stack| stack.borrow().len()), depth - 1);
            // The frame object took the home's reference over.
            assert_eq!(refs(first), 2);
            assert_eq!(bound(py, payload)[0].1, first);
            // Its last owner's release destroys it with the binding.
            dec_ref_bits(py, payload);
            assert_eq!(refs(first), 1);
            dec_ref_bits(py, first);
            dec_ref_bits(py, code);
        });
    });
}

#[test]
fn an_unshared_exit_releases_every_binding_its_homes_own() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let code = slot_code(py, &[b"only", b"shared"], &[b"shared"]);
        let value = string(py, b"binding");
        let contents = string(py, b"cell contents");
        let cell = MoltObject::from_ptr(crate::object::cells::alloc_cell(py, contents)).bits();
        let (homes, index) = enter_frame(py, code, 2);
        unsafe {
            bind(py, homes, 0, value);
            // The home holds the frame's reference to its cell.
            set_home(homes, 1, FRAME_HOME_CELL, cell);
        }
        // Observed, but only the entry holds the payload.
        assert_ne!(frame_bindings_observe(py, index).expect("payload"), 0);
        inc_ref_bits(py, cell);
        frame_stack_pop(py);
        assert!(!exception_pending(py));
        assert_eq!(refs(value), 1, "the exit released the home's binding");
        assert_eq!(refs(cell), 1, "and the frame's reference to its cell");
        for bits in [cell, contents, value, code] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn retired_bindings_release_in_the_target_clear_and_destroy_orders() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for minor in [12, 13, 14] {
            for release in [FrameRelease::Clear, FrameRelease::Destroy] {
                with_target(py, minor, || {
                    let code = slot_code(py, &[b"first", b"second"], &[]);
                    let first = string(py, b"first binding");
                    let second = string(py, b"second binding");
                    let extra_key = string(py, b"extra");
                    let (homes, index) = enter_frame(py, code, 2);
                    unsafe {
                        bind(py, homes, 0, first);
                        bind(py, homes, 1, second);
                    }
                    let payload = frame_bindings_observe(py, index).expect("payload");
                    inc_ref_bits(py, payload);
                    // The frame object's own dict: before PEP 667 its
                    // `f_locals`, afterwards the names a proxy added.
                    let locals = if minor >= 13 {
                        frame_bindings_write(py, payload, extra_key, Some(first)).expect("write");
                        let ptr = frame_bindings_ptr(py, payload).unwrap();
                        let dict = frame_bindings_extra_locals(py, ptr, false)
                            .unwrap()
                            .unwrap();
                        MoltObject::from_ptr(dict).bits()
                    } else {
                        let dict = frame_bindings_locals(py, payload).expect("locals");
                        dec_ref_bits(py, dict);
                        dict
                    };
                    frame_stack_pop(py);
                    let mut order = Vec::new();
                    let ptr = obj_from_bits(payload).as_ptr().unwrap();
                    unsafe { frame_bindings_detach(ptr, release, |bits| order.push(bits)) };
                    // 3.14 clears a frame from its last slot, earlier targets
                    // from its first. CPython's frame_tp_clear and
                    // frame_dealloc order the slots and the frame object's
                    // own dict differently; the code object goes last.
                    let slots = if minor >= 14 {
                        [second, first]
                    } else {
                        [first, second]
                    };
                    let expected = match (release, minor >= 13) {
                        (FrameRelease::Clear, false) | (FrameRelease::Destroy, true) => {
                            [slots[0], slots[1], locals, code]
                        }
                        (FrameRelease::Clear, true) | (FrameRelease::Destroy, false) => {
                            [locals, slots[0], slots[1], code]
                        }
                    };
                    assert_eq!(order, expected, "3.{minor} {release:?}");
                    for bits in order {
                        dec_ref_bits(py, bits);
                    }
                    dec_ref_bits(py, payload);
                    for bits in [first, second] {
                        assert_eq!(refs(bits), 1);
                        dec_ref_bits(py, bits);
                    }
                    dec_ref_bits(py, extra_key);
                    dec_ref_bits(py, code);
                });
            }
        }
    });
}

#[test]
fn invalid_homes_are_reported_never_empty() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let code = slot_code(py, &[b"plain", b"shared"], &[b"shared"]);
        let value = string(py, b"binding");
        let (homes, index) = enter_frame(py, code, 2);
        let payload = frame_bindings_observe(py, index).expect("payload");
        // A cell home must hold a cell.
        unsafe { set_home(homes, 1, FRAME_HOME_CELL, value) };
        assert!(frame_bindings_items(py, payload).is_err());
        dec_ref_bits(py, take_pending(py));
        // An unknown storage kind is a codegen defect.
        unsafe {
            set_home(homes, 1, FRAME_HOME_UNBOUND, 0);
            set_home(homes, 0, 9, value);
        }
        assert!(frame_bindings_items(py, payload).is_err());
        dec_ref_bits(py, take_pending(py));
        // A cell home shows the cell's contents.
        let cell = MoltObject::from_ptr(crate::object::cells::alloc_cell(py, value)).bits();
        unsafe {
            set_home(homes, 0, FRAME_HOME_UNBOUND, 0);
            inc_ref_bits(py, cell);
            set_home(homes, 1, FRAME_HOME_CELL, cell);
        }
        assert_eq!(bound(py, payload), [("shared".to_string(), value)]);
        frame_stack_pop(py);
        for bits in [cell, value, code] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn a_malformed_home_retires_as_malformed_and_is_reported() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let code = slot_code(py, &[b"broken", b"fine"], &[]);
        let value = string(py, b"binding");
        let (homes, index) = enter_frame(py, code, 2);
        unsafe {
            set_home(homes, 0, 9, value);
            bind(py, homes, 1, value);
        }
        let payload = frame_bindings_observe(py, index).expect("payload");
        inc_ref_bits(py, payload);
        frame_stack_pop(py);
        // Distinguished from an unbound slot, and reported at retirement.
        dec_ref_bits(py, take_pending(py));
        assert_eq!(
            refs(value),
            2,
            "the valid slot's binding moved to the payload"
        );
        // Every later observation reports it too.
        assert!(frame_bindings_items(py, payload).is_err());
        dec_ref_bits(py, take_pending(py));
        // With an exception already pending, retirement keeps that one.
        let (homes, index) = enter_frame(py, code, 2);
        unsafe { set_home(homes, 0, 9, value) };
        let second = frame_bindings_observe(py, index).expect("payload");
        inc_ref_bits(py, second);
        let _ = raise_exception::<u64>(py, "ValueError", "exceptional exit");
        let before = crate::builtins::exceptions::molt_exception_last_pending();
        frame_stack_pop(py);
        let after = take_pending(py);
        assert_eq!(before, after);
        for bits in [before, after, second, payload, value, code] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn a_proxy_write_replaces_the_binding_its_home_owns() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        with_target(py, 13, || {
            let code = slot_code(py, &[b"x"], &[]);
            let original = string(py, b"compiled binding");
            let written = string(py, b"proxy binding");
            let rewritten = string(py, b"second proxy binding");
            let (homes, index) = enter_frame(py, code, 1);
            unsafe { bind(py, homes, 0, original) };
            let payload = frame_bindings_observe(py, index).expect("payload");
            let x = string(py, b"x");
            frame_bindings_write(py, payload, x, Some(written)).expect("write");
            assert_eq!(unsafe { home_kind(homes, 0) }, FRAME_HOME_PLAIN);
            assert_eq!(refs(written), 2, "the home owns what it was given");
            assert_eq!(refs(original), 1, "3.13 releases what the write displaced");
            // A compiled read after the write sees it.
            assert_eq!(molt_frame_home_load(home(homes, 0)), written);
            frame_bindings_write(py, payload, x, Some(rewritten)).expect("write");
            assert_eq!(refs(written), 1);
            assert_eq!(refs(rewritten), 2);
            assert_eq!(bound(py, payload), [("x".to_string(), rewritten)]);
            // The exit releases what the home owns.
            frame_stack_pop(py);
            assert!(!exception_pending(py));
            assert_eq!(refs(rewritten), 1);
            for bits in [x, original, written, rewritten, code] {
                dec_ref_bits(py, bits);
            }
        });
    });
}

#[test]
fn from_314_the_frame_object_keeps_what_proxy_writes_displace() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        with_target(py, 14, || {
            let code = slot_code(py, &[b"x"], &[]);
            let original = string(py, b"compiled binding");
            let first = string(py, b"first proxy binding");
            let second = string(py, b"second proxy binding");
            let (homes, index) = enter_frame(py, code, 1);
            unsafe { bind(py, homes, 0, original) };
            let payload = frame_bindings_observe(py, index).expect("payload");
            // The frame object, which outlives the frame.
            inc_ref_bits(py, payload);
            let x = string(py, b"x");
            frame_bindings_write(py, payload, x, Some(first)).expect("write");
            assert_eq!(
                refs(original),
                2,
                "the frame object keeps the displaced object"
            );
            frame_bindings_write(py, payload, x, Some(second)).expect("write");
            assert_eq!(
                refs(first),
                2,
                "a displaced proxy binding is kept, not released"
            );
            frame_bindings_write(py, payload, x, Some(second)).expect("write");
            assert_eq!(
                refs(second),
                2,
                "writing the identical object keeps nothing more"
            );
            frame_stack_pop(py);
            let mut order = Vec::new();
            let ptr = obj_from_bits(payload).as_ptr().unwrap();
            unsafe { frame_bindings_detach(ptr, FrameRelease::Destroy, |bits| order.push(bits)) };
            // frame_dealloc: the slots, then the overwritten objects newest
            // first, then the code object.
            assert_eq!(order, [second, first, original, code]);
            for bits in order {
                dec_ref_bits(py, bits);
            }
            dec_ref_bits(py, payload);
            for bits in [x, original, first, second, code] {
                assert_eq!(refs(bits), 1);
                dec_ref_bits(py, bits);
            }
        });
    });
}

#[test]
fn a_proxy_writes_cells_retired_frames_and_extra_locals() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        with_target(py, 13, || {
            let code = slot_code(py, &[b"x", b"shared"], &[b"shared"]);
            let original = string(py, b"compiled binding");
            let contents = string(py, b"cell contents");
            let written = string(py, b"proxy binding");
            let cell = MoltObject::from_ptr(crate::object::cells::alloc_cell(py, contents)).bits();
            let (homes, index) = enter_frame(py, code, 2);
            unsafe {
                bind(py, homes, 0, original);
                inc_ref_bits(py, cell);
                set_home(homes, 1, FRAME_HOME_CELL, cell);
            }
            let payload = frame_bindings_observe(py, index).expect("payload");
            inc_ref_bits(py, payload);
            let [x, shared, extra] =
                [&b"x"[..], &b"shared"[..], &b"extra"[..]].map(|name| string(py, name));
            // A cell slot's contents change in place.
            frame_bindings_write(py, payload, shared, Some(written)).expect("write");
            assert_eq!(refs(contents), 1, "the cell released its old contents");
            // A code slot cannot be deleted; other names come and go.
            assert!(frame_bindings_write(py, payload, x, None).is_err());
            dec_ref_bits(py, take_pending(py));
            frame_bindings_write(py, payload, extra, Some(original)).expect("write");
            let snapshot = frame_bindings_snapshot(py, payload).expect("snapshot");
            let lookup = |key: u64| unsafe {
                dict_get_in_place(py, obj_from_bits(snapshot).as_ptr().unwrap(), key)
            };
            assert_eq!(lookup(extra), Some(original));
            assert_eq!(lookup(shared), Some(written));
            dec_ref_bits(py, snapshot);
            frame_bindings_write(py, payload, extra, None).expect("delete");
            assert!(frame_bindings_write(py, payload, extra, None).is_err());
            dec_ref_bits(py, take_pending(py));
            // After the frame exits, the retired payload owns what it stores.
            frame_stack_pop(py);
            let before = refs(original);
            frame_bindings_write(py, payload, x, Some(written)).expect("write");
            assert_eq!(
                refs(original),
                before - 1,
                "the retired pair released its object"
            );
            assert_eq!(bound(py, payload)[0], ("x".to_string(), written));
            dec_ref_bits(py, payload);
            for bits in [x, shared, extra, original, contents, written, cell, code] {
                dec_ref_bits(py, bits);
            }
        });
    });
}

#[test]
fn compiled_code_borrows_only_homes_the_frame_has() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let code = slot_code(py, &[b"a", b"b"], &[]);
        let (homes, _) = enter_frame(py, code, 2);
        assert_eq!(
            molt_frame_homes(1) as usize as *mut u64,
            homes,
            "a split chunk"
        );
        // More slots than the frame has is a layout disagreement: 0 and
        // SystemError, never storage.
        assert_eq!(molt_frame_homes(3), 0);
        dec_ref_bits(py, take_pending(py));
        // A failed entry leaves its exception pending: 0, the same exception.
        let _ = raise_exception::<u64>(py, "ValueError", "entry failed");
        let before = crate::builtins::exceptions::molt_exception_last_pending();
        assert_eq!(molt_frame_homes(2), 0);
        let after = take_pending(py);
        assert_eq!(before, after);
        frame_stack_pop(py);
        // A frame without homes (a module body) lends none.
        inc_ref_bits(py, code);
        frame_stack_push_owned(py, code, 0, 0, 0);
        assert!(frame_stack_enter_homes(FramePlan::default()));
        assert_eq!(molt_frame_homes(1), 0);
        dec_ref_bits(py, take_pending(py));
        frame_stack_pop(py);
        for bits in [before, after, code] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn locals_identity_follows_the_target_version() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for minor in [12, 13] {
            with_target(py, minor, || {
                let code = slot_code(py, &[b"kept", b"dropped"], &[]);
                let kept = string(py, b"kept binding");
                let dropped = string(py, b"dropped binding");
                let (homes, index) = enter_frame(py, code, 2);
                unsafe {
                    bind(py, homes, 0, kept);
                    bind(py, homes, 1, dropped);
                }
                let payload = frame_bindings_observe(py, index).expect("payload");
                let first = frame_bindings_locals(py, payload).expect("locals");
                // `del dropped` in the running frame: its home releases it.
                unsafe { set_home(homes, 1, FRAME_HOME_UNBOUND, 0) };
                dec_ref_bits(py, dropped);
                let second = frame_bindings_locals(py, payload).expect("locals");
                let key = string(py, b"dropped");
                let lookup = |dict: u64| unsafe {
                    dict_get_in_place(py, obj_from_bits(dict).as_ptr().unwrap(), key)
                };
                if minor == 12 {
                    // One refreshed dict: unbound slots are removed from it.
                    assert_eq!(first, second);
                    assert_eq!(lookup(first), None);
                } else {
                    // PEP 667: independent snapshots.
                    assert_ne!(first, second);
                    assert_eq!(lookup(first), Some(dropped));
                    assert_eq!(lookup(second), None);
                }
                frame_stack_pop(py);
                for bits in [first, second, key, kept, dropped, code] {
                    dec_ref_bits(py, bits);
                }
            });
        }
    });
}

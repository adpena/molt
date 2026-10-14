use super::*;
use crate::builtins::frames::{frame_stack_pop, frame_stack_push_owned};
use crate::object::ops::{dict_del_in_place, dict_set_in_place};

fn key(py: &PyToken<'_>, name: &[u8]) -> u64 {
    attr_name_bits_from_bytes(py, name).unwrap()
}

fn guard_bits(dict: *mut u8) -> u64 {
    unsafe { namespace_state(dict) }.expect("tracked namespace") & BUILTIN_BINDING_GUARD_ALL_MASK
}

fn bit(name: &str) -> u64 {
    builtin_binding_guard_bit(name.as_bytes())
}

fn new_namespace(py: &PyToken<'_>) -> *mut u8 {
    let dict = alloc_namespace_dict(py);
    assert!(!dict.is_null());
    dict
}

fn guard(mask: u64) -> bool {
    let bits = molt_guard_builtin_bindings(MoltObject::from_int(mask as i64).bits());
    obj_from_bits(bits).as_bool().expect("guard returns a bool")
}

#[test]
fn module_namespace_records_each_builtin_name_it_binds() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dict = new_namespace(py);
        let value = MoltObject::from_int(1).bits();
        assert_eq!(guard_bits(dict), 0);
        assert!(unsafe { dict_set_in_place(py, dict, key(py, b"total"), value) });
        assert_eq!(guard_bits(dict), 0, "a non-builtin name sets no bit");
        assert!(unsafe { dict_set_in_place(py, dict, key(py, b"len"), value) });
        assert_eq!(guard_bits(dict), bit("len"));
        assert_eq!(
            unsafe { dict_del_in_place(py, dict, key(py, b"len")) },
            Ok(true)
        );
        assert_eq!(guard_bits(dict), bit("len"), "bits are sticky");
        let tuple = MoltObject::from_ptr(alloc_tuple(py, &[value])).bits();
        assert!(unsafe { dict_set_in_place(py, dict, tuple, value) });
        assert_eq!(
            guard_bits(dict),
            BUILTIN_BINDING_GUARD_ALL_MASK,
            "a key that is not an exact str may equal any name"
        );
        dec_ref_bits(py, tuple);
        let plain = alloc_dict_with_pairs(py, &[]);
        assert!(unsafe { namespace_state(plain) }.is_none());
        for bits in [
            MoltObject::from_ptr(dict).bits(),
            MoltObject::from_ptr(plain).bits(),
        ] {
            dec_ref_bits(py, bits);
        }
    })
}

#[test]
fn sealed_builtins_record_rebinding_deletion_and_reset() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dict = new_namespace(py);
        let first = MoltObject::from_int(1).bits();
        let second = MoltObject::from_int(2).bits();
        for name in [&b"len"[..], b"range", b"print"] {
            assert!(unsafe { dict_set_in_place(py, dict, key(py, name), first) });
        }
        seal_builtins_namespace(py, dict, 0);
        let published = bit("len") | bit("range") | bit("print");
        assert_eq!(
            guard_bits(dict),
            BUILTIN_BINDING_GUARD_ALL_MASK & !published,
            "an unpublished name stays unproven"
        );
        assert!(unsafe { dict_set_in_place(py, dict, key(py, b"len"), first) });
        assert_eq!(
            guard_bits(dict) & bit("len"),
            0,
            "storing the same value changes nothing"
        );
        assert!(unsafe { dict_set_in_place(py, dict, key(py, b"len"), second) });
        assert_ne!(guard_bits(dict) & bit("len"), 0);
        assert_eq!(
            unsafe { dict_del_in_place(py, dict, key(py, b"range")) },
            Ok(true)
        );
        assert_ne!(guard_bits(dict) & bit("range"), 0);
        assert_eq!(guard_bits(dict) & bit("print"), 0);
        unsafe { crate::object::ops::dict_clear_in_place(py, dict) };
        assert_eq!(guard_bits(dict), BUILTIN_BINDING_GUARD_ALL_MASK);
        dec_ref_bits(py, MoltObject::from_ptr(dict).bits());

        // A binding made before publication is not the runtime's object.
        let republished = new_namespace(py);
        assert!(unsafe { dict_set_in_place(py, republished, key(py, b"len"), first) });
        let prior = namespace_guard_bits(republished);
        assert_eq!(prior, bit("len"));
        assert!(unsafe { dict_set_in_place(py, republished, key(py, b"range"), first) });
        seal_builtins_namespace(py, republished, prior);
        assert_ne!(guard_bits(republished) & bit("len"), 0);
        assert_eq!(guard_bits(republished) & bit("range"), 0);
        dec_ref_bits(py, MoltObject::from_ptr(republished).bits());
    })
}

#[test]
fn guard_holds_only_for_the_active_frame_namespaces() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let value = MoltObject::from_int(1).bits();
        let builtins = new_namespace(py);
        for name in [&b"len"[..], b"range"] {
            assert!(unsafe { dict_set_in_place(py, builtins, key(py, name), value) });
        }
        seal_builtins_namespace(py, builtins, 0);
        let globals = new_namespace(py);
        let both = bit("len") | bit("range");
        assert!(!guard(both), "no active frame proves nothing");

        let push = |globals: *mut u8, builtins: *mut u8| {
            let globals = MoltObject::from_ptr(globals).bits();
            let builtins = MoltObject::from_ptr(builtins).bits();
            inc_ref_bits(py, globals);
            inc_ref_bits(py, builtins);
            frame_stack_push_owned(py, 0, globals, builtins, 0);
        };
        push(globals, builtins);
        assert!(guard(both));
        assert!(!guard(bit("print")), "an unpublished builtin fails");
        assert_eq!(
            molt_guard_builtin_bindings(MoltObject::from_int(-1).bits()),
            MoltObject::from_bool(false).bits(),
            "a mask outside the table fails"
        );
        assert_eq!(
            molt_guard_builtin_bindings(MoltObject::none().bits()),
            MoltObject::from_bool(false).bits(),
            "a mask that is not an int fails"
        );
        assert!(unsafe { dict_set_in_place(py, globals, key(py, b"len"), value) });
        assert!(!guard(both), "the globals shadow len");
        assert!(guard(bit("range")));
        frame_stack_pop(py);

        let unsealed = new_namespace(py);
        let fresh = new_namespace(py);
        push(fresh, unsealed);
        assert!(
            !guard(bit("range")),
            "builtins that were never sealed prove nothing"
        );
        frame_stack_pop(py);

        let user_globals = alloc_dict_with_pairs(py, &[]);
        push(user_globals, builtins);
        assert!(
            !guard(bit("range")),
            "an untracked globals dict proves nothing"
        );
        frame_stack_pop(py);

        assert!(unsafe {
            dict_set_in_place(
                py,
                builtins,
                key(py, b"range"),
                MoltObject::from_int(2).bits(),
            )
        });
        push(fresh, builtins);
        assert!(!guard(bit("range")), "a rebound builtin fails");
        assert!(guard(bit("len")));
        frame_stack_pop(py);
        for dict in [builtins, globals, unsealed, fresh, user_globals] {
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
        }
    })
}

/// A sealed builtins namespace that publishes `len`, `int` and `isinstance`.
fn sealed_builtins(py: &PyToken<'_>) -> *mut u8 {
    let dict = new_namespace(py);
    for name in [&b"len"[..], b"int", b"isinstance"] {
        assert!(unsafe {
            dict_set_in_place(py, dict, key(py, name), MoltObject::from_int(1).bits())
        });
    }
    seal_builtins_namespace(py, dict, 0);
    assert_eq!(
        guard_bits(dict) & (bit("len") | bit("int") | bit("isinstance")),
        0
    );
    dict
}

#[test]
fn every_whole_contents_reset_path_updates_guard_state() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let value = MoltObject::from_int(1).bits();

        // Cycle and lifecycle clearing empties sealed builtins: every name may
        // now be unbound, so no guard may hold.
        let builtins = sealed_builtins(py);
        unsafe { crate::object::heap_lifecycle::clear_cycle_edges(py, builtins) };
        assert_eq!(guard_bits(builtins), BUILTIN_BINDING_GUARD_ALL_MASK);

        // `dict.clear()` and the C-API `PyDict_Clear` share `molt_dict_clear`.
        let cleared = sealed_builtins(py);
        let result = crate::molt_dict_clear(MoltObject::from_ptr(cleared).bits());
        assert!(obj_from_bits(result).is_none() && !exception_pending(py));
        assert_eq!(guard_bits(cleared), BUILTIN_BINDING_GUARD_ALL_MASK);

        // A module namespace emptied by lifecycle clearing records the names
        // bound again afterwards.
        let module = new_namespace(py);
        assert!(unsafe { dict_set_in_place(py, module, key(py, b"total"), value) });
        unsafe { crate::object::heap_lifecycle::clear_cycle_edges(py, module) };
        assert_eq!(guard_bits(module), 0);
        assert!(unsafe { dict_set_in_place(py, module, key(py, b"int"), value) });
        assert_eq!(guard_bits(module), bit("int"));

        // Staged publication swaps whole contents: the module now binds `len`.
        let staged = alloc_dict_with_pairs(py, &[key(py, b"len"), value]);
        assert!(!staged.is_null());
        unsafe { crate::object::ops::dict_publish_staged(py, module, staged) };
        assert_eq!(guard_bits(module), bit("int") | bit("len"));

        for dict in [builtins, cleared, module, staged] {
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
        }
    })
}

/// Free-threaded binders OR their bits into one state word at once. A load
/// followed by a store loses one writer's bit when two writers interleave.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn concurrent_binders_never_lose_a_guard_bit() {
    const THREADS: usize = 4;
    const ROUNDS: usize = 256;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let address = crate::with_gil_entry_nopanic!(py, { new_namespace(py) as usize });
    let barrier = std::sync::Barrier::new(THREADS + 1);
    let mut incomplete_rounds = 0;
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let barrier = &barrier;
            scope.spawn(move || {
                let dict = address as *mut u8;
                for _ in 0..ROUNDS {
                    barrier.wait();
                    for index in (thread..BUILTIN_BINDING_GUARD_NAMES.len()).step_by(THREADS) {
                        let state = unsafe { namespace_state(dict) }.expect("tracked namespace");
                        unsafe { add_bits(dict, state, 1 << index) };
                    }
                    barrier.wait();
                }
            });
        }
        let dict = address as *mut u8;
        for _ in 0..ROUNDS {
            object_set_state(dict, NAMESPACE_TRACKED as i64);
            barrier.wait();
            barrier.wait();
            // Count rather than assert: a panic here would strand the binders
            // at the barrier and hang the test instead of failing it.
            if guard_bits(dict) != BUILTIN_BINDING_GUARD_ALL_MASK {
                incomplete_rounds += 1;
            }
        }
    });
    assert_eq!(incomplete_rounds, 0, "a concurrent binder lost a guard bit");
    crate::with_gil_entry_nopanic!(py, {
        dec_ref_bits(py, MoltObject::from_ptr(address as *mut u8).bits());
    });
}

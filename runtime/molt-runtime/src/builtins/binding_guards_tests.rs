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
        unsafe { dict_set_in_place(py, dict, key(py, b"total"), value) };
        assert_eq!(guard_bits(dict), 0, "a non-builtin name sets no bit");
        unsafe { dict_set_in_place(py, dict, key(py, b"len"), value) };
        assert_eq!(guard_bits(dict), bit("len"));
        unsafe { dict_del_in_place(py, dict, key(py, b"len")) };
        assert_eq!(guard_bits(dict), bit("len"), "bits are sticky");
        let tuple = MoltObject::from_ptr(alloc_tuple(py, &[value])).bits();
        unsafe { dict_set_in_place(py, dict, tuple, value) };
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
            unsafe { dict_set_in_place(py, dict, key(py, name), first) };
        }
        seal_builtins_namespace(py, dict, 0);
        let published = bit("len") | bit("range") | bit("print");
        assert_eq!(
            guard_bits(dict),
            BUILTIN_BINDING_GUARD_ALL_MASK & !published,
            "an unpublished name stays unproven"
        );
        unsafe { dict_set_in_place(py, dict, key(py, b"len"), first) };
        assert_eq!(
            guard_bits(dict) & bit("len"),
            0,
            "storing the same value changes nothing"
        );
        unsafe { dict_set_in_place(py, dict, key(py, b"len"), second) };
        assert_ne!(guard_bits(dict) & bit("len"), 0);
        unsafe { dict_del_in_place(py, dict, key(py, b"range")) };
        assert_ne!(guard_bits(dict) & bit("range"), 0);
        assert_eq!(guard_bits(dict) & bit("print"), 0);
        unsafe { crate::object::ops::dict_clear_in_place(py, dict) };
        assert_eq!(guard_bits(dict), BUILTIN_BINDING_GUARD_ALL_MASK);
        dec_ref_bits(py, MoltObject::from_ptr(dict).bits());

        // A binding made before publication is not the runtime's object.
        let republished = new_namespace(py);
        unsafe { dict_set_in_place(py, republished, key(py, b"len"), first) };
        let prior = namespace_guard_bits(republished);
        assert_eq!(prior, bit("len"));
        unsafe { dict_set_in_place(py, republished, key(py, b"range"), first) };
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
            unsafe { dict_set_in_place(py, builtins, key(py, name), value) };
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
        unsafe { dict_set_in_place(py, globals, key(py, b"len"), value) };
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

        unsafe {
            dict_set_in_place(
                py,
                builtins,
                key(py, b"range"),
                MoltObject::from_int(2).bits(),
            )
        };
        push(fresh, builtins);
        assert!(!guard(bit("range")), "a rebound builtin fails");
        assert!(guard(bit("len")));
        frame_stack_pop(py);
        for dict in [builtins, globals, unsealed, fresh, user_globals] {
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
        }
    })
}

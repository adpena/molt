use super::{
    molt_operator_attrgetter_type, molt_operator_itemgetter_type, molt_operator_length_hint,
    molt_operator_methodcaller_type, operator_clear_runtime_state,
};
use crate::builtins::exceptions::{molt_exception_kind, molt_exception_last_pending};
use crate::{
    MoltObject, alloc_string, dec_ref_bits, exception_pending, obj_from_bits, runtime_state,
    string_obj_to_owned, to_i64,
};
use std::sync::atomic::Ordering;

#[test]
fn operator_type_caches_are_runtime_owned_and_clearable() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        operator_clear_runtime_state(_py, state);

        let itemgetter_bits = molt_operator_itemgetter_type();
        let attrgetter_bits = molt_operator_attrgetter_type();
        let methodcaller_bits = molt_operator_methodcaller_type();
        assert!(!obj_from_bits(itemgetter_bits).is_none());
        assert!(!obj_from_bits(attrgetter_bits).is_none());
        assert!(!obj_from_bits(methodcaller_bits).is_none());

        assert_eq!(
            state.operator.itemgetter_class.load(Ordering::Acquire),
            itemgetter_bits
        );
        assert_eq!(
            state.operator.attrgetter_class.load(Ordering::Acquire),
            attrgetter_bits
        );
        assert_eq!(
            state.operator.methodcaller_class.load(Ordering::Acquire),
            methodcaller_bits
        );
        assert_ne!(state.operator.itemgetter_call.load(Ordering::Acquire), 0);
        assert_ne!(state.operator.attrgetter_call.load(Ordering::Acquire), 0);
        assert_ne!(state.operator.methodcaller_call.load(Ordering::Acquire), 0);
        assert_ne!(state.operator.itemgetter_init.load(Ordering::Acquire), 0);
        assert_ne!(state.operator.attrgetter_init.load(Ordering::Acquire), 0);
        assert_ne!(state.operator.methodcaller_init.load(Ordering::Acquire), 0);

        operator_clear_runtime_state(_py, state);
        for slot in state.operator.slots() {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    });
}

#[test]
fn operator_length_hint_validates_default_before_fallback() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();

        let negative = molt_operator_length_hint(none_bits, MoltObject::from_int(-1).bits());
        assert_eq!(to_i64(obj_from_bits(negative)), Some(-1));

        let truthy = molt_operator_length_hint(none_bits, MoltObject::from_bool(true).bits());
        assert_eq!(to_i64(obj_from_bits(truthy)), Some(1));

        let invalid_ptr = alloc_string(_py, b"x");
        assert!(!invalid_ptr.is_null());
        let invalid_bits = MoltObject::from_ptr(invalid_ptr).bits();
        let result = molt_operator_length_hint(none_bits, invalid_bits);
        dec_ref_bits(_py, invalid_bits);

        assert!(obj_from_bits(result).is_none());
        assert!(exception_pending(_py));
        let exc_bits = molt_exception_last_pending();
        let kind_bits = molt_exception_kind(exc_bits);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(kind_bits)).as_deref(),
            Some("TypeError")
        );
        dec_ref_bits(_py, kind_bits);
        dec_ref_bits(_py, exc_bits);
        crate::molt_exception_clear();
    });
}

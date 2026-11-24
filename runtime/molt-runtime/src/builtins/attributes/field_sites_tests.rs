//! Warm attribute sites must give the answers of the generic transaction.
//! The oracle is CPython's lookup order: a class data descriptor takes
//! priority over instance storage, an instance dictionary owns its fields, an
//! unset field raises, and a custom `__setattr__` sees every write.

use super::field_sites::{FieldAccess, cached_offset_for_test};
use super::*;
use crate::builtins::functions::alloc_runtime_function_obj;
use std::sync::atomic::{AtomicU64, Ordering};

const FIELD: &[u8] = b"field";

fn name(py: &PyToken<'_>, bytes: &[u8]) -> u64 {
    dynamic_attr_name_bits(py, bytes).unwrap()
}

/// A class built like a compiled class statement, whose layout places
/// `field` and `later` in consecutive inferred slots.
fn field_class(py: &PyToken<'_>, class_name: &[u8]) -> u64 {
    let builtins = crate::builtin_classes(py);
    let class_name = name(py, class_name);
    let field = name(py, FIELD);
    let later = name(py, b"later");
    let offsets = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
        py,
        &[
            field,
            MoltObject::from_int(0).bits(),
            later,
            MoltObject::from_int(8).bits(),
        ],
    ))
    .bits();
    let namespace = crate::molt_dict_new_attached_body(py, 2);
    for (key, value) in [
        (&b"__molt_field_offsets__"[..], offsets),
        (
            &b"__molt_layout_size__"[..],
            MoltObject::from_int(24).bits(),
        ),
    ] {
        let key = name(py, key);
        assert_eq!(
            crate::c_api::molt_mapping_setitem_attached_body(py, namespace, key, value),
            0
        );
        dec_ref_bits(py, key);
    }
    let class = crate::builtins::types::molt_type_new_attached_body(
        py,
        builtins.type_obj,
        class_name,
        MoltObject::none().bits(),
        namespace,
        MoltObject::none().bits(),
    );
    assert!(!obj_from_bits(class).is_none());
    assert!(!exception_pending(py));
    for bits in [class_name, field, later, offsets, namespace] {
        dec_ref_bits(py, bits);
    }
    class
}

fn instance(py: &PyToken<'_>, class: u64) -> u64 {
    let object =
        unsafe { crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
    assert!(!obj_from_bits(object).is_none());
    object
}

fn site(id: i64) -> u64 {
    MoltObject::from_int(id).bits()
}

fn read(py: &PyToken<'_>, object: u64, site_id: i64) -> u64 {
    unsafe {
        crate::molt_get_attr_object_ic_attached_body(
            py,
            object,
            FIELD.as_ptr(),
            FIELD.len() as u64,
            site(site_id),
        )
    }
}

fn write(py: &PyToken<'_>, object: u64, value: u64, site_id: i64) {
    let out = unsafe {
        crate::builtins::attributes::molt_set_attr_object_ic_attached_body(
            py,
            object,
            FIELD.as_ptr(),
            FIELD.len() as u64,
            value,
            site(site_id),
        )
    };
    assert_eq!(out, MoltObject::none().bits());
}

fn refcount(bits: u64) -> u32 {
    unsafe { (*header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot() }
}

fn take_attribute_error(py: &PyToken<'_>) {
    let exception = crate::builtins::exceptions::molt_exception_last_pending_attached_body(py);
    assert!(crate::builtins::exceptions::exception_matches_kind(
        py,
        exception,
        crate::BuiltinExc::AttributeError
    ));
    clear_exception(py);
    dec_ref_bits(py, exception);
}

fn function(py: &PyToken<'_>, symbol: &str, target: *const (), arity: u64) -> u64 {
    let ptr = alloc_runtime_function_obj(py, crate::test_named_callable_ref(symbol, target), arity);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

#[test]
fn dictionary_write_sites_use_live_storage_and_observe_descriptor_publication() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class_name = name(py, b"DictionaryWriteOwner");
        let namespace = crate::molt_dict_new_attached_body(py, 0);
        let class = crate::builtins::types::molt_type_new_attached_body(
            py,
            crate::builtin_classes(py).type_obj,
            class_name,
            MoltObject::none().bits(),
            namespace,
            MoltObject::none().bits(),
        );
        let object = instance(py, class);
        let other = instance(py, class);
        let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        let before = refcount(value);
        write(py, object, value, 0x5601);
        // A code object's exact str can equal the lexical name without being
        // its interned object. The first dictionary insertion owns that key.
        let code_name =
            MoltObject::from_ptr(crate::object::builders::alloc_string_uninterned(py, FIELD))
                .bits();
        let prior = refcount(code_name);
        set_attr_name_at_site(py, other, code_name, value, 0x5601);
        assert!(!exception_pending(py));
        assert_eq!(refcount(code_name), prior + 1);
        dec_ref_bits(py, code_name);
        for target in [object, other, object] {
            write(py, target, value, 0x5601);
            let got = read(py, target, 0x5602);
            assert_eq!(got, value);
            dec_ref_bits(py, got);
        }
        assert_eq!(refcount(value), before + 2);
        let replacement = crate::molt_dict_new_attached_body(py, 0);
        unsafe {
            crate::object::field_storage::replace_dictionary(
                py,
                obj_from_bits(object).as_ptr().unwrap(),
                Some(replacement),
            )
            .unwrap();
        }
        assert_eq!(refcount(value), before + 1);
        write(py, object, value, 0x5601);
        assert_eq!(refcount(value), before + 2);

        let get = function(
            py,
            "field_site_descriptor_get",
            descriptor_get as *const (),
            1,
        );
        let set = function(
            py,
            "field_site_descriptor_set",
            descriptor_set as *const (),
            2,
        );
        let property =
            crate::molt_property_new_attached_body(py, get, set, MoltObject::none().bits());
        let field = name(py, FIELD);
        crate::molt_set_attr_name_attached_body(py, class, field, property);
        SETTER_VALUE.store(0, Ordering::SeqCst);
        write(py, object, MoltObject::from_int(17).bits(), 0x5601);
        assert_eq!(
            SETTER_VALUE.load(Ordering::SeqCst),
            MoltObject::from_int(17).bits()
        );
        assert_eq!(read(py, object, 0x5602), MoltObject::from_int(99).bits());
        assert!(!exception_pending(py));
        for bits in [
            field,
            property,
            get,
            set,
            replacement,
            object,
            other,
            value,
            class,
            namespace,
            class_name,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn warm_sites_read_and_write_inferred_fields_with_generic_ownership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"WarmOwner");
        let object = instance(py, class);
        let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        let before = refcount(value);

        write(py, object, value, 0x5101);
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5101, object, FIELD).is_some());
        assert_eq!(refcount(value), before + 1, "the field owns its value");
        write(py, object, value, 0x5101);
        assert_eq!(
            refcount(value),
            before + 1,
            "a warm self assignment keeps one owner"
        );

        for _ in 0..2 {
            let got = read(py, object, 0x5102);
            assert_eq!(got, value);
            assert_eq!(
                refcount(value),
                before + 2,
                "a read returns an owned reference"
            );
            dec_ref_bits(py, got);
        }
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5102, object, FIELD).is_some());
        // Reads and writes never share an entry, even under one site id.
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5102, object, FIELD).is_none());

        write(py, object, MoltObject::none().bits(), 0x5101);
        assert_eq!(
            refcount(value),
            before,
            "replacement releases the field's owner"
        );
        assert!(!exception_pending(py));
        for bits in [object, class, value] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn warmed_first_write_observes_later_live_fields_on_each_receiver() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"OrderedWrites");
        let cold = instance(py, class);
        let ordered = instance(py, class);
        let reversed = instance(py, class);
        let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        let before = refcount(value);
        write(py, cold, value, 0x5810);
        write(py, ordered, value, 0x5810);
        unsafe {
            assert!(!crate::object::field_storage::dictionary_backed(
                obj_from_bits(ordered).as_ptr().unwrap()
            ));
            crate::builtins::attributes::molt_set_attr_object_ic_attached_body(
                py,
                reversed,
                b"later".as_ptr(),
                5,
                value,
                site(0x5811),
            );
            write(py, reversed, value, 0x5810);
            assert!(crate::object::field_storage::dictionary_backed(
                obj_from_bits(reversed).as_ptr().unwrap()
            ));
        }
        assert!(!exception_pending(py));
        assert_eq!(refcount(value), before + 4);
        for object in [cold, ordered, reversed] {
            dec_ref_bits(py, object);
        }
        assert_eq!(refcount(value), before);
        dec_ref_bits(py, value);
        dec_ref_bits(py, class);
    });
}

static SETTER_VALUE: AtomicU64 = AtomicU64::new(0);

extern "C" fn descriptor_get(__context: u64, _receiver: u64) -> u64 {
    assert_eq!(
        __context,
        crate::state::molt_attached_context(),
        "Python callback context identity"
    );
    let __py = unsafe { crate::PyToken::from_attached_context(__context) };

    MoltObject::from_int(99).bits()
}

extern "C" fn descriptor_set(__context: u64, _receiver: u64, value: u64) -> u64 {
    assert_eq!(
        __context,
        crate::state::molt_attached_context(),
        "Python callback context identity"
    );
    let __py = unsafe { crate::PyToken::from_attached_context(__context) };

    SETTER_VALUE.store(value, Ordering::SeqCst);
    MoltObject::none().bits()
}

#[test]
fn a_reused_lexical_site_resolves_each_actual_field_name() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"ClonedGetters");
        let object = instance(py, class);
        let first = MoltObject::from_int(42).bits();
        let second = MoltObject::from_int(91).bits();
        let other = b"other";
        write(py, object, first, 0x5701);
        assert_eq!(read(py, object, 0x5702), first);
        let miss = unsafe {
            crate::molt_get_attr_object_ic_attached_body(
                py,
                object,
                other.as_ptr(),
                other.len() as u64,
                site(0x5702),
            )
        };
        assert_eq!(miss, MoltObject::none().bits());
        take_attribute_error(py);
        unsafe {
            crate::builtins::attributes::molt_set_attr_object_ic_attached_body(
                py,
                object,
                other.as_ptr(),
                other.len() as u64,
                second,
                site(0x5701),
            );
        }
        assert!(!exception_pending(py));
        assert_eq!(read(py, object, 0x5702), first);
        let actual = unsafe {
            crate::molt_get_attr_object_ic_attached_body(
                py,
                object,
                other.as_ptr(),
                other.len() as u64,
                site(0x5702),
            )
        };
        assert_eq!(actual, second);
        for owner in [object, class] {
            dec_ref_bits(py, owner);
        }
    });
}

#[test]
fn a_data_descriptor_published_after_warmup_takes_priority() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"DescriptorOwner");
        let object = instance(py, class);
        write(py, object, MoltObject::from_int(1).bits(), 0x5201);
        assert_eq!(read(py, object, 0x5202), MoltObject::from_int(1).bits());
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5202, object, FIELD).is_some());
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5201, object, FIELD).is_some());

        let get = function(
            py,
            "field_site_descriptor_get",
            descriptor_get as *const (),
            1,
        );
        let set = function(
            py,
            "field_site_descriptor_set",
            descriptor_set as *const (),
            2,
        );
        let property =
            crate::molt_property_new_attached_body(py, get, set, MoltObject::none().bits());
        let field = name(py, FIELD);
        crate::molt_set_attr_name_attached_body(py, class, field, property);
        assert!(!exception_pending(py));

        // The class mutation retires both entries, and a name the MRO
        // defines is never recorded again.
        for _ in 0..2 {
            assert_eq!(read(py, object, 0x5202), MoltObject::from_int(99).bits());
            SETTER_VALUE.store(0, Ordering::SeqCst);
            write(py, object, MoltObject::from_int(5).bits(), 0x5201);
            assert_eq!(
                SETTER_VALUE.load(Ordering::SeqCst),
                MoltObject::from_int(5).bits()
            );
            assert!(cached_offset_for_test(FieldAccess::Read, 0x5202, object, FIELD).is_none());
            assert!(cached_offset_for_test(FieldAccess::Write, 0x5201, object, FIELD).is_none());
        }
        assert!(!exception_pending(py));
        for bits in [field, property, get, set, object, class] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn instance_dictionaries_and_unset_fields_defer_to_the_generic_lookup() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"DeferringOwner");
        let unset = instance(py, class);
        let object = instance(py, class);
        write(py, object, MoltObject::from_int(3).bits(), 0x5301);
        assert_eq!(read(py, object, 0x5302), MoltObject::from_int(3).bits());

        // A warm site reaches an object of the same class whose field is unset.
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5302, unset, FIELD).is_some());
        assert_eq!(read(py, unset, 0x5302), MoltObject::none().bits());
        take_attribute_error(py);

        // A materialized dictionary owns the fields; the site defers to it.
        let dict_name = name(py, b"__dict__");
        let dict = crate::molt_get_attr_name_attached_body(py, object, dict_name);
        assert!(obj_from_bits(dict).as_ptr().is_some());
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5302, object, FIELD).is_none());
        assert_eq!(read(py, object, 0x5302), MoltObject::from_int(3).bits());
        write(py, object, MoltObject::from_int(4).bits(), 0x5301);
        let field = name(py, FIELD);
        let stored = unsafe { dict_get_in_place(py, obj_from_bits(dict).as_ptr().unwrap(), field) };
        assert_eq!(stored, Ok(Some(MoltObject::from_int(4).bits())));
        assert!(!exception_pending(py));
        for bits in [dict_name, dict, field, object, unset, class] {
            dec_ref_bits(py, bits);
        }
    });
}

static SETATTR_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn counting_setattr(__context: u64, _receiver: u64, _name: u64, _value: u64) -> u64 {
    assert_eq!(
        __context,
        crate::state::molt_attached_context(),
        "Python callback context identity"
    );
    let __py = unsafe { crate::PyToken::from_attached_context(__context) };

    SETATTR_CALLS.fetch_add(1, Ordering::SeqCst);
    MoltObject::none().bits()
}

#[test]
fn a_custom_setattr_sees_every_write_from_a_warm_read_site() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"HookOwner");
        let object = instance(py, class);
        let hook = function(
            py,
            "field_site_counting_setattr",
            counting_setattr as *const (),
            3,
        );
        let hook_name = name(py, b"__setattr__");
        crate::molt_set_attr_name_attached_body(py, class, hook_name, hook);
        assert!(!exception_pending(py));

        // The read protocol stays plain, so the read site may record.
        assert_eq!(read(py, object, 0x5401), MoltObject::none().bits());
        take_attribute_error(py);
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5401, object, FIELD).is_some());

        SETATTR_CALLS.store(0, Ordering::SeqCst);
        for _ in 0..3 {
            write(py, object, MoltObject::from_int(8).bits(), 0x5401);
        }
        assert_eq!(SETATTR_CALLS.load(Ordering::SeqCst), 3);
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5401, object, FIELD).is_none());
        assert!(!exception_pending(py));
        for bits in [hook_name, hook, object, class] {
            dec_ref_bits(py, bits);
        }
    });
}

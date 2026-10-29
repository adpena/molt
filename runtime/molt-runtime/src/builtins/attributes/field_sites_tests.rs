//! Warm attribute sites must give the answers of the generic transaction.
//! The oracle is CPython's lookup order: a class data descriptor takes
//! priority over instance storage, an instance dictionary owns its fields, an
//! unset field raises, and a custom `__setattr__` sees every write.

use super::field_sites::{FieldAccess, cached_offset_for_test};
use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::sync::atomic::{AtomicU64, Ordering};

const FIELD: &[u8] = b"field";

fn name(py: &PyToken<'_>, bytes: &[u8]) -> u64 {
    attr_name_bits_from_bytes(py, bytes).unwrap()
}

/// A class built like a compiled class statement, whose layout places
/// `field` in an inferred slot at offset 0.
fn field_class(py: &PyToken<'_>, class_name: &[u8]) -> u64 {
    let builtins = crate::builtin_classes(py);
    let class_name = name(py, class_name);
    let field = name(py, FIELD);
    let offsets = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
        py,
        &[field, MoltObject::from_int(0).bits()],
    ))
    .bits();
    let namespace = crate::molt_dict_new(2);
    for (key, value) in [
        (&b"__molt_field_offsets__"[..], offsets),
        (
            &b"__molt_layout_size__"[..],
            MoltObject::from_int(16).bits(),
        ),
    ] {
        let key = name(py, key);
        assert_eq!(crate::c_api::molt_mapping_setitem(namespace, key, value), 0);
        dec_ref_bits(py, key);
    }
    let class = crate::builtins::types::molt_type_new(
        builtins.type_obj,
        class_name,
        MoltObject::none().bits(),
        namespace,
        MoltObject::none().bits(),
    );
    assert!(!obj_from_bits(class).is_none());
    assert!(!exception_pending(py));
    for bits in [class_name, field, offsets, namespace] {
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

fn read(object: u64, site_id: i64) -> u64 {
    unsafe {
        crate::molt_get_attr_object_ic(object, FIELD.as_ptr(), FIELD.len() as u64, site(site_id))
    }
}

fn write(object: u64, value: u64, site_id: i64) {
    let out = unsafe {
        molt_set_attr_object_ic(
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
    let exception = crate::builtins::exceptions::molt_exception_last_pending();
    assert!(crate::builtins::exceptions::exception_matches_kind(
        py,
        exception,
        crate::BuiltinExc::AttributeError
    ));
    clear_exception(py);
    dec_ref_bits(py, exception);
}

fn function(py: &PyToken<'_>, symbol: &str, target: *const (), arity: u64) -> u64 {
    let ptr = alloc_runtime_function_obj(py, runtime_fn_addr(symbol, target), arity);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

#[test]
fn warm_sites_read_and_write_inferred_fields_with_generic_ownership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"WarmOwner");
        let object = instance(py, class);
        let value = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        let before = refcount(value);

        write(object, value, 0x5101);
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5101, object).is_some());
        assert_eq!(refcount(value), before + 1, "the field owns its value");
        write(object, value, 0x5101);
        assert_eq!(
            refcount(value),
            before + 1,
            "a warm self assignment keeps one owner"
        );

        for _ in 0..2 {
            let got = read(object, 0x5102);
            assert_eq!(got, value);
            assert_eq!(
                refcount(value),
                before + 2,
                "a read returns an owned reference"
            );
            dec_ref_bits(py, got);
        }
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5102, object).is_some());
        // Reads and writes never share an entry, even under one site id.
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5102, object).is_none());

        write(object, MoltObject::none().bits(), 0x5101);
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

static SETTER_VALUE: AtomicU64 = AtomicU64::new(0);

extern "C" fn descriptor_get(_receiver: u64) -> u64 {
    MoltObject::from_int(99).bits()
}

extern "C" fn descriptor_set(_receiver: u64, value: u64) -> u64 {
    SETTER_VALUE.store(value, Ordering::SeqCst);
    MoltObject::none().bits()
}

#[test]
fn a_data_descriptor_published_after_warmup_takes_priority() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = field_class(py, b"DescriptorOwner");
        let object = instance(py, class);
        write(object, MoltObject::from_int(1).bits(), 0x5201);
        assert_eq!(read(object, 0x5202), MoltObject::from_int(1).bits());
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5202, object).is_some());
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5201, object).is_some());

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
        let property = crate::molt_property_new(get, set, MoltObject::none().bits());
        let field = name(py, FIELD);
        crate::molt_set_attr_name(class, field, property);
        assert!(!exception_pending(py));

        // The class mutation retires both entries, and a name the MRO
        // defines is never recorded again.
        for _ in 0..2 {
            assert_eq!(read(object, 0x5202), MoltObject::from_int(99).bits());
            SETTER_VALUE.store(0, Ordering::SeqCst);
            write(object, MoltObject::from_int(5).bits(), 0x5201);
            assert_eq!(
                SETTER_VALUE.load(Ordering::SeqCst),
                MoltObject::from_int(5).bits()
            );
            assert!(cached_offset_for_test(FieldAccess::Read, 0x5202, object).is_none());
            assert!(cached_offset_for_test(FieldAccess::Write, 0x5201, object).is_none());
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
        write(object, MoltObject::from_int(3).bits(), 0x5301);
        assert_eq!(read(object, 0x5302), MoltObject::from_int(3).bits());

        // A warm site reaches an object of the same class whose field is unset.
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5302, unset).is_some());
        assert_eq!(read(unset, 0x5302), MoltObject::none().bits());
        take_attribute_error(py);

        // A materialized dictionary owns the fields; the site defers to it.
        let dict_name = name(py, b"__dict__");
        let dict = crate::molt_get_attr_name(object, dict_name);
        assert!(obj_from_bits(dict).as_ptr().is_some());
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5302, object).is_none());
        assert_eq!(read(object, 0x5302), MoltObject::from_int(3).bits());
        write(object, MoltObject::from_int(4).bits(), 0x5301);
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

extern "C" fn counting_setattr(_receiver: u64, _name: u64, _value: u64) -> u64 {
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
        crate::molt_set_attr_name(class, hook_name, hook);
        assert!(!exception_pending(py));

        // The read protocol stays plain, so the read site may record.
        assert_eq!(read(object, 0x5401), MoltObject::none().bits());
        take_attribute_error(py);
        assert!(cached_offset_for_test(FieldAccess::Read, 0x5401, object).is_some());

        SETATTR_CALLS.store(0, Ordering::SeqCst);
        for _ in 0..3 {
            write(object, MoltObject::from_int(8).bits(), 0x5401);
        }
        assert_eq!(SETATTR_CALLS.load(Ordering::SeqCst), 3);
        assert!(cached_offset_for_test(FieldAccess::Write, 0x5401, object).is_none());
        assert!(!exception_pending(py));
        for bits in [hook_name, hook, object, class] {
            dec_ref_bits(py, bits);
        }
    });
}

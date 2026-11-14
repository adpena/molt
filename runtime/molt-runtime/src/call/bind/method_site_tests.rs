//! The cold side of a compiled method site against CPython's `LOAD_ATTR`
//! method-form contract: what the load returns, and when the record may let
//! compiled code repeat it (invariant J3).

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

extern "C" fn one(__context: u64, _self_bits: u64) -> u64 {
    let _py = unsafe { crate::PyToken::from_attached_context(__context) };
    MoltObject::from_int(1).bits()
}

extern "C" fn two(__context: u64, _self_bits: u64) -> u64 {
    let _py = unsafe { crate::PyToken::from_attached_context(__context) };
    MoltObject::from_int(2).bits()
}

fn function(py: &PyToken<'_>, body: *const ()) -> u64 {
    let pointer = crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(body),
        1,
    );
    assert!(!pointer.is_null());
    MoltObject::from_ptr(pointer).bits()
}

/// A finished class with `value = function(body)`.
fn class_with_value(py: &PyToken<'_>, label: &[u8], body: *const ()) -> u64 {
    let label = crate::dynamic_attr_name_bits(py, label).unwrap();
    let class = crate::molt_class_new_attached_body(py, label);
    dec_ref_bits(py, label);
    crate::molt_class_set_base_attached_body(py, class, builtin_classes(py).object);
    let class_ptr = obj_from_bits(class).as_ptr().unwrap();
    unsafe { crate::object::class_finish_definition(py, class_ptr) }.unwrap();
    set_attr(py, class, b"value", function(py, body));
    assert!(!exception_pending(py));
    class
}

/// `target.name = value`, consuming `value`.
fn set_attr(py: &PyToken<'_>, target: u64, name: &[u8], value: u64) {
    let name = crate::dynamic_attr_name_bits(py, name).unwrap();
    crate::molt_set_attr_name_attached_body(py, target, name, value);
    dec_ref_bits(py, name);
    dec_ref_bits(py, value);
}

fn instance(py: &PyToken<'_>, class: u64) -> u64 {
    let receiver =
        unsafe { crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
    assert!(obj_from_bits(receiver).as_ptr().is_some());
    receiver
}

/// Load `value` at `site` and call the pair as compiled code does.
fn call(py: &PyToken<'_>, site: &TestSite, receiver: u64) -> u64 {
    unsafe {
        let loaded = site.load(py, receiver);
        super::super::inline_cache::call_loaded_method(
            py,
            loaded.callable,
            loaded.self_or_null,
            &[],
        )
    }
}

fn refs(bits: u64) -> u32 {
    let pointer = obj_from_bits(bits).as_ptr().unwrap();
    unsafe { (*crate::object::header_from_obj_ptr(pointer)).ref_count_snapshot() }
}

fn take_error(py: &PyToken<'_>, kind: BuiltinExc) {
    let error = crate::builtins::exceptions::molt_exception_last_pending_attached_body(py);
    assert!(crate::builtins::exceptions::exception_matches_kind(
        py, error, kind
    ));
    crate::molt_exception_clear_attached_body(py);
    dec_ref_bits(py, error);
}

#[test]
fn a_miss_fills_the_record_that_compiled_code_repeats() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = class_with_value(py, b"SiteFill", one as *const ());
        let receiver = instance(py, class);
        let site = TestSite::new(b"value");
        assert_eq!(site.word(METHOD_SITE_CLASS_OFFSET), METHOD_SITE_EMPTY_CLASS);
        let result = call(py, &site, receiver);
        assert_eq!(obj_from_bits(result).as_int(), Some(1));
        assert_eq!(site.word(METHOD_SITE_CLASS_OFFSET), class);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        assert_eq!(site.word(METHOD_SITE_VERSION_OFFSET), unsafe {
            class_layout_version_bits(class_ptr)
        });
        let receiver_ptr = obj_from_bits(receiver).as_ptr().unwrap();
        let header = unsafe { &*header_from_obj_ptr(receiver_ptr) };
        assert_eq!(
            site.word(METHOD_SITE_SHAPE_OFFSET),
            method_site_header_shape(header.size_class)
        );
        let dict_slot = unsafe { crate::object::instance_dict_bits_ptr(receiver_ptr) };
        assert_eq!(
            site.word(METHOD_SITE_DICT_OFFSET),
            (dict_slot as usize - receiver_ptr as usize) as u64
        );
        // The name is interned once and kept.
        let name = site.word(METHOD_SITE_NAME_OFFSET);
        assert_ne!(name, 0);
        let result2 = call(py, &site, receiver);
        assert_eq!(obj_from_bits(result2).as_int(), Some(1));
        assert_eq!(site.word(METHOD_SITE_NAME_OFFSET), name);
        // The record owns the class and the function; release returns them.
        let function = site.word(METHOD_SITE_FUNCTION_OFFSET);
        let (class_refs, function_refs) = (refs(class), refs(function));
        site.release(py);
        assert_eq!(refs(class), class_refs - 1);
        assert_eq!(refs(function), function_refs - 1);
        for bits in [result, result2, receiver, class] {
            dec_ref_bits(py, bits);
        }
    });
}

/// J3: a class mutation, an instance attribute that shadows the method and a
/// `__class__` assignment each change what the next load selects.
#[test]
fn each_mutation_is_observed_at_the_next_load() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = class_with_value(py, b"SiteMutated", one as *const ());
        let other = class_with_value(py, b"SiteOther", two as *const ());
        let receiver = instance(py, class);
        let site = TestSite::new(b"value");
        let expect = |want: i64| {
            let result = call(py, &site, receiver);
            assert!(!exception_pending(py));
            assert_eq!(obj_from_bits(result).as_int(), Some(want));
            dec_ref_bits(py, result);
        };
        expect(1);
        // Rebind the method on the class: the version moves, the site refills.
        let version = site.word(METHOD_SITE_VERSION_OFFSET);
        set_attr(py, class, b"value", function(py, two as *const ()));
        expect(2);
        assert_ne!(site.word(METHOD_SITE_VERSION_OFFSET), version);
        // An instance attribute shadows it: the load returns the attribute
        // with the null sentinel, and the class side stays recorded.
        set_attr(py, receiver, b"value", MoltObject::from_int(7).bits());
        let loaded = unsafe { site.load(py, receiver) };
        assert_eq!(loaded.callable, MoltObject::from_int(7).bits());
        assert_eq!(loaded.self_or_null, LoadedMethod::NULL);
        assert_eq!(site.word(METHOD_SITE_CLASS_OFFSET), class);
        let name = crate::dynamic_attr_name_bits(py, b"value").unwrap();
        crate::molt_del_attr_name_attached_body(py, receiver, name);
        dec_ref_bits(py, name);
        assert!(!exception_pending(py));
        expect(2);
        // Assign `__class__`: the class word moves.
        set_attr(py, receiver, b"__class__", {
            inc_ref_bits(py, other);
            other
        });
        assert!(!exception_pending(py));
        expect(2);
        assert_eq!(site.word(METHOD_SITE_CLASS_OFFSET), other);
        site.release(py);
        for bits in [receiver, other, class] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn a_function_mutation_forgets_the_call_plan_but_not_the_load() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = class_with_value(py, b"SitePlan", one as *const ());
        let receiver = instance(py, class);
        let site = TestSite::new(b"value");
        let result = call(py, &site, receiver);
        dec_ref_bits(py, result);
        let function = site.word(METHOD_SITE_FUNCTION_OFFSET);
        // Stand in for a compiled entry: the runtime function has none.
        unsafe { site.record().set(METHOD_SITE_ENTRY_OFFSET, 0x1000) };
        let defaults = crate::alloc_tuple(py, &[MoltObject::from_int(5).bits()]);
        assert!(!defaults.is_null());
        set_attr(
            py,
            function,
            b"__defaults__",
            MoltObject::from_ptr(defaults).bits(),
        );
        assert!(!exception_pending(py));
        assert_eq!(site.word(METHOD_SITE_ENTRY_OFFSET), 0);
        assert_eq!(site.word(METHOD_SITE_FUNCTION_OFFSET), function);
        site.release(py);
        for bits in [receiver, class] {
            dec_ref_bits(py, bits);
        }
    });
}

static SHADOW_ARMED: AtomicBool = AtomicBool::new(false);
static SHADOW_CLASS: AtomicU64 = AtomicU64::new(0);
static SHADOW_NAME: AtomicU64 = AtomicU64::new(0);
static SHADOW_HASH: AtomicU64 = AtomicU64::new(0);
static SHADOW_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn colliding_shadow_hash(__context: u64, _self: u64) -> u64 {
    let py = unsafe { crate::PyToken::from_attached_context(__context) };
    let bits = SHADOW_HASH.load(Ordering::SeqCst);
    inc_ref_bits(py, bits);
    bits
}

/// An `__eq__` that, once armed, deletes the class method mid-lookup.
extern "C" fn colliding_shadow_eq(__context: u64, _self: u64, _other: u64) -> u64 {
    let py = unsafe { crate::PyToken::from_attached_context(__context) };
    if SHADOW_ARMED.swap(false, Ordering::SeqCst) {
        SHADOW_CALLS.fetch_add(1, Ordering::SeqCst);
        let result = crate::molt_del_attr_name_attached_body(
            py,
            SHADOW_CLASS.load(Ordering::SeqCst),
            SHADOW_NAME.load(Ordering::SeqCst),
        );
        dec_ref_bits(py, result);
    }
    MoltObject::from_bool(false).bits()
}

/// The instance-dictionary probe can run Python equality that deletes the
/// method: the load keeps the function it selected, as CPython's does, and
/// only the next load observes the deletion.
#[test]
fn a_shadow_probe_that_deletes_the_method_keeps_this_load() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for warm in [false, true] {
            let class = class_with_value(py, b"SiteShadowOwner", one as *const ());
            let receiver = instance(py, class);
            let key_class = class_with_value(py, b"SiteShadowKey", one as *const ());
            for (name, target, arity) in [
                (
                    b"__hash__".as_slice(),
                    colliding_shadow_hash as *const (),
                    1,
                ),
                (b"__eq__".as_slice(), colliding_shadow_eq as *const (), 2),
            ] {
                let pointer = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::provenance::abi::expose_function_address(target),
                    arity,
                );
                assert!(!pointer.is_null());
                set_attr(py, key_class, name, MoltObject::from_ptr(pointer).bits());
            }
            let key = instance(py, key_class);
            let name = crate::dynamic_attr_name_bits(py, b"value").unwrap();
            let hash = crate::molt_hash_builtin_attached_body(py, name);
            SHADOW_HASH.store(hash, Ordering::SeqCst);
            SHADOW_CLASS.store(class, Ordering::SeqCst);
            SHADOW_NAME.store(name, Ordering::SeqCst);
            SHADOW_CALLS.store(0, Ordering::SeqCst);
            SHADOW_ARMED.store(false, Ordering::SeqCst);
            let dict_name = crate::dynamic_attr_name_bits(py, b"__dict__").unwrap();
            let dictionary = crate::molt_get_attr_name_attached_body(py, receiver, dict_name);
            let dict_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
            assert_eq!(unsafe { object_type_id(dict_ptr) }, TYPE_ID_DICT);
            assert!(unsafe {
                crate::dict_set_in_place(py, dict_ptr, key, MoltObject::none().bits())
            });
            let site = TestSite::new(b"value");
            if warm {
                let result = call(py, &site, receiver);
                assert_eq!(obj_from_bits(result).as_int(), Some(1));
                dec_ref_bits(py, result);
                assert_eq!(site.word(METHOD_SITE_CLASS_OFFSET), class);
            }
            SHADOW_ARMED.store(true, Ordering::SeqCst);
            let result = call(py, &site, receiver);
            assert_eq!(SHADOW_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(obj_from_bits(result).as_int(), Some(1));
            assert!(!exception_pending(py));
            dec_ref_bits(py, result);
            let result = call(py, &site, receiver);
            take_error(py, BuiltinExc::AttributeError);
            dec_ref_bits(py, result);
            site.release(py);
            for bits in [
                dictionary, dict_name, hash, name, key, key_class, receiver, class,
            ] {
                dec_ref_bits(py, bits);
            }
            SHADOW_HASH.store(0, Ordering::SeqCst);
            SHADOW_CLASS.store(0, Ordering::SeqCst);
            SHADOW_NAME.store(0, Ordering::SeqCst);
        }
    });
}

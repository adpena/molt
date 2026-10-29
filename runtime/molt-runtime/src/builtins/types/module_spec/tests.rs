use super::*;

#[test]
fn suffix_policy_survives_failed_initialization_and_commits_one_owner() {
    use crate::builtins::exceptions::ExceptionValue;
    struct RestoreSuffixPolicy(u64, u64);
    impl Drop for RestoreSuffixPolicy {
        fn drop(&mut self) {
            crate::with_gil_entry_nopanic!(py, {
                let state = types_state(py);
                let displaced = state
                    .module_spec_suffix_namespace
                    .swap(self.0, AtomicOrdering::AcqRel);
                state
                    .module_spec_suffix_bound
                    .store(self.1, AtomicOrdering::Release);
                if displaced != 0 {
                    dec_ref_bits(py, displaced);
                }
            });
        }
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let state = types_state(py);
        let _restore = RestoreSuffixPolicy(
            state
                .module_spec_suffix_namespace
                .swap(0, AtomicOrdering::AcqRel),
            state
                .module_spec_suffix_bound
                .swap(0, AtomicOrdering::AcqRel),
        );
        let original = ExceptionValue::adopt(
            py,
            module_spec_suffix_value(py, b"SOURCE_SUFFIXES").unwrap(),
        );
        let stage = ExceptionValue::adopt(py, stage_module_spec_suffix_namespace(py));
        let source_key = ExceptionValue::adopt(py, str_bits(py, b"SOURCE_SUFFIXES"));
        let bytecode_key = ExceptionValue::adopt(py, str_bits(py, b"BYTECODE_SUFFIXES"));
        // A partially assigned defining body is not an admissible owner.
        let partial = alloc_dict_with_pairs(py, &[source_key.bits(), MoltObject::none().bits()]);
        let partial = ExceptionValue::adopt(py, MoltObject::from_ptr(partial).bits());
        assert!(!commit_module_spec_suffix_namespace(py, partial.bits()));
        assert!(crate::builtins::platform::clear_pending_if_kind(
            py,
            &[crate::BuiltinExc::NameError]
        ));
        let after_failure = ExceptionValue::adopt(
            py,
            module_spec_suffix_value(py, b"SOURCE_SUFFIXES").unwrap(),
        );
        assert_eq!(after_failure.bits(), original.bits());
        drop(stage);

        // A successful retry publishes its defining dictionary. Subsequent
        // rebinding is observed there, not in a later replacement module.
        let bytecode = ExceptionValue::adopt(
            py,
            module_spec_suffix_value(py, b"BYTECODE_SUFFIXES").unwrap(),
        );
        let complete = alloc_dict_with_pairs(
            py,
            &[
                source_key.bits(),
                original.bits(),
                bytecode_key.bits(),
                bytecode.bits(),
            ],
        );
        let complete = ExceptionValue::adopt(py, MoltObject::from_ptr(complete).bits());
        assert!(commit_module_spec_suffix_namespace(py, complete.bits()));
        unsafe {
            let _ = dict_set_in_place(
                py,
                obj_from_bits(complete.bits()).as_ptr().unwrap(),
                source_key.bits(),
                MoltObject::from_int(37).bits(),
            );
        }
        assert!(!exception_pending(py));
        let rebound = ExceptionValue::adopt(
            py,
            module_spec_suffix_value(py, b"SOURCE_SUFFIXES").unwrap(),
        );
        assert_eq!(rebound.bits(), MoltObject::from_int(37).bits());
        assert!(commit_module_spec_suffix_namespace(py, partial.bits()));
        let after_replacement = ExceptionValue::adopt(
            py,
            module_spec_suffix_value(py, b"SOURCE_SUFFIXES").unwrap(),
        );
        assert_eq!(after_replacement.bits(), rebound.bits());
    });
}

fn str_bits(py: &PyToken<'_>, value: &[u8]) -> u64 {
    let ptr = alloc_string(py, value);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn attr(py: &PyToken<'_>, obj_bits: u64, name: &[u8]) -> u64 {
    let value = get_spec_attr(py, obj_bits, name).expect("ModuleSpec attribute");
    assert!(!exception_pending(py));
    value
}

fn attr_text(py: &PyToken<'_>, obj_bits: u64, name: &[u8]) -> String {
    let value = attr(py, obj_bits, name);
    let text = string_obj_to_owned(obj_from_bits(value)).expect("str attribute");
    dec_ref_bits(py, value);
    text
}

extern "C" fn observed_setattr(self_bits: u64, name_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::object::ops_format::string_obj_matches_bytes(obj_from_bits(name_bits), b"name") {
            let observed = str_bits(py, b"observed.leaf");
            let result = crate::molt_object_setattr(self_bits, name_bits, observed);
            dec_ref_bits(py, observed);
            result
        } else {
            crate::molt_object_setattr(self_bits, name_bits, value_bits)
        }
    })
}

#[test]
fn runtime_factory_obeys_mutable_class_attribute_protocol() {
    struct RemoveAttribute(u64, u64);
    impl Drop for RemoveAttribute {
        fn drop(&mut self) {
            crate::with_gil_entry_nopanic!(py, {
                crate::molt_del_attr_name(self.0, self.1);
                dec_ref_bits(py, self.1);
                dec_ref_bits(py, self.0);
            });
        }
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = module_spec_class(py);
        let setter = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(observed_setattr as *const ()),
            3,
        ))
        .bits();
        let setter_name = str_bits(py, b"__setattr__");
        inc_ref_bits(py, class);
        let _restore_class = RemoveAttribute(class, setter_name);
        crate::molt_set_attr_name(class, setter_name, setter);
        dec_ref_bits(py, setter);
        assert!(!exception_pending(py));
        let name = str_bits(py, b"original");
        let none = MoltObject::none().bits();
        let spec = alloc_module_spec(py, name, none, none, none).expect("observed spec");
        assert_eq!(attr_text(py, spec, b"name"), "observed.leaf");
        assert_eq!(attr_text(py, spec, b"parent"), "observed");
        for bits in [spec, name] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn runtime_factory_and_facade_type_share_one_canonical_class() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class_bits = module_spec_class(py);
        assert_ne!(class_bits, 0);
        assert_eq!(
            attr_text(py, class_bits, b"__module__"),
            "_frozen_importlib"
        );
        let facade_bits = molt_importlib_module_spec_type();
        assert_eq!(facade_bits, class_bits);
        dec_ref_bits(py, facade_bits);

        let none = MoltObject::none().bits();
        let name = str_bits(py, b"pkg.leaf");
        let origin = str_bits(py, b"/tmp/leaf.so");
        let not_package = MoltObject::from_bool(false).bits();
        let spec = alloc_module_spec(py, name, none, origin, not_package).expect("module spec");
        assert_eq!(type_of_bits(py, spec), class_bits);
        assert_eq!(attr_text(py, spec, b"name"), "pkg.leaf");
        assert_eq!(attr_text(py, spec, b"origin"), "/tmp/leaf.so");
        assert_eq!(attr_text(py, spec, b"parent"), "pkg");
        for (field, expected) in [
            (b"loader".as_slice(), none),
            (b"loader_state".as_slice(), none),
            (b"cached".as_slice(), none),
            (b"submodule_search_locations".as_slice(), none),
            (
                b"has_location".as_slice(),
                MoltObject::from_bool(false).bits(),
            ),
        ] {
            assert_eq!(attr(py, spec, field), expected);
        }
        let repr = molt_importlib_module_spec_repr(spec);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(repr)).as_deref(),
            Some("ModuleSpec(name='pkg.leaf', loader=None, origin='/tmp/leaf.so')")
        );
        dec_ref_bits(py, repr);

        let is_package = MoltObject::from_bool(true).bits();
        let package = alloc_module_spec(py, name, none, none, is_package).expect("package spec");
        assert_eq!(attr_text(py, package, b"parent"), "pkg.leaf");
        let package_repr = molt_repr_from_obj(package);
        assert!(!exception_pending(py));
        assert_eq!(
            string_obj_to_owned(obj_from_bits(package_repr)).as_deref(),
            Some("ModuleSpec(name='pkg.leaf', loader=None, submodule_search_locations=[])")
        );
        dec_ref_bits(py, package_repr);
        assert_eq!(
            attr(py, package, b"has_location"),
            MoltObject::from_bool(false).bits()
        );
        let locations = attr(py, package, b"submodule_search_locations");
        assert_eq!(molt_len(locations), MoltObject::from_int(0).bits());
        dec_ref_bits(py, locations);

        for bits in [package, spec, origin, name] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn class_call_binds_positional_and_keyword_initializer_arguments() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class_bits = module_spec_class(py);
        assert_ne!(class_bits, 0);
        let none = MoltObject::none().bits();
        let name = str_bits(py, b"top");
        let origin = str_bits(py, b"built-in");
        let positional = unsafe { call_callable2(py, class_bits, name, none) };
        assert!(!exception_pending(py));
        assert_eq!(type_of_bits(py, positional), class_bits);
        assert_eq!(attr_text(py, positional, b"parent"), "");
        assert_eq!(attr(py, positional, b"origin"), none);

        let origin_key = str_bits(py, b"origin");
        let package_key = str_bits(py, b"is_package");
        let builder = molt_callargs_new(1, 2);
        unsafe {
            let _ = molt_callargs_push_pos(builder, name);
            let _ = crate::molt_callargs_push_kw(builder, origin_key, origin);
            let _ =
                crate::molt_callargs_push_kw(builder, package_key, MoltObject::from_int(1).bits());
        }
        let keyword = molt_call_bind(class_bits, builder);
        assert!(!exception_pending(py));
        assert_eq!(attr_text(py, keyword, b"origin"), "built-in");
        assert_eq!(attr(py, keyword, b"loader"), none);
        assert_eq!(
            attr(py, keyword, b"has_location"),
            MoltObject::from_bool(false).bits()
        );
        assert_eq!(attr_text(py, keyword, b"parent"), "top");

        let parent_name = str_bits(py, b"parent");
        let _ = molt_set_attr_name(keyword, parent_name, name);
        assert!(exception_pending(py), "parent is a read-only property");
        clear_exception(py);

        for bits in [
            parent_name,
            keyword,
            package_key,
            origin_key,
            positional,
            origin,
            name,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

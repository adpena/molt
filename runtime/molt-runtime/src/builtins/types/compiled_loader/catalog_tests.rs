use super::*;

#[test]
fn compiled_finder_binds_defaults_on_class_instance_and_subclass() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let builtin = compiled_loader_builtin_class(py);
        let instance = molt_importlib_compiled_loader();
        let child_name =
            attr_name_bits_from_bytes(py, b"CatalogFinderSubclass").expect("subclass name");
        let child = crate::molt_class_new(child_name);
        let outcome = crate::molt_class_set_base(child, builtin);
        dec_ref_bits(py, outcome);
        assert!(!exception_pending(py));
        let missing = attr_name_bits_from_bytes(py, b"__molt_catalog_finder_missing_control__")
            .expect("missing name");
        let marker = MoltObject::from_int(39).bits();
        for receiver in [builtin, instance, child] {
            let find = required_attr(py, receiver, b"find_spec").expect("bound finder");
            let result = unsafe { call_callable1(py, find, missing) };
            assert!(obj_from_bits(result).is_none());
            assert!(!exception_pending(py));
            // Supplying path/target must not trigger iterable conversion or
            // callbacks: the catalog describes only immutable image rows.
            let result = unsafe { crate::call_callable3(py, find, missing, marker, marker) };
            assert!(obj_from_bits(result).is_none());
            assert!(!exception_pending(py));
            dec_ref_bits(py, find);
        }
        for bits in [missing, child, child_name, instance] {
            dec_ref_bits(py, bits);
        }
    });
}

use crate::*;
use molt_runtime_core::OwnedRuntimeValue;
use molt_runtime_regex::regex::{
    molt_re_compile, molt_re_pattern_info, molt_re_split, molt_re_sub,
};

#[test]
fn regex_aggregates_retain_each_created_child_once() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let zero = MoltObject::from_int(0).bits();
        for byte_mode in [false, true] {
            let make = |data: &[u8]| {
                let ptr = if byte_mode {
                    alloc_bytes(py, data)
                } else {
                    alloc_string(py, data)
                };
                assert!(!ptr.is_null());
                unsafe {
                    OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        MoltObject::from_ptr(ptr).bits(),
                    )
                }
            };
            let source = make(b"left_owner::right_owner");
            let expression = make(b"::");
            let replacement = make(b"replacement_owner");
            let pattern = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_compile(expression.bits(), zero),
                )
            };
            assert!(!exception_pending(py));

            let split = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_split(pattern.bits(), source.bits(), zero),
                )
            };
            assert!(!exception_pending(py));
            let children = unsafe { seq_snapshot(ptr_from_bits(split.bits())) };
            assert_eq!(children.len(), 2);
            for &child in children.iter() {
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(ptr_from_bits(child))).ref_count_snapshot() },
                    1
                );
            }

            let replaced = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_sub(pattern.bits(), replacement.bits(), source.bits(), zero),
                )
            };
            assert!(!exception_pending(py));
            let children = unsafe { seq_snapshot(ptr_from_bits(replaced.bits())) };
            assert_eq!(children.len(), 2);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(ptr_from_bits(children[0]))).ref_count_snapshot() },
                1
            );

            let named_expression = make(b"(?P<retained_owner>item)");
            let named_pattern = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_compile(named_expression.bits(), zero),
                )
            };
            assert!(!exception_pending(py));
            let info = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    molt_re_pattern_info(named_pattern.bits()),
                )
            };
            assert!(!exception_pending(py));
            let children = unsafe { seq_snapshot(ptr_from_bits(info.bits())) };
            assert_eq!(children.len(), 4);
            assert_eq!(
                unsafe { (*header_from_obj_ptr(ptr_from_bits(children[1]))).ref_count_snapshot() },
                1
            );
        }
    });
}

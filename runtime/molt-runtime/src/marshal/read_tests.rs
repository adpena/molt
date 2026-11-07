use super::*;

#[test]
fn completed_immutable_key_cycle_is_rejected_without_recursive_hashing() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::concurrency::gil::with_gil(|py| {
        let py = &py;
        // An independent marshal record constructs a completed direct tuple
        // cycle. Keep its returned owner so the fixture can retire its edge;
        // immutable-only cycles deliberately have no public tp_clear.
        let value = decode(py, Source::memory(b"\xa9\x01r\0\0\0\0")).unwrap();
        let reader = Reader {
            py,
            source: Source::memory(&[]),
            references: Vec::new(),
            frames: Vec::new(),
            constructing_tuples: HashSet::new(),
        };
        assert!(reader.key_ready(value.bits()).is_err());
        super::super::tests::assert_pending(
            py,
            BuiltinExc::ValueError,
            "bad marshal data (invalid reference)",
        );
        let pointer = obj_from_bits(value.bits()).as_ptr().unwrap();
        let old = unsafe {
            crate::object::seq_access::replace_capi_item(py, pointer, 0, MoltObject::none().bits())
        }
        .unwrap();
        dec_ref_bits(py, old);
    });
}

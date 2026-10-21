use super::*;
#[repr(C)]
struct Record {
    name: *const libc::c_char,
}
unsafe fn copy(record: &Record) -> Result<Option<Vec<u8>>> {
    unsafe { copy_field(record.name) }
}
#[test]
fn reentrant_query_grows_then_copies_before_scratch_retirement() {
    let mut calls = 0;
    let value = lookup(
        libc::_SC_GETPW_R_SIZE_MAX,
        |entry: *mut Record, buffer, _size, result| {
            calls += 1;
            if calls == 1 {
                return libc::ERANGE;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(c"owned".as_ptr(), buffer, 6);
                entry.write(Record { name: buffer });
                result.write(entry);
            }
            0
        },
        copy,
    )
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(value, Some(Some(b"owned".to_vec())));
    for status in [0, libc::EIO] {
        let result = lookup(
            libc::_SC_GETPW_R_SIZE_MAX,
            |_entry: *mut Record, _buffer, _size, _result| status,
            copy,
        )
        .unwrap();
        assert_eq!(result, None);
    }
}

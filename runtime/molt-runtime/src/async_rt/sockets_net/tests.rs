use super::*;

fn assert_pending_error(py: &PyToken<'_>, expected_class: &str, expected_errno: i64) {
    assert!(exception_pending(py));
    let error = crate::molt_exception_last_pending();
    let class = crate::builtins::exceptions::exception_class(py, error).unwrap();
    assert_eq!(
        class.bits(),
        exception_type_bits_from_name(py, expected_class)
    );
    clear_exception(py);
    let name = crate::attr_name_bits_from_bytes(py, b"errno").unwrap();
    let code = crate::molt_getattr_builtin(error, name, MoltObject::none().bits());
    assert_eq!(to_i64(obj_from_bits(code)), Some(expected_errno));
    dec_ref_bits(py, name);
    dec_ref_bits(py, code);
    let name = crate::attr_name_bits_from_bytes(py, b"strerror").unwrap();
    let message = crate::molt_getattr_builtin(error, name, MoltObject::none().bits());
    assert!(string_obj_to_owned(obj_from_bits(message)).is_some());
    dec_ref_bits(py, name);
    dec_ref_bits(py, message);
    dec_ref_bits(py, error);
    assert!(!exception_pending(py));
}

#[test]
fn resolver_host_status_preserves_overlapping_domains_and_capacity() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        // Independent wire values: EAI_SERVICE9 collides with ordinary EBADF9.
        assert!(getaddrinfo_host_status(py, 9, 0, 4096).is_err());
        assert_pending_error(py, "gaierror", 9);
        assert!(getaddrinfo_host_status(py, -9, 0, 4096).is_err());
        assert_pending_error(py, "OSError", 9);
        assert!(getaddrinfo_host_status(py, 8, 0, 4096).is_err());
        assert_pending_error(py, "gaierror", 8);
        assert_eq!(
            getaddrinfo_host_status(py, 0, 12, 4096),
            Ok(AddrInfoHostStatus::Complete)
        );
        assert_eq!(
            getaddrinfo_host_status(py, -12, 8192, 4096),
            Ok(AddrInfoHostStatus::Grow(8192))
        );
        // EAI_MEMORY6 is not the negative errno12 buffer-capacity request.
        assert!(getaddrinfo_host_status(py, 6, 8192, 4096).is_err());
        assert_pending_error(py, "gaierror", 6);
        assert!(getaddrinfo_host_status(py, -12, 4096, 4096).is_err());
        assert_pending_error(py, "OSError", 12);
        assert_eq!(
            getaddrinfo_host_status(py, -38, 0, 4096),
            Ok(AddrInfoHostStatus::NumericOnly)
        );
        for (malformed, length) in [(11, 0), (123456, 0), (-123456, 0), (i32::MIN, 0), (0, 8192)] {
            assert!(getaddrinfo_host_status(py, malformed, length, 4096).is_err());
            let error = crate::molt_exception_last_pending();
            let class = crate::builtins::exceptions::exception_class(py, error).unwrap();
            assert_eq!(
                class.bits(),
                exception_type_bits_from_name(py, "RuntimeError")
            );
            clear_exception(py);
            dec_ref_bits(py, error);
        }
    });
}

#[cfg(molt_has_net_io)]
#[test]
fn resolver_native_system_error_preserves_oserror_class_and_errno() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let captured = std::io::Error::from_raw_os_error(libc::EAGAIN);
        assert_eq!(
            raise_native_resolver_error(py, libc::EAI_SYSTEM, captured, "getaddrinfo"),
            MoltObject::none().bits()
        );
        assert_pending_error(py, "BlockingIOError", libc::EAGAIN as i64);
        let captured = std::io::Error::from_raw_os_error(libc::EBADF);
        assert_eq!(
            raise_native_resolver_error(py, libc::EAI_SYSTEM, captured, "getnameinfo"),
            MoltObject::none().bits()
        );
        assert_pending_error(py, "OSError", libc::EBADF as i64);
        let ignored_errno = std::io::Error::from_raw_os_error(libc::EBADF);
        assert_eq!(
            raise_native_resolver_error(py, libc::EAI_NONAME, ignored_errno, "getaddrinfo"),
            MoltObject::none().bits()
        );
        assert_pending_error(py, "gaierror", libc::EAI_NONAME as i64);
        #[cfg(target_vendor = "apple")]
        {
            let ignored_errno = std::io::Error::from_raw_os_error(libc::EBADF);
            let code = molt_runtime_platform::native_error_codes::EAI_BADHINTS
                .expect("Darwin SDK resolver code");
            raise_native_resolver_error(py, code, ignored_errno, "getaddrinfo");
            assert_pending_error(py, "gaierror", code as i64);
        }
    });
}

#[cfg(molt_has_net_io)]
#[test]
fn resolver_reverse_lookup_converts_only_resolver_errors() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        raise_os_error_errno::<u64>(py, libc::EBADF as i64, "native system failure");
        let original = crate::molt_exception_last_pending();
        assert_eq!(reverse_lookup_error(py), MoltObject::none().bits());
        assert_eq!(exception_last_bits_noinc(py), Some(original));
        assert_pending_error(py, "OSError", libc::EBADF as i64);
        dec_ref_bits(py, original);

        raise_gaierror(py, libc::EAI_NONAME as i64, "resolver failure");
        assert_eq!(reverse_lookup_error(py), MoltObject::none().bits());
        let error = crate::molt_exception_last_pending();
        let class = crate::builtins::exceptions::exception_class(py, error).unwrap();
        assert_eq!(class.bits(), exception_type_bits_from_name(py, "herror"));
        clear_exception(py);
        let name = crate::attr_name_bits_from_bytes(py, b"errno").unwrap();
        let code = crate::molt_getattr_builtin(error, name, MoltObject::none().bits());
        assert!(obj_from_bits(code).is_none(), "no invented legacy h_errno");
        dec_ref_bits(py, name);
        dec_ref_bits(py, code);
        dec_ref_bits(py, error);
    });
}

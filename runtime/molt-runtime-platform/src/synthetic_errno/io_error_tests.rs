use super::*;
use std::io::{Error, ErrorKind};

#[test]
fn raw_wasi_parser_and_guest_projection_preserve_separate_numeric_domains() {
    for (raw, guest) in [(44, 2), (2, 13), (6, 11), (26, 115), (76, 13)] {
        assert_eq!(
            RawWasiErrno::from_code(raw).unwrap().to_guest().code(),
            guest
        );
    }
    for unknown in [0, -1, 108, i32::MAX] {
        assert_eq!(RawWasiErrno::from_code(unknown), None);
    }
}

#[test]
fn rawless_io_errors_use_one_declared_guest_policy() {
    // Independent public guest ABI values, including the old metadata policy.
    for (kind, expected) in [
        (ErrorKind::NotFound, 2),
        (ErrorKind::PermissionDenied, 13),
        (ErrorKind::AlreadyExists, 17),
        (ErrorKind::IsADirectory, 21),
        (ErrorKind::NotADirectory, 20),
        (ErrorKind::ReadOnlyFilesystem, 30),
        (ErrorKind::WouldBlock, 11),
        (ErrorKind::TimedOut, 110),
        (ErrorKind::Unsupported, 38),
        (ErrorKind::Other, 5),
    ] {
        let error = Error::new(kind, "typed SDK-less error");
        assert_eq!(from_io_error(&error).code(), expected, "{kind:?}");
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
#[test]
fn native_sdk_io_errors_adopt_target_codes_once() {
    assert_eq!(
        from_io_error(&Error::from_raw_os_error(libc::ENOENT)).code(),
        2
    );
    assert_eq!(
        from_io_error(&Error::from_raw_os_error(libc::EAGAIN)).code(),
        11
    );
    assert_eq!(
        from_io_error(&Error::from_raw_os_error(libc::ESPIPE)).code(),
        29
    );
}

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
#[test]
fn wasi_sdk_io_errors_preserve_raw_and_guest_domain_distinction() {
    // Raw preview1 ENOENT44 and EACCES2 must not be interpreted as guest codes.
    assert_eq!(from_io_error(&Error::from_raw_os_error(44)).code(), 2);
    assert_eq!(from_io_error(&Error::from_raw_os_error(2)).code(), 13);
    assert_eq!(from_io_error(&Error::from_raw_os_error(8)).code(), 9);
    assert_eq!(from_io_error(&Error::from_raw_os_error(70)).code(), 29);
    assert_eq!(from_io_error(&Error::from_raw_os_error(76)).code(), 13);
    assert_eq!(from_io_error(&Error::from_raw_os_error(i32::MAX)).code(), 5);
}

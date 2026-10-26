use super::*;

#[test]
fn synthetic_errno_protocol_preserves_values_and_alias_publication() {
    assert_eq!(GuestErrno::EINPROGRESS.code(), 115);
    assert_eq!(GuestErrno::EAGAIN.code(), 11);
    assert_eq!(GuestErrno::EWOULDBLOCK, GuestErrno::EAGAIN);
    assert_eq!(GuestErrno::ECONNREFUSED.code(), 111);
    assert_eq!(GuestErrno::ECONNRESET.code(), 104);
    assert_eq!(GuestErrno::ETIMEDOUT.code(), 110);
    assert_eq!(GuestResolverError::EAI_NONAME.code(), 8);

    assert!(
        ERRNO_CONSTANTS
            .iter()
            .all(|(name, _)| !name.starts_with("EAI_"))
    );
    let reverse: std::collections::HashMap<_, _> = ERRNO_CONSTANTS
        .iter()
        .map(|&(name, value)| (value, name))
        .collect();
    assert_eq!(reverse[&11], "EAGAIN");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn synthetic_errno_native_conversion_closes_platform_number_domains() {
    assert_eq!(
        from_native_errno(libc::EINPROGRESS),
        GuestErrno::EINPROGRESS
    );
    assert_eq!(from_native_errno(libc::EAGAIN), GuestErrno::EAGAIN);
    assert_eq!(
        from_native_errno(libc::ECONNREFUSED),
        GuestErrno::ECONNREFUSED
    );
    assert_eq!(from_native_errno(libc::ECONNRESET), GuestErrno::ECONNRESET);
    assert_eq!(from_native_errno(libc::ETIMEDOUT), GuestErrno::ETIMEDOUT);
    assert_eq!(from_native_errno(i32::MAX), GuestErrno::EIO);
    let native_noname = crate::native_error_codes::EAI_NONAME.unwrap();
    let NativeResolverError::Resolver(code) = from_native_resolver_error(native_noname).unwrap()
    else {
        panic!("resolver classified as System");
    };
    assert_eq!(code.code(), native_noname);
    assert_eq!(
        project_guest_resolver_error(code),
        Ok(GuestResolverError::EAI_NONAME)
    );
    if let Some(code) = crate::native_error_codes::EAI_SYSTEM {
        assert_eq!(
            from_native_resolver_error(code),
            Ok(NativeResolverError::System)
        );
    }
    assert_eq!(
        from_native_resolver_error(0),
        Err(InvalidNativeResolverStatus(0))
    );
    let NativeResolverError::Resolver(vendor) = from_native_resolver_error(i32::MAX).unwrap()
    else {
        panic!("unknown vendor code classified as System");
    };
    assert_eq!(vendor.code(), i32::MAX);
    assert_eq!(
        project_guest_resolver_error(vendor),
        Err(UnknownNativeResolverError(i32::MAX))
    );
    #[cfg(windows)]
    {
        assert_eq!(from_native_errno(10035), GuestErrno::EAGAIN);
        assert_eq!(from_native_errno(10036), GuestErrno::EINPROGRESS);
        assert_eq!(from_native_errno(10061), GuestErrno::ECONNREFUSED);
    }
}

#[test]
fn resolver_wire_independent_statuses_preserve_collisions_and_refuse_sentinels() {
    assert_eq!(decode_resolver_status(0), Ok(ResolverStatus::Success));
    assert_eq!(
        decode_resolver_status(9),
        Ok(ResolverStatus::Resolver(GuestResolverError::EAI_SERVICE))
    );
    assert_eq!(
        decode_resolver_status(-9),
        Ok(ResolverStatus::System(GuestErrno::EBADF))
    );
    assert_eq!(
        decode_resolver_status(8),
        Ok(ResolverStatus::Resolver(GuestResolverError::EAI_NONAME))
    );
    assert_eq!(
        decode_resolver_status(-12),
        Ok(ResolverStatus::System(GuestErrno::ENOMEM))
    );
    assert_eq!(
        decode_resolver_status(6),
        Ok(ResolverStatus::Resolver(GuestResolverError::EAI_MEMORY))
    );
    for status in [11, 15, i32::MIN, i32::MAX, -i32::MAX] {
        assert_eq!(
            decode_resolver_status(status),
            Err(InvalidResolverStatus(status))
        );
    }
    assert_eq!(
        ResolverFailure::Resolver(GuestResolverError::EAI_SERVICE).encode(),
        9
    );
    assert_eq!(ResolverFailure::System(GuestErrno::EBADF).encode(), -9);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_resolver_alias_uses_declared_semantics_and_sdk_presence() {
    let Some(nodata) = crate::native_error_codes::EAI_NODATA else {
        return;
    };
    let NativeResolverError::Resolver(code) = from_native_resolver_error(nodata).unwrap() else {
        panic!("NODATA classified as system");
    };
    let expected = if crate::native_error_codes::EAI_NONAME == Some(nodata) {
        GuestResolverError::EAI_NONAME
    } else {
        GuestResolverError::EAI_NODATA
    };
    assert_eq!(project_guest_resolver_error(code), Ok(expected));
}
